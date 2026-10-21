//! Synthetic traffic for `clankmon --demo`, screenshots and snapshot tests.
//! Deterministic for a given seed so renders are reproducible.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use tokio::sync::mpsc::UnboundedSender;

use crate::model::*;

/// Tiny xorshift so the demo needs no rand dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo).max(1)
    }
    pub fn chance(&mut self, p: f64) -> bool {
        (self.next_u64() % 10_000) as f64 / 10_000.0 < p
    }
}

pub struct DemoSession {
    pub id: String,
    pub pid: u32,
    pub cwd: String,
    pub name: String,
    pub model: &'static str,
    pub effort: &'static str,
    pub context: u64,
}

pub fn sessions() -> Vec<DemoSession> {
    let mk = |i: u32, cwd: &str, name: &str, model, effort, ctx| DemoSession {
        id: format!("d3m0{i:04}-7c1a-4b2e-9f00-{i:012}"),
        pid: 41_000 + i * 37,
        cwd: format!("/home/neo/src/{cwd}"),
        name: name.into(),
        model,
        effort,
        context: ctx,
    };
    vec![
        mk(1, "night-city/api", "night-city", "claude-opus-5-5", "xhigh", 182_000),
        mk(2, "arasaka/ice-breaker", "ice-breaker", "claude-fable-5-1", "high", 96_000),
        mk(3, "netrunner/deck", "deck-firmware", "claude-sonnet-5-5", "medium", 41_000),
        mk(4, "afterlife/bar-tab", "bar-tab", "claude-haiku-4-5", "low", 12_000),
    ]
}

const TOOLS: [&str; 8] = ["Bash", "Read", "Edit", "Grep", "Write", "Glob", "WebFetch", "mcp__github__create_pull_request"];

pub fn request(rng: &mut Rng, s: &DemoSession, ts: DateTime<Utc>, n: u64) -> Request {
    let fresh = rng.range(2, 900);
    let read = s.context + rng.range(0, 8_000);
    let write = if rng.chance(0.3) { rng.range(2_000, 30_000) } else { rng.range(0, 1_500) };
    let out = rng.range(80, 2_400);
    let tool = rng.chance(0.7).then(|| TOOLS[rng.range(0, TOOLS.len() as u64) as usize].to_string());
    Request {
        key: format!("req_demo{}{n:08}", &s.id[4..8]),
        request_id: Some(format!("req_demo{}{n:08}", &s.id[4..8])),
        ts,
        session_id: Some(s.id.clone()),
        cwd: Some(s.cwd.clone()),
        model: s.model.into(),
        usage: Usage {
            input: fresh,
            output: out,
            cache_read: read,
            cache_write_1h: write,
            thinking: out / 3,
            ..Default::default()
        },
        stop_reason: Some(if tool.is_some() { "tool_use" } else { "end_turn" }.into()),
        ttft_ms: Some(rng.range(450, 2_200)),
        duration_ms: Some(rng.range(1_800, 24_000)),
        http_status: Some(200),
        effort: Some(s.effort.into()),
        tools: tool.into_iter().collect(),
        sidechain: rng.chance(0.15),
        sources: Sources(Sources::JSONL | if rng.chance(0.5) { Sources::PROXY } else { 0 }),
        ..Default::default()
    }
}

fn snapshot(ts: DateTime<Utc>, five: f64, seven: f64, now: DateTime<Utc>) -> LimitSnapshot {
    LimitSnapshot {
        ts,
        source: LimitSource::Statusline,
        five_hour: Some(WindowReading {
            used_pct: five,
            resets_at: Some(now + Duration::minutes(97)),
            status: Some("allowed".into()),
        }),
        seven_day: Some(WindowReading {
            used_pct: seven,
            resets_at: Some(now + Duration::hours(61)),
            status: Some("allowed".into()),
        }),
        extra: BTreeMap::new(),
        overall_status: Some("allowed".into()),
        representative_claim: Some("five_hour".into()),
        overage_status: None,
        session_id: None,
    }
}

