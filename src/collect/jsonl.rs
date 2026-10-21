//! Claude Code transcript parsing and tailing.
//!
//! Transcripts live at `~/.claude/projects/<slug>/<session>.jsonl`, with
//! subagent transcripts under `<slug>/<session>/subagents/agent-*.jsonl`.
//! Each line is one JSON object. Assistant lines carry `message.usage`;
//! streaming writes one line per content block with the same `message.id`
//! and `requestId`, so the app merges them by key.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::model::{
    Activity, AppEvent, Collector, CollectorHealth, Incident, Request, SessionState, Sources, ToolCall, Usage,
};
use crate::pricing::Pricing;

// ---------------------------------------------------------------------------
// Line schema (only the fields clankmon needs; everything else is skipped).
// ---------------------------------------------------------------------------

/// Borrowed string: no allocation unless the JSON contains escapes.
type S<'a> = Option<Cow<'a, str>>;

fn own(s: &S<'_>) -> Option<String> {
    s.as_deref().map(str::to_string)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    kind: S<'a>,
    #[serde(borrow)]
    subtype: S<'a>,
    timestamp: Option<DateTime<Utc>>,
    #[serde(borrow)]
    session_id: S<'a>,
    #[serde(borrow)]
    request_id: S<'a>,
    #[serde(borrow)]
    uuid: S<'a>,
    #[serde(borrow)]
    cwd: S<'a>,
    #[serde(borrow)]
    git_branch: S<'a>,
    #[serde(borrow)]
    version: S<'a>,
    #[serde(default)]
    is_sidechain: bool,
    #[serde(default)]
    is_meta: bool,
    #[serde(default)]
    is_api_error_message: bool,
    api_error_status: Option<serde_json::Value>,
    #[serde(borrow)]
    effort: S<'a>,
    #[serde(borrow)]
    per_turn_effort: S<'a>,
    quota_limits: Option<QuotaLimits>,
    tool_use_result: Option<serde::de::IgnoredAny>,
    #[serde(borrow)]
    message: Option<Message<'a>>,
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Message<'a> {
    #[serde(borrow)]
    id: S<'a>,
    #[serde(borrow)]
    model: S<'a>,
    #[serde(borrow)]
    stop_reason: S<'a>,
    usage: Option<RawUsage>,
    #[serde(borrow)]
    content: Option<Content<'a>>,
}

/// Message content: a plain string or a list of blocks.
///
/// Hand-written instead of `#[serde(untagged)]`: untagged enums buffer the whole
/// value into an intermediate tree before trying variants, which copies every
/// multi-megabyte tool result. This visitor skips strings without allocating.
#[derive(Debug)]
enum Content<'a> {
    Text,
    Blocks(Vec<Block<'a>>),
}

impl<'de: 'a, 'a> Deserialize<'de> for Content<'a> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<'a>(std::marker::PhantomData<&'a ()>);
        impl<'de: 'a, 'a> serde::de::Visitor<'de> for V<'a> {
            type Value = Content<'a>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string or an array of content blocks")
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Content<'a>, E> {
                Ok(Content::Text)
            }
            fn visit_borrowed_str<E: serde::de::Error>(self, _: &'de str) -> Result<Content<'a>, E> {
                Ok(Content::Text)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Content<'a>, E> {
                Ok(Content::Text)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Content<'a>, A::Error> {
                let mut out = Vec::new();
                while let Some(b) = seq.next_element::<Block<'a>>()? {
                    out.push(b);
                }
                Ok(Content::Blocks(out))
            }
        }
        d.deserialize_any(V(std::marker::PhantomData))
    }
}

#[derive(Debug, Deserialize)]
struct Block<'a> {
    #[serde(rename = "type", borrow)]
    kind: S<'a>,
    #[serde(borrow)]
    name: S<'a>,
    #[serde(borrow)]
    id: S<'a>,
    #[serde(borrow)]
    tool_use_id: S<'a>,
    is_error: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
    output_tokens_details: Option<OutputDetails>,
    service_tier: Option<String>,
    speed: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Debug, Default, Deserialize)]
