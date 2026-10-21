//! `clankmon doctor`: which data sources are live and how to enable the rest.

use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::collect::{jsonl, oauth, procs};
use crate::config::{Config, Paths};

const OK: &str = "\x1b[38;2;57;255;136m✓\x1b[0m";
const NO: &str = "\x1b[38;2;255;59;59m✗\x1b[0m";
const INFO: &str = "\x1b[38;2;0;229;255m•\x1b[0m";
const DIM: &str = "\x1b[38;2;124;129;154m";
const X: &str = "\x1b[0m";

fn row(mark: &str, name: &str, detail: impl AsRef<str>) {
    println!("  {mark} {name:<14} {}", detail.as_ref());
}

fn hint(s: &str) {
    println!("    {DIM}↳ {s}{X}");
}

fn age(t: SystemTime) -> String {
    crate::ui::fmt::dur_secs(SystemTime::now().duration_since(t).unwrap_or_default().as_secs() as i64)
}

pub fn run(paths: &Paths, cfg: &Config) -> i32 {
    println!("\x1b[38;2;255;43;214m◢ CLANKMON DOCTOR ◣\x1b[0m\n");
    println!("  claude dir     {}", paths.claude_dir.display());
    println!("  clankmon dir   {}\n", paths.home.display());

    // Transcripts.
    let files = jsonl::discover(&paths.projects(), Duration::from_secs(u64::from(cfg.history_days) * 86_400));
    if files.is_empty() {
        row(NO, "transcripts", "no transcripts found");
        hint("run Claude Code once, or pass --claude-dir / set CLAUDE_CONFIG_DIR");
    } else {
        let bytes: u64 = files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
        row(
            OK,
            "transcripts",
            format!("{} files, {} in the last {} days", files.len(), crate::ui::fmt::si(bytes) + "B", cfg.history_days),
        );
    }

    // Processes.
    let live = procs::read_sessions(&paths.sessions());
    row(
        if live.is_empty() { INFO } else { OK },
        "processes",
        format!("{} live Claude Code session{}", live.len(), if live.len() == 1 { "" } else { "s" }),
    );

    // Settings: statusline and hooks.
    let settings: Value = std::fs::read_to_string(paths.claude_settings())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let sl = settings.pointer("/statusLine/command").and_then(Value::as_str).unwrap_or("");
    if sl.contains("clankmon") {
        let last = std::fs::metadata(paths.last_statusline()).and_then(|m| m.modified()).ok();
        let has_limits = std::fs::read_to_string(paths.last_statusline())
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .is_some_and(|v| v.get("rate_limits").is_some());
        row(
            OK,
            "statusline",
            match last {
                Some(t) => format!(
                    "installed · last frame {} ago · {}",
                    age(t),
                    if has_limits {
                        "rate limits present"
                    } else {
                        "no rate_limits (API key or before first response)"
                    }
                ),
                None => "installed · no frame received yet".into(),
            },
        );
    } else {
        row(NO, "statusline", if sl.is_empty() { "not configured".to_string() } else { format!("points at `{sl}`") });
        hint("clankmon install   (wraps any existing status line)");
    }
    let hooks_on = settings
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|h| h.values().any(|v| v.to_string().contains("clankmon")));
    row(if hooks_on { OK } else { NO }, "hooks", if hooks_on { "installed" } else { "not installed" });
    if !hooks_on {
        hint("clankmon install   (async hooks: permission waits, compaction, session start/end)");
    }

    // TUI socket and spool.
    let tui = std::os::unix::net::UnixStream::connect(paths.socket()).is_ok();
    row(if tui { OK } else { INFO }, "tui", if tui { "running, socket accepting frames" } else { "not running" });
    if let Ok(m) = std::fs::metadata(paths.spool()) {
        row(INFO, "spool", format!("{} buffered for the next TUI start", crate::ui::fmt::si(m.len()) + "B"));
    }

    // Proxy.
    let proxy_up =
        std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], cfg.proxy.port).into(), Duration::from_millis(200))
            .is_ok();
    let base = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default();
    row(
        if proxy_up { OK } else { INFO },
        "proxy",
        format!(
            "{} on :{} · ANTHROPIC_BASE_URL {}",
            if proxy_up { "listening" } else { "not running" },
            cfg.proxy.port,
            if base.is_empty() { "unset in this shell".into() } else { base }
        ),
    );
    if !proxy_up {
        hint(&format!("clankmon proxy, then: export ANTHROPIC_BASE_URL=http://127.0.0.1:{}", cfg.proxy.port));
    }

    // OpenTelemetry.
    let env_otel = settings.pointer("/env/OTEL_EXPORTER_OTLP_ENDPOINT").and_then(Value::as_str).unwrap_or("");
    row(
        if env_otel.contains("4318") || env_otel.contains(&cfg.otel.port.to_string()) { OK } else { INFO },
        "otel",
        if env_otel.is_empty() {
            "Claude Code telemetry export not configured".to_string()
        } else {
            format!("exporting to {env_otel}")
        },
    );
    if env_otel.is_empty() {
        hint("clankmon install --otel, then run clankmon --otel");
    }

    // Credentials.
    match oauth::read_credentials(&paths.credentials_file()) {
        Some(c) => row(
            OK,
            "account",
            format!(
                "{} · tier {} · token {}",
                c.subscription.unwrap_or_else(|| "?".into()),
                c.tier.unwrap_or_else(|| "?".into()),
                match c.expires_at {
                    Some(e) if e < chrono::Utc::now() => "expired (Claude Code refreshes it)".to_string(),
                    Some(e) => format!("valid until {}", crate::ui::fmt::hm(e)),
                    None => "present".into(),
                }
            ),
        ),
        None => row(INFO, "account", "no Claude Code OAuth credentials (API key user?)"),
    }

    // Store.
    if let Ok(m) = std::fs::metadata(paths.db()) {
        row(OK, "store", format!("{} at {}", crate::ui::fmt::si(m.len()) + "B", paths.db().display()));
    }
    println!();
    0
}
