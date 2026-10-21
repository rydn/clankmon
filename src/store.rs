//! SQLite persistence for data that cannot be re-derived from transcripts:
//! proxy/OTel request details, limit snapshots and live incidents.
//! Writes go through a dedicated thread so the UI never waits on disk.

use std::path::Path;
use std::sync::mpsc;

use chrono::{DateTime, Duration, Utc};
use color_eyre::eyre::{Context, Result};
use rusqlite::{Connection, params};

use crate::model::{AppEvent, Incident, LimitSnapshot, Request};

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS requests (
    key  TEXT PRIMARY KEY,
    ts   INTEGER NOT NULL,
    json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS requests_ts ON requests(ts);
CREATE TABLE IF NOT EXISTS limits (
    ts     INTEGER NOT NULL,
    source TEXT NOT NULL,
    json   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS limits_ts ON limits(ts);
CREATE TABLE IF NOT EXISTS incidents (
    ts   INTEGER NOT NULL,
    json TEXT NOT NULL
);
";

pub enum StoreMsg {
    Request(Box<Request>),
    Limits(Box<LimitSnapshot>),
    Incident(Incident),
}

pub struct Store {
    tx: mpsc::Sender<StoreMsg>,
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path).wrap_err_with(|| format!("opening {}", path.display()))?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Load what the UI needs on startup and prune old rows.
pub fn load_recent(conn: &Connection, days: i64, now: DateTime<Utc>) -> Result<Vec<AppEvent>> {
    let since = (now - Duration::days(days)).timestamp_millis();
    let prune = (now - Duration::days(days.max(35))).timestamp_millis();
    conn.execute("DELETE FROM limits WHERE ts < ?1", params![prune])?;
    conn.execute("DELETE FROM requests WHERE ts < ?1", params![prune])?;
    conn.execute("DELETE FROM incidents WHERE ts < ?1", params![prune])?;

    let mut out = Vec::new();
    let mut stmt = conn.prepare("SELECT json FROM requests WHERE ts >= ?1 ORDER BY ts")?;
    for row in stmt.query_map(params![since], |r| r.get::<_, String>(0))? {
        if let Ok(r) = serde_json::from_str::<Request>(&row?) {
            out.push(AppEvent::Request(Box::new(r)));
        }
    }
    let mut stmt = conn.prepare("SELECT json FROM limits WHERE ts >= ?1 ORDER BY ts")?;
    for row in stmt.query_map(params![since], |r| r.get::<_, String>(0))? {
        if let Ok(l) = serde_json::from_str::<LimitSnapshot>(&row?) {
            out.push(AppEvent::Limits(Box::new(l)));
        }
    }
    let mut stmt = conn.prepare("SELECT json FROM incidents WHERE ts >= ?1 ORDER BY ts")?;
    for row in stmt.query_map(params![since], |r| r.get::<_, String>(0))? {
        if let Ok(i) = serde_json::from_str::<Incident>(&row?) {
            out.push(AppEvent::Incident(i));
        }
    }
    Ok(out)
}

fn write(conn: &Connection, msg: &StoreMsg) -> rusqlite::Result<()> {
    match msg {
        StoreMsg::Request(r) => {
            let json = serde_json::to_string(r).unwrap_or_default();
            conn.execute(
                "INSERT INTO requests (key, ts, json) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET ts = excluded.ts, json = excluded.json",
                params![r.key, r.ts.timestamp_millis(), json],
            )?;
        }
        StoreMsg::Limits(l) => {
            let json = serde_json::to_string(l).unwrap_or_default();
            conn.execute(
                "INSERT INTO limits (ts, source, json) VALUES (?1, ?2, ?3)",
                params![l.ts.timestamp_millis(), l.source.label(), json],
            )?;
        }
        StoreMsg::Incident(i) => {
            let json = serde_json::to_string(i).unwrap_or_default();
            conn.execute(
                "INSERT INTO incidents (ts, json) VALUES (?1, ?2)",
                params![i.ts.timestamp_millis(), json],
            )?;
        }
    }
    Ok(())
}

impl Store {
    /// Start the writer thread on an open connection.
    pub fn spawn(conn: Connection) -> Store {
        let (tx, rx) = mpsc::channel::<StoreMsg>();
        std::thread::Builder::new()
            .name("clankmon-store".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    // Batch whatever is queued into one transaction.
                    let mut batch = vec![first];
                    while let Ok(m) = rx.try_recv() {
                        batch.push(m);
                    }
                    let res = (|| -> rusqlite::Result<()> {
                        let txn = conn.unchecked_transaction()?;
                        for m in &batch {
                            write(&txn, m)?;
                        }
                        txn.commit()
                    })();
                    if let Err(e) = res {
                        tracing::warn!("store write failed: {e}");
                    }
                }
            })
            .expect("spawn store thread");
        Store { tx }
    }

    pub fn send(&self, msg: StoreMsg) {
        let _ = self.tx.send(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LimitSource, Sources, WindowReading};
    use std::collections::BTreeMap;

    #[test]
    fn writes_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("t.db")).unwrap();
        let now = Utc::now();
        let r = Request { key: "req_1".into(), ts: now, sources: Sources(Sources::PROXY), ttft_ms: Some(700), ..Default::default() };
        write(&conn, &StoreMsg::Request(Box::new(r.clone()))).unwrap();
        // Upsert replaces.
        let r2 = Request { ttft_ms: Some(650), ..r };
        write(&conn, &StoreMsg::Request(Box::new(r2))).unwrap();
        let l = LimitSnapshot {
            ts: now,
            source: LimitSource::Statusline,
            five_hour: Some(WindowReading { used_pct: 12.0, resets_at: None, status: None }),
            seven_day: None,
            extra: BTreeMap::new(),
            overall_status: None,
            representative_claim: None,
            overage_status: None,
            session_id: None,
        };
        write(&conn, &StoreMsg::Limits(Box::new(l))).unwrap();
        let ev = load_recent(&conn, 7, now).unwrap();
        assert_eq!(ev.len(), 2);
        assert!(matches!(&ev[0], AppEvent::Request(r) if r.ttft_ms == Some(650)));
    }
}