struct OutputDetails {
    #[serde(default)]
    thinking_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaLimits {
    status: Option<String>,
    resets_at: Option<i64>,
    rate_limit_type: Option<String>,
    overage_status: Option<String>,
}

impl RawUsage {
    fn to_usage(&self) -> Usage {
        let (w5, w1) = match &self.cache_creation {
            Some(cc) if cc.ephemeral_5m_input_tokens + cc.ephemeral_1h_input_tokens > 0 => {
                (cc.ephemeral_5m_input_tokens, cc.ephemeral_1h_input_tokens)
            }
            // No TTL breakdown: the API default TTL is 5 minutes.
            _ => (self.cache_creation_input_tokens, 0),
        };
        Usage {
            input: self.input_tokens,
            output: self.output_tokens,
            cache_read: self.cache_read_input_tokens,
            cache_write_5m: w5,
            cache_write_1h: w1,
            thinking: self.output_tokens_details.as_ref().map_or(0, |d| d.thinking_tokens),
        }
    }
}

/// Convert an API `usage` object (transcript, SSE or JSON response) to [`Usage`].
pub fn usage_from_value(v: &serde_json::Value) -> Usage {
    RawUsage::deserialize(v).map(|r| r.to_usage()).unwrap_or_default()
}

/// The `service_tier` / `speed` fields of an API `usage` object.
pub fn usage_tier(v: &serde_json::Value) -> (Option<String>, Option<String>) {
    RawUsage::deserialize(v).map(|r| (r.service_tier, r.speed)).unwrap_or_default()
}

/// Cheap pre-filter: only these line types matter, and a substring check is far
/// cheaper than deserialising multi-megabyte tool results.
fn interesting(line: &str) -> bool {
    line.contains("\"type\":\"assistant\"")
        || line.contains("\"type\":\"user\"")
        || line.contains("\"type\":\"system\"")
}

/// Parse one transcript line into app events. `file_session` is the session id
/// implied by the file name, used when a line lacks `sessionId`.
pub fn parse_line(raw: &str, file_session: Option<&str>, pricing: &Pricing) -> Vec<AppEvent> {
    if !interesting(raw) {
        return Vec::new();
    }
    let Ok(line) = serde_json::from_str::<Line>(raw) else {
        return Vec::new();
    };
    let Some(ts) = line.timestamp else {
        return Vec::new();
    };
    let session_id = own(&line.session_id).or_else(|| file_session.map(str::to_string));
    let mut out = Vec::new();

    match line.kind.as_deref() {
        Some("assistant") => assistant_events(&line, ts, session_id, pricing, &mut out),
        Some("user") => user_events(&line, ts, session_id, &mut out),
        Some("system") => system_events(&line, ts, session_id, &mut out),
        _ => {}
    }
    out
}

fn activity(line: &Line<'_>, ts: DateTime<Utc>, sid: &str, state: SessionState) -> Activity {
    Activity {
        session_id: sid.to_string(),
        ts,
        state,
        cwd: own(&line.cwd),
        branch: own(&line.git_branch),
        version: own(&line.version),
        model: None,
        effort: own(&line.per_turn_effort).or_else(|| own(&line.effort)),
        turns: 0,
        compactions: 0,
    }
}

fn assistant_events(
    line: &Line<'_>,
    ts: DateTime<Utc>,
    session_id: Option<String>,
    pricing: &Pricing,
    out: &mut Vec<AppEvent>,
) {
    let Some(msg) = &line.message else { return };
    let model = own(&msg.model).unwrap_or_default();

    // API error lines: a synthetic assistant message carrying the failure.
    if line.is_api_error_message {
        let status = line
            .api_error_status
            .as_ref()
            .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .map(|s| s as u16);
        if let Some(q) = &line.quota_limits {
            out.push(AppEvent::Incident(Incident {
                ts,
                session_id: session_id.clone(),
                kind: q.rate_limit_type.clone().unwrap_or_else(|| "rate_limit".into()),
                message: format!(
                    "quota {} (overage {})",
                    q.status.as_deref().unwrap_or("rejected"),
                    q.overage_status.as_deref().unwrap_or("n/a")
                ),
                resets_at: q.resets_at.and_then(|s| DateTime::from_timestamp(s, 0)),
                http_status: status.or(Some(429)),
            }));
        } else if status.is_some() {
            out.push(AppEvent::Incident(Incident {
                ts,
                session_id: session_id.clone(),
                kind: "api_error".into(),
                message: format!("HTTP {}", status.unwrap_or(0)),
                resets_at: None,
                http_status: status,
            }));
        }
        return;
    }

    let tools: Vec<(String, Option<String>)> = match &msg.content {
        Some(Content::Blocks(blocks)) => blocks
            .iter()
            .filter(|b| b.kind.as_deref() == Some("tool_use"))
            .filter_map(|b| own(&b.name).map(|n| (n, own(&b.id))))
            .collect(),
        _ => Vec::new(),
    };

    if let Some(raw) = &msg.usage {
        let usage = raw.to_usage();
        let key = own(&line.request_id).or_else(|| own(&msg.id)).or_else(|| own(&line.uuid)).unwrap_or_default();
        if !key.is_empty() && model != "<synthetic>" {
            let (cost, estimated) = pricing.cost(&model, &usage);
            out.push(AppEvent::Request(Box::new(Request {
                key,
                request_id: own(&line.request_id),
                message_id: own(&msg.id),
                ts,
                session_id: session_id.clone(),
                cwd: own(&line.cwd),
                model: model.clone(),
                usage,
                cost_usd: cost,
                cost_estimated: estimated,
                stop_reason: own(&msg.stop_reason),
                sidechain: line.is_sidechain,
                effort: own(&line.per_turn_effort).or_else(|| own(&line.effort)),
                service_tier: raw.service_tier.clone(),
                speed: raw.speed.clone(),
                tools: tools.iter().map(|(n, _)| n.clone()).collect(),
                sources: Sources(Sources::JSONL),
                ..Default::default()
            })));
        }
    }

    // Subagent lines describe the subagent, not the session's main loop.
    if line.is_sidechain {
        return;
    }
    let Some(sid) = session_id else { return };
    let state = if let Some((name, _)) = tools.last() {
        SessionState::Tool(name.clone())
    } else {
        match msg.stop_reason.as_deref() {
            Some("end_turn") | Some("stop_sequence") | Some("max_tokens") | Some("refusal") => SessionState::Idle,
            _ => SessionState::Thinking,
        }
    };
    let mut a = activity(line, ts, &sid, state);
    a.model = (!model.is_empty() && model != "<synthetic>").then_some(model);
    out.push(AppEvent::Activity(a));
    for (name, id) in tools {
        out.push(AppEvent::Tool(ToolCall {
            session_id: Some(sid.clone()),
            ts,
            name,
            success: None,
            duration_ms: None,
            tool_use_id: id,
        }));
    }
}

fn user_events(line: &Line<'_>, ts: DateTime<Utc>, session_id: Option<String>, out: &mut Vec<AppEvent>) {
    if line.is_sidechain || line.is_meta {
        return;
    }
    let Some(sid) = session_id else { return };
    let blocks = match line.message.as_ref().and_then(|m| m.content.as_ref()) {
        Some(Content::Blocks(b)) => Some(b),
        _ => None,
    };
    let tool_results: Vec<&Block<'_>> =
        blocks.map(|b| b.iter().filter(|b| b.kind.as_deref() == Some("tool_result")).collect()).unwrap_or_default();

    if line.tool_use_result.is_some() || !tool_results.is_empty() {
        // A tool finished; the model is now working on the result.
        out.push(AppEvent::Activity(activity(line, ts, &sid, SessionState::Thinking)));
        for b in tool_results {
            out.push(AppEvent::Tool(ToolCall {
                session_id: Some(sid.clone()),
                ts,
                name: String::new(),
                success: Some(!b.is_error.unwrap_or(false)),
                duration_ms: None,
                tool_use_id: own(&b.tool_use_id),
            }));
        }
    } else {
        let mut a = activity(line, ts, &sid, SessionState::Thinking);
        a.turns = 1;
        out.push(AppEvent::Activity(a));
    }
}

fn system_events(line: &Line<'_>, ts: DateTime<Utc>, session_id: Option<String>, out: &mut Vec<AppEvent>) {
    let Some(sid) = session_id else { return };
    match line.subtype.as_deref() {
        Some("compact_boundary") => {
            let mut a = activity(line, ts, &sid, SessionState::Thinking);
            a.compactions = 1;
            out.push(AppEvent::Activity(a));
        }
        Some("turn_duration") | Some("stop_hook_summary") => {
            out.push(AppEvent::Activity(activity(line, ts, &sid, SessionState::Idle)));
        }
        Some("api_error") => {
            let msg = line
                .error
                .as_ref()
                .map(|e| match e {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| "api error".into());
            out.push(AppEvent::Incident(Incident {
                ts,
                session_id: Some(sid),
                kind: "api_error".into(),
                message: truncate(&msg, 160),
                resets_at: None,
                http_status: None,
            }));
        }
        _ => {}
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}

// ---------------------------------------------------------------------------
// Initial-scan coalescing.
// ---------------------------------------------------------------------------

/// Coalesces one file's events during the initial scan so memory stays flat:
/// one merged record per request (streaming writes several lines each), one
/// activity summary per session, and aggregated tool counts.
#[derive(Default)]
struct ScanAcc {
    requests: HashMap<String, Request>,
    activity: HashMap<String, Activity>,
    tools: HashMap<String, (u64, u64, DateTime<Utc>)>,
    tool_names: HashMap<String, String>,
    other: Vec<AppEvent>,
}

impl ScanAcc {
    fn push(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Request(r) => match self.requests.get_mut(&r.key) {
                Some(cur) => cur.merge(&r),
                None => {
                    self.requests.insert(r.key.clone(), *r);
                }
            },
            AppEvent::Activity(a) => match self.activity.get_mut(&a.session_id) {
                Some(cur) => {
                    cur.turns += a.turns;
                    cur.compactions += a.compactions;
                    let newer = a.ts >= cur.ts;
                    if newer {
                        cur.ts = a.ts;
                        cur.state = a.state;
                    }
                    for (dst, src) in [
                        (&mut cur.cwd, a.cwd),
                        (&mut cur.branch, a.branch),
                        (&mut cur.version, a.version),
                        (&mut cur.model, a.model),
                        (&mut cur.effort, a.effort),
                    ] {
                        if src.is_some() && (newer || dst.is_none()) {
                            *dst = src;
                        }
                    }
                }
                None => {
                    self.activity.insert(a.session_id.clone(), a);
                }
            },
            AppEvent::Tool(t) => {
                if !t.name.is_empty() {
                    // Count each tool_use_id once, even if a stream line repeats.
                    let first = match &t.tool_use_id {
                        Some(id) => self.tool_names.insert(id.clone(), t.name.clone()).is_none(),
                        None => true,
                    };
                    let e = self.tools.entry(t.name.clone()).or_insert((0, 0, t.ts));
                    if first {
                        e.0 += 1;
                    }
                    e.2 = e.2.max(t.ts);
                }
                if t.success == Some(false) {
                    let name = if t.name.is_empty() {
                        t.tool_use_id.as_ref().and_then(|id| self.tool_names.get(id)).cloned()
                    } else {
                        Some(t.name.clone())
                    };
                    if let Some(n) = name {
                        self.tools.entry(n).or_insert((0, 0, t.ts)).1 += 1;
                    }
                }
            }
            other => self.other.push(other),
        }
    }

    fn finish(self) -> Vec<AppEvent> {
        let mut out: Vec<AppEvent> = self.requests.into_values().map(|r| AppEvent::Request(Box::new(r))).collect();
        out.extend(self.activity.into_values().map(AppEvent::Activity));
        if !self.tools.is_empty() {
            out.push(AppEvent::ToolTotals(self.tools.into_iter().map(|(n, (c, f, l))| (n, c, f, l)).collect()));
        }
        out.extend(self.other);
        out
    }
}

// ---------------------------------------------------------------------------
// Discovery, initial scan and tailing.
// ---------------------------------------------------------------------------

/// Session id implied by a transcript path: the file stem for top-level
/// transcripts, the parent session dir for subagent transcripts.
pub fn session_for_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if stem.starts_with("agent-") {
        // <slug>/<session>/subagents/agent-x.jsonl
        return path.parent()?.parent()?.file_name()?.to_str().map(str::to_string);
    }
    Some(stem.to_string())
}

/// All transcripts modified within `max_age`.
pub fn discover(projects: &Path, max_age: Duration) -> Vec<PathBuf> {
    let cutoff = SystemTime::now().checked_sub(max_age).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut out = Vec::new();
    let mut stack = vec![projects.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                let recent = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t >= cutoff);
                if recent {
                    out.push(path);
                }
            }
        }
    }
    out
}