/// A full history: 30 days of requests, live processes, limits and a 429.
pub fn history(now: DateTime<Utc>, seed: u64) -> Vec<AppEvent> {
    let mut rng = Rng::new(seed);
    let ss = sessions();
    let mut ev = Vec::new();
    let mut n = 0;
    // Older days: a few bursts per day.
    for day in (1..30).rev() {
        for _ in 0..rng.range(4, 30) {
            let s = &ss[rng.range(0, ss.len() as u64) as usize];
            let ts = now - Duration::days(day) + Duration::minutes(rng.range(0, 1440) as i64 - 720);
            n += 1;
            ev.push(AppEvent::Request(Box::new(request(&mut rng, s, ts, n))));
        }
    }
    // The last two hours: dense traffic for the graphs.
    for m in (0..120).rev() {
        for s in &ss {
            if rng.chance(if s.name == "bar-tab" { 0.1 } else { 0.55 }) {
                let ts = now - Duration::minutes(m) - Duration::seconds(rng.range(1, 59) as i64);
                n += 1;
                ev.push(AppEvent::Request(Box::new(request(&mut rng, s, ts, n))));
            }
        }
    }
    for (i, s) in ss.iter().enumerate() {
        ev.push(AppEvent::Activity(Activity {
            session_id: s.id.clone(),
            ts: now - Duration::minutes(120),
            state: SessionState::Idle,
            cwd: Some(s.cwd.clone()),
            branch: Some(["main", "feat/ice", "fix/deck", "main"][i].into()),
            version: Some("2.1.292".into()),
            model: Some(s.model.into()),
            effort: Some(s.effort.into()),
            user_prompt: true,
            compaction: false,
        }));
        for _ in 0..rng.range(3, 25) {
            ev.push(AppEvent::Activity(Activity {
                session_id: s.id.clone(),
                ts: now - Duration::minutes(rng.range(5, 119) as i64),
                state: SessionState::Thinking,
                cwd: None,
                branch: None,
                version: None,
                model: None,
                effort: None,
                user_prompt: true,
                compaction: false,
            }));
        }
        let state = match i {
            0 => SessionState::Tool("Bash".into()),
            1 => SessionState::Thinking,
            2 => SessionState::Waiting,
            _ => SessionState::Idle,
        };
        ev.push(AppEvent::Activity(Activity {
            session_id: s.id.clone(),
            ts: now - Duration::seconds(5 + i as i64 * 40),
            state,
            cwd: None,
            branch: None,
            version: None,
            model: None,
            effort: None,
            user_prompt: false,
            compaction: false,
        }));
        ev.push(AppEvent::Statusline(Box::new(StatuslineInfo {
            ts: now - Duration::seconds(10),
            session_id: s.id.clone(),
            session_name: Some(s.name.clone()),
            context_used_pct: Some(s.context as f64 / if s.model.contains("haiku") { 2_000.0 } else { 10_000.0 }),
            cost_usd: None,
            cache_warm: Some(true),
            ..Default::default()
        })));
    }
    for t in TOOLS {
        for _ in 0..rng.range(5, 120) {
            n += 1;
            ev.push(AppEvent::Tool(ToolCall {
                session_id: None,
                ts: now,
                name: t.into(),
                success: Some(!rng.chance(0.06)),
                duration_ms: None,
                tool_use_id: Some(format!("toolu_demo{n}")),
            }));
        }
    }
    ev.push(AppEvent::Procs(
        ss.iter()
            .enumerate()
            .map(|(i, s)| ProcInfo {
                pid: s.pid,
                session_id: s.id.clone(),
                cwd: s.cwd.clone(),
                name: Some(s.name.clone()),
                kind: Some("interactive".into()),
                entrypoint: Some("cli".into()),
                version: Some("2.1.292".into()),
                started_at: Some(now - Duration::minutes(130)),
                cpu_pct: Some([18.4, 7.2, 0.4, 0.0][i]),
                rss_kb: Some([412_000, 388_000, 296_000, 201_000][i]),
            })
            .collect(),
    ));
    // Limits climbing over the last 90 minutes.
    for i in 0..=18 {
        let ts = now - Duration::minutes(90 - i * 5);
        ev.push(AppEvent::Limits(Box::new(snapshot(ts, 38.0 + i as f64 * 2.4, 61.0 + i as f64 * 0.5, now))));
    }
    ev.push(AppEvent::Incident(Incident {
        ts: now - Duration::days(2),
        session_id: Some(ss[1].id.clone()),
        kind: "five_hour".into(),
        message: "quota rejected (overage rejected)".into(),
        resets_at: Some(now - Duration::days(2) + Duration::hours(3)),
        http_status: Some(429),
    }));
    for c in [Collector::Jsonl, Collector::Procs, Collector::Statusline, Collector::Hooks, Collector::Proxy] {
        ev.push(AppEvent::Collector(c, CollectorHealth::Ok));
    }
    ev.push(AppEvent::ScanDone { files: 417, lines: 1_204_332, millis: 2_140 });
    ev
}

/// Keep generating live traffic every few hundred milliseconds.
pub async fn live(tx: UnboundedSender<AppEvent>, seed: u64) {
    let mut rng = Rng::new(seed ^ 0x9e37_79b9);
    let ss = sessions();
    let mut n = 1_000_000;
    let mut five = 81.2;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(rng.range(250, 1400))).await;
        let now = Utc::now();
        let s = &ss[rng.range(0, 3) as usize];
        n += 1;
        let r = request(&mut rng, s, now, n);
        let state = match r.tools.first() {
            Some(t) => SessionState::Tool(t.clone()),
            None => SessionState::Thinking,
        };
        let mut batch = vec![
            AppEvent::Request(Box::new(r)),
            AppEvent::Activity(Activity {
                session_id: s.id.clone(),
                ts: now,
                state,
                cwd: None,
                branch: None,
                version: None,
                model: None,
                effort: None,
                user_prompt: rng.chance(0.1),
                compaction: false,
            }),
        ];
        if rng.chance(0.2) {
            five = (five + rng.range(0, 10) as f64 / 10.0).min(99.0);
            batch.push(AppEvent::Limits(Box::new(snapshot(now, five, 70.3, now))));
        }
        if tx.send(AppEvent::Batch(batch)).is_err() {
            return;
        }
    }
}
