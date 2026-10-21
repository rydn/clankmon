//! Claude Code's OAuth credentials (plan detection) and the opt-in poller for
//! the undocumented `/api/oauth/usage` endpoint behind `/usage`.
//!
//! The endpoint 429s aggressively (anthropics/claude-code#31637), so polling is
//! slow, backs off on errors, and never refreshes the token: Claude Code owns it.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use http_body_util::{BodyExt, Full};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::model::{AppEvent, Collector, CollectorHealth, LimitSnapshot, LimitSource, OauthExtra, WindowReading};
use crate::net::https_client;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

#[derive(Debug, Clone, Default)]
pub struct Credentials {
    pub access_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub subscription: Option<String>,
    pub tier: Option<String>,
}

fn parse_credentials(text: &str) -> Option<Credentials> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let o = v.get("claudeAiOauth")?;
    Some(Credentials {
        access_token: o.get("accessToken").and_then(Value::as_str).map(str::to_string),
        expires_at: o.get("expiresAt").and_then(Value::as_i64).and_then(DateTime::from_timestamp_millis),
        subscription: o.get("subscriptionType").and_then(Value::as_str).map(str::to_string),
        tier: o.get("rateLimitTier").and_then(Value::as_str).map(str::to_string),
    })
}

/// Read credentials from the macOS Keychain, falling back to `~/.claude/.credentials.json`.
pub fn read_credentials(file: &Path) -> Option<Credentials> {
    if cfg!(target_os = "macos")
        && let Ok(out) = std::process::Command::new("security")
            .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
            .output()
        && out.status.success()
        && let Some(c) = parse_credentials(&String::from_utf8_lossy(&out.stdout))
    {
        return Some(c);
    }
    parse_credentials(&std::fs::read_to_string(file).ok()?)
}

/// Turn a usage response into a limit snapshot plus extras.
pub fn parse_usage(v: &Value, ts: DateTime<Utc>) -> (LimitSnapshot, Option<Value>) {
    let mut five = None;
    let mut seven = None;
    let mut extra = BTreeMap::new();
    if let Some(obj) = v.as_object() {
        for (k, w) in obj {
            let Some(util) = w.get("utilization").and_then(Value::as_f64) else { continue };
            let reading = WindowReading {
                used_pct: util,
                resets_at: w
                    .get("resets_at")
                    .and_then(Value::as_str)
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc)),
                status: None,
            };
            match k.as_str() {
                "five_hour" => five = Some(reading),
                "seven_day" => seven = Some(reading),
                "extra_usage" => {}
                other => {
                    extra.insert(other.to_string(), reading);
                }
            }
        }
    }
    let snap = LimitSnapshot {
        ts,
        source: LimitSource::OAuth,
        five_hour: five,
        seven_day: seven,
        extra,
        overall_status: None,
        representative_claim: None,
        overage_status: None,
        session_id: None,
    };
    (snap, v.get("extra_usage").cloned())
}