/// Per-file read position.
#[derive(Debug, Default, Clone)]
struct FileCursor {
    offset: u64,
    /// Bytes after the last newline (a line still being written).
    partial: Vec<u8>,
}

/// Read complete lines from `cursor.offset` to EOF.
fn read_new_lines(path: &Path, cursor: &mut FileCursor, mut f: impl FnMut(&str)) -> std::io::Result<u64> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len < cursor.offset {
        // Truncated or replaced: start over.
        cursor.offset = 0;
        cursor.partial.clear();
    }
    if len == cursor.offset {
        return Ok(0);
    }
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file.take(len - cursor.offset));
    let mut lines = 0;
    let mut buf = std::mem::take(&mut cursor.partial);
    loop {
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        if buf.last() == Some(&b'\n') {
            if let Ok(s) = std::str::from_utf8(&buf) {
                f(s.trim_end());
            }
            lines += 1;
            buf.clear();
        }
    }
    // Keep only the bytes of an unfinished line. Reusing `buf` would retain its
    // capacity (the file's longest line) in every cursor: ~85 MB over 400 files.
    cursor.partial = if buf.is_empty() { Vec::new() } else { buf[..].to_vec() };
    cursor.offset = len;
    Ok(lines)
}

/// Scans every recent transcript in parallel, then tails them.
pub struct Tailer {
    projects: PathBuf,
    max_age: Duration,
    pricing: Arc<Pricing>,
    tx: UnboundedSender<AppEvent>,
}

