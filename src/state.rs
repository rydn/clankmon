//! The reducer: every collector event lands here and becomes one consistent view.
//! Kept free of UI code so it can be unit tested and reused by `export`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Duration, Local, NaiveDate, Utc};

use crate::model::*;
use crate::pricing::Pricing;

/// How long a session stays in the process list after its last activity
/// when no live process backs it (desktop app, IDE, or ended).
const RECENT_SESSION_MINS: i64 = 15;
/// A busy state older than this with no process is considered stale.
const STALE_BUSY_MINS: i64 = 10;
/// Limit history kept in memory.
const LIMIT_HISTORY_DAYS: i64 = 8;

#[derive(Debug, Clone, Default)]
pub struct ToolStat {
    pub calls: u64,
    pub failures: u64,
    pub last_ts: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub branch: Option<String>,
    pub version: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub first_ts: DateTime<Utc>,
    pub last_ts: DateTime<Utc>,
    pub state: SessionState,
    pub state_ts: DateTime<Utc>,
    pub turns: u32,
    pub requests: u32,
    pub subagent_requests: u32,
    pub errors: u32,
    pub usage: Usage,
    pub cost: f64,
    /// Prompt size of the latest main-loop request: the live context.
    pub context_tokens: u64,
    pub context_ts: Option<DateTime<Utc>>,
    pub compactions: u32,
    pub tools: HashMap<String, u32>,
    pub status: Option<StatuslineInfo>,
    pub proc: Option<ProcInfo>,
    /// Wall-clock time the process disappeared, for the fade-out.
    pub ended_at: Option<DateTime<Utc>>,
}

impl Session {
    fn new(id: &str, ts: DateTime<Utc>) -> Self {
        Session {
            id: id.to_string(),
            cwd: None,
            name: None,
            branch: None,
            version: None,
            model: None,
            effort: None,
            first_ts: ts,
            last_ts: ts,
            state: SessionState::Idle,
            state_ts: DateTime::<Utc>::MIN_UTC,
            turns: 0,
            requests: 0,
            subagent_requests: 0,
            errors: 0,
            usage: Usage::default(),
            cost: 0.0,
            context_tokens: 0,
            context_ts: None,
            compactions: 0,
            tools: HashMap::new(),
            status: None,
            proc: None,
            ended_at: None,
        }
    }

    pub fn alive(&self) -> bool {
        self.proc.is_some()
    }

    /// Short display name: statusline name, process name, or the cwd's last component.
    pub fn display_name(&self) -> String {
        if let Some(n) = self.status.as_ref().and_then(|s| s.session_name.clone()) {
            return n;
        }
        if let Some(n) = self.proc.as_ref().and_then(|p| p.name.clone()) {
            return n;
        }
        self.cwd
            .as_deref()
            .and_then(|c| c.rsplit('/').find(|s| !s.is_empty()))
            .map(str::to_string)
            .unwrap_or_else(|| self.id.chars().take(8).collect())
    }

    /// Context use 0..100: the statusline figure when fresh, else derived.
    pub fn context_pct(&self, pricing: &Pricing, now: DateTime<Utc>) -> Option<f64> {
        if let Some(s) = &self.status
            && let Some(p) = s.context_used_pct
            && now - s.ts < Duration::minutes(30)
            && self.context_ts.is_none_or(|t| s.ts >= t - Duration::seconds(5))
        {
            return Some(p);
        }
        let model = self.model.as_deref()?;
        if self.context_tokens == 0 {
            return None;
        }
        let window = self
            .status
            .as_ref()
            .and_then(|s| s.context_window_size)
            .unwrap_or_else(|| pricing.context_window(model));
        Some(self.context_tokens as f64 / window.max(1) as f64 * 100.0)
    }

    /// cache_read / prompt over the session, 0..1.
    pub fn cache_hit(&self) -> Option<f64> {
        if let Some(r) = self.status.as_ref().and_then(|s| s.cache_hit_ratio) {
            return Some(r);
        }
        let p = self.usage.prompt_total();
        (p > 0).then(|| self.usage.cache_read as f64 / p as f64)
    }
}

