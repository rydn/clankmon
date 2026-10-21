//! Transparent Anthropic API proxy.
//!
//! Point Claude Code at it with `ANTHROPIC_BASE_URL=http://127.0.0.1:8484`.
//! Requests are forwarded untouched (auth included) and responses are streamed
//! back chunk by chunk with no added buffering; a copy of each chunk feeds the
//! SSE parser. Nothing is logged about prompt or completion content unless
//! body capture is turned on explicitly.

pub mod headers;
pub mod sse;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use http::header::{self, HeaderMap, HeaderName};
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Method, Response, StatusCode, Uri};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::collect::jsonl::{usage_from_value, usage_tier};
use crate::ipc::{AsyncSink, Frame as IpcFrame};
use crate::model::{AppEvent, Collector, CollectorHealth, Incident, LimitSnapshot, Request, RequestShape, Sources};
use crate::net::{Body, HttpsClient, full, https_client, json_response};
use sse::SseParser;

/// Where proxy observations go: straight into the TUI, or over IPC from the standalone proxy.
#[derive(Clone)]
pub enum Sink {
    Channel(UnboundedSender<AppEvent>),
    Ipc { sink: Arc<AsyncSink>, print: bool },
}

impl Sink {
    async fn request(&self, r: Request, limits: Option<&LimitSnapshot>) {
        match self {
            Sink::Channel(tx) => {
                let _ = tx.send(AppEvent::Request(Box::new(r)));
            }
            Sink::Ipc { sink, print } => {
                if *print {
                    println!("{}", log_line(&r, limits));
                }
                sink.send(&IpcFrame::request(&r)).await;
            }
        }
    }

    async fn limits(&self, l: LimitSnapshot) {
        match self {
            Sink::Channel(tx) => {
                let _ = tx.send(AppEvent::Limits(Box::new(l)));
            }
            Sink::Ipc { sink, .. } => sink.send(&IpcFrame::limits(&l)).await,
        }
    }

    async fn incident(&self, i: Incident) {
        match self {
            Sink::Channel(tx) => {
                let _ = tx.send(AppEvent::Incident(i));
            }
            Sink::Ipc { sink, print } => {
                if *print {
                    println!("\x1b[38;2;255;59;59m⚠ {} {}\x1b[0m", i.kind, i.message);
                }
                sink.send(&IpcFrame::incident(&i)).await;
            }
        }
    }

    fn health(&self, h: CollectorHealth) {
        if let Sink::Channel(tx) = self {
            let _ = tx.send(AppEvent::Collector(Collector::Proxy, h));
        }
    }
}

pub struct Proxy {
    client: HttpsClient,
    upstream: String,
    sink: Sink,
    capture_dir: Option<PathBuf>,
}

impl Proxy {
    pub fn new(upstream: &str, sink: Sink, capture_dir: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Proxy {
            client: https_client(),
            upstream: upstream.trim_end_matches('/').to_string(),
            sink,
            capture_dir,
        })
    }
}

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus ones the proxy recomputes.
fn skip_request_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host" | "connection" | "keep-alive" | "proxy-connection" | "proxy-authorization" | "te" | "trailer"
            | "transfer-encoding" | "upgrade" | "content-length" | "accept-encoding"
    )
}

fn skip_response_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection" | "keep-alive" | "proxy-connection" | "te" | "trailer" | "transfer-encoding" | "upgrade"
            | "content-length"
    )
}

/// Session id from Claude Code's request: a session header, or
/// `metadata.user_id` of the form `user_<h>_account_<uuid>_session_<uuid>`.
fn session_from(headers: &HeaderMap, body: Option<&Value>) -> Option<String> {
    for (k, v) in headers {
        if k.as_str().contains("session-id")
            && let Ok(s) = v.to_str()
        {
            return Some(s.to_string());
        }
    }
    let uid = body?.pointer("/metadata/user_id")?.as_str()?;
    if let Ok(v) = serde_json::from_str::<Value>(uid)
        && let Some(s) = v.get("session_id").and_then(Value::as_str)
    {
        return Some(s.to_string());
    }
    uid.split("_session_").nth(1).map(str::to_string)
}

pub fn shape_of(body: &Value, headers: &HeaderMap, body_bytes: usize) -> RequestShape {
    RequestShape {
        max_tokens: body.get("max_tokens").and_then(Value::as_u64),
        tool_count: body.get("tools").and_then(Value::as_array).map_or(0, Vec::len),
        system_bytes: body.get("system").map_or(0, |s| s.to_string().len()),
        message_count: body.get("messages").and_then(Value::as_array).map_or(0, Vec::len),
        body_bytes,
        betas: headers
            .get_all("anthropic-beta")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|s| s.split(','))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        thinking: body.pointer("/thinking/type").and_then(Value::as_str).map(str::to_string),
        stream: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
        limit_headers: Vec::new(),
    }
}

