//! The OTLP http/json receiver over real HTTP.

use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

use clankmon::collect::otel;
use clankmon::model::{AppEvent, Collector, CollectorHealth};

async fn post(port: u16, path: &str, ctype: &str, body: &str) -> u16 {
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let req = Request::post(format!("http://127.0.0.1:{port}{path}"))
        .header("content-type", ctype)
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap();
    let resp = client.request(req).await.unwrap();
    let s = resp.status().as_u16();
    let _ = resp.into_body().collect().await;
    s
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[tokio::test]
async fn receives_api_request_events() {
    let port = free_port();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(otel::run(port, tx));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let logs = r#"{"resourceLogs":[{"resource":{"attributes":[{"key":"session.id","value":{"stringValue":"s-otel"}}]},
      "scopeLogs":[{"logRecords":[{"timeUnixNano":"1792600000000000000",
        "attributes":[{"key":"event.name","value":{"stringValue":"api_request"}},
                      {"key":"model","value":{"stringValue":"claude-sonnet-5-5"}},
                      {"key":"input_tokens","value":{"intValue":"10"}},
                      {"key":"output_tokens","value":{"intValue":"20"}},
                      {"key":"request_id","value":{"stringValue":"req_otel"}}]}]}]}]}"#;
    assert_eq!(post(port, "/v1/logs", "application/json", logs).await, 200);
    assert_eq!(post(port, "/v1/metrics", "application/json", r#"{"resourceMetrics":[]}"#).await, 200);
    assert_eq!(post(port, "/v1/logs", "application/x-protobuf", "\x0a\x00").await, 415);
    assert_eq!(post(port, "/nope", "application/json", "{}").await, 404);

    let mut got_request = false;
    let mut got_protobuf_error = false;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
        match ev {
            AppEvent::Batch(evs) => {
                for e in evs {
                    if let AppEvent::Request(r) = e {
                        assert_eq!(r.key, "req_otel");
                        assert_eq!(r.session_id.as_deref(), Some("s-otel"));
                        assert_eq!(r.usage.output, 20);
                        got_request = true;
                    }
                }
            }
            AppEvent::Collector(Collector::Otel, CollectorHealth::Error(e)) => {
                got_protobuf_error = e.contains("http/json");
            }
            _ => {}
        }
        if got_request && got_protobuf_error {
            break;
        }
    }
    assert!(got_request);
    assert!(got_protobuf_error);
}
