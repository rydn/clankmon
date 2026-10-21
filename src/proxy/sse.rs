//! Incremental parser for the Messages API server-sent event stream.
//! Fed raw response chunks as they pass through the proxy; never buffers more
//! than one incomplete event.

use serde_json::Value;

use crate::collect::jsonl::{usage_from_value, usage_tier};
use crate::model::Usage;

#[derive(Debug, Default, Clone)]
pub struct StreamSummary {
    pub message_id: Option<String>,
    pub model: Option<String>,
    pub usage: Usage,
    pub stop_reason: Option<String>,
    pub tools: Vec<String>,
    pub error: Option<String>,
    pub service_tier: Option<String>,
    pub speed: Option<String>,
    /// True once the first content block started (time to first token).
    pub saw_content: bool,
    pub events: u64,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    pub summary: StreamSummary,
}

impl SseParser {
    /// Feed a chunk. Returns true when this chunk carried the first content block.
    pub fn feed(&mut self, chunk: &[u8]) -> bool {
        self.buf.extend_from_slice(chunk);
        let had_content = self.summary.saw_content;
        while let Some((end, sep)) = find_event_end(&self.buf) {
            let event: Vec<u8> = self.buf.drain(..end + sep).collect();
            if let Ok(text) = std::str::from_utf8(&event[..end]) {
                self.handle_event(text);
            }
        }
        !had_content && self.summary.saw_content
    }

    fn handle_event(&mut self, text: &str) {
        let mut data = String::new();
        for line in text.lines() {
            if let Some(d) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(d.trim_start());
            }
        }
        if data.is_empty() {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(&data) else { return };
        self.summary.events += 1;
        let s = &mut self.summary;
        match v.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(m) = v.get("message") {
                    s.message_id = m.get("id").and_then(Value::as_str).map(str::to_string);
                    s.model = m.get("model").and_then(Value::as_str).map(str::to_string);
                    if let Some(u) = m.get("usage") {
                        s.usage.merge_max(&usage_from_value(u));
                        let (tier, speed) = usage_tier(u);
                        s.service_tier = s.service_tier.take().or(tier);
                        s.speed = s.speed.take().or(speed);
                    }
                }
            }
            Some("content_block_start") => {
                s.saw_content = true;
                if let Some(b) = v.get("content_block") {
                    let kind = b.get("type").and_then(Value::as_str).unwrap_or("");
                    if (kind == "tool_use" || kind == "server_tool_use" || kind == "mcp_tool_use")
                        && let Some(n) = b.get("name").and_then(Value::as_str)
                    {
                        s.tools.push(n.to_string());
                    }
                }
            }
            Some("content_block_delta") => s.saw_content = true,
            Some("message_delta") => {
                if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    s.stop_reason = Some(r.to_string());
                }
                // Usage in message_delta is cumulative.
                if let Some(u) = v.get("usage") {
                    s.usage.merge_max(&usage_from_value(u));
                }
            }
            Some("error") => {
                let t = v.pointer("/error/type").and_then(Value::as_str).unwrap_or("error");
                let m = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("");
                s.error = Some(format!("{t}: {m}"));
            }
            _ => {}
        }
    }
}

/// Position of the first event terminator and its length (`\n\n` or `\r\n\r\n`).
fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|p| (p, 2));
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| (p, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const STREAM: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-5-5\",\"usage\":{\"input_tokens\":25,\"cache_read_input_tokens\":9000,\"cache_creation_input_tokens\":120,\"cache_creation\":{\"ephemeral_5m_input_tokens\":0,\"ephemeral_1h_input_tokens\":120},\"output_tokens\":1,\"service_tier\":\"standard\"}}}\n\n",
        "event: ping\ndata: {\"type\":\"ping\"}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"Bash\",\"input\":{}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":315}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );

    #[test]
    fn parses_full_stream_in_odd_chunks() {
        let mut p = SseParser::default();
        let bytes = STREAM.as_bytes();
        let mut first_content_at = None;
        // Feed in 7-byte chunks to exercise event reassembly.
        for (i, chunk) in bytes.chunks(7).enumerate() {
            if p.feed(chunk) {
                first_content_at = Some(i);
            }
        }
        let s = &p.summary;
        assert_eq!(s.message_id.as_deref(), Some("msg_1"));
        assert_eq!(s.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(s.usage.input, 25);
        assert_eq!(s.usage.cache_read, 9000);
        assert_eq!(s.usage.cache_write_1h, 120);
        assert_eq!(s.usage.output, 315);
        assert_eq!(s.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(s.tools, vec!["Bash"]);
        assert_eq!(s.service_tier.as_deref(), Some("standard"));
        assert!(first_content_at.is_some());
        assert_eq!(s.events, 7);
    }

    #[test]
    fn crlf_and_errors() {
        let mut p = SseParser::default();
        p.feed(b"event: error\r\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\r\n\r\n");
        assert_eq!(p.summary.error.as_deref(), Some("overloaded_error: Overloaded"));
    }
}