impl Tailer {
    pub fn new(projects: PathBuf, history_days: u32, pricing: Arc<Pricing>, tx: UnboundedSender<AppEvent>) -> Self {
        Tailer { projects, max_age: Duration::from_secs(u64::from(history_days.max(1)) * 86_400), pricing, tx }
    }

    /// Blocking: run on a dedicated thread.
    pub fn run(self) {
        let started = Instant::now();
        let files = discover(&self.projects, self.max_age);
        let total = files.len();
        let _ = self.tx.send(AppEvent::ScanProgress { files_done: 0, files_total: total });

        let cursors = self.initial_scan(&files);
        let lines: u64 = cursors.values().map(|c| c.1).sum();
        crate::mem::release();
        let mut cursors: HashMap<PathBuf, FileCursor> = cursors.into_iter().map(|(p, (c, _))| (p, c)).collect();
        let _ = self.tx.send(AppEvent::ScanDone { files: total, lines, millis: started.elapsed().as_millis() as u64 });
        let _ = self.tx.send(AppEvent::Collector(Collector::Jsonl, CollectorHealth::Ok));

        self.tail(&mut cursors);
    }

    /// Scan once and stop (for `export`). Sends `ScanDone` at the end.
    pub fn scan_only(self) {
        let started = Instant::now();
        let files = discover(&self.projects, self.max_age);
        let res = self.initial_scan(&files);
        crate::mem::release();
        let _ = self.tx.send(AppEvent::ScanDone {
            files: files.len(),
            lines: res.values().map(|c| c.1).sum(),
            millis: started.elapsed().as_millis() as u64,
        });
    }