async fn fetch(token: &str) -> Result<Value, (Option<u16>, String)> {
    let client = https_client();
    let req = hyper::Request::get(USAGE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("accept", "application/json")
        .header("user-agent", concat!("clankmon/", env!("CARGO_PKG_VERSION")))
        .body(Full::new(Bytes::new()))
        .map_err(|e| (None, e.to_string()))?;
    let resp = tokio::time::timeout(Duration::from_secs(15), client.request(req))
        .await
        .map_err(|_| (None, "timeout".to_string()))?
        .map_err(|e| (None, e.to_string()))?;
    let status = resp.status().as_u16();
    let body = resp.into_body().collect().await.map_err(|e| (Some(status), e.to_string()))?.to_bytes();
    if status != 200 {
        let msg = String::from_utf8_lossy(&body).chars().take(200).collect::<String>();
        return Err((Some(status), msg));
    }
    serde_json::from_slice(&body).map_err(|e| (Some(status), e.to_string()))
}

pub async fn run(cred_file: std::path::PathBuf, interval_secs: u64, tx: UnboundedSender<AppEvent>) {
    let base = Duration::from_secs(interval_secs.max(300));
    let mut wait = Duration::from_secs(2);
    let _ = tx.send(AppEvent::Collector(Collector::OAuth, CollectorHealth::Listening));
    loop {
        tokio::time::sleep(wait).await;
        if tx.is_closed() {
            return;
        }
        let file = cred_file.clone();
        let creds = tokio::task::spawn_blocking(move || read_credentials(&file)).await.ok().flatten();
        let Some(token) = creds.as_ref().and_then(|c| c.access_token.clone()) else {
            let _ = tx.send(AppEvent::Collector(
                Collector::OAuth,
                CollectorHealth::Error("no Claude Code OAuth token".into()),
            ));
            wait = base;
            continue;
        };
        if creds.as_ref().and_then(|c| c.expires_at).is_some_and(|e| e < Utc::now()) {
            let _ = tx.send(AppEvent::Collector(
                Collector::OAuth,
                CollectorHealth::Error("token expired; Claude Code refreshes it on next use".into()),
            ));
            wait = base;
            continue;
        }
        let now = Utc::now();
        match fetch(&token).await {
            Ok(v) => {
                let (snap, extra) = parse_usage(&v, now);
                wait = base;
                let _ = tx.send(AppEvent::Batch(vec![
                    AppEvent::Limits(Box::new(snap)),
                    AppEvent::Oauth(OauthExtra {
                        ts: Some(now),
                        extra_usage: extra,
                        error: None,
                        next_poll: Some(now + chrono::Duration::from_std(wait).unwrap_or_default()),
                    }),
                    AppEvent::Collector(Collector::OAuth, CollectorHealth::Ok),
                ]));
            }
            Err((status, msg)) => {
                // Exponential backoff, capped at one hour.
                wait = (wait.max(base) * 2).min(Duration::from_secs(3600));
                let label = match status {
                    Some(429) => "rate limited by /api/oauth/usage".to_string(),
                    Some(s) => format!("HTTP {s}: {msg}"),
                    None => msg,
                };
                let _ = tx.send(AppEvent::Batch(vec![
                    AppEvent::Oauth(OauthExtra {
                        ts: None,
                        extra_usage: None,
                        error: Some(label.clone()),
                        next_poll: Some(now + chrono::Duration::from_std(wait).unwrap_or_default()),
                    }),
                    AppEvent::Collector(Collector::OAuth, CollectorHealth::Error(label)),
                ]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_credentials_without_exposing_more_than_needed() {
        let c = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"r","expiresAt":1792636935681,"scopes":[],"subscriptionType":"pro","rateLimitTier":"default_claude_ai"},"organizationUuid":"x"}"#,
        )
        .unwrap();
        assert_eq!(c.subscription.as_deref(), Some("pro"));
        assert_eq!(c.tier.as_deref(), Some("default_claude_ai"));
        assert_eq!(c.access_token.as_deref(), Some("tok"));
        assert!(c.expires_at.is_some());
    }

    #[test]
    fn parses_usage_windows() {
        let v = json!({
            "five_hour": {"utilization": 23.0, "resets_at": "2026-10-21T22:00:00+00:00"},
            "seven_day": {"utilization": 41.5, "resets_at": "2026-10-25T08:00:00Z"},
            "seven_day_opus": {"utilization": 12.0, "resets_at": null},
            "seven_day_sonnet": null,
            "extra_usage": {"is_enabled": false}
        });
        let (s, extra) = parse_usage(&v, Utc::now());
        assert_eq!(s.five_hour.unwrap().used_pct, 23.0);
        assert_eq!(s.seven_day.as_ref().unwrap().resets_at.unwrap().timestamp(), 1_792_915_200);
        assert_eq!(s.extra["seven_day_opus"].used_pct, 12.0);
        assert!(!s.extra.contains_key("seven_day_sonnet"));
        assert!(extra.is_some());
    }
}
