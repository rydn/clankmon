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

/// Estimated energy: "≈350 mWh", "≈4.3 Wh", "≈1.62 kWh". Always carries the estimate sign.
pub fn energy(wh: f64) -> String {
    // Thresholds sit at the rounding boundary of the unit below, so a value that
    // rounds up (999.6 Wh) moves to the next unit instead of printing "1000 Wh".
    if !wh.is_finite() || wh < 5e-4 {
        "≈0 Wh".into()
    } else if wh < 0.9995 {
        format!("≈{:.0} mWh", wh * 1e3)
    } else if wh < 9.95 {
        format!("≈{wh:.1} Wh")
    } else if wh < 999.5 {
        format!("≈{wh:.0} Wh")
    } else if wh < 9_995.0 {
        format!("≈{:.2} kWh", wh / 1e3)
    } else if wh < 999_950.0 {
        format!("≈{:.1} kWh", wh / 1e3)
    } else {
        format!("≈{:.2} MWh", wh / 1e6)
    }
}

/// Estimated mean power: "≈850 W", "≈1.5 kW".
pub fn power(w: f64) -> String {
    if !w.is_finite() || w <= 0.0 {
        "≈0 W".into()
    } else if w < 9.95 {
        format!("≈{w:.1} W")
    } else if w < 999.5 {
        format!("≈{w:.0} W")
    } else {
        format!("≈{:.1} kW", w / 1e3)
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
    // One decimal near the ends, so 99.6% never rounds up to a misleading 100%.
    if p < 10.0 || (99.0..100.0).contains(&p) { format!("{p:.1}%") } else { format!("{p:.0}%") }
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
        assert_eq!(pct(99.62), "99.6%");
        assert_eq!(pct(100.0), "100%");
    }

    #[test]
    fn energy_and_power() {
        assert_eq!(energy(0.0), "≈0 Wh");
        assert_eq!(energy(0.35), "≈350 mWh");
        assert_eq!(energy(4.26), "≈4.3 Wh");
        assert_eq!(energy(412.0), "≈412 Wh");
        assert_eq!(energy(1620.0), "≈1.62 kWh");
        assert_eq!(energy(f64::NAN), "≈0 Wh");
        assert_eq!(power(850.0), "≈850 W");
        assert_eq!(power(1500.0), "≈1.5 kW");
        assert_eq!(power(-1.0), "≈0 W");
    }

    #[test]
    fn energy_unit_boundaries() {
        assert_eq!(energy(0.9994), "≈999 mWh");
        assert_eq!(energy(1.0), "≈1.0 Wh");
        assert_eq!(energy(9.94), "≈9.9 Wh");
        assert_eq!(energy(10.0), "≈10 Wh");
        assert_eq!(energy(999.0), "≈999 Wh");
        assert_eq!(energy(1000.0), "≈1.00 kWh");
        assert_eq!(energy(9990.0), "≈9.99 kWh");
        assert_eq!(energy(10_000.0), "≈10.0 kWh");
        assert_eq!(energy(1e6), "≈1.00 MWh");
        assert_eq!(energy(f64::INFINITY), "≈0 Wh");
        assert_eq!(energy(0.9996), "≈1.0 Wh");
        assert_eq!(energy(9.96), "≈10 Wh");
        assert_eq!(energy(999.6), "≈1.00 kWh");
        assert_eq!(energy(9999.5), "≈10.0 kWh");
        assert_eq!(energy(999_960.0), "≈1.00 MWh");
        assert_eq!(energy(0.0004), "≈0 Wh");
        assert_eq!(energy(0.0006), "≈1 mWh");
        assert_eq!(power(9.96), "≈10 W");
        assert_eq!(power(999.6), "≈1.0 kW");
        assert_eq!(power(9.9), "≈9.9 W");
        assert_eq!(power(999.0), "≈999 W");
        assert_eq!(power(1000.0), "≈1.0 kW");
        assert_eq!(power(f64::NAN), "≈0 W");
    }
}
