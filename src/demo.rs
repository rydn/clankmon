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

const TOOLS: [&str; 8] =
    ["Bash", "Read", "Edit", "Grep", "Write", "Glob", "WebFetch", "mcp__github__create_pull_request"];

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
            turns: 1,
            compactions: 0,
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
                turns: 1,
                compactions: 0,
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
            turns: 0,
            compactions: 0,
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
    // Two requests mid-flight through the proxy.
    ev.extend(LiveSim::new(seed, now).backfill(now));
    for c in [Collector::Jsonl, Collector::Procs, Collector::Statusline, Collector::Hooks, Collector::Proxy] {
        ev.push(AppEvent::Collector(c, CollectorHealth::Ok));
    }
    ev.push(AppEvent::ScanDone { files: 417, lines: 1_204_332, millis: 2_140 });
    ev
}

/// A synthetic request streaming through the proxy.
pub struct SimStream {
    pub key: String,
    pub session: usize,
    pub started: DateTime<Utc>,
    pub final_req: Request,
    /// Output tokens per second while generating.
    pub tps: f64,
    /// Seconds before the first token.
    pub ttft: f64,
}

impl SimStream {
    pub fn new(rng: &mut Rng, ss: &[DemoSession], session: usize, started: DateTime<Utc>, n: u64) -> Self {
        let mut final_req = request(rng, &ss[session], started, n);
        final_req.ts = started;
        final_req.sources = Sources(Sources::PROXY | Sources::JSONL);
        final_req.sidechain = false;
        SimStream {
            key: final_req.key.clone(),
            session,
            started,
            tps: 45.0 + rng.range(0, 90) as f64,
            ttft: 0.6 + rng.range(0, 1400) as f64 / 1000.0,
            final_req,
        }
    }

    /// Visible output so far; thinking tokens never stream as text.
    fn out_at(&self, now: DateTime<Utc>) -> u64 {
        let t = (now - self.started).num_milliseconds() as f64 / 1000.0 - self.ttft;
        let visible = self.final_req.usage.output - self.final_req.usage.thinking;
        ((t.max(0.0) * self.tps) as u64).min(visible)
    }

    fn finished(&self, now: DateTime<Utc>) -> bool {
        self.out_at(now) >= self.final_req.usage.output - self.final_req.usage.thinking
    }

    pub fn progress(&self, ss: &[DemoSession], now: DateTime<Utc>, done: bool) -> AppEvent {
        let mut usage = self.final_req.usage;
        usage.output = self.out_at(now).max(1);
        usage.thinking = 0;
        let elapsed = (now - self.started).num_milliseconds() as f64 / 1000.0;
        AppEvent::Stream(Box::new(StreamProgress {
            key: self.key.clone(),
            ts: now,
            started: self.started,
            session_id: Some(ss[self.session].id.clone()),
            model: ss[self.session].model.into(),
            usage,
            ttft_ms: (elapsed >= self.ttft).then_some((self.ttft * 1000.0) as u64),
            done,
        }))
    }

    pub fn finish(mut self, now: DateTime<Utc>) -> Request {
        let dur = (now - self.started).num_milliseconds().max(1) as u64;
        self.final_req.duration_ms = Some(dur);
        self.final_req.ttft_ms = Some((self.ttft * 1000.0) as u64);
        self.final_req
    }
}

/// Progress events for a stream from its start up to `now`, every 250 ms.
pub fn stream_history(sim: &SimStream, ss: &[DemoSession], now: DateTime<Utc>) -> Vec<AppEvent> {
    let mut out = Vec::new();
    let mut t = sim.started + Duration::milliseconds(150);
    while t <= now {
        out.push(sim.progress(ss, t, false));
        t += Duration::milliseconds(250);
    }
    out
}

/// Deterministic live traffic: requests stream through a simulated proxy for a
/// few seconds each, then land as final records. Drives `--demo` and the docs.
pub struct LiveSim {
    rng: Rng,
    ss: Vec<DemoSession>,
    n: u64,
    five: f64,
    streams: Vec<SimStream>,
}

impl LiveSim {
    /// Starts with two requests already in flight at `now`: one generating,
    /// one still waiting for its first token.
    pub fn new(seed: u64, now: DateTime<Utc>) -> Self {
        let mut rng = Rng::new(seed ^ 0x9e37_79b9);
        let ss = sessions();
        let mut generating = SimStream {
            tps: 84.0,
            ttft: 0.9,
            ..SimStream::new(&mut rng, &ss, 1, now - Duration::milliseconds(4200), 900_001)
        };
        // A long reply, so it is still generating at `now`.
        generating.final_req.usage.output = 1500;
        generating.final_req.usage.thinking = 300;
        let waiting =
            SimStream { ttft: 2.5, ..SimStream::new(&mut rng, &ss, 0, now - Duration::milliseconds(1300), 900_002) };
        LiveSim { rng, ss, n: 1_000_000, five: 81.2, streams: vec![generating, waiting] }
    }

    /// Progress events for the in-flight requests from their start up to `now`.
    pub fn backfill(&self, now: DateTime<Utc>) -> Vec<AppEvent> {
        self.streams.iter().flat_map(|s| stream_history(s, &self.ss, now)).collect()
    }

    /// Advance to `now` (call every ~250 ms).
    pub fn step(&mut self, now: DateTime<Utc>) -> Vec<AppEvent> {
        let mut batch = Vec::new();
        let activity = |sid: &str, state: SessionState, turns: u32| {
            AppEvent::Activity(Activity {
                session_id: sid.to_string(),
                ts: now,
                state,
                cwd: None,
                branch: None,
                version: None,
                model: None,
                effort: None,
                turns,
                compactions: 0,
            })
        };
        // Start a new request now and then, at most one per session at a time.
        if self.rng.chance(0.12) {
            let s = self.rng.range(0, 3) as usize;
            if !self.streams.iter().any(|x| x.session == s) {
                self.n += 1;
                let sim = SimStream::new(&mut self.rng, &self.ss, s, now, self.n);
                let turns = u32::from(self.rng.chance(0.3));
                batch.push(activity(&self.ss[s].id, SessionState::Thinking, turns));
                self.streams.push(sim);
            }
        }
        let mut i = 0;
        while i < self.streams.len() {
            if self.streams[i].finished(now) {
                let sim = self.streams.swap_remove(i);
                batch.push(sim.progress(&self.ss, now, true));
                let sid = self.ss[sim.session].id.clone();
                let r = sim.finish(now);
                let state = match r.tools.first() {
                    Some(t) => SessionState::Tool(t.clone()),
                    None => SessionState::Idle,
                };
                batch.push(AppEvent::Request(Box::new(r)));
                batch.push(activity(&sid, state, 0));
            } else {
                batch.push(self.streams[i].progress(&self.ss, now, false));
                i += 1;
            }
        }
        if self.rng.chance(0.05) {
            self.five = (self.five + self.rng.range(0, 10) as f64 / 10.0).min(99.0);
            batch.push(AppEvent::Limits(Box::new(snapshot(now, self.five, 70.3, now))));
        }
        batch
    }
}

/// Feed live demo traffic into the app every 250 ms.
pub async fn live(tx: UnboundedSender<AppEvent>, seed: u64) {
    let mut sim = LiveSim::new(seed, Utc::now());
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    loop {
        tick.tick().await;
        let batch = sim.step(Utc::now());
        if !batch.is_empty() && tx.send(AppEvent::Batch(batch)).is_err() {
            return;
        }
    }
}
