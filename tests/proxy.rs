//! End-to-end proxy test against a mock Anthropic upstream.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;

use clankmon::model::{AppEvent, LimitSource, Sources};
use clankmon::proxy::{self, Proxy, Sink};

const STREAM: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-haiku-4-5-20251001\",\"usage\":{\"input_tokens\":25,\"cache_read_input_tokens\":9000,\"cache_creation_input_tokens\":120,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"Read\",\"input\":{}}}\n\n",
    "event: message_delta\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":315}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Mock upstream: streams STREAM in uneven chunks, or 429s when asked.
async fn upstream(seen: Seen) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let seen = seen.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let seen = seen.clone();
                    async move {
                        {
                            let mut v = seen.lock().unwrap();
                            for (k, val) in req.headers() {
                                v.push((k.as_str().to_string(), val.to_str().unwrap_or("").to_string()));
                            }
                            v.push((":path".into(), req.uri().path().to_string()));
                        }
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        let want_429 = String::from_utf8_lossy(&body).contains("please-429");
                        if want_429 {
                            return Ok::<_, Infallible>(
                                Response::builder()
                                    .status(429)
                                    .header("content-type", "application/json")
                                    .header("retry-after", "30")
                                    .header("request-id", "req_limited")
                                    .header("anthropic-ratelimit-unified-5h-utilization", "1.0")
                                    .header("anthropic-ratelimit-unified-5h-status", "rate_limited")
                                    .header("anthropic-ratelimit-unified-representative-claim", "five_hour")
                                    .body(
                                        Full::new(Bytes::from_static(br#"{"type":"error","error":{"type":"rate_limit_error","message":"limited"}}"#))
                                            .map_err(|n| match n {})
                                            .boxed(),
                                    )
                                    .unwrap(),
                            );
                        }
                        // Uneven chunks, with small gaps, to exercise reassembly.
                        let bytes = STREAM.as_bytes().to_vec();
                        let chunks: Vec<Bytes> = bytes.chunks(37).map(Bytes::copy_from_slice).collect();
                        let stream = futures::stream::unfold(chunks.into_iter(), |mut it| async move {
                            let c = it.next()?;
                            tokio::time::sleep(Duration::from_millis(2)).await;
                            Some((Ok::<_, Infallible>(Frame::data(c)), it))
                        });
                        Ok(Response::builder()
                            .header("content-type", "text/event-stream")
                            .header("request-id", "req_test_1")
                            .header("anthropic-ratelimit-unified-5h-utilization", "0.42")
                            .header("anthropic-ratelimit-unified-5h-reset", "4102444800")
                            .header("anthropic-ratelimit-unified-5h-status", "allowed")
                            .header("anthropic-ratelimit-unified-7d-utilization", "0.913")
                            .header("anthropic-ratelimit-unified-7d-status", "allowed_warning")
                            .body(StreamBody::new(stream).boxed())
                            .unwrap())
                    }
                });
                let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    port
}

async fn start_proxy(upstream_url: &str) -> (u16, UnboundedReceiver<AppEvent>) {
    clankmon::net::init_crypto();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let px = Proxy::new(upstream_url, Sink::Channel(tx), None);
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(proxy::serve_listener(l, px));
    (port, rx)
}

async fn post(port: u16, body: &str) -> (u16, hyper::HeaderMap, Bytes) {
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let req = Request::post(format!("http://127.0.0.1:{port}/v1/messages?beta=true"))
        .header("authorization", "Bearer sk-ant-oat-test")
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("accept-encoding", "gzip, br")
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap();
    let resp = client.request(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, bytes)
}

async fn collect(rx: &mut UnboundedReceiver<AppEvent>, want: usize) -> Vec<AppEvent> {
    let mut out = Vec::new();
    while out.len() < want {
        match tokio::time::timeout(Duration::from_secs(3), rx.recv()).await {
            Ok(Some(AppEvent::Collector(..))) => continue,
            Ok(Some(e)) => out.push(e),
            _ => break,
        }
    }
    out
}

