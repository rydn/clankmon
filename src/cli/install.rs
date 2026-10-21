//! `clankmon install` / `uninstall`: wire Claude Code's statusline, hooks and
//! (optionally) OpenTelemetry export to clankmon by editing settings.json.
//! Always backs up first, shows the change, and asks before writing.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Context, Result, bail};
use serde_json::{Map, Value, json};

/// Hook events clankmon listens to. All run with `async: true`, so they never
/// delay Claude Code.
pub const HOOK_EVENTS: [&str; 9] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PostToolUse",
    "PostToolUseFailure",
    "Notification",
    "PreCompact",
    "PostCompact",
    "Stop",
];

pub const OTEL_ENV: [(&str, &str); 7] = [
    ("CLAUDE_CODE_ENABLE_TELEMETRY", "1"),
    ("OTEL_LOGS_EXPORTER", "otlp"),
    ("OTEL_METRICS_EXPORTER", "otlp"),
    ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json"),
    ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318"),
    ("OTEL_LOGS_EXPORT_INTERVAL", "2000"),
    ("OTEL_METRIC_EXPORT_INTERVAL", "10000"),
];

#[derive(Debug, Clone)]
pub struct Options {
    pub statusline: bool,
    pub hooks: bool,
    pub otel: bool,
    pub yes: bool,
    pub dry_run: bool,
    pub settings: PathBuf,
    pub bin: PathBuf,
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn is_ours(cmd: &str) -> bool {
    cmd.contains("clankmon")
}

fn load(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(json!({})),
        Ok(s) => serde_json::from_str(&s).wrap_err_with(|| format!("{} is not valid JSON", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e).wrap_err_with(|| format!("reading {}", path.display())),
    }
}

/// Apply the install to a settings document. Returns human-readable changes.
pub fn apply_install(doc: &mut Value, o: &Options) -> Vec<String> {
    let mut changes = Vec::new();
    let bin = o.bin.to_string_lossy().to_string();
    let root = doc.as_object_mut().expect("settings root is an object");

    if o.statusline {
        let ours = format!("{} statusline", shell_quote(&bin));
        let existing = root
            .get("statusLine")
            .and_then(|s| s.get("command"))
            .and_then(Value::as_str)
            .map(str::to_string);
        match existing {
            Some(cmd) if is_ours(&cmd) => changes.push("statusLine already points at clankmon".into()),
            Some(cmd) => {
                let wrapped = format!("{ours} --wrap {}", shell_quote(&cmd));
                let sl = root.get_mut("statusLine").and_then(Value::as_object_mut).expect("statusLine object");
                sl.insert("command".into(), Value::String(wrapped));
                sl.entry("refreshInterval").or_insert(json!(60));
                changes.push(format!("statusLine: wrap existing command `{cmd}` (its output is shown unchanged)"));
            }
            None => {
                root.insert(
                    "statusLine".into(),
                    json!({"type": "command", "command": ours, "padding": 0, "refreshInterval": 60}),
                );
                changes.push("statusLine: clankmon's neon status line (feeds quota, context and cost)".into());
            }
        }
    }

    if o.hooks {
        let hooks = root.entry("hooks").or_insert_with(|| json!({}));
        if !hooks.is_object() {
            *hooks = json!({});
        }
        let hooks = hooks.as_object_mut().expect("hooks object");
        let mut added = Vec::new();
        for ev in HOOK_EVENTS {
            let groups = hooks.entry(ev).or_insert_with(|| json!([]));
            if !groups.is_array() {
                *groups = json!([]);
            }
            let groups = groups.as_array_mut().expect("array");
            let present = groups.iter().any(|g| {
                g.get("hooks").and_then(Value::as_array).is_some_and(|hs| {
                    hs.iter().any(|h| h.get("command").and_then(Value::as_str).is_some_and(is_ours))
                })
            });
            if present {
                continue;
            }
            let mut group = Map::new();
            if matches!(ev, "PostToolUse" | "PostToolUseFailure") {
                group.insert("matcher".into(), json!("*"));
            }
            group.insert(
                "hooks".into(),
                json!([{"type": "command", "command": bin, "args": ["hook"], "async": true, "timeout": 5}]),
            );
            groups.push(Value::Object(group));
            added.push(ev);
        }
        if added.is_empty() {
            changes.push("hooks already installed".into());
        } else {
            changes.push(format!("hooks: async `clankmon hook` on {}", added.join(", ")));
        }
    }

    if o.otel {
        let env = root.entry("env").or_insert_with(|| json!({}));
        if !env.is_object() {
            *env = json!({});
        }
        let env = env.as_object_mut().expect("env object");
        let mut set = Vec::new();
        for (k, v) in OTEL_ENV {
            if env.get(k).and_then(Value::as_str) != Some(v) {
                env.insert(k.into(), json!(v));
                set.push(k);
            }
        }
        if set.is_empty() {
            changes.push("OpenTelemetry env already set".into());
        } else {
            changes.push(format!("env: OTLP http/json export to 127.0.0.1:4318 ({})", set.join(", ")));
        }
    }
    changes
}

/// Remove everything clankmon added, restoring a wrapped status line.
pub fn apply_uninstall(doc: &mut Value) -> Vec<String> {
    let mut changes = Vec::new();
    let Some(root) = doc.as_object_mut() else { return changes };

    if let Some(cmd) = root.get("statusLine").and_then(|s| s.get("command")).and_then(Value::as_str).map(str::to_string)
        && is_ours(&cmd)
    {
        match unwrap_command(&cmd) {
            Some(prev) => {
                if let Some(sl) = root.get_mut("statusLine").and_then(Value::as_object_mut) {
                    sl.insert("command".into(), Value::String(prev.clone()));
                }
                changes.push(format!("statusLine: restored `{prev}`"));
            }
            None => {
                root.remove("statusLine");
                changes.push("statusLine: removed".into());
            }
        }
    }

    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        let mut removed = 0;
        for groups in hooks.values_mut() {
            if let Some(arr) = groups.as_array_mut() {
                for g in arr.iter_mut() {
                    if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                        let before = hs.len();
                        hs.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(is_ours));
                        removed += before - hs.len();
                    }
                }
                arr.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|h| !h.is_empty()));
            }
        }
        hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
        if removed > 0 {
            changes.push(format!("hooks: removed {removed} clankmon entries"));
        }
    }
    if root.get("hooks").and_then(Value::as_object).is_some_and(Map::is_empty) {
        root.remove("hooks");
    }

    if let Some(env) = root.get_mut("env").and_then(Value::as_object_mut)
        && env.get("OTEL_EXPORTER_OTLP_ENDPOINT").and_then(Value::as_str) == Some("http://127.0.0.1:4318")
    {
        for (k, _) in OTEL_ENV {
            env.remove(k);
        }
        changes.push("env: removed clankmon OpenTelemetry export".into());
    }
    changes
}

