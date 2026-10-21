//! Daily totals, an hour×weekday heatmap and the model mix.

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, Local, NaiveDate, Timelike};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::DayAgg;
use crate::state::State;
use crate::ui::App;
use crate::ui::fmt;
use crate::ui::theme::{Theme, lerp};
use crate::ui::widgets::{BarGraph, bar_graph, hbar, line, panel};

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App, st: &State) {
    let t = app.theme.clone();
    let [top, mid, bottom] =
        Layout::vertical([Constraint::Percentage(36), Constraint::Percentage(30), Constraint::Min(10)]).areas(area);
    let today = st.now().with_timezone(&Local).date_naive();
    // Show back to the first day with data, at least a week, at most what fits.
    let first = st
        .days
        .keys()
        .next()
        .copied()
        .into_iter()
        .chain(st.stats_cache.as_ref().and_then(|s| s.days.keys().next().copied()))
        .min()
        .unwrap_or(today);
    let span_days = ((today - first).num_days() + 1).max(7);

    // Daily cost.
    let inner = panel(buf, top, &t, "DAILY COST", Some(note(&t, "list-price equivalent")), false);
    if inner.height >= 3 {
        let days = day_range(today, ((inner.width / 3).max(1) as i64).min(span_days));
        let costs: Vec<f64> = days.iter().map(|d| st.days.get(d).map_or(0.0, |a| a.cost)).collect();
        bars_with_labels(buf, inner, &days, &costs, fmt::money, &t, (t.tertiary, t.primary));
    }

    // Daily tokens by kind.
    let inner = panel(buf, mid, &t, "DAILY TOKENS", Some(note(&t, "■ read ■ write ■ new ■ out")), false);
    if inner.height >= 3 {
        let n = ((inner.width / 3).max(1) as i64).min(span_days);
        let days = day_range(today, n);
        let mut cols: Vec<Vec<f64>> = Vec::new();
        for d in &days {
            let a = st.days.get(d).cloned().unwrap_or_default();
            let fallback = st
                .stats_cache
                .as_ref()
                .and_then(|s| s.days.get(d))
                .map_or(0.0, |s| s.tokens_by_model.values().sum::<u64>() as f64);
            let total = a.usage.total() as f64;
            let v = if total == 0.0 && fallback > 0.0 {
                vec![fallback, 0.0, 0.0, 0.0]
            } else {
                vec![
                    a.usage.cache_read as f64,
                    a.usage.cache_write() as f64,
                    a.usage.input as f64,
                    a.usage.output as f64,
                ]
            };
            // Each day is three columns wide: two bar columns and a gap.
            cols.push(v.clone());
            cols.push(v);
            cols.push(vec![0.0]);
        }
        let max = cols.iter().map(|c| c.iter().sum::<f64>()).fold(0.0, f64::max);
        let graph = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
        let g = BarGraph {
            columns: &cols,
            colors: &[t.l_cache_read, t.l_cache_write, t.l_input, t.l_output],
            max,
            log: app.log_scale,
            live_last: false,
            ticks: &[],
        };
        bar_graph(buf, graph, &g, &t);
        line(
            buf,
            inner,
            inner.height - 1,
            &Line::from(Span::styled(
                format!(
                    "{} · peak {} tokens/day",
                    if app.log_scale { "log scale" } else { "linear (g: log)" },
                    fmt::si_f(max)
                ),
                Style::new().fg(t.dim),
            )),
        );
    }

    let [heat, mix] = Layout::horizontal([Constraint::Length(80), Constraint::Min(30)]).areas(bottom);
    heatmap(buf, heat, &t, st);
    model_mix(buf, mix, &t, st, today);
}

fn note<'a>(t: &Theme, s: &str) -> Line<'a> {
    Line::from(Span::styled(format!(" {s} "), Style::new().fg(t.dim)))
}

fn day_range(today: NaiveDate, n: i64) -> Vec<NaiveDate> {
    (0..n).rev().map(|i| today - Duration::days(i)).collect()
}

