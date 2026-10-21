//! Payloads Claude Code pushes to clankmon: statusline JSON and hook JSON.
//! Both arrive through `clankmon statusline` / `clankmon hook` and the IPC socket.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::model::{
    Activity, AppEvent, LimitSnapshot, LimitSource, SessionState, StatuslineInfo, ToolCall, WindowReading,
};

fn s(v: &Value, path: &[&str]) -> Option<String> {
    get(v, path).and_then(|x| x.as_str()).map(str::to_string)
}

fn f(v: &Value, path: &[&str]) -> Option<f64> {
    get(v, path).and_then(Value::as_f64)
}

fn u(v: &Value, path: &[&str]) -> Option<u64> {
    get(v, path).and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
}

fn b(v: &Value, path: &[&str]) -> Option<bool> {
    get(v, path).and_then(Value::as_bool)
}

fn get<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    (!cur.is_null()).then_some(cur)
}

fn window(v: &Value, key: &str) -> Option<WindowReading> {
    let w = v.get(key)?;
    let used = w.get("used_percentage").and_then(Value::as_f64)?;
    Some(WindowReading {
        used_pct: used,
        resets_at: w.get("resets_at").and_then(Value::as_i64).and_then(|t| DateTime::from_timestamp(t, 0)),
        status: None,
    })
}

/// Map a statusline frame to app events.
pub fn statusline_events(v: &Value, ts: DateTime<Utc>) -> Vec<AppEvent> {
    let mut out = Vec::new();
    let Some(session_id) = s(v, &["session_id"]) else {
        return out;
    };

    let info = StatuslineInfo {
        ts,
        session_id: session_id.clone(),
        session_name: s(v, &["session_name"]),
        cwd: s(v, &["workspace", "current_dir"]).or_else(|| s(v, &["cwd"])),
        model_id: s(v, &["model", "id"]),
        model_name: s(v, &["model", "display_name"]),
        version: s(v, &["version"]),
        effort: s(v, &["effort", "level"]),
        fast_mode: b(v, &["fast_mode"]),
        context_used_pct: f(v, &["context_window", "used_percentage"]),
        context_window_size: u(v, &["context_window", "context_window_size"]),
        cost_usd: f(v, &["cost", "total_cost_usd"]),
        duration_ms: u(v, &["cost", "total_duration_ms"]),
        api_duration_ms: u(v, &["cost", "total_api_duration_ms"]),
        lines_added: u(v, &["cost", "total_lines_added"]),
        lines_removed: u(v, &["cost", "total_lines_removed"]),
        cache_warm: b(v, &["prompt_cache", "warm"]),
        cache_hit_ratio: f(v, &["prompt_cache", "hit_ratio"]),
        cache_misses: u(v, &["prompt_cache", "misses"]),
        agent: s(v, &["agent", "name"]),
    };
    out.push(AppEvent::Statusline(Box::new(info)));

    if let Some(rl) = v.get("rate_limits").filter(|x| x.is_object()) {
        let mut extra = BTreeMap::new();
        if let Some(sl) = window(rl, "spend_limit") {
            extra.insert("spend_limit".to_string(), sl);
        }
        let snap = LimitSnapshot {
            ts,
            source: LimitSource::Statusline,
            five_hour: window(rl, "five_hour"),
            seven_day: window(rl, "seven_day"),
            extra,
            overall_status: None,
            representative_claim: None,
            overage_status: None,
            session_id: Some(session_id),
        };
        if snap.five_hour.is_some() || snap.seven_day.is_some() || !snap.extra.is_empty() {
            out.push(AppEvent::Limits(Box::new(snap)));
        }
    }
    out
}