/// `clankmon statusline --wrap 'cmd'` -> `cmd`.
fn unwrap_command(cmd: &str) -> Option<String> {
    let rest = cmd.split_once(" --wrap ")?.1.trim();
    if let Some(inner) = rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        return Some(inner.replace("'\\''", "'"));
    }
    Some(rest.to_string())
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

fn write_with_backup(path: &Path, doc: &Value) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = path.with_file_name(format!(
        "{}.clankmon-bak-{stamp}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("settings.json")
    ));
    if path.exists() {
        std::fs::copy(path, &backup).wrap_err("backing up settings")?;
    }
    let mut text = serde_json::to_string_pretty(doc)?;
    text.push('\n');
    // Write atomically so Claude Code never reads a half-written file.
    let tmp = path.with_extension("json.clankmon-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(backup)
}

fn finish(path: &Path, doc: Value, changes: Vec<String>, yes: bool, dry_run: bool) -> Result<()> {
    println!("\x1b[38;2;255;43;214m◢ clankmon\x1b[0m  {}", path.display());
    for c in &changes {
        println!("  \x1b[38;2;0;229;255m▸\x1b[0m {c}");
    }
    if dry_run {
        println!("\n{}", serde_json::to_string_pretty(&doc)?);
        println!("\n(dry run: nothing written)");
        return Ok(());
    }
    if !yes && !confirm("\nWrite these changes?") {
        bail!("aborted; settings unchanged");
    }
    let backup = write_with_backup(path, &doc)?;
    println!("  written · backup at {}", backup.display());
    Ok(())
}

