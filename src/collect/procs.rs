//! Live Claude Code processes from `~/.claude/sessions/<pid>.json`, with CPU
//! and memory from `ps`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::DateTime;
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::model::{AppEvent, Collector, CollectorHealth, ProcInfo};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionFile {
    pid: u32,
    session_id: String,
    #[serde(default)]
    cwd: String,
    name: Option<String>,
    kind: Option<String>,
    entrypoint: Option<String>,
    version: Option<String>,
    started_at: Option<i64>,
}

/// True when a process with this pid exists (EPERM still means it exists).
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: kill with signal 0 performs only the existence/permission check.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Parse every `<digits>.json` in the sessions dir whose process is alive.
pub fn read_sessions(dir: &Path) -> Vec<ProcInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path: PathBuf = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if path.extension().is_none_or(|e| e != "json") || !stem.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(s) = serde_json::from_str::<SessionFile>(&text) else { continue };
        if !pid_alive(s.pid) {
            continue;
        }
        out.push(ProcInfo {
            pid: s.pid,
            session_id: s.session_id,
            cwd: s.cwd,
            name: s.name.filter(|n| !n.is_empty()),
            kind: s.kind,
            entrypoint: s.entrypoint,
            version: s.version,
            started_at: s.started_at.and_then(DateTime::from_timestamp_millis),
            cpu_pct: None,
            rss_kb: None,
        });
    }
    out.sort_by_key(|p| p.pid);
    out
}

/// `ps -o pid=,%cpu=,rss= -p 1,2,3` parsed into pid -> (cpu%, rss KiB).
pub fn ps_stats(pids: &[u32]) -> HashMap<u32, (f32, u64)> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let Ok(out) = std::process::Command::new("ps").args(["-o", "pid=,%cpu=,rss=", "-p", &list]).output() else {
        return HashMap::new();
    };
    parse_ps(&String::from_utf8_lossy(&out.stdout))
}

fn parse_ps(text: &str) -> HashMap<u32, (f32, u64)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let cpu = it.next()?.replace(',', ".").parse().ok()?;
            let rss = it.next()?.parse().ok()?;
            Some((pid, (cpu, rss)))
        })
        .collect()
}

/// Poll the registry every second and `ps` every fifth tick; send only changes.
pub async fn run(dir: PathBuf, tx: UnboundedSender<AppEvent>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut n: u64 = 0;
    let mut last_stats: HashMap<u32, (f32, u64)> = HashMap::new();
    let mut last_sent: Option<Vec<ProcInfo>> = None;
    let _ = tx.send(AppEvent::Collector(Collector::Procs, CollectorHealth::Ok));
    loop {
        tick.tick().await;
        let dir2 = dir.clone();
        let refresh_ps = n.is_multiple_of(5);
        let prev = last_stats.clone();
        let res = tokio::task::spawn_blocking(move || {
            let mut procs = read_sessions(&dir2);
            let pids: Vec<u32> = procs.iter().map(|p| p.pid).collect();
            let stats = if refresh_ps { ps_stats(&pids) } else { prev };
            for p in &mut procs {
                if let Some((cpu, rss)) = stats.get(&p.pid) {
                    p.cpu_pct = Some(*cpu);
                    p.rss_kb = Some(*rss);
                }
            }
            (procs, stats)
        })
        .await;
        if let Ok((procs, stats)) = res {
            last_stats = stats;
            // Only send changes (plus a 30 s heartbeat for stale-state checks):
            // every send triggers a redraw, and a fixed 1 s send at its own
            // phase would pre-empt the UI's clock-aligned tick.
            let changed = last_sent.as_ref() != Some(&procs);
            if changed || n.is_multiple_of(30) {
                if tx.send(AppEvent::Procs(procs.clone())).is_err() {
                    return;
                }
                last_sent = Some(procs);
            }
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_live_sessions_only() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        std::fs::write(
            dir.path().join(format!("{me}.json")),
            format!(r#"{{"pid":{me},"sessionId":"abc","cwd":"/x","startedAt":1792603162510,"name":"spendl","kind":"interactive","version":"2.1.292"}}"#),
        )
        .unwrap();
        // A pid that cannot exist.
        std::fs::write(dir.path().join("4000000.json"), r#"{"pid":4000000,"sessionId":"dead","cwd":"/y"}"#).unwrap();
        // Peer token files are ignored.
        std::fs::write(dir.path().join(format!("{me}.deadbeef.key")), "{}").unwrap();
        let procs = read_sessions(dir.path());
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].session_id, "abc");
        assert_eq!(procs[0].name.as_deref(), Some("spendl"));
        assert!(procs[0].started_at.is_some());
    }

    #[test]
    fn parses_ps_output() {
        let m = parse_ps("  123   4.5  20480\n 99 0,0 100\nbad line\n");
        assert_eq!(m[&123], (4.5, 20480));
        assert_eq!(m[&99], (0.0, 100));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn own_pid_is_alive() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0));
    }
}
