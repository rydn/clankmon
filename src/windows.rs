//! Time-window math: 5-hour blocks, burn rates, per-minute buckets and quota projection.

use chrono::{DateTime, Duration, DurationRound, Utc};

use crate::model::{Request, Usage};

pub const FIVE_HOURS: i64 = 5 * 3600;
pub const SEVEN_DAYS: i64 = 7 * 86_400;

/// A ccusage-style billing block: starts at the first request (floored to the
/// hour) and lasts five hours; a gap of five hours also starts a new block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Block {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub usage: Usage,
    pub cost: f64,
    pub requests: u64,
}

impl Block {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        now < self.end && now - self.last_activity < Duration::seconds(FIVE_HOURS)
    }
}

/// Blocks over requests already sorted by timestamp.
pub fn blocks<'a>(sorted: impl IntoIterator<Item = &'a Request>) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for r in sorted {
        let need_new = match out.last() {
            None => true,
            Some(b) => r.ts >= b.end || r.ts - b.last_activity >= Duration::seconds(FIVE_HOURS),
        };
        if need_new {
            let start = r.ts.duration_trunc(Duration::hours(1)).unwrap_or(r.ts);
            out.push(Block {
                start,
                end: start + Duration::seconds(FIVE_HOURS),
                last_activity: r.ts,
                ..Default::default()
            });
        }
        let b = out.last_mut().expect("block exists");
        b.last_activity = r.ts;
        b.usage.add(&r.usage);
        b.cost += r.cost_usd;
        b.requests += 1;
    }
    out
}

/// Rates over a trailing window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Burn {
    pub prompt_per_min: f64,
    pub output_per_min: f64,
    pub requests_per_min: f64,
    pub cost_per_hour: f64,
    /// cache_read / prompt_total over the window, 0..1.
    pub cache_hit: Option<f64>,
    pub ttft_p50_ms: Option<u64>,
    pub duration_p50_ms: Option<u64>,
}

pub fn burn<'a>(requests: impl IntoIterator<Item = &'a Request>, now: DateTime<Utc>, minutes: i64) -> Burn {
    let since = now - Duration::minutes(minutes);
    let mut u = Usage::default();
    let mut n = 0u64;
    let mut cost = 0.0;
    let mut ttfts = Vec::new();
    let mut durs = Vec::new();
    for r in requests {
        if r.ts < since || r.ts > now {
            continue;
        }
        u.add(&r.usage);
        n += 1;
        cost += r.cost_usd;
        if let Some(t) = r.ttft_ms {
            ttfts.push(t);
        }
        if let Some(d) = r.duration_ms {
            durs.push(d);
        }
    }
    let m = minutes.max(1) as f64;
    let prompt = u.prompt_total();
    Burn {
        prompt_per_min: prompt as f64 / m,
        output_per_min: u.output as f64 / m,
        requests_per_min: n as f64 / m,
        cost_per_hour: cost * 60.0 / m,
        cache_hit: (prompt > 0).then(|| u.cache_read as f64 / prompt as f64),
        ttft_p50_ms: median(&mut ttfts),
        duration_p50_ms: median(&mut durs),
    }
}

fn median(v: &mut [u64]) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

/// Traffic in one time slot of the throughput graphs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bucket {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub thinking: u64,
    pub requests: u64,
    pub errors: u64,
    pub cost: f64,
}

impl Bucket {
    pub fn prompt(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    pub fn add(&mut self, o: &Bucket) {
        self.input += o.input;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.output += o.output;
        self.thinking += o.thinking;
        self.requests += o.requests;
        self.errors += o.errors;
        self.cost += o.cost;
    }

    pub fn is_empty(&self) -> bool {
        self.prompt() == 0 && self.output == 0 && self.requests == 0
    }
}

/// Fixed-size ring of time slots `width` seconds wide, indexed by absolute slot
/// number (`epoch / width`), so slots never move once written.
#[derive(Debug, Clone)]
struct Ring {
    width: i64,
    slots: Vec<i64>,
    data: Vec<Bucket>,
}

impl Ring {
    fn new(width: i64, len: usize) -> Self {
        Ring { width, slots: vec![i64::MIN; len], data: vec![Bucket::default(); len] }
    }

    fn add(&mut self, epoch: i64, b: &Bucket, newest: i64) {
        let slot = epoch.div_euclid(self.width);
        let len = self.slots.len() as i64;
        // Too old for the ring, or absurdly far in the future.
        if slot <= newest.div_euclid(self.width) - len || slot > newest.div_euclid(self.width) + 1 {
            return;
        }
        let i = slot.rem_euclid(len) as usize;
        if self.slots[i] != slot {
            self.slots[i] = slot;
            self.data[i] = Bucket::default();
        }
        self.data[i].add(b);
    }

