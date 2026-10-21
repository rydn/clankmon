//! `clankmon statusline` and `clankmon hook`: tiny clients Claude Code runs on
//! every update. They must be fast, silent on failure, and never block.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::config::Paths;
use crate::ipc::{Frame, FrameKind, send_blocking};

fn read_stdin() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().take(4 * 1024 * 1024).read_to_string(&mut s);
    s
}

/// Run as Claude Code's statusLine command.
pub fn statusline(paths: &Paths, wrap: Option<String>, quiet: bool) -> i32 {
    let input = read_stdin();
    let parsed: Option<Value> = serde_json::from_str(&input).ok();
    if let Some(v) = &parsed {
        send_blocking(&paths.socket(), &paths.spool(), &Frame::new(FrameKind::Statusline, v.clone()));
        let _ = std::fs::create_dir_all(&paths.home);
        let _ = std::fs::write(paths.last_statusline(), &input);
    }
    if let Some(cmd) = wrap {
        return run_wrapped(&cmd, &input);
    }
    if !quiet && let Some(v) = &parsed {
        println!("{}", render_line(v));
    }
    0
}

/// Pipe the same JSON to the user's previous status line command and echo its output.
fn run_wrapped(cmd: &str, input: &str) -> i32 {
    let child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    let Ok(mut child) = child else { return 0 };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    child.wait().map(|s| s.code().unwrap_or(0)).unwrap_or(0)
}

/// clankmon's own status line: model, context, quota and cost in neon ANSI.
pub fn render_line(v: &Value) -> String {
    const M: &str = "\x1b[38;2;255;43;214m";
    const C: &str = "\x1b[38;2;0;229;255m";
    const Y: &str = "\x1b[38;2;244;255;90m";
    const D: &str = "\x1b[38;2;124;129;154m";
    const G: &str = "\x1b[38;2;42;47;69m";
    const R: &str = "\x1b[38;2;255;59;59m";
    const O: &str = "\x1b[38;2;255;158;44m";
    const B: &str = "\x1b[1m";
    const X: &str = "\x1b[0m";
    let level = |p: f64| if p >= 90.0 { R } else if p >= 80.0 { O } else if p >= 50.0 { Y } else { C };
    let sep = format!(" {D}▸{X} ");

    let model = v.pointer("/model/display_name").and_then(Value::as_str).unwrap_or("Claude");
    let dir = v
        .pointer("/workspace/current_dir")
        .and_then(Value::as_str)
        .and_then(|d| d.rsplit('/').find(|s| !s.is_empty()))
        .unwrap_or("");
    let mut out = format!("{M}◢{X} {B}{model}{X} {M}◣{X} {D}{dir}{X}");

    if let Some(p) = v.pointer("/context_window/used_percentage").and_then(Value::as_f64) {
        let filled = ((p / 100.0) * 8.0).round().clamp(0.0, 8.0) as usize;
        out.push_str(&format!(
            "{sep}{D}ctx{X} {}{}{X}{G}{}{X} {}{p:.0}%{X}",
            level(p),
            "▰".repeat(filled),
            "▱".repeat(8 - filled),
            level(p)
        ));
    }
    let now = chrono::Utc::now().timestamp();
    for (key, label) in [("five_hour", "5h"), ("seven_day", "7d")] {
        if let Some(w) = v.pointer(&format!("/rate_limits/{key}"))
            && let Some(p) = w.get("used_percentage").and_then(Value::as_f64)
        {
            out.push_str(&format!("{sep}{D}{label}{X} {}{p:.0}%{X}", level(p)));
            if let Some(r) = w.get("resets_at").and_then(Value::as_i64) {
                let left = r - now;
                if left > 0 {
                    out.push_str(&format!(" {D}⟳{}{X}", crate::ui::fmt::dur_secs(left)));
                }
            }
        }
    }
    if let Some(c) = v.pointer("/cost/total_cost_usd").and_then(Value::as_f64) {
        out.push_str(&format!("{sep}{Y}${c:.2}{X}"));
    }
    out
}

/// Run as a Claude Code hook: forward the event and exit 0 without output.
pub fn hook(paths: &Paths) -> i32 {
    let input = read_stdin();
    if let Ok(v) = serde_json::from_str::<Value>(&input) {
        send_blocking(&paths.socket(), &paths.spool(), &Frame::new(FrameKind::Hook, v));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_contains_the_essentials() {
        let v = serde_json::json!({
            "model": {"display_name": "Opus 5.5"},
            "workspace": {"current_dir": "/x/clankmon"},
            "context_window": {"used_percentage": 34},
            "rate_limits": {"five_hour": {"used_percentage": 23.0, "resets_at": 4102444800i64}},
            "cost": {"total_cost_usd": 1.234}
        });
        let s = render_line(&v);
        assert!(s.contains("Opus 5.5"));
        assert!(s.contains("clankmon"));
        assert!(s.contains("34%"));
        assert!(s.contains("23%"));
        assert!(s.contains("$1.23"));
        assert!(s.contains("▰▰▰"));
    }
}