/// Best current reading per window, plus history for trends.
#[derive(Debug, Clone, Default)]
pub struct Limits {
    pub by_source: BTreeMap<LimitSource, LimitSnapshot>,
    pub five_hour: Option<(WindowReading, LimitSource, DateTime<Utc>)>,
    pub seven_day: Option<(WindowReading, LimitSource, DateTime<Utc>)>,
    pub extra: BTreeMap<String, (WindowReading, LimitSource, DateTime<Utc>)>,
    pub overall_status: Option<String>,
    pub representative_claim: Option<String>,
    pub five_history: VecDeque<(DateTime<Utc>, f64)>,
    pub seven_history: VecDeque<(DateTime<Utc>, f64)>,
}

impl Limits {
    pub fn has_data(&self) -> bool {
        self.five_hour.is_some() || self.seven_day.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Warn,
    Danger,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub ts: DateTime<Utc>,
    pub level: ToastLevel,
    pub text: String,
}

/// Visual effects the UI should play, queued by the reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxCue {
    Glitch,
    PulseFive,
    PulseSeven,
    NewRequest,
}

/// Last persisted (time, 5h %, 7d %) per limit source.
type PersistMark = (DateTime<Utc>, Option<f64>, Option<f64>);

#[derive(Debug, Clone, Default)]
pub struct ScanState {
    pub files_done: usize,
    pub files_total: usize,
    pub finished: bool,
    pub lines: u64,
    pub millis: u64,
}

pub struct State {
    pub pricing: Arc<Pricing>,
    pub requests: Vec<Request>,
    index: HashMap<String, usize>,
    sorted: Vec<usize>,
    needs_sort: bool,
    pub days: BTreeMap<NaiveDate, DayAgg>,
    pub sessions: HashMap<String, Session>,
    pub procs: Vec<ProcInfo>,
    pub limits: Limits,
    pub tools: HashMap<String, ToolStat>,
    tool_ids: HashMap<String, (String, bool)>,
    pub incidents: VecDeque<Incident>,
    pub collectors: BTreeMap<Collector, (CollectorHealth, Option<DateTime<Utc>>)>,
    pub scan: ScanState,
    pub stats_cache: Option<StatsCache>,
    pub oauth: OauthExtra,
    pub plan: Option<String>,
    pub tier: Option<String>,
    pub toasts: VecDeque<Toast>,
    pub fx: Vec<FxCue>,
    /// Bumped on every change, so views can cache derived data.
    pub revision: u64,
    /// Requests the store should persist (non-transcript sources).
    pub persist_requests: Vec<Request>,
    pub persist_limits: Vec<LimitSnapshot>,
    pub persist_incidents: Vec<Incident>,
    last_limit_persist: HashMap<LimitSource, PersistMark>,
    pub started: DateTime<Utc>,
    /// Override "now" in tests and snapshot renders.
    pub clock: Option<DateTime<Utc>>,
}

impl State {
    pub fn new(pricing: Arc<Pricing>) -> Self {
        let mut collectors = BTreeMap::new();
        for c in Collector::ALL {
            collectors.insert(c, (CollectorHealth::Disabled, None));
        }
        State {
            pricing,
            requests: Vec::new(),
            index: HashMap::new(),
            sorted: Vec::new(),
            needs_sort: false,
            days: BTreeMap::new(),
            sessions: HashMap::new(),
            procs: Vec::new(),
            limits: Limits::default(),
            tools: HashMap::new(),
            tool_ids: HashMap::new(),
            incidents: VecDeque::new(),
            collectors,
            scan: ScanState::default(),
            stats_cache: None,
            oauth: OauthExtra::default(),
            plan: None,
            tier: None,
            toasts: VecDeque::new(),
            fx: Vec::new(),
            revision: 0,
            persist_requests: Vec::new(),
            persist_limits: Vec::new(),
            persist_incidents: Vec::new(),
            last_limit_persist: HashMap::new(),
            started: Utc::now(),
            clock: None,
        }
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.clock.unwrap_or_else(Utc::now)
    }

    /// Live events (not the initial scan) raise toasts and effects.
    fn live(&self) -> bool {
        self.scan.finished
    }

    pub fn apply(&mut self, ev: AppEvent) {
        self.revision += 1;
        match ev {
            AppEvent::Batch(evs) => {
                for e in evs {
                    self.apply_one(e);
                }
            }
            e => self.apply_one(e),
        }
    }