    fn get(&self, slot: i64) -> Bucket {
        let i = slot.rem_euclid(self.slots.len() as i64) as usize;
        if self.slots[i] == slot { self.data[i] } else { Bucket::default() }
    }

    /// Sum of slots covering [from, to) in epoch seconds (both multiples of `width`).
    fn sum(&self, from: i64, to: i64) -> Bucket {
        let mut out = Bucket::default();
        let mut slot = from.div_euclid(self.width);
        let end = to.div_euclid(self.width);
        while slot < end {
            out.add(&self.get(slot));
            slot += 1;
        }
        out
    }
}

/// Throughput time series for the graphs: per-second slots for the last two
/// hours and per-minute slots for the last two days.
///
/// Tokens are recorded when clankmon observes them (or at their transcript
/// timestamp during backfill), so a column never changes after it has passed.
#[derive(Debug, Clone)]
pub struct Series {
    sec: Ring,
    min: Ring,
    /// Newest epoch second seen, used to reject stale writes.
    pub newest: i64,
}

pub const SERIES_SECONDS: i64 = 2 * 3600;
pub const SERIES_MINUTES: i64 = 2 * 24 * 60;

impl Default for Series {
    fn default() -> Self {
        Series { sec: Ring::new(1, SERIES_SECONDS as usize), min: Ring::new(60, SERIES_MINUTES as usize), newest: 0 }
    }
}

impl Series {
    pub fn record(&mut self, ts: DateTime<Utc>, b: &Bucket) {
        let t = ts.timestamp();
        self.newest = self.newest.max(t);
        self.sec.add(t, b, self.newest);
        self.min.add(t, b, self.newest);
    }

    /// `n` columns `width` seconds wide, aligned to the wall clock. The last
    /// column is the one containing `now`, still filling up.
    pub fn columns(&self, now: DateTime<Utc>, n: usize, width: i64) -> Vec<Bucket> {
        let width = width.max(1);
        let now_s = now.timestamp();
        let last_start = now_s.div_euclid(width) * width;
        let first_start = last_start - (n as i64 - 1) * width;
        // Per-second slots while they cover the range and the width isn't minute-aligned.
        let use_sec = width % 60 != 0 || first_start > now_s - SERIES_SECONDS + 60;
        let ring = if use_sec { &self.sec } else { &self.min };
        (0..n as i64)
            .map(|k| {
                let a = first_start + k * width;
                ring.sum(a, a + width)
            })
            .collect()
    }

    /// Exponentially weighted rate per minute over the last few `tau` seconds:
    /// rises instantly on a burst and decays smoothly, never jumping when a
    /// request leaves a fixed window.
    pub fn ewma_per_min(&self, now: DateTime<Utc>, tau: f64, field: impl Fn(&Bucket) -> u64) -> f64 {
        let now_s = now.timestamp();
        let span = (tau * 5.0).ceil() as i64;
        let frac = now.timestamp_subsec_millis() as f64 / 1000.0;
        let mut acc = 0.0;
        for age in 0..span.min(SERIES_SECONDS) {
            let v = field(&self.sec.get(now_s - age));
            if v > 0 {
                acc += v as f64 * (-(age as f64 + frac) / tau).exp();
            }
        }
        acc / tau * 60.0
    }
}

/// An axis ceiling that ignores rare outliers: the 90th percentile of the
/// non-empty columns plus 30 % headroom, rounded up to a nice number. With
/// fewer than five active columns it falls back to the peak. Columns above the
/// ceiling are drawn clipped with an overflow cap.
pub fn robust_ceiling(values: &[f64]) -> f64 {
    let mut nz: Vec<f64> = values.iter().copied().filter(|v| *v > 0.0).collect();
    if nz.len() < 5 {
        return nice_ceil(nz.iter().copied().fold(0.0, f64::max));
    }
    nz.sort_by(f64::total_cmp);
    let p90 = nz[((nz.len() - 1) as f64 * 0.9).round() as usize];
    nice_ceil(p90 * 1.3)
}

/// Round up to 1, 2, 2.5 or 5 times a power of ten, for readable axis ceilings.
pub fn nice_ceil(x: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return 0.0;
    }
    let p = 10f64.powf(x.log10().floor());
    let m = x / p;
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].into_iter().find(|s| m <= *s + 1e-9).unwrap_or(10.0);
    step * p
}