    fn initial_scan(&self, files: &[PathBuf]) -> HashMap<PathBuf, (FileCursor, u64)> {
        // Largest first so the slowest files start early.
        let mut files: Vec<(PathBuf, u64)> =
            files.iter().map(|p| (p.clone(), std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))).collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.1));
        let queue = Arc::new(parking_lot::Mutex::new(files));
        let done = Arc::new(AtomicUsize::new(0));
        let total = queue.lock().len();
        let threads = std::env::var("CLANKMON_SCAN_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).min(8))
            .max(1);
        let results = Arc::new(parking_lot::Mutex::new(HashMap::new()));

        std::thread::scope(|s| {
            for _ in 0..threads {
                let queue = queue.clone();
                let done = done.clone();
                let results = results.clone();
                let tx = self.tx.clone();
                let pricing = self.pricing.clone();
                s.spawn(move || {
                    loop {
                        let Some((path, _)) = queue.lock().pop() else { break };
                        let sid = session_for_path(&path);
                        let mut cursor = FileCursor::default();
                        let mut acc = ScanAcc::default();
                        let n = read_new_lines(&path, &mut cursor, |l| {
                            for e in parse_line(l, sid.as_deref(), &pricing) {
                                acc.push(e);
                            }
                        })
                        .unwrap_or(0);
                        let batch = acc.finish();
                        if !batch.is_empty() {
                            let _ = tx.send(AppEvent::Batch(batch));
                        }
                        results.lock().insert(path, (cursor, n));
                        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if d.is_multiple_of(16) || d == total {
                            let _ = tx.send(AppEvent::ScanProgress { files_done: d, files_total: total });
                        }
                    }
                });
            }
        });
        Arc::try_unwrap(results).map(|m| m.into_inner()).unwrap_or_default()
    }

    fn tail(&self, cursors: &mut HashMap<PathBuf, FileCursor>) {
        use notify::{RecursiveMode, Watcher};
        let (wtx, wrx) = std::sync::mpsc::channel::<PathBuf>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                for p in ev.paths {
                    if p.extension().is_some_and(|e| e == "jsonl") {
                        let _ = wtx.send(p);
                    }
                }
            }
        });
        let _watcher = match watcher {
            Ok(mut w) => match w.watch(&self.projects, RecursiveMode::Recursive) {
                Ok(()) => Some(w),
                Err(e) => {
                    tracing::warn!("file watch failed, falling back to polling: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::warn!("file watcher unavailable: {e}");
                None
            }
        };

        let mut last_rescan = Instant::now();
        let mut sizes: HashMap<PathBuf, u64> = HashMap::new();
        let mut hot: Vec<PathBuf> = discover(&self.projects, Duration::from_secs(3600));
        loop {
            // Wake on a file event or every second as a safety net (FSEvents coalesces).
            let mut dirty: Vec<PathBuf> = Vec::new();
            match wrx.recv_timeout(Duration::from_secs(1)) {
                Ok(p) => {
                    dirty.push(p);
                    // Coalesce a burst of writes into one read.
                    std::thread::sleep(Duration::from_millis(30));
                    while let Ok(p) = wrx.try_recv() {
                        dirty.push(p);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            if self.tx.is_closed() {
                return;
            }

            // Safety net for missed file events: stat only recently active files
            // each second (old sessions never change), and rediscover every 5 s.
            if last_rescan.elapsed() > Duration::from_secs(5) {
                last_rescan = Instant::now();
                hot = discover(&self.projects, Duration::from_secs(3600));
                for p in &hot {
                    if !cursors.contains_key(p) {
                        dirty.push(p.clone());
                    }
                }
            }
            for path in &hot {
                let Some(cursor) = cursors.get(path) else { continue };
                if let Ok(m) = std::fs::metadata(path) {
                    let len = m.len();
                    if len != cursor.offset && sizes.get(path) != Some(&len) {
                        dirty.push(path.clone());
                    }
                    sizes.insert(path.clone(), len);
                }
            }

            dirty.sort();
            dirty.dedup();
            let mut batch = Vec::new();
            for path in dirty {
                let cursor = cursors.entry(path.clone()).or_default();
                let sid = session_for_path(&path);
                let _ = read_new_lines(&path, cursor, |l| {
                    batch.extend(parse_line(l, sid.as_deref(), &self.pricing));
                });
            }
            if !batch.is_empty() && self.tx.send(AppEvent::Batch(batch)).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSISTANT: &str = r#"{"parentUuid":"6384","isSidechain":false,"thinkingDurationMs":24891,"requestId":"req_011CgFuuxWMe6UxDdZwoYJSi","type":"assistant","uuid":"a0e9","timestamp":"2026-10-21T19:12:36.011Z","effort":"high","perTurnEffort":"high","message":{"model":"claude-fable-5-1","id":"msg_01","type":"message","role":"assistant","content":[{"type":"thinking","thinking":""},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}],"stop_reason":"tool_use","stop_sequence":null,"stop_details":null,"usage":{"input_tokens":2,"cache_creation_input_tokens":36262,"cache_read_input_tokens":25235,"output_tokens":1915,"output_tokens_details":{"thinking_tokens":1620},"server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},"service_tier":"standard","cache_creation":{"ephemeral_1h_input_tokens":36262,"ephemeral_5m_input_tokens":0}}},"cwd":"/Users/me/Dev/clankmon","sessionId":"3836","version":"2.1.292","gitBranch":"main"}"#;

    const TOOL_RESULT: &str = r#"{"parentUuid":"ffef","isSidechain":false,"type":"user","uuid":"f02d","timestamp":"2026-10-21T19:12:39.989Z","toolUseResult":"Error: denied","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","is_error":true,"content":"Error: denied"}]},"sessionId":"3836","cwd":"/x"}"#;

    const PROMPT: &str = r#"{"type":"user","isSidechain":false,"timestamp":"2026-10-21T19:10:00Z","message":{"role":"user","content":"build a tui"},"sessionId":"3836","cwd":"/x"}"#;

    const QUOTA: &str = r#"{"type":"assistant","timestamp":"2026-10-05T10:00:00Z","isApiErrorMessage":true,"apiErrorStatus":429,"error":"rate_limit","quotaLimits":{"status":"rejected","resetsAt":1791213000,"unifiedRateLimitFallbackAvailable":false,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"out_of_credits","isUsingOverage":false},"message":{"model":"<synthetic>","id":"x","content":[{"type":"text","text":"limit"}],"usage":{"input_tokens":0,"output_tokens":0}},"sessionId":"s9"}"#;

    const COMPACT: &str = r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-10-19T08:51:05.675Z","sessionId":"8d5b","cwd":"/x","isSidechain":false}"#;

    fn p() -> Pricing {
        Pricing::default()
    }

    #[test]
    fn assistant_line_yields_request_activity_and_tool() {
        let ev = parse_line(ASSISTANT, None, &p());
        let req = ev.iter().find_map(|e| match e {
            AppEvent::Request(r) => Some(r),
            _ => None,
        });
        let req = req.expect("request");
        assert_eq!(req.key, "req_011CgFuuxWMe6UxDdZwoYJSi");
        assert_eq!(req.model, "claude-fable-5-1");
        assert_eq!(req.usage.cache_write_1h, 36262);
        assert_eq!(req.usage.cache_write_5m, 0);
        assert_eq!(req.usage.thinking, 1620);
        assert_eq!(req.tools, vec!["Bash"]);
        assert_eq!(req.effort.as_deref(), Some("high"));
        assert!(req.cost_usd > 0.8 && req.cost_usd < 0.9, "{}", req.cost_usd);

        assert!(ev.iter().any(|e| matches!(e, AppEvent::Activity(a) if a.state == SessionState::Tool("Bash".into()))));
        assert!(ev.iter().any(|e| matches!(e, AppEvent::Tool(t) if t.name == "Bash")));
    }

    #[test]
    fn tool_result_marks_thinking_and_failure() {
        let ev = parse_line(TOOL_RESULT, None, &p());
        assert!(
            ev.iter().any(|e| matches!(e, AppEvent::Activity(a) if a.state == SessionState::Thinking && a.turns == 0))
        );
        assert!(ev.iter().any(|e| matches!(e, AppEvent::Tool(t) if t.success == Some(false))));
    }

    #[test]
    fn prompt_counts_a_turn() {
        let ev = parse_line(PROMPT, None, &p());
        assert!(matches!(&ev[0], AppEvent::Activity(a) if a.turns == 1));
    }

    #[test]
    fn quota_rejection_is_an_incident_not_a_request() {
        let ev = parse_line(QUOTA, None, &p());
        assert_eq!(ev.len(), 1);
        match &ev[0] {
            AppEvent::Incident(i) => {
                assert_eq!(i.kind, "five_hour");
                assert_eq!(i.http_status, Some(429));
                assert_eq!(i.resets_at.unwrap().timestamp(), 1791213000);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn compact_boundary_flags_compaction() {
        let ev = parse_line(COMPACT, None, &p());
        assert!(matches!(&ev[0], AppEvent::Activity(a) if a.compactions == 1));
    }

    #[test]
    fn garbage_and_uninteresting_lines_are_ignored() {
        assert!(parse_line("not json", None, &p()).is_empty());
        assert!(
            parse_line(r#"{"type":"file-history-snapshot","timestamp":"2026-10-21T00:00:00Z"}"#, None, &p()).is_empty()
        );
    }

    #[test]
    fn scan_accumulator_coalesces() {
        let p = p();
        let mut acc = ScanAcc::default();
        // The same assistant message twice (streaming), a prompt, a failing tool result.
        for l in [PROMPT, ASSISTANT, ASSISTANT, TOOL_RESULT] {
            for e in parse_line(l, None, &p) {
                acc.push(e);
            }
        }
        let out = acc.finish();
        let reqs = out.iter().filter(|e| matches!(e, AppEvent::Request(_))).count();
        assert_eq!(reqs, 1);
        let acts: Vec<_> = out
            .iter()
            .filter_map(|e| match e {
                AppEvent::Activity(a) => Some(a),
                _ => None,
            })
            .collect();
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].turns, 1);
        assert_eq!(acts[0].state, SessionState::Thinking, "latest line is the tool result");
        let tools = out
            .iter()
            .find_map(|e| match e {
                AppEvent::ToolTotals(t) => Some(t),
                _ => None,
            })
            .unwrap();
        let bash = tools.iter().find(|t| t.0 == "Bash").unwrap();
        assert_eq!((bash.1, bash.2), (1, 1), "one call despite the repeated line, and one failure");
    }

    #[test]
    fn session_from_path() {
        let top = Path::new("/p/-Users-me/3836.jsonl");
        assert_eq!(session_for_path(top).as_deref(), Some("3836"));
        let sub = Path::new("/p/-Users-me/3836/subagents/agent-abc.jsonl");
        assert_eq!(session_for_path(sub).as_deref(), Some("3836"));
    }

    #[test]
    fn partial_lines_wait_for_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, format!("{PROMPT}\n{{\"type\":\"user\"")).unwrap();
        let mut cur = FileCursor::default();
        let mut seen = Vec::new();
        read_new_lines(&path, &mut cur, |l| seen.push(l.to_string())).unwrap();
        assert_eq!(seen.len(), 1);
        assert!(!cur.partial.is_empty());

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, ",\"timestamp\":\"2026-10-21T00:00:00Z\"}}").unwrap();
        read_new_lines(&path, &mut cur, |l| seen.push(l.to_string())).unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[1].starts_with("{\"type\":\"user\""));
        assert!(cur.partial.is_empty());
    }
}