struct Pending {
    ts: DateTime<Utc>,
    started: Instant,
    model: String,
    session: Option<String>,
    effort: Option<String>,
    shape: Option<RequestShape>,
    status: u16,
    request_id: Option<String>,
}

impl Pending {
    fn finish(self, summary: &sse::StreamSummary, ttft_ms: Option<u64>) -> Request {
        let key = self
            .request_id
            .clone()
            .or_else(|| summary.message_id.clone())
            .unwrap_or_else(|| format!("proxy-{}", self.ts.timestamp_nanos_opt().unwrap_or(0)));
        let error = summary.error.clone().or_else(|| (self.status >= 400).then(|| format!("HTTP {}", self.status)));
        Request {
            key,
            request_id: self.request_id,
            message_id: summary.message_id.clone(),
            ts: self.ts,
            session_id: self.session,
            model: summary.model.clone().unwrap_or(self.model),
            usage: summary.usage,
            stop_reason: summary.stop_reason.clone(),
            duration_ms: Some(self.started.elapsed().as_millis() as u64),
            ttft_ms,
            http_status: Some(self.status),
            error,
            effort: self.effort,
            service_tier: summary.service_tier.clone(),
            speed: summary.speed.clone(),
            tools: summary.tools.clone(),
            sources: Sources(Sources::PROXY),
            shape: self.shape,
            ..Default::default()
        }
    }
}

/// Summary of a non-streaming JSON response.
fn summary_from_json(v: &Value) -> sse::StreamSummary {
    let mut s = sse::StreamSummary::default();
    if v.get("type").and_then(Value::as_str) == Some("error") {
        let t = v.pointer("/error/type").and_then(Value::as_str).unwrap_or("error");
        let m = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("");
        s.error = Some(format!("{t}: {m}"));
        return s;
    }
    s.message_id = v.get("id").and_then(Value::as_str).map(str::to_string);
    s.model = v.get("model").and_then(Value::as_str).map(str::to_string);
    s.stop_reason = v.get("stop_reason").and_then(Value::as_str).map(str::to_string);
    if let Some(u) = v.get("usage") {
        s.usage = usage_from_value(u);
        (s.service_tier, s.speed) = usage_tier(u);
    }
    if let Some(blocks) = v.get("content").and_then(Value::as_array) {
        s.saw_content = !blocks.is_empty();
        for b in blocks {
            if b.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(n) = b.get("name").and_then(Value::as_str)
            {
                s.tools.push(n.to_string());
            }
        }
    }
    s
}

async fn capture(dir: Option<PathBuf>, name: String, data: Bytes) {
    let Some(dir) = dir else { return };
    let _ = tokio::task::spawn_blocking(move || {
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join(name), &data)
    })
    .await;
}

