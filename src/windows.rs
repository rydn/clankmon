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

/// One minute of traffic for the bmon-style graphs.
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
}

/// `n` buckets of `width` seconds ending at `now`; index `n-1` is the current one.
pub fn buckets<'a>(
    requests: impl IntoIterator<Item = &'a Request>,
    now: DateTime<Utc>,
    n: usize,
    width_secs: i64,
) -> Vec<Bucket> {
    let mut out = vec![Bucket::default(); n];
    if n == 0 {
        return out;
    }
    let width = width_secs.max(1);
    let end = now.timestamp();
    let start = end - width * n as i64;
    for r in requests {
        let t = r.ts.timestamp();
        if t <= start || t > end {
            continue;
        }
        let idx = (((t - start - 1) / width) as usize).min(n - 1);
        let b = &mut out[idx];
        b.input += r.usage.input;
        b.cache_read += r.usage.cache_read;
        b.cache_write += r.usage.cache_write();
        b.output += r.usage.output;
        b.thinking += r.usage.thinking;
        b.requests += 1;
        b.errors += u64::from(r.is_error());
        b.cost += r.cost_usd;
    }
    out
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

    #[test]
    fn buckets_place_requests_by_minute() {
        let now = at(12, 0);
        let rs = vec![req(at(11, 59) + Duration::seconds(30), 5, 0.0), req(at(11, 57) + Duration::seconds(1), 7, 0.0)];
        let b = buckets(&rs, now, 5, 60);
        assert_eq!(b[4].output, 5);
        assert_eq!(b[2].output, 7);
        assert_eq!(b.iter().map(|x| x.requests).sum::<u64>(), 2);
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
