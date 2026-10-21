//! Rate-limit response headers.
//!
//! Subscription (OAuth) traffic carries the undocumented unified headers:
//! `anthropic-ratelimit-unified-{5h,7d,...}-{utilization,reset,status}` where
//! utilization is a 0..1 fraction and reset is epoch seconds.
//! API-key traffic carries the documented
//! `anthropic-ratelimit-{requests,tokens,input-tokens,output-tokens}-{limit,remaining,reset}`
//! with RFC 3339 resets.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use http::HeaderMap;

use crate::model::{LimitSnapshot, LimitSource, WindowReading};

const UNIFIED: &str = "anthropic-ratelimit-unified-";
const STANDARD: &str = "anthropic-ratelimit-";

fn hv<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

fn parse_reset(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(n) = s.trim().parse::<f64>() {
        return DateTime::from_timestamp(n as i64, 0);
    }
    DateTime::parse_from_rfc3339(s.trim()).ok().map(|d| d.with_timezone(&Utc))
}

/// Raw headers worth showing in the LIMITS view.
pub fn interesting_headers(h: &HeaderMap) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = h
        .iter()
        .filter(|(k, _)| {
            let k = k.as_str();
            k.starts_with("anthropic-ratelimit")
                || k.starts_with("anthropic-priority")
                || k == "retry-after"
                || k == "request-id"
                || k == "anthropic-organization-id"
                || k == "anthropic-workspace-id"
        })
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    out.sort();
    out
}

/// Build a snapshot from response headers, if any rate-limit headers are present.
pub fn snapshot(h: &HeaderMap, ts: DateTime<Utc>, session_id: Option<String>) -> Option<LimitSnapshot> {
    let mut five = None;
    let mut seven = None;
    let mut extra = BTreeMap::new();

    // Unified windows: discover every `<window>-utilization` header.
    for (k, v) in h {
        let k = k.as_str();
        let Some(rest) = k.strip_prefix(UNIFIED) else { continue };
        let Some(window) = rest.strip_suffix("-utilization") else { continue };
        let Ok(util) = v.to_str().unwrap_or("").trim().parse::<f64>() else { continue };
        let reading = WindowReading {
            used_pct: util * 100.0,
            resets_at: hv(h, &format!("{UNIFIED}{window}-reset")).and_then(parse_reset),
            status: hv(h, &format!("{UNIFIED}{window}-status")).map(str::to_string),
        };
        match window {
            "5h" => five = Some(reading),
            "7d" => seven = Some(reading),
            other => {
                extra.insert(format!("unified:{other}"), reading);
            }
        }
    }

    // Documented API-key buckets.
    for bucket in ["requests", "tokens", "input-tokens", "output-tokens"] {
        let limit = hv(h, &format!("{STANDARD}{bucket}-limit")).and_then(|s| s.trim().parse::<f64>().ok());
        let remaining = hv(h, &format!("{STANDARD}{bucket}-remaining")).and_then(|s| s.trim().parse::<f64>().ok());
        if let (Some(l), Some(r)) = (limit, remaining)
            && l > 0.0
        {
            extra.insert(
                format!("api:{bucket}"),
                WindowReading {
                    used_pct: ((l - r) / l * 100.0).clamp(0.0, 100.0),
                    resets_at: hv(h, &format!("{STANDARD}{bucket}-reset")).and_then(parse_reset),
                    status: Some(format!("{r:.0}/{l:.0} left")),
                },
            );
        }
    }

    if five.is_none() && seven.is_none() && extra.is_empty() {
        return None;
    }
    if let Some(fb) = hv(h, &format!("{UNIFIED}fallback-percentage")).and_then(|s| s.trim().parse::<f64>().ok()) {
        extra.insert(
            "unified:fallback".into(),
            WindowReading { used_pct: fb * 100.0, resets_at: None, status: Some("throughput when limited".into()) },
        );
    }
    Some(LimitSnapshot {
        ts,
        source: LimitSource::Proxy,
        five_hour: five,
        seven_day: seven,
        extra,
        overall_status: hv(h, &format!("{UNIFIED}status")).map(str::to_string),
        representative_claim: hv(h, &format!("{UNIFIED}representative-claim")).map(str::to_string),
        overage_status: hv(h, &format!("{UNIFIED}overage-status")).map(str::to_string),
        session_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn unified_subscription_headers() {
        let h = headers(&[
            ("anthropic-ratelimit-unified-5h-utilization", "0.42"),
            ("anthropic-ratelimit-unified-5h-reset", "1792620000"),
            ("anthropic-ratelimit-unified-5h-status", "allowed"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.913"),
            ("anthropic-ratelimit-unified-7d-reset", "1792900000"),
            ("anthropic-ratelimit-unified-7d-status", "allowed_warning"),
            ("anthropic-ratelimit-unified-status", "allowed_warning"),
            ("anthropic-ratelimit-unified-representative-claim", "seven_day"),
            ("anthropic-ratelimit-unified-fallback-percentage", "0.5"),
            ("request-id", "req_123"),
        ]);
        let s = snapshot(&h, Utc::now(), None).unwrap();
        let f = s.five_hour.unwrap();
        assert!((f.used_pct - 42.0).abs() < 1e-9);
        assert_eq!(f.resets_at.unwrap().timestamp(), 1792620000);
        assert_eq!(f.status.as_deref(), Some("allowed"));
        assert!((s.seven_day.unwrap().used_pct - 91.3).abs() < 1e-9);
        assert_eq!(s.representative_claim.as_deref(), Some("seven_day"));
        assert_eq!(s.extra["unified:fallback"].used_pct, 50.0);
        assert_eq!(interesting_headers(&h).len(), 10);
    }

    #[test]
    fn api_key_headers() {
        let h = headers(&[
            ("anthropic-ratelimit-requests-limit", "4000"),
            ("anthropic-ratelimit-requests-remaining", "3000"),
            ("anthropic-ratelimit-requests-reset", "2026-10-21T19:13:00Z"),
            ("anthropic-ratelimit-output-tokens-limit", "400000"),
            ("anthropic-ratelimit-output-tokens-remaining", "396000"),
        ]);
        let s = snapshot(&h, Utc::now(), None).unwrap();
        assert!(s.five_hour.is_none());
        assert_eq!(s.extra["api:requests"].used_pct, 25.0);
        assert!(s.extra["api:requests"].resets_at.is_some());
        assert!((s.extra["api:output-tokens"].used_pct - 1.0).abs() < 1e-9);
    }

    #[test]
    fn no_headers_no_snapshot() {
        assert!(snapshot(&HeaderMap::new(), Utc::now(), None).is_none());
    }
}
