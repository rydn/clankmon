//! Unix-socket IPC between short-lived clients (`clankmon statusline`,
//! `clankmon hook`, `clankmon proxy`) and the TUI. When the TUI is not running,
//! clients append frames to a spool file that the TUI drains on startup.

use std::io::Write;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::UnboundedSender;

use crate::collect::push;
use crate::model::{AppEvent, Collector, CollectorHealth, Incident, LimitSnapshot, Request, StreamProgress};

/// Spool files larger than this are rotated so they never grow without bound.
const SPOOL_MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub kind: FrameKind,
    pub ts: DateTime<Utc>,
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    Statusline,
    Hook,
    Request,
    Limits,
    Incident,
    Stream,
}

impl Frame {
    pub fn new(kind: FrameKind, payload: Value) -> Self {
        Frame { kind, ts: Utc::now(), payload }
    }

    pub fn request(r: &Request) -> Self {
        Frame::new(FrameKind::Request, serde_json::to_value(r).unwrap_or(Value::Null))
    }

    pub fn limits(l: &LimitSnapshot) -> Self {
        Frame::new(FrameKind::Limits, serde_json::to_value(l).unwrap_or(Value::Null))
    }

    pub fn incident(i: &Incident) -> Self {
        Frame::new(FrameKind::Incident, serde_json::to_value(i).unwrap_or(Value::Null))
    }

    pub fn stream(p: &StreamProgress) -> Self {
        Frame::new(FrameKind::Stream, serde_json::to_value(p).unwrap_or(Value::Null))
    }

    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_default();
        s.push('\n');
        s
    }

    pub fn into_events(self) -> Vec<AppEvent> {
        match self.kind {
            FrameKind::Statusline => push::statusline_events(&self.payload, self.ts),
            FrameKind::Hook => {
                let mut ev = push::hook_events(&self.payload, self.ts);
                ev.push(AppEvent::Collector(Collector::Hooks, CollectorHealth::Ok));
                ev
            }
            FrameKind::Request => serde_json::from_value::<Request>(self.payload)
                .map(|r| vec![AppEvent::Request(Box::new(r))])
                .unwrap_or_default(),
            FrameKind::Limits => serde_json::from_value::<LimitSnapshot>(self.payload)
                .map(|l| vec![AppEvent::Limits(Box::new(l))])
                .unwrap_or_default(),
            FrameKind::Incident => serde_json::from_value::<Incident>(self.payload)
                .map(|i| vec![AppEvent::Incident(i)])
                .unwrap_or_default(),
            FrameKind::Stream => serde_json::from_value::<StreamProgress>(self.payload)
                .map(|p| vec![AppEvent::Stream(Box::new(p))])
                .unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Client side (blocking, used by sub-50ms commands).
// ---------------------------------------------------------------------------

/// Deliver a frame to the TUI, or spool it. Never blocks for long and never fails loudly.
pub fn send_blocking(socket: &Path, spool: &Path, frame: &Frame) {
    let line = frame.to_line();
    if let Ok(mut s) = StdUnixStream::connect(socket) {
        let _ = s.set_write_timeout(Some(Duration::from_millis(100)));
        if s.write_all(line.as_bytes()).is_ok() {
            return;
        }
    }
    let _ = append_spool(spool, &line);
}

pub fn append_spool(spool: &Path, line: &str) -> std::io::Result<()> {
    if let Ok(m) = std::fs::metadata(spool)
        && m.len() > SPOOL_MAX_BYTES
    {
        let _ = std::fs::rename(spool, spool.with_extension("old.jsonl"));
    }
    if let Some(dir) = spool.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // O_APPEND keeps concurrent single-line writers from interleaving.
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(spool)?;
    f.write_all(line.as_bytes())
}

/// Async sink used by the standalone proxy: keeps one connection, reconnects, spools on failure.
pub struct AsyncSink {
    socket: PathBuf,
    spool: PathBuf,
    conn: tokio::sync::Mutex<Option<UnixStream>>,
}

impl AsyncSink {
    pub fn new(socket: PathBuf, spool: PathBuf) -> Self {
        AsyncSink { socket, spool, conn: tokio::sync::Mutex::new(None) }
    }

    pub async fn send(&self, frame: &Frame) {
        let line = frame.to_line();
        let mut guard = self.conn.lock().await;
        for _ in 0..2 {
            if guard.is_none() {
                *guard = UnixStream::connect(&self.socket).await.ok();
            }
            match guard.as_mut() {
                Some(s) => {
                    if s.write_all(line.as_bytes()).await.is_ok() {
                        return;
                    }
                    *guard = None;
                }
                None => break,
            }
        }
        let spool = self.spool.clone();
        let _ = tokio::task::spawn_blocking(move || append_spool(&spool, &line)).await;
    }
}

// ---------------------------------------------------------------------------
// Server side (inside the TUI).
// ---------------------------------------------------------------------------

/// Bind the socket. Refuses to steal it from another running clankmon.
pub async fn bind(socket: &Path) -> std::io::Result<UnixListener> {
    if socket.exists() {
        if UnixStream::connect(socket).await.is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "another clankmon instance owns the socket",
            ));
        }
        let _ = std::fs::remove_file(socket);
    }
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    UnixListener::bind(socket)
}