    fn apply_one(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Request(r) => self.on_request(*r),
            AppEvent::Activity(a) => self.on_activity(a),
            AppEvent::Tool(t) => self.on_tool(t),
            AppEvent::Procs(p) => self.on_procs(p),
            AppEvent::Limits(l) => self.on_limits(*l),
            AppEvent::Statusline(s) => self.on_statusline(*s),
            AppEvent::Incident(i) => self.on_incident(i),
            AppEvent::StatsCache(s) => self.stats_cache = Some(s),
            AppEvent::Oauth(o) => self.oauth = o,
            AppEvent::Collector(c, h) => {
                let now = self.now();
                let e = self.collectors.entry(c).or_insert((CollectorHealth::Disabled, None));
                e.0 = h;
                e.1 = Some(now);
            }
            AppEvent::ScanProgress { files_done, files_total } => {
                self.scan.files_done = self.scan.files_done.max(files_done);
                self.scan.files_total = files_total;
            }
            AppEvent::ScanDone { files, lines, millis } => {
                self.scan.finished = true;
                self.scan.files_done = files;
                self.scan.files_total = files;
                self.scan.lines = lines;
                self.scan.millis = millis;
                self.refresh_session_states();
            }
            AppEvent::Batch(evs) => {
                for e in evs {
                    self.apply_one(e);
                }
            }
        }
    }

    fn touch(&mut self, c: Collector) {
        let now = self.now();
        let e = self.collectors.entry(c).or_insert((CollectorHealth::Disabled, None));
        if matches!(e.0, CollectorHealth::Disabled | CollectorHealth::Listening) {
            e.0 = CollectorHealth::Ok;
        }
        e.1 = Some(now);
    }

    // -- requests ----------------------------------------------------------

    fn on_request(&mut self, mut r: Request) {
        if r.sources.has(Sources::PROXY) {
            self.touch(Collector::Proxy);
        }
        if r.sources.has(Sources::OTEL) {
            self.touch(Collector::Otel);
        }
        if let Some(&i) = self.index.get(&r.key) {
            let old = self.requests[i].clone();
            self.unaccount(&old);
            let cur = &mut self.requests[i];
            cur.merge(&r);
            let (c, est) = self.pricing.cost(&cur.model, &cur.usage);
            cur.cost_usd = c;
            cur.cost_estimated = est;
            if cur.ts != old.ts {
                self.needs_sort = true;
            }
            let merged = self.requests[i].clone();
            self.account(&merged, false);
            if merged.sources.0 & (Sources::PROXY | Sources::OTEL) != 0 {
                self.persist_requests.push(merged);
            }
        } else {
            let (c, est) = self.pricing.cost(&r.model, &r.usage);
            r.cost_usd = c;
            r.cost_estimated = est;
            let i = self.requests.len();
            if let Some(&last) = self.sorted.last()
                && self.requests[last].ts > r.ts
            {
                self.needs_sort = true;
            }
            self.index.insert(r.key.clone(), i);
            self.sorted.push(i);
            if r.sources.0 & (Sources::PROXY | Sources::OTEL) != 0 {
                self.persist_requests.push(r.clone());
            }
            self.account(&r, true);
            if self.live() {
                self.fx.push(FxCue::NewRequest);
            }
            self.requests.push(r);
        }
    }

    fn account(&mut self, r: &Request, new: bool) {
        let day = r.ts.with_timezone(&Local).date_naive();
        let d = self.days.entry(day).or_default();
        d.usage.add(&r.usage);
        d.cost += r.cost_usd;
        d.requests += 1;
        d.errors += u64::from(r.is_error());
        let m = d.by_model.entry(self.pricing.label(&r.model)).or_insert((0, 0.0));
        m.0 += r.usage.total();
        m.1 += r.cost_usd;

        if let Some(sid) = &r.session_id {
            let s = self
                .sessions
                .entry(sid.clone())
                .or_insert_with(|| Session::new(sid, r.ts));
            s.usage.add(&r.usage);
            s.cost += r.cost_usd;
            s.requests += 1;
            if r.sidechain {
                s.subagent_requests += 1;
            }
            if r.is_error() {
                s.errors += 1;
            }
            if r.ts < s.first_ts {
                s.first_ts = r.ts;
            }
            if r.ts > s.last_ts {
                s.last_ts = r.ts;
            }
            if s.cwd.is_none() {
                s.cwd.clone_from(&r.cwd);
            }
            if !r.sidechain && s.context_ts.is_none_or(|t| r.ts >= t) && r.usage.prompt_total() > 0 {
                s.context_tokens = r.usage.prompt_total();
                s.context_ts = Some(r.ts);
                if !r.model.is_empty() {
                    s.model = Some(r.model.clone());
                }
            }
            if new {
                for t in &r.tools {
                    *s.tools.entry(t.clone()).or_default() += 1;
                }
            }
        }
    }

    fn unaccount(&mut self, r: &Request) {
        let day = r.ts.with_timezone(&Local).date_naive();
        if let Some(d) = self.days.get_mut(&day) {
            d.usage.sub(&r.usage);
            d.cost -= r.cost_usd;
            d.requests = d.requests.saturating_sub(1);
            d.errors = d.errors.saturating_sub(u64::from(r.is_error()));
            if let Some(m) = d.by_model.get_mut(&self.pricing.label(&r.model)) {
                m.0 = m.0.saturating_sub(r.usage.total());
                m.1 -= r.cost_usd;
            }
        }
        if let Some(s) = r.session_id.as_ref().and_then(|sid| self.sessions.get_mut(sid)) {
            s.usage.sub(&r.usage);
            s.cost -= r.cost_usd;
            s.requests = s.requests.saturating_sub(1);
            if r.sidechain {
                s.subagent_requests = s.subagent_requests.saturating_sub(1);
            }
            if r.is_error() {
                s.errors = s.errors.saturating_sub(1);
            }
        }
    }

    /// Re-sort the timeline if arrivals were out of order. Call once per frame
    /// (and before `timeline`/`since`/`recent` after applying events).
    pub fn ensure_sorted(&mut self) {
        if self.needs_sort {
            let reqs = &self.requests;
            self.sorted.sort_by_key(|&i| reqs[i].ts);
            self.needs_sort = false;
        }
    }

    /// Requests in timestamp order (call `ensure_sorted` first).
    pub fn timeline(&self) -> impl DoubleEndedIterator<Item = &Request> {
        self.sorted.iter().map(|&i| &self.requests[i])
    }

    /// Requests since `t`, oldest first (call `ensure_sorted` first).
    pub fn since(&self, t: DateTime<Utc>) -> Vec<&Request> {
        let reqs = &self.requests;
        let start = self.sorted.partition_point(|&i| reqs[i].ts < t);
        self.sorted[start..].iter().map(|&i| &reqs[i]).collect()
    }

    /// Newest `n` requests, newest first (call `ensure_sorted` first).
    pub fn recent(&self, n: usize) -> Vec<&Request> {
        self.sorted.iter().rev().take(n).map(|&i| &self.requests[i]).collect()
    }

    pub fn request(&self, key: &str) -> Option<&Request> {
        self.index.get(key).map(|&i| &self.requests[i])
    }

    pub fn today(&self) -> DayAgg {
        let today = self.now().with_timezone(&Local).date_naive();
        self.days.get(&today).cloned().unwrap_or_default()
    }

    // -- sessions ----------------------------------------------------------

    fn session_mut(&mut self, id: &str, ts: DateTime<Utc>) -> &mut Session {
        self.sessions
            .entry(id.to_string())
            .or_insert_with(|| Session::new(id, ts))
    }

    fn on_activity(&mut self, a: Activity) {
        if a.state == SessionState::Waiting && self.live() {
            let name = self
                .sessions
                .get(&a.session_id)
                .map(Session::display_name)
                .unwrap_or_else(|| a.session_id.chars().take(8).collect());
            self.toast(ToastLevel::Warn, format!("⏸ {name} is waiting for permission"));
        }
        let s = self.session_mut(&a.session_id, a.ts);
        if a.ts > s.last_ts {
            s.last_ts = a.ts;
        }
        if a.ts < s.first_ts {
            s.first_ts = a.ts;
        }
        if a.cwd.is_some() {
            s.cwd = a.cwd;
        }
        if a.branch.is_some() {
            s.branch = a.branch;
        }
        if a.version.is_some() {
            s.version = a.version;
        }
        if a.model.is_some() {
            s.model = a.model;
        }
        if a.effort.is_some() {
            s.effort = a.effort;
        }
        if a.user_prompt {
            s.turns += 1;
        }
        if a.compaction {
            s.compactions += 1;
        }
        // Out-of-order lines (parallel scan, hooks racing the transcript) must
        // not move the state backwards.
        if a.ts >= s.state_ts {
            s.state = a.state;
            s.state_ts = a.ts;
            if s.state == SessionState::Ended && s.ended_at.is_none() {
                s.ended_at = Some(a.ts);
            }
        }
    }

    fn on_tool(&mut self, t: ToolCall) {
        let now = self.now();
        // Correlate by tool_use_id so the transcript and hooks don't double count.
        let (name, count_call) = match &t.tool_use_id {
            Some(id) => {
                let entry = self.tool_ids.entry(id.clone()).or_insert((String::new(), false));
                if entry.0.is_empty() && !t.name.is_empty() {
                    entry.0 = t.name.clone();
                }
                let name = entry.0.clone();
                let first_named = !name.is_empty() && !entry.1;
                if first_named {
                    entry.1 = true;
                }
                (name, first_named)
            }
            None => (t.name.clone(), !t.name.is_empty()),
        };
        if name.is_empty() {
            return;
        }
        let stat = self.tools.entry(name).or_default();
        if count_call {
            stat.calls += 1;
        }
        if t.success == Some(false) {
            stat.failures += 1;
        }
        stat.last_ts = Some(t.ts.max(stat.last_ts.unwrap_or(t.ts)));
        if t.success.is_some() && self.tool_ids.len() > 20_000 {
            // Bound memory: old ids are no longer needed once resolved.
            self.tool_ids.clear();
        }
        let _ = now;
    }

    fn on_procs(&mut self, procs: Vec<ProcInfo>) {
        self.touch(Collector::Procs);
        let now = self.now();
        let live: HashMap<&str, &ProcInfo> = procs.iter().map(|p| (p.session_id.as_str(), p)).collect();
        for s in self.sessions.values_mut() {
            match live.get(s.id.as_str()) {
                Some(p) => {
                    s.proc = Some((*p).clone());
                    s.ended_at = None;
                    if s.state == SessionState::Ended {
                        s.state = SessionState::Idle;
                    }
                }
                None => {
                    if s.proc.take().is_some() {
                        s.ended_at = Some(now);
                        s.state = SessionState::Ended;
                        s.state_ts = now;
                    }
                }
            }
        }
        for p in &procs {
            if !self.sessions.contains_key(&p.session_id) {
                let ts = p.started_at.unwrap_or(now);
                let mut s = Session::new(&p.session_id, ts);
                s.cwd = Some(p.cwd.clone());
                s.version.clone_from(&p.version);
                s.proc = Some(p.clone());
                self.sessions.insert(p.session_id.clone(), s);
            }
        }
        self.procs = procs;
        self.refresh_session_states();
    }

    /// Demote stale busy states for sessions with no live process.
    fn refresh_session_states(&mut self) {
        let now = self.now();
        for s in self.sessions.values_mut() {
            if s.proc.is_none() && s.state.is_busy() && now - s.state_ts > Duration::minutes(STALE_BUSY_MINS) {
                s.state = SessionState::Idle;
            }
        }
    }

    /// Sessions for the process table: live ones, plus recently active or just ended.
    pub fn visible_sessions(&self) -> Vec<&Session> {
        let now = self.now();
        let mut v: Vec<&Session> = self
            .sessions
            .values()
            .filter(|s| {
                s.alive()
                    || now - s.last_ts < Duration::minutes(RECENT_SESSION_MINS)
                    || s.ended_at.is_some_and(|e| now - e < Duration::seconds(60))
            })
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.last_ts));
        v
    }

    fn on_statusline(&mut self, info: StatuslineInfo) {
        self.touch(Collector::Statusline);
        let s = self.session_mut(&info.session_id, info.ts);
        if info.ts > s.last_ts {
            s.last_ts = info.ts;
        }
        if info.cwd.is_some() {
            s.cwd.clone_from(&info.cwd);
        }
        if let Some(m) = &info.model_id {
            s.model = Some(m.clone());
        }
        if info.effort.is_some() {
            s.effort.clone_from(&info.effort);
        }
        if info.version.is_some() {
            s.version.clone_from(&info.version);
        }
        if s.status.as_ref().is_none_or(|old| info.ts >= old.ts) {
            s.status = Some(info);
        }
    }

    // -- limits ------------------------------------------------------------

    fn on_limits(&mut self, snap: LimitSnapshot) {
        match snap.source {
            LimitSource::Statusline => self.touch(Collector::Statusline),
            LimitSource::Proxy => self.touch(Collector::Proxy),
            LimitSource::OAuth => self.touch(Collector::OAuth),
            LimitSource::Transcript => {}
        }
        let prev5 = self.limits.five_hour.as_ref().map(|w| w.0.used_pct);
        let prev7 = self.limits.seven_day.as_ref().map(|w| w.0.used_pct);

        let ts = snap.ts;
        let src = snap.source;
        let take = |cur: &mut Option<(WindowReading, LimitSource, DateTime<Utc>)>, new: &Option<WindowReading>| {
            if let Some(w) = new
                && cur.as_ref().is_none_or(|c| ts >= c.2 || (src < c.1 && ts >= c.2 - Duration::seconds(30)))
            {
                *cur = Some((w.clone(), src, ts));
            }
        };
        take(&mut self.limits.five_hour, &snap.five_hour);
        take(&mut self.limits.seven_day, &snap.seven_day);
        for (k, w) in &snap.extra {
            let cur = self.limits.extra.get(k);
            if cur.is_none_or(|c| ts >= c.2) {
                self.limits.extra.insert(k.clone(), (w.clone(), src, ts));
            }
        }
        if snap.overall_status.is_some() {
            self.limits.overall_status.clone_from(&snap.overall_status);
        }
        if snap.representative_claim.is_some() {
            self.limits.representative_claim.clone_from(&snap.representative_claim);
        }

        if let Some(w) = &snap.five_hour {
            push_history(&mut self.limits.five_history, ts, w.used_pct);
        }
        if let Some(w) = &snap.seven_day {
            push_history(&mut self.limits.seven_history, ts, w.used_pct);
        }

        // Persist when the values change or once a minute per source.
        if src != LimitSource::Transcript {
            let f = snap.five_hour.as_ref().map(|w| w.used_pct);
            let s = snap.seven_day.as_ref().map(|w| w.used_pct);
            let last = self.last_limit_persist.get(&src);
            if last.is_none_or(|(t, lf, ls)| *lf != f || *ls != s || ts - *t > Duration::seconds(60)) {
                self.last_limit_persist.insert(src, (ts, f, s));
                self.persist_limits.push(snap.clone());
            }
        }
        self.limits.by_source.insert(src, snap);

        if self.live() {
            let now5 = self.limits.five_hour.as_ref().map(|w| w.0.used_pct);
            let now7 = self.limits.seven_day.as_ref().map(|w| w.0.used_pct);
            if let Some(t) = crossed(prev5, now5) {
                self.fx.push(FxCue::PulseFive);
                self.toast(level_for(t), format!("5h quota crossed {t:.0}%"));
            }
            if let Some(t) = crossed(prev7, now7) {
                self.fx.push(FxCue::PulseSeven);
                self.toast(level_for(t), format!("7d quota crossed {t:.0}%"));
            }
        }
    }

    fn on_incident(&mut self, i: Incident) {
        let now = self.now();
        // A recent quota rejection with a future reset is itself a limit reading.
        if let Some(reset) = i.resets_at
            && reset > now
            && (i.kind == "five_hour" || i.kind == "seven_day")
        {
            let w = Some(WindowReading { used_pct: 100.0, resets_at: Some(reset), status: Some("rejected".into()) });
            let snap = LimitSnapshot {
                ts: i.ts,
                source: LimitSource::Transcript,
                five_hour: if i.kind == "five_hour" { w.clone() } else { None },
                seven_day: if i.kind == "seven_day" { w } else { None },
                extra: BTreeMap::new(),
                overall_status: Some("rejected".into()),
                representative_claim: Some(i.kind.clone()),
                overage_status: None,
                session_id: i.session_id.clone(),
            };
            self.on_limits(snap);
        }
        if self.live() && now - i.ts < Duration::minutes(2) {
            self.fx.push(FxCue::Glitch);
            self.toast(ToastLevel::Danger, format!("⚠ {}: {}", i.kind, i.message));
            self.persist_incidents.push(i.clone());
        }
        let pos = self.incidents.partition_point(|x| x.ts <= i.ts);
        self.incidents.insert(pos, i);
        while self.incidents.len() > 500 {
            self.incidents.pop_front();
        }
    }

    pub fn toast(&mut self, level: ToastLevel, text: String) {
        let ts = self.now();
        self.toasts.push_back(Toast { ts, level, text });
        while self.toasts.len() > 4 {
            self.toasts.pop_front();
        }
    }

    /// Drop expired toasts; returns true when something changed.
    pub fn expire_toasts(&mut self) -> bool {
        let now = self.now();
        let before = self.toasts.len();
        self.toasts.retain(|t| now - t.ts < Duration::seconds(6));
        before != self.toasts.len()
    }

    /// Current 5h/7d reading, treating a passed reset as zero usage.
    pub fn window(&self, five: bool) -> Option<(WindowReading, LimitSource, DateTime<Utc>)> {
        let w = if five { &self.limits.five_hour } else { &self.limits.seven_day };
        let now = self.now();
        w.clone().map(|(mut r, s, t)| {
            if r.resets_at.is_some_and(|reset| reset <= now) {
                r.used_pct = 0.0;
                r.status = Some("reset".into());
                r.resets_at = None;
            }
            (r, s, t)
        })
    }
}

