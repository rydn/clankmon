//! Plain data types shared by collectors, the store and the UI.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// Token usage for one API turn, split the way pricing needs it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Fresh (uncached) input tokens.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Cache writes at the 5-minute TTL (1.25x input price).
    pub cache_write_5m: u64,
    /// Cache writes at the 1-hour TTL (2x input price).
    pub cache_write_1h: u64,
    /// Thinking tokens, a subset of `output`.
    pub thinking: u64,
}

impl Usage {
    pub fn cache_write(&self) -> u64 {
        self.cache_write_5m + self.cache_write_1h
    }

    /// Everything the model read this turn: the size of the prompt.
    pub fn prompt_total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write()
    }

    pub fn total(&self) -> u64 {
        self.prompt_total() + self.output
    }

    /// Field-wise max. Streaming writes several transcript lines per message
    /// and sources disagree on partial counts; the largest value is the final one.
    pub fn merge_max(&mut self, other: &Usage) {
        self.input = self.input.max(other.input);
        self.output = self.output.max(other.output);
        self.cache_read = self.cache_read.max(other.cache_read);
        // Sources disagree on the TTL split (OTel reports only a total), so take
        // the whole split from whichever side reports more cache writes.
        // Field-wise max would double-count a 1h write reported as 5m elsewhere.
        if other.cache_write() > self.cache_write() {
            self.cache_write_5m = other.cache_write_5m;
            self.cache_write_1h = other.cache_write_1h;
        }
        self.thinking = self.thinking.max(other.thinking);
    }

    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
        self.thinking += other.thinking;
    }

    pub fn sub(&mut self, other: &Usage) {
        self.input = self.input.saturating_sub(other.input);
        self.output = self.output.saturating_sub(other.output);
        self.cache_read = self.cache_read.saturating_sub(other.cache_read);
        self.cache_write_5m = self.cache_write_5m.saturating_sub(other.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.saturating_sub(other.cache_write_1h);
        self.thinking = self.thinking.saturating_sub(other.thinking);
    }
}

/// Which collectors contributed to a record. A request seen in the transcript
/// and through the proxy carries both bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sources(pub u8);

impl Sources {
    pub const JSONL: u8 = 1;
    pub const PROXY: u8 = 2;
    pub const OTEL: u8 = 4;
    pub const STATUSLINE: u8 = 8;
    pub const HOOK: u8 = 16;
    pub const OAUTH: u8 = 32;

    pub fn has(&self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    pub fn tag(&self) -> String {
        let mut s = String::new();
        for (bit, c) in [
            (Self::JSONL, 'J'),
            (Self::PROXY, 'P'),
            (Self::OTEL, 'O'),
        ] {
            s.push(if self.has(bit) { c } else { '·' });
        }
        s
    }
}

/// One API turn (one `POST /v1/messages`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Request {
    /// Merge key: `requestId` when known, otherwise the message id.
    pub key: String,
    pub request_id: Option<String>,
    pub message_id: Option<String>,
    pub ts: DateTime<Utc>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub model: String,
    pub usage: Usage,
    pub cost_usd: f64,
    /// True when the model was not in the pricing table and a fallback price was used.
    pub cost_estimated: bool,
    pub stop_reason: Option<String>,
    pub duration_ms: Option<u64>,
    pub ttft_ms: Option<u64>,
    pub http_status: Option<u16>,
    pub error: Option<String>,
    pub sidechain: bool,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
    pub speed: Option<String>,
    pub query_source: Option<String>,
    pub tools: Vec<String>,
    pub sources: Sources,
    /// Proxy-only request shape.
    pub shape: Option<RequestShape>,
}

impl Request {
    pub fn is_error(&self) -> bool {
        self.error.is_some() || self.http_status.is_some_and(|s| s >= 400)
    }

    /// Merge another observation of the same request into this one.
    pub fn merge(&mut self, other: &Request) {
        if self.request_id.is_none() {
            self.request_id.clone_from(&other.request_id);
        }
        if self.message_id.is_none() {
            self.message_id.clone_from(&other.message_id);
        }
        // Keep the earliest timestamp: it is closest to when the request started.
        if other.ts < self.ts && other.ts.timestamp() > 0 {
            self.ts = other.ts;
        }
        fill(&mut self.session_id, &other.session_id);
        fill(&mut self.cwd, &other.cwd);
        if self.model.is_empty() || self.model == "<synthetic>" {
            self.model.clone_from(&other.model);
        }
        self.usage.merge_max(&other.usage);
        fill(&mut self.stop_reason, &other.stop_reason);
        fill_copy(&mut self.duration_ms, other.duration_ms);
        fill_copy(&mut self.ttft_ms, other.ttft_ms);
        fill_copy(&mut self.http_status, other.http_status);
        fill(&mut self.error, &other.error);
        self.sidechain |= other.sidechain;
        fill(&mut self.effort, &other.effort);
        fill(&mut self.service_tier, &other.service_tier);
        fill(&mut self.speed, &other.speed);
        fill(&mut self.query_source, &other.query_source);
        for t in &other.tools {
            if !self.tools.contains(t) {
                self.tools.push(t.clone());
            }
        }
        self.sources.0 |= other.sources.0;
        if self.shape.is_none() {
            self.shape.clone_from(&other.shape);
        }
    }
}

