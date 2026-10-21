//! A minimal OTLP/HTTP JSON receiver for Claude Code's telemetry.
//!
//! Claude Code exports with `OTEL_EXPORTER_OTLP_PROTOCOL=http/json` to
//! `POST /v1/logs` (events such as `api_request`, `tool_result`, `api_error`)
//! and `POST /v1/metrics`. Only the JSON encoding is supported, which avoids a
//! protobuf dependency.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request as HttpRequest, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::model::{
    Activity, AppEvent, Collector, CollectorHealth, Incident, Request, SessionState, Sources,
    ToolCall, Usage,
};
use crate::net::{Body, json_response};

/// Flatten OTLP `attributes: [{key, value: {stringValue|intValue|doubleValue|boolValue}}]`.
fn attrs(list: Option<&Value>) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    let Some(arr) = list.and_then(Value::as_array) else {
        return out;
    };
    for kv in arr {
        let Some(k) = kv.get("key").and_then(Value::as_str) else { continue };
        let Some(v) = kv.get("value") else { continue };
        let val = if let Some(s) = v.get("stringValue") {
            s.clone()
        } else if let Some(i) = v.get("intValue") {
            // proto3 JSON encodes int64 as a string.
            match i {
                Value::String(s) => s.parse::<i64>().map(Value::from).unwrap_or(i.clone()),
                other => other.clone(),
            }
        } else if let Some(d) = v.get("doubleValue") {
            d.clone()
        } else if let Some(b) = v.get("boolValue") {
            b.clone()
        } else {
            continue;
        };
        out.insert(k.to_string(), val);
    }
    out
}

fn a_str(m: &HashMap<String, Value>, k: &str) -> Option<String> {
    m.get(k).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

fn a_u64(m: &HashMap<String, Value>, k: &str) -> Option<u64> {
    m.get(k).and_then(|v| match v {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)),
        Value::String(s) => s.parse::<f64>().ok().map(|f| f.max(0.0) as u64),
        _ => None,
    })
}

fn nanos_to_ts(v: Option<&Value>) -> Option<DateTime<Utc>> {
    let n: i64 = match v? {
        Value::String(s) => s.parse().ok()?,
        Value::Number(n) => n.as_i64()?,
        _ => return None,
    };
    (n > 0).then(|| DateTime::from_timestamp_nanos(n))
}

/// Map an `ExportLogsServiceRequest` JSON body to app events.
pub fn logs_to_events(body: &Value) -> Vec<AppEvent> {
    let mut out = Vec::new();
    let Some(rls) = body.get("resourceLogs").and_then(Value::as_array) else {
        return out;
    };
    for rl in rls {
        let res = attrs(rl.pointer("/resource/attributes"));
        let Some(sls) = rl.get("scopeLogs").and_then(Value::as_array) else { continue };
        for sl in sls {
            let Some(recs) = sl.get("logRecords").and_then(Value::as_array) else { continue };
            for rec in recs {
                let mut a = res.clone();
                a.extend(attrs(rec.get("attributes")));
                let name = a_str(&a, "event.name")
                    .or_else(|| rec.pointer("/body/stringValue").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                let name = name.strip_prefix("claude_code.").unwrap_or(&name).to_string();
                let ts = a_str(&a, "event.timestamp")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc)))
                    .or_else(|| nanos_to_ts(rec.get("timeUnixNano")))
                    .or_else(|| nanos_to_ts(rec.get("observedTimeUnixNano")))
                    .unwrap_or_else(Utc::now);
                event_to_app(&name, &a, ts, &mut out);
            }
        }
    }
    out
}

fn event_to_app(name: &str, a: &HashMap<String, Value>, ts: DateTime<Utc>, out: &mut Vec<AppEvent>) {
    let session = a_str(a, "session.id");
    match name {
        "api_request" => {
            let key = a_str(a, "request_id")
                .or_else(|| a_str(a, "client_request_id"))
                .unwrap_or_else(|| format!("otel-{}", ts.timestamp_nanos_opt().unwrap_or(0)));
            let usage = Usage {
                input: a_u64(a, "input_tokens").unwrap_or(0),
                output: a_u64(a, "output_tokens").unwrap_or(0),
                cache_read: a_u64(a, "cache_read_tokens").unwrap_or(0),
                cache_write_5m: a_u64(a, "cache_creation_tokens").unwrap_or(0),
                ..Default::default()
            };
            // OTel timestamps mark the end of the request; back-date by its duration
            // so it lines up with the transcript's start-of-response timestamp.
            let duration = a_u64(a, "duration_ms");
            out.push(AppEvent::Request(Box::new(Request {
                key,
                request_id: a_str(a, "request_id"),
                ts,
                session_id: session,
                model: a_str(a, "model").unwrap_or_default(),
                usage,
                duration_ms: duration,
                ttft_ms: a_u64(a, "ttft_ms"),
                http_status: Some(200),
                effort: a_str(a, "effort"),
                speed: a_str(a, "speed"),
                query_source: a_str(a, "query_source"),
                sidechain: a_str(a, "query_source").is_some_and(|q| q != "repl_main_thread" && q != "compact"),
                sources: Sources(Sources::OTEL),
                ..Default::default()
            })));
        }
        "api_error" => {
            out.push(AppEvent::Incident(Incident {
                ts,
                session_id: session,
                kind: "api_error".into(),
                message: a_str(a, "error").unwrap_or_else(|| "API error".into()),
                resets_at: None,
                http_status: a_u64(a, "status_code").map(|s| s as u16),
            }));
        }
        "tool_result" => {
            out.push(AppEvent::Tool(ToolCall {
                session_id: session,
                ts,
                name: a_str(a, "tool_name").unwrap_or_default(),
                success: a_str(a, "success").map(|s| s == "true"),
                duration_ms: a_u64(a, "duration_ms"),
                tool_use_id: a_str(a, "tool_use_id"),
            }));
        }
        "user_prompt" => {
            if let Some(sid) = session {
                out.push(AppEvent::Activity(Activity {
                    session_id: sid,
                    ts,
                    state: SessionState::Thinking,
                    cwd: None,
                    branch: None,
                    version: a_str(a, "app.version"),
                    model: None,
                    effort: None,
                    turns: 0,
                    compactions: 0,
                }));
            }
        }
        _ => {}
    }
}