/// Vertical bars, three columns per day, labels underneath.
fn bars_with_labels(
    buf: &mut Buffer,
    inner: Rect,
    days: &[NaiveDate],
    vals: &[f64],
    label: impl Fn(f64) -> String,
    t: &Theme,
    (lo, hi): (Color, Color),
) {
    let h = inner.height.saturating_sub(2);
    let max = vals.iter().cloned().fold(0.0, f64::max);
    let eighths = ["", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    for (i, (d, v)) in days.iter().zip(vals).enumerate() {
        let x = inner.x + i as u16 * 3;
        if x + 2 > inner.right() {
            break;
        }
        let total = if max > 0.0 { (v / max * h as f64 * 8.0).round() as u16 } else { 0 };
        for row in 0..h {
            let fill = total.saturating_sub(row * 8).min(8) as usize;
            if fill == 0 {
                continue;
            }
            let y = inner.y + h - 1 - row;
            let c = lerp(lo, hi, row as f64 / h.max(1) as f64);
            buf.set_string(x, y, eighths[fill].repeat(2), Style::new().fg(c));
        }
        let weekend = matches!(d.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        buf.set_string(
            x,
            inner.y + h,
            format!("{:02}", d.day()),
            Style::new().fg(if weekend { t.primary } else { t.muted }),
        );
        if *v > 0.0 && *v == max {
            let s = label(*v);
            let lx = x.saturating_sub(s.len() as u16 / 2).max(inner.x);
            buf.set_string(lx, inner.y + h + 1, s, Style::new().fg(t.tertiary).add_modifier(Modifier::BOLD));
        }
    }
    let total: f64 = vals.iter().sum();
    let s = format!("{} over {} days", label(total), days.len());
    let x = inner.right().saturating_sub(s.len() as u16 + 1);
    buf.set_string(x, inner.y, s, Style::new().fg(t.bright));
}

fn heatmap(buf: &mut Buffer, area: Rect, t: &Theme, st: &State) {
    let inner = panel(buf, area, t, "ACTIVITY · HOUR × WEEKDAY", None, false);
    if inner.height < 3 {
        return;
    }
    let mut grid = [[0u64; 24]; 7];
    for r in st.timeline() {
        let l = r.ts.with_timezone(&Local);
        grid[l.weekday().num_days_from_monday() as usize][l.hour() as usize] += 1;
    }
    let max = grid.iter().flatten().copied().max().unwrap_or(0).max(1) as f64;
    let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let cell_w = ((inner.width.saturating_sub(4)) / 24).clamp(1, 3);
    let mut hdr = vec![Span::raw("    ")];
    for h in 0..24 {
        let s = if h % 3 == 0 { format!("{h:<w$}", w = cell_w as usize * 3) } else { String::new() };
        hdr.push(Span::styled(s, Style::new().fg(t.dim)));
    }
    line(buf, inner, 0, &Line::from(hdr));
    for (d, row) in grid.iter().enumerate() {
        let y = d as u16 + 1;
        if y >= inner.height {
            break;
        }
        let mut spans = vec![Span::styled(format!("{} ", days[d]), Style::new().fg(t.muted))];
        for &v in row {
            let k = (v as f64 / max).sqrt();
            let c = if v == 0 {
                t.grid
            } else if k < 0.5 {
                lerp(t.grid, t.secondary, k * 2.0)
            } else {
                lerp(t.secondary, t.primary, (k - 0.5) * 2.0)
            };
            spans.push(Span::styled("■".repeat(cell_w as usize), Style::new().fg(c)));
        }
        line(buf, inner, y, &Line::from(spans));
    }
    if inner.height > 9 {
        let total: u64 = grid.iter().flatten().sum();
        line(
            buf,
            inner,
            9,
            &Line::from(Span::styled(
                format!("{total} requests in the loaded history · sqrt colour scale"),
                Style::new().fg(t.dim),
            )),
        );
    }
}

fn model_mix(buf: &mut Buffer, area: Rect, t: &Theme, st: &State, today: NaiveDate) {
    let inner = panel(buf, area, t, "MODEL MIX · 30D", None, false);
    if inner.height == 0 {
        return;
    }
    let since = today - Duration::days(29);
    let mut agg: BTreeMap<String, (u64, f64)> = BTreeMap::new();
    let mut tot = DayAgg::default();
    for (d, a) in st.days.range(since..) {
        let _ = d;
        tot.cost += a.cost;
        tot.requests += a.requests;
        for (m, (tok, c)) in &a.by_model {
            let e = agg.entry(m.clone()).or_default();
            e.0 += tok;
            e.1 += c;
        }
    }
    let mut v: Vec<_> = agg.into_iter().filter(|x| x.1.0 > 0).collect();
    v.sort_by(|a, b| b.1.1.total_cmp(&a.1.1).then(a.0.cmp(&b.0)));
    line(
        buf,
        inner,
        0,
        &Line::from(vec![
            Span::styled(fmt::money(tot.cost), Style::new().fg(t.tertiary).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" · {} requests", tot.requests), Style::new().fg(t.muted)),
        ]),
    );
    let bar_w = inner.width.saturating_sub(11 + 8 + 7 + 2) as usize;
    for (i, (m, (tok, c))) in v.iter().enumerate() {
        let y = i as u16 + 2;
        if y >= inner.height {
            break;
        }
        let share = if tot.cost > 0.0 { c / tot.cost } else { 0.0 };
        line(
            buf,
            inner,
            y,
            &Line::from(vec![
                Span::styled(format!("{:<11}", fmt::fit(m, 11)), Style::new().fg(t.primary)),
                hbar(share, 1.0, bar_w, t.secondary),
                Span::styled(format!(" {:>7}", fmt::money(*c)), Style::new().fg(t.tertiary)),
                Span::styled(format!(" {:>6}", fmt::si(*tok)), Style::new().fg(t.muted)),
            ]),
        );
    }
}
