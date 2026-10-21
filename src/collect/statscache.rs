//! Cold-start history from `~/.claude/stats-cache.json`.

use std::path::Path;

use chrono::NaiveDate;
use serde::Deserialize;

use crate::model::{StatsCache, StatsDay};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    #[serde(default)]
    daily_activity: Vec<Activity>,
    #[serde(default)]
    daily_model_tokens: Vec<ModelTokens>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Activity {
    date: String,
    #[serde(default)]
    message_count: u64,
    #[serde(default)]
    session_count: u64,
    #[serde(default)]
    tool_call_count: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelTokens {
    date: String,
    #[serde(default)]
    tokens_by_model: std::collections::BTreeMap<String, u64>,
}

pub fn parse(text: &str) -> Option<StatsCache> {
    let f: File = serde_json::from_str(text).ok()?;
    let mut out = StatsCache::default();
    for a in f.daily_activity {
        let Ok(d) = NaiveDate::parse_from_str(&a.date, "%Y-%m-%d") else { continue };
        let day = out.days.entry(d).or_insert_with(StatsDay::default);
        day.messages = a.message_count;
        day.sessions = a.session_count;
        day.tool_calls = a.tool_call_count;
    }
    for t in f.daily_model_tokens {
        let Ok(d) = NaiveDate::parse_from_str(&t.date, "%Y-%m-%d") else { continue };
        out.days.entry(d).or_default().tokens_by_model = t.tokens_by_model;
    }
    Some(out)
}

pub fn load(path: &Path) -> Option<StatsCache> {
    parse(&std::fs::read_to_string(path).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_activity_and_tokens() {
        let s = parse(
            r#"{"version":5,"lastComputedDate":"2026-10-12",
            "dailyActivity":[{"date":"2026-10-11","messageCount":120,"sessionCount":4,"toolCallCount":60}],
            "dailyModelTokens":[{"date":"2026-10-11","tokensByModel":{"claude-opus-5-5":1000}}],
            "modelUsage":{}}"#,
        )
        .unwrap();
        let d = &s.days[&NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()];
        assert_eq!(d.messages, 120);
        assert_eq!(d.sessions, 4);
        assert_eq!(d.tokens_by_model["claude-opus-5-5"], 1000);
    }
}