async fn handle(req: HttpRequest<Incoming>, tx: UnboundedSender<AppEvent>) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path().to_string();
    if req.method() != Method::POST {
        return Ok(json_response(405, r#"{"error":"POST only"}"#));
    }
    let ctype = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if ctype.contains("protobuf") {
        let _ = tx.send(AppEvent::Collector(
            Collector::Otel,
            CollectorHealth::Error("protobuf received; set OTEL_EXPORTER_OTLP_PROTOCOL=http/json".into()),
        ));
        return Ok(json_response(415, r#"{"error":"clankmon accepts OTLP http/json only"}"#));
    }
    let Ok(body) = req.into_body().collect().await.map(|b| b.to_bytes()) else {
        return Ok(json_response(400, r#"{"error":"bad body"}"#));
    };
    let Ok(v) = serde_json::from_slice::<Value>(&body) else {
        return Ok(json_response(400, r#"{"error":"invalid json"}"#));
    };
    let mut events = match path.as_str() {
        "/v1/logs" => logs_to_events(&v),
        "/v1/metrics" | "/v1/traces" => Vec::new(),
        _ => return Ok(json_response(404, r#"{"error":"not found"}"#)),
    };
    events.push(AppEvent::Collector(Collector::Otel, CollectorHealth::Ok));
    let _ = tx.send(AppEvent::Batch(events));
    Ok(json_response(200, "{}"))
}

/// Serve on loopback until the channel closes.
pub async fn run(port: u16, tx: UnboundedSender<AppEvent>) {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            let _ = tx.send(AppEvent::Collector(Collector::Otel, CollectorHealth::Error(format!("bind :{port}: {e}"))));
            return;
        }
    };
    let _ = tx.send(AppEvent::Collector(Collector::Otel, CollectorHealth::Listening));
    let tx = Arc::new(tx);
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let tx = tx.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req| handle(req, (*tx).clone()));
            let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), svc)
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn api_request_event_maps_to_request() {
        let body = json!({"resourceLogs": [{
            "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "claude-code"}}]},
            "scopeLogs": [{"logRecords": [{
                "timeUnixNano": "1792600000000000000",
                "body": {"stringValue": "claude_code.api_request"},
                "attributes": [
                    {"key": "event.name", "value": {"stringValue": "api_request"}},
                    {"key": "session.id", "value": {"stringValue": "s1"}},
                    {"key": "model", "value": {"stringValue": "claude-opus-5-5"}},
                    {"key": "input_tokens", "value": {"intValue": "12"}},
                    {"key": "output_tokens", "value": {"intValue": 340}},
                    {"key": "cache_read_tokens", "value": {"intValue": "9000"}},
                    {"key": "cache_creation_tokens", "value": {"intValue": "500"}},
                    {"key": "duration_ms", "value": {"doubleValue": 2310.5}},
                    {"key": "request_id", "value": {"stringValue": "req_abc"}},
                    {"key": "query_source", "value": {"stringValue": "repl_main_thread"}},
                    {"key": "cost_usd", "value": {"doubleValue": 0.01}}
                ]
            }]}]
        }]});
        let ev = logs_to_events(&body);
        let AppEvent::Request(r) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(r.key, "req_abc");
        assert_eq!(r.session_id.as_deref(), Some("s1"));
        assert_eq!(r.usage.input, 12);
        assert_eq!(r.usage.output, 340);
        assert_eq!(r.usage.cache_read, 9000);
        assert_eq!(r.duration_ms, Some(2310));
        assert!(!r.sidechain);
        assert_eq!(r.ts.timestamp(), 1_792_600_000);
    }

    #[test]
    fn tool_result_event() {
        let body = json!({"resourceLogs": [{"scopeLogs": [{"logRecords": [{
            "attributes": [
                {"key": "event.name", "value": {"stringValue": "tool_result"}},
                {"key": "tool_name", "value": {"stringValue": "Bash"}},
                {"key": "success", "value": {"stringValue": "false"}},
                {"key": "tool_use_id", "value": {"stringValue": "t1"}}
            ]}]}]}]});
        let ev = logs_to_events(&body);
        assert!(matches!(&ev[0], AppEvent::Tool(t) if t.name == "Bash" && t.success == Some(false)));
    }

    #[test]
    fn empty_or_foreign_bodies_are_ignored() {
        assert!(logs_to_events(&json!({})).is_empty());
        assert!(logs_to_events(&json!({"resourceLogs": [{"scopeLogs": [{"logRecords": [{"attributes": []}]}]}]})).is_empty());
    }
}