/// Map a hook frame to app events.
pub fn hook_events(v: &Value, ts: DateTime<Utc>) -> Vec<AppEvent> {
    let mut out = Vec::new();
    let Some(sid) = s(v, &["session_id"]) else {
        return out;
    };
    let event = s(v, &["hook_event_name"]).unwrap_or_default();
    let tool = s(v, &["tool_name"]);
    let mk = |state: SessionState| Activity {
        session_id: sid.clone(),
        ts,
        state,
        cwd: s(v, &["cwd"]),
        branch: None,
        version: None,
        model: s(v, &["model"]),
        effort: None,
        turns: 0,
        compactions: 0,
    };
    match event.as_str() {
        "SessionStart" | "UserPromptSubmit" => {
            // Turns are counted from the transcript; the hook only moves the state early.
            let a = mk(if event == "SessionStart" { SessionState::Idle } else { SessionState::Thinking });
            out.push(AppEvent::Activity(a));
        }
        "PreToolUse" => {
            if let Some(t) = tool {
                out.push(AppEvent::Activity(mk(SessionState::Tool(t))));
            }
        }
        "PostToolUse" | "PostToolUseFailure" => {
            out.push(AppEvent::Activity(mk(SessionState::Thinking)));
            if let Some(t) = tool {
                out.push(AppEvent::Tool(ToolCall {
                    session_id: Some(sid.clone()),
                    ts,
                    name: t,
                    success: Some(event == "PostToolUse"),
                    duration_ms: u(v, &["duration_ms"]),
                    tool_use_id: s(v, &["tool_use_id"]),
                }));
            }
        }
        "Notification" | "PermissionRequest" => {
            // Notifications fire for permission prompts and idle reminders.
            let msg = s(v, &["message"]).unwrap_or_default().to_lowercase();
            let ntype = s(v, &["notification_type"]).unwrap_or_default();
            if event == "PermissionRequest" || msg.contains("permission") || ntype.contains("permission") {
                out.push(AppEvent::Activity(mk(SessionState::Waiting)));
            }
        }
        "PreCompact" => out.push(AppEvent::Activity(mk(SessionState::Compacting))),
        "PostCompact" => {
            let mut a = mk(SessionState::Thinking);
            a.compactions = 1;
            out.push(AppEvent::Activity(a));
        }
        "Stop" | "StopFailure" => out.push(AppEvent::Activity(mk(SessionState::Idle))),
        "SessionEnd" => out.push(AppEvent::Activity(mk(SessionState::Ended))),
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_792_600_000, 0).unwrap()
    }

    #[test]
    fn statusline_full_frame() {
        let v = json!({
            "session_id": "abc",
            "model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"},
            "workspace": {"current_dir": "/x/clankmon"},
            "cost": {"total_cost_usd": 1.25, "total_duration_ms": 45000, "total_api_duration_ms": 2300, "total_lines_added": 156, "total_lines_removed": 23},
            "context_window": {"used_percentage": 8, "context_window_size": 1000000},
            "effort": {"level": "high"},
            "prompt_cache": {"warm": true, "hit_ratio": 0.93, "misses": 1},
            "rate_limits": {
                "five_hour": {"used_percentage": 23.5, "resets_at": 1738425600},
                "seven_day": {"used_percentage": 41.2, "resets_at": 1738857600}
            }
        });
        let ev = statusline_events(&v, now());
        assert_eq!(ev.len(), 2);
        let AppEvent::Statusline(info) = &ev[0] else { panic!() };
        assert_eq!(info.context_used_pct, Some(8.0));
        assert_eq!(info.cache_hit_ratio, Some(0.93));
        assert_eq!(info.effort.as_deref(), Some("high"));
        let AppEvent::Limits(l) = &ev[1] else { panic!() };
        assert_eq!(l.five_hour.as_ref().unwrap().used_pct, 23.5);
        assert_eq!(l.seven_day.as_ref().unwrap().resets_at.unwrap().timestamp(), 1738857600);
        assert_eq!(l.source, LimitSource::Statusline);
    }

    #[test]
    fn statusline_without_limits_for_api_users() {
        let v = json!({"session_id": "abc", "model": {"id": "claude-sonnet-5-5"}});
        assert_eq!(statusline_events(&v, now()).len(), 1);
        assert!(statusline_events(&json!({"model": {}}), now()).is_empty());
    }

    #[test]
    fn hooks_drive_state() {
        let pre = json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Bash"});
        assert!(
            matches!(&hook_events(&pre, now())[0], AppEvent::Activity(a) if a.state == SessionState::Tool("Bash".into()))
        );

        let post =
            json!({"session_id": "s", "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_use_id": "t1"});
        let ev = hook_events(&post, now());
        assert!(matches!(&ev[1], AppEvent::Tool(t) if t.success == Some(true)));

        let perm = json!({"session_id": "s", "hook_event_name": "Notification", "message": "Claude needs your permission to use Bash"});
        assert!(matches!(&hook_events(&perm, now())[0], AppEvent::Activity(a) if a.state == SessionState::Waiting));

        let idle_note = json!({"session_id": "s", "hook_event_name": "Notification", "message": "Claude is waiting for your input"});
        assert!(hook_events(&idle_note, now()).is_empty());

        let end = json!({"session_id": "s", "hook_event_name": "SessionEnd"});
        assert!(matches!(&hook_events(&end, now())[0], AppEvent::Activity(a) if a.state == SessionState::Ended));
    }
}