fn push_history(h: &mut VecDeque<(DateTime<Utc>, f64)>, ts: DateTime<Utc>, pct: f64) {
    // Keep it ordered; statusline frames from several sessions interleave.
    if let Some(last) = h.back()
        && ts < last.0
    {
        let pos = h.partition_point(|p| p.0 <= ts);
        h.insert(pos, (ts, pct));
    } else if h.back().is_some_and(|l| (ts - l.0) < Duration::seconds(20) && l.1 == pct) {
        // Identical reading moments later: skip.
    } else {
        h.push_back((ts, pct));
    }
    while h.front().is_some_and(|f| ts - f.0 > Duration::days(LIMIT_HISTORY_DAYS)) {
        h.pop_front();
    }
}

fn crossed(prev: Option<f64>, now: Option<f64>) -> Option<f64> {
    let (p, n) = (prev?, now?);
    [100.0, 90.0, 80.0].into_iter().find(|&t| p < t && n >= t)
}

fn level_for(t: f64) -> ToastLevel {
    if t >= 90.0 { ToastLevel::Danger } else { ToastLevel::Warn }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(min: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_792_600_000 + min * 60, 0).unwrap()
    }

    fn state() -> State {
        let mut s = State::new(Arc::new(Pricing::default()));
        s.clock = Some(ts(100));
        s
    }

    fn req(key: &str, min: i64, out: u64, src: u8) -> Request {
        Request {
            key: key.into(),
            ts: ts(min),
            session_id: Some("s1".into()),
            model: "claude-opus-5-5".into(),
            usage: Usage { input: 100, output: out, cache_read: 900, ..Default::default() },
            sources: Sources(src),
            ..Default::default()
        }
    }

    #[test]
    fn duplicate_lines_merge_without_double_counting() {
        let mut s = state();
        s.apply(AppEvent::Request(Box::new(req("r1", 90, 10, Sources::JSONL))));
        s.apply(AppEvent::Request(Box::new(req("r1", 90, 500, Sources::JSONL))));
        s.apply(AppEvent::Request(Box::new(req("r1", 90, 500, Sources::PROXY))));
        assert_eq!(s.requests.len(), 1);
        let sess = &s.sessions["s1"];
        assert_eq!(sess.requests, 1);
        assert_eq!(sess.usage.output, 500);
        let today = s.days.values().next().unwrap();
        assert_eq!(today.requests, 1);
        assert_eq!(today.usage.output, 500);
        assert!((today.cost - s.requests[0].cost_usd).abs() < 1e-12);
        assert_eq!(s.persist_requests.len(), 1, "only the proxy-tagged merge persists");
    }

    #[test]
    fn timeline_sorts_out_of_order_arrivals() {
        let mut s = state();
        s.apply(AppEvent::Request(Box::new(req("b", 95, 1, Sources::JSONL))));
        s.apply(AppEvent::Request(Box::new(req("a", 91, 1, Sources::JSONL))));
        s.ensure_sorted();
        let keys: Vec<_> = s.timeline().map(|r| r.key.clone()).collect();
        assert_eq!(keys, vec!["a", "b"]);
        assert_eq!(s.since(ts(92)).len(), 1);
        assert_eq!(s.recent(1)[0].key, "b");
    }

    #[test]
    fn activity_does_not_move_backwards() {
        let mut s = state();
        let mk = |min, state| Activity {
            session_id: "s1".into(),
            ts: ts(min),
            state,
            cwd: Some("/x/clankmon".into()),
            branch: None,
            version: None,
            model: None,
            effort: None,
            user_prompt: false,
            compaction: false,
        };
        s.apply(AppEvent::Activity(mk(99, SessionState::Tool("Bash".into()))));
        s.apply(AppEvent::Activity(mk(98, SessionState::Idle)));
        assert_eq!(s.sessions["s1"].state, SessionState::Tool("Bash".into()));
        assert_eq!(s.sessions["s1"].display_name(), "clankmon");
    }

    #[test]
    fn procs_attach_and_end_sessions() {
        let mut s = state();
        let p = ProcInfo { pid: 42, session_id: "s1".into(), cwd: "/x".into(), ..Default::default() };
        s.apply(AppEvent::Procs(vec![p]));
        assert!(s.sessions["s1"].alive());
        assert_eq!(s.visible_sessions().len(), 1);
        s.apply(AppEvent::Procs(vec![]));
        assert_eq!(s.sessions["s1"].state, SessionState::Ended);
        assert!(s.sessions["s1"].ended_at.is_some());
    }

    #[test]
    fn tools_correlate_by_id() {
        let mut s = state();
        let use_ = ToolCall { session_id: None, ts: ts(1), name: "Bash".into(), success: None, duration_ms: None, tool_use_id: Some("t1".into()) };
        let result = ToolCall { name: String::new(), success: Some(false), ..use_.clone() };
        let hook = ToolCall { name: "Bash".into(), success: Some(false), ..use_.clone() };
        s.apply(AppEvent::Tool(use_));
        s.apply(AppEvent::Tool(result));
        s.apply(AppEvent::Tool(hook));
        assert_eq!(s.tools["Bash"].calls, 1);
        assert_eq!(s.tools["Bash"].failures, 2, "transcript and hook both report the failure");
    }

    #[test]
    fn limits_take_freshest_and_toast_on_crossing() {
        let mut s = state();
        s.scan.finished = true;
        let snap = |min, pct, src| LimitSnapshot {
            ts: ts(min),
            source: src,
            five_hour: Some(WindowReading { used_pct: pct, resets_at: Some(ts(200)), status: None }),
            seven_day: None,
            extra: BTreeMap::new(),
            overall_status: None,
            representative_claim: None,
            overage_status: None,
            session_id: None,
        };
        s.apply(AppEvent::Limits(Box::new(snap(90, 75.0, LimitSource::Statusline))));
        s.apply(AppEvent::Limits(Box::new(snap(95, 82.0, LimitSource::Proxy))));
        let (w, src, _) = s.window(true).unwrap();
        assert_eq!(w.used_pct, 82.0);
        assert_eq!(src, LimitSource::Proxy);
        assert!(s.fx.contains(&FxCue::PulseFive));
        assert!(s.toasts.iter().any(|t| t.text.contains("80%")));
        // An older reading does not overwrite.
        s.apply(AppEvent::Limits(Box::new(snap(80, 10.0, LimitSource::OAuth))));
        assert_eq!(s.window(true).unwrap().0.used_pct, 82.0);
        assert_eq!(s.limits.five_history.len(), 3);
    }

    #[test]
    fn passed_reset_reads_as_zero() {
        let mut s = state();
        s.apply(AppEvent::Limits(Box::new(LimitSnapshot {
            ts: ts(10),
            source: LimitSource::Statusline,
            five_hour: Some(WindowReading { used_pct: 64.0, resets_at: Some(ts(50)), status: None }),
            seven_day: None,
            extra: BTreeMap::new(),
            overall_status: None,
            representative_claim: None,
            overage_status: None,
            session_id: None,
        })));
        let (w, _, _) = s.window(true).unwrap();
        assert_eq!(w.used_pct, 0.0);
        assert_eq!(w.status.as_deref(), Some("reset"));
    }

    #[test]
    fn future_quota_rejection_becomes_full_window() {
        let mut s = state();
        s.apply(AppEvent::Incident(Incident {
            ts: ts(99),
            session_id: None,
            kind: "five_hour".into(),
            message: "quota rejected".into(),
            resets_at: Some(ts(160)),
            http_status: Some(429),
        }));
        assert_eq!(s.window(true).unwrap().0.used_pct, 100.0);
        assert_eq!(s.incidents.len(), 1);
    }

    #[test]
    fn usage_merge_does_not_double_count_ttl_split() {
        let mut a = Usage { cache_write_1h: 36262, ..Default::default() };
        a.merge_max(&Usage { cache_write_5m: 36262, ..Default::default() });
        assert_eq!(a.cache_write(), 36262);
        assert_eq!(a.cache_write_1h, 36262);
    }
}