/// Accept connections and forward every frame as app events.
pub async fn serve(listener: UnixListener, tx: UnboundedSender<AppEvent>) {
    let _ = tx.send(AppEvent::Collector(Collector::Statusline, CollectorHealth::Listening));
    let _ = tx.send(AppEvent::Collector(Collector::Hooks, CollectorHealth::Listening));
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(frame) = serde_json::from_str::<Frame>(&line) {
                    let ev = frame.into_events();
                    if !ev.is_empty() && tx.send(AppEvent::Batch(ev)).is_err() {
                        return;
                    }
                }
            }
        });
    }
}

/// Read the spool without clearing it (for `snapshot`).
pub fn peek_spool(spool: &Path) -> Vec<AppEvent> {
    let mut events = Vec::new();
    for path in [spool.with_extension("old.jsonl"), spool.to_path_buf()] {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for line in text.lines() {
            if let Ok(frame) = serde_json::from_str::<Frame>(line) {
                events.extend(frame.into_events());
            }
        }
    }
    events
}

/// Read and clear the spool. Returns events in file order.
pub fn drain_spool(spool: &Path) -> Vec<AppEvent> {
    let mut events = Vec::new();
    for path in [spool.with_extension("old.jsonl"), spool.to_path_buf()] {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for line in text.lines() {
            if let Ok(frame) = serde_json::from_str::<Frame>(line) {
                events.extend(frame.into_events());
            }
        }
        let _ = std::fs::remove_file(&path);
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spool_round_trip_when_tui_absent() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let spool = dir.path().join("spool.jsonl");
        let frame = Frame::new(
            FrameKind::Statusline,
            serde_json::json!({"session_id": "s", "rate_limits": {"five_hour": {"used_percentage": 12.5, "resets_at": 1792700000}}}),
        );
        send_blocking(&sock, &spool, &frame);
        send_blocking(&sock, &spool, &frame);
        let ev = drain_spool(&spool);
        assert_eq!(ev.iter().filter(|e| matches!(e, AppEvent::Limits(_))).count(), 2);
        assert!(!spool.exists());
    }

    #[tokio::test]
    async fn socket_delivers_frames() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let listener = bind(&sock).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(serve(listener, tx));
        let spool = dir.path().join("spool.jsonl");
        let sock2 = sock.clone();
        tokio::task::spawn_blocking(move || {
            send_blocking(
                &sock2,
                &spool,
                &Frame::new(FrameKind::Hook, serde_json::json!({"session_id": "s", "hook_event_name": "Stop"})),
            );
        })
        .await
        .unwrap();
        let mut got_activity = false;
        for _ in 0..5 {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Some(AppEvent::Batch(evs))) => {
                    got_activity = evs.iter().any(|e| matches!(e, AppEvent::Activity(_)));
                    if got_activity {
                        break;
                    }
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(got_activity);
        // A second server must refuse to steal the live socket.
        assert!(bind(&sock).await.is_err());
    }

    #[test]
    fn request_frames_round_trip() {
        let r = Request { key: "req_x".into(), model: "claude-opus-5-5".into(), ..Default::default() };
        let ev = serde_json::from_str::<Frame>(Frame::request(&r).to_line().trim()).unwrap().into_events();
        assert!(matches!(&ev[0], AppEvent::Request(x) if x.key == "req_x"));
    }
}