/// Where a quota window is heading.
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    /// Utilisation a perfectly even pace would have reached by now, 0..100.
    pub pace_pct: f64,
    /// Percentage points per hour over the fit window.
    pub slope_per_hour: f64,
    /// When the window hits 100 % at the current slope, if before the reset.
    pub hits_full_at: Option<DateTime<Utc>>,
    /// Utilisation projected at reset time (can exceed 100).
    pub at_reset_pct: Option<f64>,
}

/// Least-squares slope over readings in the current window, last `fit_minutes` only.
pub fn project(
    readings: &[(DateTime<Utc>, f64)],
    current_pct: f64,
    resets_at: Option<DateTime<Utc>>,
    window_secs: i64,
    now: DateTime<Utc>,
    fit_minutes: i64,
) -> Projection {
    let pace_pct = resets_at.map_or(0.0, |r| {
        let start = r - Duration::seconds(window_secs);
        let elapsed = (now - start).num_seconds().clamp(0, window_secs) as f64;
        elapsed / window_secs as f64 * 100.0
    });
    let window_start = resets_at.map(|r| r - Duration::seconds(window_secs));
    let fit_from = now - Duration::minutes(fit_minutes);
    let pts: Vec<(f64, f64)> = readings
        .iter()
        .filter(|(t, _)| *t >= fit_from && window_start.is_none_or(|ws| *t >= ws))
        .map(|(t, p)| ((*t - fit_from).num_seconds() as f64 / 3600.0, *p))
        .collect();
    let slope = slope(&pts).unwrap_or(0.0);

    let (hits_full_at, at_reset_pct) = if slope > 0.01 {
        let hours_to_full = (100.0 - current_pct).max(0.0) / slope;
        let hit = now + Duration::seconds((hours_to_full * 3600.0) as i64);
        match resets_at {
            Some(r) => {
                let hours_left = (r - now).num_seconds().max(0) as f64 / 3600.0;
                ((hit < r).then_some(hit), Some(current_pct + slope * hours_left))
            }
            None => (Some(hit), None),
        }
    } else {
        (None, Some(current_pct))
    };
    Projection { pace_pct, slope_per_hour: slope, hits_full_at, at_reset_pct }
}