pub fn install(o: &Options) -> Result<()> {
    let mut doc = load(&o.settings)?;
    if !doc.is_object() {
        bail!("{} must contain a JSON object", o.settings.display());
    }
    let changes = apply_install(&mut doc, o);
    finish(&o.settings, doc, changes, o.yes, o.dry_run)?;
    if o.otel {
        println!("  OpenTelemetry events reach clankmon only while it runs with --otel (or otel.enabled in config).");
    }
    println!("  Restart running Claude Code sessions to pick up hooks; the status line updates immediately.");
    Ok(())
}

pub fn uninstall(settings: &Path, yes: bool, dry_run: bool) -> Result<()> {
    let mut doc = load(settings)?;
    let changes = apply_uninstall(&mut doc);
    if changes.is_empty() {
        println!("nothing to remove from {}", settings.display());
        return Ok(());
    }
    finish(settings, doc, changes, yes, dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        Options {
            statusline: true,
            hooks: true,
            otel: false,
            yes: true,
            dry_run: false,
            settings: PathBuf::from("/tmp/s.json"),
            bin: PathBuf::from("/Users/me/.cargo/bin/clankmon"),
        }
    }

    #[test]
    fn install_preserves_existing_hooks_and_is_idempotent() {
        let mut doc = json!({
            "model": "fable",
            "hooks": {"Notification": [{"hooks": [{"type": "command", "command": "/Users/me/.config/iterm2/cc-status"}]}]}
        });
        let c1 = apply_install(&mut doc, &opts());
        assert!(c1.iter().any(|c| c.starts_with("hooks: async")));
        let notif = doc["hooks"]["Notification"].as_array().unwrap();
        assert_eq!(notif.len(), 2, "user's hook kept, ours appended");
        assert_eq!(doc["hooks"]["PostToolUse"][0]["matcher"], "*");
        assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["async"], true);
        assert_eq!(doc["statusLine"]["refreshInterval"], 60);
        let snapshot = doc.clone();
        let c2 = apply_install(&mut doc, &opts());
        assert_eq!(doc, snapshot);
        assert!(c2.iter().any(|c| c.contains("already")));
        // Key order is preserved: "model" stays first.
        assert_eq!(doc.as_object().unwrap().keys().next().unwrap(), "model");
    }

    #[test]
    fn wraps_and_restores_existing_status_line() {
        let mut doc = json!({"statusLine": {"type": "command", "command": "~/.claude/my line.sh"}});
        apply_install(&mut doc, &opts());
        let cmd = doc["statusLine"]["command"].as_str().unwrap().to_string();
        assert!(cmd.contains("statusline --wrap '~/.claude/my line.sh'"), "{cmd}");
        let changes = apply_uninstall(&mut doc);
        assert!(changes[0].contains("restored"));
        assert_eq!(doc["statusLine"]["command"], "~/.claude/my line.sh");
    }

    #[test]
    fn uninstall_removes_only_ours() {
        let mut doc = json!({"hooks": {"Notification": [{"hooks": [{"type": "command", "command": "/x/cc-status"}]}]}});
        let mut o = opts();
        o.otel = true;
        apply_install(&mut doc, &o);
        apply_uninstall(&mut doc);
        assert_eq!(
            doc,
            json!({"hooks": {"Notification": [{"hooks": [{"type": "command", "command": "/x/cc-status"}]}]}, "env": {}})
        );
    }

    #[test]
    fn otel_env_block() {
        let mut doc = json!({"env": {"ENABLE_TOOL_SEARCH": "true"}});
        let mut o = opts();
        o.statusline = false;
        o.hooks = false;
        o.otel = true;
        apply_install(&mut doc, &o);
        assert_eq!(doc["env"]["OTEL_EXPORTER_OTLP_PROTOCOL"], "http/json");
        assert_eq!(doc["env"]["ENABLE_TOOL_SEARCH"], "true");
    }
}