fn fill<T: Clone>(dst: &mut Option<T>, src: &Option<T>) {
    if dst.is_none() {
        dst.clone_from(src);
    }
}

fn fill_copy<T: Copy>(dst: &mut Option<T>, src: Option<T>) {
    if dst.is_none() {
        *dst = src;
    }
}

/// What the proxy saw about the outgoing request body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestShape {
    pub max_tokens: Option<u64>,
    pub tool_count: usize,
    pub system_bytes: usize,
    pub message_count: usize,
    pub body_bytes: usize,
    pub betas: Vec<String>,
    pub thinking: Option<String>,
    pub stream: bool,
    /// Raw rate-limit related response headers, for the LIMITS view.
    pub limit_headers: Vec<(String, String)>,
}

/// What a session is doing right now, htop-style.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SessionState {
    /// Waiting for the model.
    Thinking,
    /// Running a tool.
    Tool(String),
    /// Blocked on a permission prompt.
    Waiting,
    /// Compacting the conversation.
    Compacting,
    /// Turn finished, waiting for the user.
    #[default]
    Idle,
    /// Process exited.
    Ended,
}

impl SessionState {
    pub fn label(&self) -> String {
        match self {
            SessionState::Thinking => "thinking".into(),
            SessionState::Tool(t) => short_tool(t),
            SessionState::Waiting => "permission".into(),
            SessionState::Compacting => "compacting".into(),
            SessionState::Idle => "idle".into(),
            SessionState::Ended => "ended".into(),
        }
    }

    pub fn glyph(&self) -> &'static str {
        match self {
            SessionState::Thinking => "◐",
            SessionState::Tool(_) => "⚙",
            SessionState::Waiting => "⏸",
            SessionState::Compacting => "◇",
            SessionState::Idle => "○",
            SessionState::Ended => "✕",
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            SessionState::Thinking | SessionState::Tool(_) | SessionState::Compacting
        )
    }
}

/// `mcp__plugin_x__tool` is too long for a table cell.
pub fn short_tool(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("mcp__") {
        let last = rest.rsplit("__").next().unwrap_or(rest);
        return format!("mcp:{last}");
    }
    name.to_string()
}

/// A change in what a session is doing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub session_id: String,
    pub ts: DateTime<Utc>,
    pub state: SessionState,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub version: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// True when this activity came from a user prompt (counts a turn).
    pub user_prompt: bool,
    pub compaction: bool,
}

/// One tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub session_id: Option<String>,
    pub ts: DateTime<Utc>,
    pub name: String,
    pub success: Option<bool>,
    pub duration_ms: Option<u64>,
    pub tool_use_id: Option<String>,
}

/// A live Claude Code process from `~/.claude/sessions/<pid>.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcInfo {
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub entrypoint: Option<String>,
    pub version: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub cpu_pct: Option<f32>,
    pub rss_kb: Option<u64>,
}

/// Where a limit reading came from, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LimitSource {
    Proxy,
    Statusline,
    OAuth,
    Transcript,
}

impl LimitSource {
    pub fn label(&self) -> &'static str {
        match self {
            LimitSource::Proxy => "proxy",
            LimitSource::Statusline => "statusline",
            LimitSource::OAuth => "oauth",
            LimitSource::Transcript => "transcript",
        }
    }
}

/// One rate-limit window reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowReading {
    /// 0..=100 (can exceed 100 when over the limit).
    pub used_pct: f64,
    pub resets_at: Option<DateTime<Utc>>,
    /// `active`, `warning`, `rate_limited`, `allowed`, `rejected`...
    pub status: Option<String>,
}

/// A snapshot of subscription limits from one source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitSnapshot {
    pub ts: DateTime<Utc>,
    pub source: LimitSource,
    pub five_hour: Option<WindowReading>,
    pub seven_day: Option<WindowReading>,
    /// Extra named windows (OAuth `seven_day_opus`, statusline `spend_limit`, ...).
    pub extra: BTreeMap<String, WindowReading>,
    pub overall_status: Option<String>,
    pub representative_claim: Option<String>,
    pub overage_status: Option<String>,
    pub session_id: Option<String>,
}