fn slope(pts: &[(f64, f64)]) -> Option<f64> {
    if pts.len() < 2 {
        return None;
    }
    let span = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max) - pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    if span < 5.0 / 60.0 {
        return None; // need at least five minutes of data
    }
    let n = pts.len() as f64;
    let mx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let num: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let den: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    (den > 0.0).then(|| num / den)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: i64, m: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_792_540_800 + h * 3600 + m * 60, 0).unwrap() // a midnight UTC
    }

    fn req(ts: DateTime<Utc>, out: u64, cost: f64) -> Request {
        Request {
            key: ts.to_rfc3339(),
            ts,
            usage: Usage { input: 10, output: out, cache_read: 90, ..Default::default() },
            cost_usd: cost,
            ..Default::default()
        }
    }

    #[test]
    fn blocks_floor_to_hour_and_split_after_five_hours() {
        let rs = vec![req(at(9, 17), 1, 1.0), req(at(13, 59), 1, 1.0), req(at(14, 30), 1, 1.0), req(at(22, 0), 1, 1.0)];
        let b = blocks(&rs);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].start, at(9, 0));
        assert_eq!(b[0].end, at(14, 0));
        assert_eq!(b[0].requests, 2);
        assert_eq!(b[1].start, at(14, 0));
        assert_eq!(b[2].start, at(22, 0));
        assert!(b[2].is_active(at(23, 0)));
        assert!(!b[0].is_active(at(23, 0)));
    }

    #[test]
    fn burn_rates() {
        let now = at(12, 0);
        let rs = vec![req(at(11, 55), 100, 0.5), req(at(11, 58), 100, 0.5), req(at(10, 0), 999, 9.0)];
        let b = burn(&rs, now, 10);
        assert_eq!(b.output_per_min, 20.0);
        assert_eq!(b.requests_per_min, 0.2);
        assert!((b.cost_per_hour - 6.0).abs() < 1e-9);
        assert_eq!(b.cache_hit, Some(0.9));
    }

    fn bucket(out: u64) -> Bucket {
        Bucket { output: out, requests: 1, ..Default::default() }
    }

    #[test]
    fn columns_are_clock_aligned_and_stable() {
        let mut s = Series::default();
        s.record(at(11, 59) + Duration::seconds(5), &bucket(5));
        s.record(at(11, 59) + Duration::seconds(25), &bucket(7));
        s.record(at(12, 0) + Duration::seconds(3), &bucket(9));
        // At 12:00:04 with 10 s columns: 11:59:00 … 11:59:50, then the live 12:00:00 column.
        let now = at(12, 0) + Duration::seconds(4);
        let out = |c: Vec<Bucket>| c.iter().map(|b| b.output).collect::<Vec<_>>();
        let c = s.columns(now, 7, 10);
        assert_eq!(out(c.clone()), vec![5, 0, 7, 0, 0, 0, 9]);
        // Seconds later nothing regroups: the same columns hold the same values.
        assert_eq!(c, s.columns(now + Duration::seconds(5), 7, 10));
        // At the next boundary everything shifts left by exactly one column.
        assert_eq!(out(s.columns(now + Duration::seconds(6), 7, 10)), vec![0, 7, 0, 0, 0, 9, 0]);
    }

    #[test]
    fn minute_columns_use_the_long_ring() {
        let mut s = Series::default();
        let now = at(12, 0);
        s.record(now - Duration::hours(10), &bucket(3));
        s.record(now - Duration::hours(30), &bucket(4));
        // 130 columns of 15 minutes reach back 32.5 hours.
        assert_eq!(s.columns(now, 130, 900).iter().map(|b| b.output).sum::<u64>(), 7);
        assert_eq!(s.columns(now, 100, 900).iter().map(|b| b.output).sum::<u64>(), 3);
        // Beyond two days is dropped.
        s.record(now - Duration::days(3), &bucket(100));
        assert_eq!(s.columns(now, 190, 900).iter().map(|b| b.output).sum::<u64>(), 7);
    }

    #[test]
    fn ewma_rises_and_decays_smoothly() {
        let mut s = Series::default();
        let t0 = at(12, 0);
        s.record(t0, &bucket(600));
        let r0 = s.ewma_per_min(t0, 30.0, |b| b.output);
        let r10 = s.ewma_per_min(t0 + Duration::seconds(10), 30.0, |b| b.output);
        let r60 = s.ewma_per_min(t0 + Duration::seconds(60), 30.0, |b| b.output);
        assert!((r0 - 1200.0).abs() < 1.0, "{r0}");
        assert!(r10 < r0 && r10 > r60 && r60 > 0.0);
        assert!((r10 / r0 - (-10.0f64 / 30.0).exp()).abs() < 1e-6);
    }

    #[test]
    fn robust_ceiling_ignores_a_lone_spike() {
        let mut v = vec![0.0; 50];
        for (i, x) in v.iter_mut().enumerate().take(20) {
            *x = 1000.0 + i as f64 * 100.0; // 1000..2900
        }
        v.push(58_000.0);
        let c = robust_ceiling(&v);
        assert!((2_900.0..58_000.0).contains(&c), "{c}");
        // Too few active columns: use the peak.
        assert_eq!(robust_ceiling(&[0.0, 300.0, 0.0, 1200.0]), 2000.0);
        assert_eq!(robust_ceiling(&[]), 0.0);
    }

    #[test]
    fn nice_ceilings() {
        assert_eq!(nice_ceil(0.0), 0.0);
        assert_eq!(nice_ceil(7.0), 10.0);
        assert_eq!(nice_ceil(1.0), 1.0);
        assert_eq!(nice_ceil(130.0), 200.0);
        assert_eq!(nice_ceil(2100.0), 2500.0);
        assert_eq!(nice_ceil(370_000.0), 500_000.0);
    }

    #[test]
    fn projection_finds_hit_before_reset() {
        let now = at(12, 0);
        let reset = at(14, 0);
        // 10 points over the last hour climbing 30 → 60: 30 pts/hour.
        let readings: Vec<_> = (0..=6).map(|i| (at(11, i * 10), 30.0 + 5.0 * i as f64)).collect();
        let p = project(&readings, 60.0, Some(reset), FIVE_HOURS, now, 90);
        assert!((p.slope_per_hour - 30.0).abs() < 1e-6, "{}", p.slope_per_hour);
        let hit = p.hits_full_at.expect("hits before reset");
        assert_eq!((hit - now).num_minutes(), 80);
        assert!((p.pace_pct - 60.0).abs() < 1e-9);
        assert!(p.at_reset_pct.unwrap() > 100.0);
    }

    #[test]
    fn flat_usage_never_hits() {
        let now = at(12, 0);
        let readings: Vec<_> = (0..6).map(|i| (at(11, i * 10), 40.0)).collect();
        let p = project(&readings, 40.0, Some(at(14, 0)), FIVE_HOURS, now, 90);
        assert_eq!(p.hits_full_at, None);
        assert_eq!(p.at_reset_pct, Some(40.0));
    }

    #[test]
    fn too_little_data_has_no_slope() {
        let now = at(12, 0);
        let p = project(&[(at(11, 59), 10.0), (at(12, 0), 11.0)], 11.0, None, FIVE_HOURS, now, 90);
        assert_eq!(p.slope_per_hour, 0.0);
    }
}