pub async fn handle(req: hyper::Request<Incoming>, px: Arc<Proxy>) -> Result<Response<Body>, Infallible> {
    let started = Instant::now();
    let ts = Utc::now();
    let (parts, body) = req.into_parts();
    let body = match body.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return Ok(json_response(400, &format!(r#"{{"error":"reading request: {e}"}}"#))),
    };
    let path_q = parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_string();
    let is_messages = parts.method == Method::POST && parts.uri.path().ends_with("/v1/messages");

    let parsed: Option<Value> = if is_messages { serde_json::from_slice(&body).ok() } else { None };
    let shape = parsed.as_ref().map(|v| shape_of(v, &parts.headers, body.len()));
    let session = session_from(&parts.headers, parsed.as_ref());
    let model = parsed
        .as_ref()
        .and_then(|v| v.get("model").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let effort = parsed
        .as_ref()
        .and_then(|v| v.pointer("/output_config/effort").and_then(Value::as_str))
        .map(str::to_string);

    let uri: Uri = match format!("{}{}", px.upstream, path_q).parse() {
        Ok(u) => u,
        Err(e) => return Ok(json_response(500, &format!(r#"{{"error":"bad upstream uri: {e}"}}"#))),
    };
    let mut builder = hyper::Request::builder().method(parts.method.clone()).uri(uri);
    for (k, v) in &parts.headers {
        if !skip_request_header(k) {
            builder = builder.header(k, v);
        }
    }
    // Ask for an uncompressed stream so the tee can read it; loopback bandwidth is free.
    builder = builder.header(header::ACCEPT_ENCODING, "identity");
    let up_req = match builder.body(Full::new(body.clone())) {
        Ok(r) => r,
        Err(e) => return Ok(json_response(500, &format!(r#"{{"error":"{e}"}}"#))),
    };

    let resp = match px.client.request(up_req).await {
        Ok(r) => r,
        Err(e) => {
            px.sink
                .incident(Incident {
                    ts,
                    session_id: session,
                    kind: "proxy_upstream".into(),
                    message: e.to_string(),
                    resets_at: None,
                    http_status: Some(502),
                })
                .await;
            px.sink.health(CollectorHealth::Error(format!("upstream: {e}")));
            return Ok(json_response(
                502,
                &format!(r#"{{"type":"error","error":{{"type":"api_error","message":"clankmon proxy: upstream unreachable: {e}"}}}}"#),
            ));
        }
    };
    px.sink.health(CollectorHealth::Ok);

    let status = resp.status();
    let rheaders = resp.headers().clone();
    let request_id = rheaders.get("request-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    let limits = headers::snapshot(&rheaders, Utc::now(), session.clone());
    if let Some(l) = &limits {
        px.sink.limits(l.clone()).await;
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry = rheaders.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<i64>().ok());
        px.sink
            .incident(Incident {
                ts,
                session_id: session.clone(),
                kind: limits
                    .as_ref()
                    .and_then(|l| l.representative_claim.clone())
                    .unwrap_or_else(|| "rate_limited".into()),
                message: format!("HTTP 429 on {}", parts.uri.path()),
                resets_at: retry.map(|r| ts + chrono::Duration::seconds(r)),
                http_status: Some(429),
            })
            .await;
    }

    let is_sse = rheaders
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("text/event-stream"));

    let mut shape = shape;
    if let Some(s) = shape.as_mut() {
        s.limit_headers = headers::interesting_headers(&rheaders);
    }
    let pending = Pending {
        ts,
        started,
        model,
        session,
        effort,
        shape,
        status: status.as_u16(),
        request_id: request_id.clone(),
    };
    let cap_name = request_id.clone().unwrap_or_else(|| format!("{}", ts.timestamp_millis()));
    if is_messages {
        capture(px.capture_dir.clone(), format!("{cap_name}.request.json"), body).await;
    }

    let (rparts, rbody) = resp.into_parts();
    let mut out = Response::builder().status(rparts.status).version(http::Version::HTTP_11);
    for (k, v) in &rparts.headers {
        if !skip_response_header(k) {
            out = out.header(k, v);
        }
    }

    if is_sse {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, hyper::Error>>(64);
        let px2 = px.clone();
        let limits2 = limits.clone();
        tokio::spawn(async move {
            let mut rbody = rbody;
            let mut parser = SseParser::default();
            let mut ttft: Option<u64> = None;
            let mut first_byte: Option<u64> = None;
            let mut captured = px2.capture_dir.as_ref().map(|_| Vec::new());
            while let Some(frame) = rbody.frame().await {
                match frame {
                    Ok(frame) => {
                        if let Some(d) = frame.data_ref() {
                            first_byte.get_or_insert_with(|| pending.started.elapsed().as_millis() as u64);
                            if parser.feed(d) {
                                ttft = Some(pending.started.elapsed().as_millis() as u64);
                            }
                            if let Some(c) = captured.as_mut() {
                                c.extend_from_slice(d);
                            }
                        }
                        if tx.send(Ok(frame)).await.is_err() {
                            // Client went away: dropping the upstream body cancels the request.
                            parser.summary.error.get_or_insert_with(|| "client disconnected".into());
                            break;
                        }
                    }
                    Err(e) => {
                        parser.summary.error.get_or_insert_with(|| e.to_string());
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                }
            }
            drop(tx);
            if is_messages {
                let r = pending.finish(&parser.summary, ttft.or(first_byte));
                if let Some(c) = captured {
                    capture(px2.capture_dir.clone(), format!("{cap_name}.response.sse"), Bytes::from(c)).await;
                }
                px2.sink.request(r, limits2.as_ref()).await;
            }
        });
        let stream = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|x| (x, rx)) });
        let body: Body = BodyExt::boxed(StreamBody::new(stream));
        return Ok(out.body(body).unwrap_or_else(|_| json_response(500, "{}")));
    }

    let bytes = match rbody.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return Ok(json_response(502, &format!(r#"{{"error":"reading upstream: {e}"}}"#))),
    };
    if is_messages {
        let summary = serde_json::from_slice::<Value>(&bytes).map(|v| summary_from_json(&v)).unwrap_or_default();
        let ttfb = pending.started.elapsed().as_millis() as u64;
        let r = pending.finish(&summary, Some(ttfb));
        capture(px.capture_dir.clone(), format!("{cap_name}.response.json"), bytes.clone()).await;
        px.sink.request(r, limits.as_ref()).await;
    }
    let mut resp = out.body(full(bytes)).unwrap_or_else(|_| json_response(500, "{}"));
    resp.headers_mut().remove(header::CONTENT_LENGTH);
    Ok(resp)
}

/// Bind and serve on loopback.
pub async fn serve(port: u16, px: Arc<Proxy>) -> std::io::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            px.sink.health(CollectorHealth::Error(format!("bind :{port}: {e}")));
            return Err(e);
        }
    };
    px.sink.health(CollectorHealth::Listening);
    serve_listener(listener, px).await
}

pub async fn serve_listener(listener: tokio::net::TcpListener, px: Arc<Proxy>) -> std::io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let _ = stream.set_nodelay(true);
        let px = px.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req| handle(req, px.clone()));
            let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), svc)
                .await;
        });
    }
}