#[tokio::test]
async fn streams_through_unchanged_and_records_everything() {
    let seen: Seen = Arc::default();
    let up = upstream(seen.clone()).await;
    let (port, mut rx) = start_proxy(&format!("http://127.0.0.1:{up}")).await;

    let body = r#"{"model":"claude-haiku-4-5-20251001","max_tokens":1024,"stream":true,
        "metadata":{"user_id":"user_abc_account_1_session_sess-42"},
        "tools":[{"name":"Read"},{"name":"Bash"}],"system":"You are Claude Code",
        "messages":[{"role":"user","content":"hi"}]}"#;
    let (status, headers, bytes) = post(port, body).await;

    assert_eq!(status, 200);
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), STREAM, "client must receive the exact upstream bytes");
    assert_eq!(headers.get("request-id").unwrap(), "req_test_1");

    // Upstream saw our auth, the path+query, and an identity encoding request.
    let seen = seen.lock().unwrap().clone();
    let get = |k: &str| seen.iter().find(|(h, _)| h == k).map(|(_, v)| v.clone());
    assert_eq!(get("authorization").as_deref(), Some("Bearer sk-ant-oat-test"));
    assert_eq!(get("accept-encoding").as_deref(), Some("identity"));
    assert_eq!(get(":path").as_deref(), Some("/v1/messages"));

    let events = collect(&mut rx, 2).await;
    let limits = events.iter().find_map(|e| match e {
        AppEvent::Limits(l) => Some(l),
        _ => None,
    });
    let l = limits.expect("limit snapshot from headers");
    assert_eq!(l.source, LimitSource::Proxy);
    assert!((l.five_hour.as_ref().unwrap().used_pct - 42.0).abs() < 1e-9);
    assert!((l.seven_day.as_ref().unwrap().used_pct - 91.3).abs() < 1e-9);
    assert_eq!(l.session_id.as_deref(), Some("sess-42"));

    let r = events
        .iter()
        .find_map(|e| match e {
            AppEvent::Request(r) => Some(r),
            _ => None,
        })
        .expect("request record");
    assert_eq!(r.key, "req_test_1");
    assert_eq!(r.session_id.as_deref(), Some("sess-42"));
    assert_eq!(r.model, "claude-haiku-4-5-20251001");
    assert_eq!(r.usage.input, 25);
    assert_eq!(r.usage.cache_read, 9000);
    assert_eq!(r.usage.output, 315);
    assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(r.tools, vec!["Read"]);
    assert_eq!(r.http_status, Some(200));
    assert!(r.ttft_ms.is_some() && r.duration_ms.is_some());
    assert!(r.sources.has(Sources::PROXY));
    let shape = r.shape.as_ref().unwrap();
    assert_eq!(shape.tool_count, 2);
    assert_eq!(shape.max_tokens, Some(1024));
    assert!(shape.stream);
    assert_eq!(shape.betas, vec!["oauth-2025-04-20"]);
    assert!(shape.limit_headers.iter().any(|(k, _)| k == "anthropic-ratelimit-unified-5h-utilization"));
}

#[tokio::test]
async fn rate_limits_become_incidents() {
    let up = upstream(Arc::default()).await;
    let (port, mut rx) = start_proxy(&format!("http://127.0.0.1:{up}")).await;
    let (status, _, bytes) = post(port, r#"{"model":"claude-haiku-4-5","messages":[{"role":"user","content":"please-429"}]}"#).await;
    assert_eq!(status, 429);
    assert!(String::from_utf8_lossy(&bytes).contains("rate_limit_error"));
    let events = collect(&mut rx, 3).await;
    let inc = events.iter().find_map(|e| match e {
        AppEvent::Incident(i) => Some(i),
        _ => None,
    });
    let inc = inc.expect("incident");
    assert_eq!(inc.http_status, Some(429));
    assert_eq!(inc.kind, "five_hour");
    assert!(inc.resets_at.is_some(), "retry-after becomes a reset time");
    let r = events.iter().find_map(|e| match e {
        AppEvent::Request(r) => Some(r),
        _ => None,
    });
    let r = r.expect("request");
    assert_eq!(r.http_status, Some(429));
    assert_eq!(r.error.as_deref(), Some("rate_limit_error: limited"));
}

#[tokio::test]
async fn unreachable_upstream_is_a_clean_502() {
    // Bind and drop to get a port nothing listens on.
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let (port, mut rx) = start_proxy(&format!("http://127.0.0.1:{dead}")).await;
    let (status, _, bytes) = post(port, r#"{"model":"x","messages":[]}"#).await;
    assert_eq!(status, 502);
    assert!(String::from_utf8_lossy(&bytes).contains("upstream unreachable"));
    let events = collect(&mut rx, 1).await;
    assert!(matches!(&events[0], AppEvent::Incident(i) if i.kind == "proxy_upstream"));
}