/// A 429 or quota rejection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Incident {
    pub ts: DateTime<Utc>,
    pub session_id: Option<String>,
    pub kind: String,
    pub message: String,
    pub resets_at: Option<DateTime<Utc>>,
    pub http_status: Option<u16>,
}

/// Session metadata pushed by Claude Code's statusline command.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatuslineInfo {
    pub ts: DateTime<Utc>,
    pub session_id: String,
    pub session_name: Option<String>,
    pub cwd: Option<String>,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub version: Option<String>,
    pub effort: Option<String>,
    pub fast_mode: Option<bool>,
    pub context_used_pct: Option<f64>,
    pub context_window_size: Option<u64>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub api_duration_ms: Option<u64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    pub cache_warm: Option<bool>,
    pub cache_hit_ratio: Option<f64>,
    pub cache_misses: Option<u64>,
    pub agent: Option<String>,
}

/// Daily totals for the HISTORY view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DayAgg {
    pub usage: Usage,
    pub cost: f64,
    pub requests: u64,
    pub errors: u64,
    pub by_model: BTreeMap<String, (u64, f64)>,
}

/// Cold-start history from `~/.claude/stats-cache.json`.
#[derive(Debug, Clone, Default)]
pub struct StatsCache {
    pub days: BTreeMap<NaiveDate, StatsDay>,
}

#[derive(Debug, Clone, Default)]
pub struct StatsDay {
    pub messages: u64,
    pub sessions: u64,
    pub tool_calls: u64,
    pub tokens_by_model: BTreeMap<String, u64>,
}

/// OAuth `/api/oauth/usage` result, kept for the LIMITS view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OauthExtra {
    pub ts: Option<DateTime<Utc>>,
    pub extra_usage: Option<serde_json::Value>,
    pub error: Option<String>,
    pub next_poll: Option<DateTime<Utc>>,
}

/// The collectors clankmon knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Collector {
    Jsonl,
    Procs,
    Statusline,
    Hooks,
    Proxy,
    Otel,
    OAuth,
}

impl Collector {
    pub const ALL: [Collector; 7] = [
        Collector::Jsonl,
        Collector::Procs,
        Collector::Statusline,
        Collector::Hooks,
        Collector::Proxy,
        Collector::Otel,
        Collector::OAuth,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Collector::Jsonl => "jsonl",
            Collector::Procs => "procs",
            Collector::Statusline => "status",
            Collector::Hooks => "hooks",
            Collector::Proxy => "proxy",
            Collector::Otel => "otel",
            Collector::OAuth => "oauth",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectorHealth {
    Disabled,
    Listening,
    Ok,
    Error(String),
}

/// Everything collectors send to the app.
#[derive(Debug, Clone)]
pub enum AppEvent {
    Request(Box<Request>),
    Activity(Activity),
    Tool(ToolCall),
    Procs(Vec<ProcInfo>),
    Limits(Box<LimitSnapshot>),
    Statusline(Box<StatuslineInfo>),
    Incident(Incident),
    StatsCache(StatsCache),
    Oauth(OauthExtra),
    Collector(Collector, CollectorHealth),
    ScanProgress { files_done: usize, files_total: usize },
    ScanDone { files: usize, lines: u64, millis: u64 },
    Batch(Vec<AppEvent>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_max_usage_and_unions_sources() {
        let mut a = Request {
            key: "req_1".into(),
            model: "claude-opus-5-5".into(),
            usage: Usage { input: 2, output: 10, ..Default::default() },
            sources: Sources(Sources::JSONL),
            tools: vec!["Bash".into()],
            ..Default::default()
        };
        let b = Request {
            key: "req_1".into(),
            usage: Usage { input: 2, output: 1915, cache_read: 25235, ..Default::default() },
            ttft_ms: Some(812),
            sources: Sources(Sources::PROXY),
            tools: vec!["Bash".into(), "Read".into()],
            ..Default::default()
        };
        a.merge(&b);
        assert_eq!(a.usage.output, 1915);
        assert_eq!(a.usage.cache_read, 25235);
        assert_eq!(a.ttft_ms, Some(812));
        assert_eq!(a.tools, vec!["Bash", "Read"]);
        assert_eq!(a.sources.tag(), "JP·");
    }

    #[test]
    fn short_tool_names() {
        assert_eq!(short_tool("mcp__plugin_eminiar_eminiar__em_execute"), "mcp:em_execute");
        assert_eq!(short_tool("Bash"), "Bash");
    }
}