/// One colourful line per request for the standalone proxy's stdout.
fn log_line(r: &Request, l: Option<&LimitSnapshot>) -> String {
    const C: &str = "\x1b[38;2;0;229;255m";
    const M: &str = "\x1b[38;2;255;43;214m";
    const Y: &str = "\x1b[38;2;244;255;90m";
    const D: &str = "\x1b[38;2;124;129;154m";
    const R: &str = "\x1b[38;2;255;59;59m";
    const X: &str = "\x1b[0m";
    let t = r.ts.with_timezone(&chrono::Local).format("%H:%M:%S");
    let model = crate::pricing::pretty_model(&r.model);
    let k = crate::ui::fmt::si;
    let mut s = format!(
        "{D}{t}{X} {M}▸{X} {C}{model:<10}{X} in {} {D}cr{X} {} {D}cw{X} {} {Y}out {}{X}",
        k(r.usage.input),
        k(r.usage.cache_read),
        k(r.usage.cache_write()),
        k(r.usage.output)
    );
    if let Some(t) = r.ttft_ms {
        s.push_str(&format!(" {D}ttft{X} {t}ms"));
    }
    if let Some(d) = r.duration_ms {
        s.push_str(&format!(" {D}dur{X} {:.1}s", d as f64 / 1000.0));
    }
    if let Some(l) = l {
        if let Some(w) = &l.five_hour {
            s.push_str(&format!(" {M}5h{X} {:.0}%", w.used_pct));
        }
        if let Some(w) = &l.seven_day {
            s.push_str(&format!(" {M}7d{X} {:.0}%", w.used_pct));
        }
    }
    match &r.error {
        Some(e) => s.push_str(&format!(" {R}✗ {e}{X}")),
        None => s.push_str(&format!(" {C}✓{X} {D}{}{X}", r.stop_reason.as_deref().unwrap_or(""))),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn session_from_metadata_user_id() {
        let body: Value = serde_json::json!({"metadata": {"user_id": "user_abc_account_123_session_9f1c-77"}});
        assert_eq!(session_from(&HeaderMap::new(), Some(&body)).as_deref(), Some("9f1c-77"));
        let json_uid: Value = serde_json::json!({"metadata": {"user_id": "{\"device_id\":\"d\",\"session_id\":\"s-42\"}"}});
        assert_eq!(session_from(&HeaderMap::new(), Some(&json_uid)).as_deref(), Some("s-42"));
        let mut h = HeaderMap::new();
        h.insert("x-claude-code-session-id", HeaderValue::from_static("hdr-1"));
        assert_eq!(session_from(&h, None).as_deref(), Some("hdr-1"));
    }

    #[test]
    fn shape_counts() {
        let body: Value = serde_json::json!({
            "model": "claude-opus-5-5", "max_tokens": 64000, "stream": true,
            "system": [{"type": "text", "text": "You are Claude Code"}],
            "tools": [{"name": "a"}, {"name": "b"}],
            "messages": [{"role": "user", "content": "hi"}],
            "thinking": {"type": "adaptive"}
        });
        let mut h = HeaderMap::new();
        h.insert("anthropic-beta", HeaderValue::from_static("oauth-2025-04-20, fast-mode-2026-02-01"));
        let s = shape_of(&body, &h, 1234);
        assert_eq!(s.tool_count, 2);
        assert_eq!(s.max_tokens, Some(64000));
        assert!(s.stream);
        assert_eq!(s.betas, vec!["oauth-2025-04-20", "fast-mode-2026-02-01"]);
        assert_eq!(s.thinking.as_deref(), Some("adaptive"));
        assert!(s.system_bytes > 10);
    }

    #[test]
    fn json_summary_and_errors() {
        let ok = summary_from_json(&serde_json::json!({
            "id": "msg_1", "model": "claude-haiku-4-5", "stop_reason": "end_turn",
            "content": [{"type": "tool_use", "name": "Read"}],
            "usage": {"input_tokens": 5, "output_tokens": 7}
        }));
        assert_eq!(ok.usage.output, 7);
        assert_eq!(ok.tools, vec!["Read"]);
        let err = summary_from_json(&serde_json::json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}}));
        assert_eq!(err.error.as_deref(), Some("rate_limit_error: slow down"));
    }
}
