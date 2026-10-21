//! Shared hyper plumbing: an HTTPS client and body helpers.

use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;

pub type Body = BoxBody<Bytes, hyper::Error>;
pub type HttpsClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

/// Install ring as the process-wide rustls provider (idempotent).
pub fn init_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn https_client() -> HttpsClient {
    init_crypto();
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    Client::builder(TokioExecutor::new()).pool_idle_timeout(std::time::Duration::from_secs(90)).build(https)
}

pub fn full(b: impl Into<Bytes>) -> Body {
    Full::new(b.into()).map_err(|never| match never {}).boxed()
}

pub fn json_response(status: u16, body: &str) -> hyper::Response<Body> {
    hyper::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(full(body.to_string()))
        .expect("static response")
}
