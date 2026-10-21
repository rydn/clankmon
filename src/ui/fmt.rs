//! Compact number and time formatting for narrow table cells.

use chrono::{DateTime, Local, Utc};

/// 1234 -> "1.2k", 3_400_000 -> "3.4M".
pub fn si(n: u64) -> String {
    let f = n as f64;
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}k", f / 1e3),
        10_000..=999_999 => format!("{:.0}k", f / 1e3),
        1_000_000..=9_999_999 => format!("{:.1}M", f / 1e6),
        10_000_000..=999_999_999 => format!("{:.0}M", f / 1e6),
        _ => format!("{:.1}B", f / 1e9),
    }
}

pub fn si_f(f: f64) -> String {
    si(f.max(0.0).round() as u64)
}

/// "$0.004", "$1.23", "$123".
pub fn money(c: f64) -> String {
    let c = c.max(0.0);
    if c == 0.0 {
        "$0".into()
    } else if c < 0.01 {
        format!("${c:.3}")
    } else if c < 100.0 {
        format!("${c:.2}")
    } else if c < 10_000.0 {
        format!("${c:.0}")
    } else {
        format!("${:.1}k", c / 1e3)
    }
}

/// Seconds as "42s", "3m", "2h05m", "3d4h".
pub fn dur_secs(s: i64) -> String {
    let s = s.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d{}h", s / 86_400, (s % 86_400) / 3600),
    }
}

pub fn ago(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    dur_secs((now - t).num_seconds())
}

pub fn ms(m: u64) -> String {
    if m < 1000 {
        format!("{m}ms")
    } else if m < 60_000 {
        format!("{:.1}s", m as f64 / 1000.0)
    } else {
        dur_secs((m / 1000) as i64)
    }
}

pub fn clock(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%H:%M:%S").to_string()
}

pub fn hm(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%H:%M").to_string()
}

pub fn pct(p: f64) -> String {
    if p < 10.0 { format!("{p:.1}%") } else { format!("{p:.0}%") }
}

/// Left-truncate with an ellipsis to fit `w` display columns.
pub fn fit(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        s.to_string()
    } else if w == 0 {
        String::new()
    } else {
        let mut t: String = s.chars().take(w.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(si(999), "999");
        assert_eq!(si(1234), "1.2k");
        assert_eq!(si(25_235), "25k");
        assert_eq!(si(4_412_718_380), "4.4B");
        assert_eq!(money(0.0042), "$0.004");
        assert_eq!(money(12.4), "$12.40");
        assert_eq!(money(1234.0), "$1234");
        assert_eq!(dur_secs(7500), "2h05m");
        assert_eq!(dur_secs(90_000), "1d1h");
        assert_eq!(ms(812), "812ms");
        assert_eq!(ms(2310), "2.3s");
        assert_eq!(fit("clankmon-monitor", 8), "clankmo…");
        assert_eq!(pct(4.25), "4.2%");
    }
}
