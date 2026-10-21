//! The default view: quota, throughput, processes, wire and model/tool mix.

use std::collections::VecDeque;

use chrono::{DateTime, Duration, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, StatefulWidget, Table, TableState};

use crate::model::{Request, SessionState, WindowReading};
use crate::state::{Session, State};
use crate::ui::fmt;
use crate::ui::theme::{Theme, lerp};
use crate::ui::widgets::{self, braille_area, hbar, line, mini_bar, neon_gauge, panel, stacked_bars};
use crate::ui::{App, SortKey};
use crate::windows::{self, FIVE_HOURS, SEVEN_DAYS};

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App, st: &State) {
    let wide = area.width >= 110;
    let sessions = sorted_sessions(st, app.sort);
    let n = sessions.len() as u16;
    if wide {
        let top_h = if area.height >= 36 { 11 } else { 9 };
        let proc_h = (n + 3).clamp(5, 12).min(area.height.saturating_sub(top_h + 6));
        let [top, procs, bottom] =
            Layout::vertical([Constraint::Length(top_h), Constraint::Length(proc_h), Constraint::Min(5)]).areas(area);
        let [q, tp] = Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)]).areas(top);
        quota(buf, q, app, st);
        throughput(buf, tp, app, st);
        processes(buf, procs, app, st, &sessions);
        let [w, side] = Layout::horizontal([Constraint::Min(60), Constraint::Length(42)]).areas(bottom);
        wire(buf, w, app, st);
        models_tools(buf, side, app, st);
    } else {
        // Narrow: stack everything, shrinking panels on short terminals.
        let roomy = area.height >= 34;
        let q_h = if roomy { 10 } else { 5 };
        let tp_h = if roomy { 9 } else { 7 };
        let proc_h = (n + 3).clamp(4, if roomy { 10 } else { 6 });
        let [q, tp, procs, w] = Layout::vertical([
            Constraint::Length(q_h),
            Constraint::Length(tp_h),
            Constraint::Length(proc_h),
            Constraint::Min(3),
        ])
        .areas(area);
        quota(buf, q, app, st);
        throughput(buf, tp, app, st);
        processes(buf, procs, app, st, &sessions);
        wire(buf, w, app, st);
    }
}

/// Sessions for the process table in the chosen order.
pub fn sorted_sessions(st: &State, sort: SortKey) -> Vec<&Session> {
    let mut v = st.visible_sessions();
    match sort {
        // Attention first: blocked on permission, then busy, then idle, then ended.
        SortKey::Activity => v.sort_by(|a, b| {
            let rank = |s: &Session| match s.state {
                SessionState::Waiting => 0,
                _ if s.state.is_busy() => 1,
                SessionState::Ended => 3,
                _ => 2,
            };
            b.alive()
                .cmp(&a.alive())
                .then(rank(a).cmp(&rank(b)))
                .then(b.last_ts.cmp(&a.last_ts))
                .then(a.id.cmp(&b.id))
        }),
        SortKey::Cost => v.sort_by(|a, b| b.cost.total_cmp(&a.cost).then(a.id.cmp(&b.id))),
        SortKey::Tokens => v.sort_by(|a, b| b.usage.total().cmp(&a.usage.total()).then(a.id.cmp(&b.id))),
        SortKey::Cpu => v.sort_by(|a, b| {
            let c = |s: &Session| s.proc.as_ref().and_then(|p| p.cpu_pct).unwrap_or(-1.0);
            c(b).total_cmp(&c(a)).then(a.id.cmp(&b.id))
        }),
        SortKey::Name => v.sort_by(|a, b| a.display_name().to_lowercase().cmp(&b.display_name().to_lowercase()).then(a.id.cmp(&b.id))),
    }
    v
}

// ---------------------------------------------------------------------------
// QUOTA
// ---------------------------------------------------------------------------

fn quota(buf: &mut Buffer, area: Rect, app: &mut App, st: &State) {
    let t = app.theme.clone();
    let now = st.now();
    let five = st.window(true);
    let seven = st.window(false);
    let newest = [five.as_ref(), seven.as_ref()]
        .into_iter()
        .flatten()
        .max_by_key(|w| w.2)
        .map(|w| (w.1, w.2));
    let note = newest.map(|(src, ts)| {
        Line::from(vec![
            Span::styled(format!(" {} ", src.label()), Style::new().fg(t.muted)),
            Span::styled(format!("{} ago ", fmt::ago(ts, now)), Style::new().fg(t.dim)),
        ])
    });
    let inner = panel(buf, area, &t, "QUOTA", note, false);
    if inner.height == 0 {
        return;
    }
    // Full mode: gauge + trend row per window. Compact: one row per window.
    let compact = inner.height < 7;

    let mut y = 0u16;
    if five.is_none() && seven.is_none() {
        let lines = [
            Line::from(Span::styled("NO LIVE QUOTA FEED", Style::new().fg(t.warn).add_modifier(Modifier::BOLD))),
            Line::from(vec![
                Span::styled("▸ clankmon install ", Style::new().fg(t.secondary)),
                Span::styled("statusline feed (Pro/Max)", Style::new().fg(t.muted)),
            ]),
            Line::from(vec![
                Span::styled("▸ clankmon proxy   ", Style::new().fg(t.secondary)),
                Span::styled("exact limits from response headers", Style::new().fg(t.muted)),
            ]),
            Line::from(vec![
                Span::styled("▸ clankmon --oauth ", Style::new().fg(t.secondary)),
                Span::styled("poll /api/oauth/usage", Style::new().fg(t.muted)),
            ]),
        ];
        for l in lines.iter().take(inner.height.saturating_sub(1) as usize) {
            line(buf, inner, y, l);
            y += 1;
        }
    } else {
        for (label, w, hist, window, five_row) in [
            ("5H", &five, &st.limits.five_history, FIVE_HOURS, true),
            ("7D", &seven, &st.limits.seven_history, SEVEN_DAYS, false),
        ] {
            let rows = if compact { 1 } else { 2 };
            if y + rows >= inner.height {
                break;
            }
            match w {
                Some((r, _, _)) => {
                    let gauge = window_rows(buf, inner, y, label, r, hist, window, now, &t, compact);
                    if five_row {
                        app.areas.five_gauge = gauge;
                    } else {
                        app.areas.seven_gauge = gauge;
                    }
                }
                None => {
                    line(
                        buf,
                        inner,
                        y,
                        &Line::from(vec![
                            Span::styled(format!("{label}  "), Style::new().fg(t.bright).add_modifier(Modifier::BOLD)),
                            Span::styled("— no reading for this window yet", Style::new().fg(t.dim)),
                        ]),
                    );
                }
            }
            y += if compact { 1 } else { 3 };
        }
        // Extra windows (per-model weekly, API buckets) while room remains above the footer rows.
        for (k, (w, _, _)) in &st.limits.extra {
            if compact || y + 3 > inner.height {
                break;
            }
            let mut spans = vec![Span::styled(format!("{:<14}", fmt::fit(k, 14)), Style::new().fg(t.muted))];
            spans.extend(mini_bar(w.used_pct, 12, &t));
            spans.push(Span::styled(format!(" {:>6}", fmt::pct(w.used_pct)), Style::new().fg(t.level(w.used_pct))));
            if let Some(r) = w.resets_at {
                spans.push(Span::styled(format!(" ⟳{}", fmt::dur_secs((r - now).num_seconds())), Style::new().fg(t.dim)));
            }
            line(buf, inner, y, &Line::from(spans));
            y += 1;
        }
    }

    // Local 5-hour block from transcript data, and where the burn rate takes it.
    let reqs = st.since(now - Duration::hours(10));
    let blocks = windows::blocks(reqs.iter().copied());
    let burn = windows::burn(reqs.iter().copied(), now, 30);
    let active = blocks.last().filter(|b| b.is_active(now));
    let sep = || Span::styled(" ▸ ", Style::new().fg(t.dim));
    if !compact
        && inner.height >= 2
        && y < inner.height - 1
        && let Some(b) = active
    {
        let hours_left = (b.end - now).num_seconds().max(0) as f64 / 3600.0;
        let projected = b.cost + burn.cost_per_hour * hours_left;
        line(
            buf,
            inner,
            inner.height - 2,
            &Line::from(vec![
                Span::styled("BURN  ", Style::new().fg(t.primary).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{}/h", fmt::money(burn.cost_per_hour)), Style::new().fg(t.tertiary)),
                sep(),
                Span::styled(format!("{} tok/m", fmt::si_f(burn.prompt_per_min + burn.output_per_min)), Style::new().fg(t.text)),
                sep(),
                Span::styled("block ends near ", Style::new().fg(t.dim)),
                Span::styled(fmt::money(projected), Style::new().fg(t.tertiary)),
            ]),
        );
    }
    let l = match active {
        Some(b) => Line::from(vec![
            Span::styled("BLOCK ", Style::new().fg(t.primary).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{}→{}", fmt::hm(b.start), fmt::hm(b.end)), Style::new().fg(t.text)),
            sep(),
            Span::styled(fmt::money(b.cost), Style::new().fg(t.tertiary)),
            sep(),
            Span::styled(format!("{} tok", fmt::si(b.usage.total())), Style::new().fg(t.text)),
            sep(),
            Span::styled(format!("{} req", b.requests), Style::new().fg(t.text)),
            Span::styled(" ▸ ends ", Style::new().fg(t.dim)),
            Span::styled(fmt::dur_secs((b.end - now).num_seconds()), Style::new().fg(t.secondary)),
        ]),
        None => Line::from(vec![
            Span::styled("BLOCK ", Style::new().fg(t.primary).add_modifier(Modifier::BOLD)),
            Span::styled("idle ▸ the next request opens a new 5h block", Style::new().fg(t.muted)),
        ]),
    };
    if y < inner.height || compact {
        line(buf, inner, inner.height - 1, &l);
    }
}

/// One window: the gauge line, plus (unless compact) the trend/projection line.
#[allow(clippy::too_many_arguments)]
fn window_rows(
    buf: &mut Buffer,
    inner: Rect,
    y: u16,
    label: &str,
    r: &WindowReading,
    hist: &VecDeque<(DateTime<Utc>, f64)>,
    window: i64,
    now: DateTime<Utc>,
    t: &Theme,
    compact: bool,
) -> Rect {
    let readings: Vec<(DateTime<Utc>, f64)> = hist.iter().copied().collect();
    let fit = if window == FIVE_HOURS { 90 } else { 24 * 60 };
    let proj = windows::project(&readings, r.used_pct, r.resets_at, window, now, fit);
    let reset = r
        .resets_at
        .map(|x| format!("⟳{}", fmt::dur_secs((x - now).num_seconds())))
        .unwrap_or_else(|| "⟳ —".into());
    let pct_txt = format!("{:>6}", fmt::pct(r.used_pct));
    let right_w = 7 + 1 + reset.chars().count() as u16;
    let gauge_w = inner.width.saturating_sub(4 + right_w + 1);
    let gx = inner.x + 4;
    buf.set_string(inner.x, inner.y + y, label, Style::new().fg(t.bright).add_modifier(Modifier::BOLD));
    let gauge = Rect::new(gx, inner.y + y, gauge_w, 1);
    neon_gauge(buf, gauge, r.used_pct, r.resets_at.map(|_| proj.pace_pct), t);
    buf.set_string(
        gx + gauge_w,
        inner.y + y,
        &pct_txt,
        Style::new().fg(t.level(r.used_pct)).add_modifier(Modifier::BOLD),
    );
    buf.set_string(gx + gauge_w + 7, inner.y + y, format!(" {reset}"), Style::new().fg(t.secondary));
    if compact {
        return gauge;
    }

    // Trend: step-sampled history over the whole window, as a braille area.
    let spark_w = (gauge_w / 2).max(8).min(gauge_w);
    let samples = spark_w as usize * 2;
    let span = Duration::seconds(window);
    let start = now - span;
    let mut vals = Vec::with_capacity(samples);
    for i in 0..samples {
        let ts = start + span * (i as i32 + 1) / samples as i32;
        let idx = readings.partition_point(|p| p.0 <= ts);
        vals.push(if idx == 0 { 0.0 } else { readings[idx - 1].1 });
    }
    let spark = Rect::new(gx, inner.y + y + 1, spark_w, 1);
    braille_area(buf, spark, &vals, 100.0, |v| t.gauge_at(v), t.bg);

    let status = r.status.clone().unwrap_or_else(|| "live".into());
    let mut spans = vec![
        Span::styled(format!(" {} ", status.to_uppercase()), Style::new().fg(t.level(r.used_pct))),
        Span::styled("pace ", Style::new().fg(t.dim)),
        Span::styled(fmt::pct(proj.pace_pct), Style::new().fg(t.text)),
    ];
    if let Some(hit) = proj.hits_full_at {
        spans.push(Span::styled(" ▸ ", Style::new().fg(t.dim)));
        spans.push(Span::styled(
            format!("100% in {}", fmt::dur_secs((hit - now).num_seconds())),
            Style::new().fg(t.danger).add_modifier(Modifier::BOLD),
        ));
    } else if let Some(at) = proj.at_reset_pct.filter(|_| r.resets_at.is_some()) {
        spans.push(Span::styled(" ▸ ", Style::new().fg(t.dim)));
        spans.push(Span::styled(format!("~{} at reset", fmt::pct(at)), Style::new().fg(t.muted)));
    }
    let info = Rect::new(spark.right(), spark.y, inner.right().saturating_sub(spark.right()), 1);
    buf.set_line(info.x, info.y, &Line::from(spans), info.width);
    gauge
}

// ---------------------------------------------------------------------------
// THROUGHPUT
// ---------------------------------------------------------------------------

fn throughput(buf: &mut Buffer, area: Rect, app: &App, st: &State) {
    let t = &app.theme;
    let secs = app.bucket_secs;
    let unit = match secs {
        10 => "10s",
        30 => "30s",
        60 => "1m",
        300 => "5m",
        _ => "15m",
    };
    let note = Line::from(Span::styled(
        format!(" col={unit}{} ", if app.log_scale { " log" } else { "" }),
        Style::new().fg(t.dim),
    ));
    let inner = panel(buf, area, t, "THROUGHPUT", Some(note), false);
    if inner.height < 3 || inner.width < 10 {
        return;
    }
    let now = st.now();
    let n = inner.width as usize;
    let reqs = st.since(now - Duration::seconds(secs * (n as i64 + 1)));
    let buckets = windows::buckets(reqs.iter().copied(), now, n, secs);
    let burn = windows::burn(reqs.iter().copied(), now, 10);
    let per_min = 60.0 / secs as f64;

    // Two graphs need head+graph+head+graph+stats; fall back to one graph when short.
    let two = inner.height >= 5;
    let gh = if two { (inner.height - 3) / 2 } else { inner.height.saturating_sub(2).max(1) };
    let prompt_cols: Vec<Vec<f64>> = buckets
        .iter()
        .map(|b| vec![b.input as f64, b.cache_write as f64, b.cache_read as f64])
        .collect();
    let out_cols: Vec<Vec<f64>> = buckets
        .iter()
        .map(|b| vec![b.output.saturating_sub(b.thinking) as f64, b.thinking as f64])
        .collect();
    let pmax = buckets.iter().map(|b| b.prompt()).max().unwrap_or(0) as f64;
    let omax = buckets.iter().map(|b| b.output).max().unwrap_or(0) as f64;

    let legend = |items: &[(&str, Color)]| -> Vec<Span<'static>> {
        let mut v = Vec::new();
        for (name, c) in items {
            v.push(Span::styled("■", Style::new().fg(*c)));
            v.push(Span::styled(format!("{name} "), Style::new().fg(t.muted)));
        }
        v
    };
    let mut head = vec![
        Span::styled("▲ PROMPT ", Style::new().fg(t.secondary).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}/m", fmt::si_f(burn.prompt_per_min)), Style::new().fg(t.bright)),
        Span::styled(format!(" pk {}/m  ", fmt::si_f(pmax * per_min)), Style::new().fg(t.dim)),
    ];
    head.extend(legend(&[("new", t.l_input), ("write", t.l_cache_write), ("read", t.l_cache_read)]));
    line(buf, inner, 0, &Line::from(head));
    let g1 = Rect::new(inner.x, inner.y + 1, inner.width, gh);
    stacked_bars(buf, g1, &prompt_cols, &[t.l_input, t.l_cache_write, t.l_cache_read], pmax, app.log_scale, t);

    if two {
        let mut head2 = vec![
            Span::styled("▼ OUTPUT ", Style::new().fg(t.tertiary).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{}/m", fmt::si_f(burn.output_per_min)), Style::new().fg(t.bright)),
            Span::styled(format!(" pk {}/m  ", fmt::si_f(omax * per_min)), Style::new().fg(t.dim)),
        ];
        head2.extend(legend(&[("text", t.l_output), ("thinking", t.l_thinking)]));
        line(buf, inner, 1 + gh, &Line::from(head2));
        let g2 = Rect::new(inner.x, inner.y + 2 + gh, inner.width, gh);
        stacked_bars(buf, g2, &out_cols, &[t.l_output, t.l_thinking], omax, app.log_scale, t);
    }

    let errors: u64 = buckets.iter().map(|b| b.errors).sum();
    let sep = || Span::styled(" ▸ ", Style::new().fg(t.dim));
    let mut stats = vec![
        Span::styled("req ", Style::new().fg(t.dim)),
        Span::styled(format!("{:.1}/m", burn.requests_per_min), Style::new().fg(t.text)),
        sep(),
        Span::styled(format!("{}/h", fmt::money(burn.cost_per_hour)), Style::new().fg(t.tertiary)),
        sep(),
        Span::styled("hit ", Style::new().fg(t.dim)),
        Span::styled(burn.cache_hit.map_or("—".into(), |h| fmt::pct(h * 100.0)), Style::new().fg(t.text)),
    ];
    if let Some(tt) = burn.ttft_p50_ms {
        stats.push(sep());
        stats.push(Span::styled("ttft ", Style::new().fg(t.dim)));
        stats.push(Span::styled(fmt::ms(tt), Style::new().fg(t.text)));
    }
    if let Some(d) = burn.duration_p50_ms {
        stats.push(sep());
        stats.push(Span::styled("p50 ", Style::new().fg(t.dim)));
        stats.push(Span::styled(fmt::ms(d), Style::new().fg(t.text)));
    }
    stats.push(sep());
    stats.push(Span::styled(
        format!("err {errors}"),
        Style::new().fg(if errors > 0 { t.danger } else { t.dim }),
    ));
    line(buf, inner, inner.height - 1, &Line::from(stats));
}

// ---------------------------------------------------------------------------
// PROCESSES
// ---------------------------------------------------------------------------

fn spinner(state: &SessionState, frame: u64) -> &'static str {
    const SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];
    match state {
        SessionState::Thinking => SPIN[(frame / 2 % 4) as usize],
        other => other.glyph(),
    }
}

fn state_color(s: &SessionState, t: &Theme) -> Color {
    match s {
        SessionState::Thinking => t.secondary,
        SessionState::Tool(_) => t.tertiary,
        SessionState::Waiting => t.warn,
        SessionState::Compacting => t.primary,
        SessionState::Idle => t.muted,
        SessionState::Ended => t.dim,
    }
}

fn effort_short(e: Option<&str>) -> &'static str {
    match e {
        Some("low") => "low",
        Some("medium") => "med",
        Some("high") => "hi",
        Some("xhigh") => "xhi",
        Some("max") => "max",
        _ => "",
    }
}

/// `/Users/me/dev/x` -> `~/dev/x`.
pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && path.starts_with(&h) => format!("~{}", &path[h.len()..]),
        _ => path.to_string(),
    }
}

fn processes(buf: &mut Buffer, area: Rect, app: &mut App, st: &State, sessions: &[&Session]) {
    let t = app.theme.clone();
    let now = st.now();
    let live = sessions.iter().filter(|s| s.alive()).count();
    let note = Line::from(Span::styled(
        format!(" {live} live · {} shown · sort {} ", sessions.len(), app.sort.label()),
        Style::new().fg(t.dim),
    ));
    let inner = panel(buf, area, &t, "PROCESSES", Some(note), false);
    if inner.height == 0 {
        return;
    }
    if sessions.is_empty() {
        line(
            buf,
            inner,
            0,
            &Line::from(Span::styled("no Claude Code sessions active in the last 15 minutes", Style::new().fg(t.muted))),
        );
        return;
    }
    app.sel_proc = app.sel_proc.min(sessions.len() - 1);
    // Column tiers: narrow < 100 <= medium < 150 <= wide.
    let medium = inner.width >= 100;
    let wide = inner.width >= 150;

    let mut header: Vec<&str> = vec!["", "PID", "SESSION", "MODEL"];
    let mut widths = vec![Constraint::Length(1), Constraint::Length(6), Constraint::Length(if medium { 18 } else { 14 }), Constraint::Length(10)];
    if wide {
        header.push("EFF");
        widths.push(Constraint::Length(3));
    }
    header.extend(["STATE", "CONTEXT"]);
    widths.extend([Constraint::Length(if medium { 15 } else { 11 }), Constraint::Length(if medium { 10 } else { 9 })]);
    if medium {
        header.extend(["TURN", "REQ", "TOKENS"]);
        widths.extend([Constraint::Length(4), Constraint::Length(7), Constraint::Length(6)]);
    } else {
        header.push("REQ");
        widths.push(Constraint::Length(5));
    }
    if wide {
        header.push("HIT");
        widths.push(Constraint::Length(4));
    }
    header.push("COST");
    widths.push(Constraint::Length(7));
    if wide {
        header.extend(["CPU", "MEM"]);
        widths.extend([Constraint::Length(5), Constraint::Length(5)]);
    }
    header.push("LAST");
    widths.push(Constraint::Length(5));
    if medium {
        header.push("PROJECT");
        widths.push(Constraint::Min(10));
    }

    let frame = app.frames;
    let bar_w = if medium { 5 } else { 4 };
    let rows: Vec<Row> = sessions
        .iter()
        .map(|s| {
            let ended = s.state == SessionState::Ended;
            let base = if ended {
                Style::new().fg(t.dim)
            } else if s.state.is_busy() {
                Style::new().fg(t.bright)
            } else {
                Style::new().fg(t.text)
            };
            let sc = state_color(&s.state, &t);
            let ctx = s.context_pct(&st.pricing, now);
            let mut ctx_spans = match ctx {
                Some(p) => mini_bar(p, bar_w, &t),
                None => vec![Span::styled("·".repeat(bar_w), Style::new().fg(t.grid))],
            };
            ctx_spans.push(Span::styled(
                ctx.map_or("    —".into(), |p| format!(" {:>4}", fmt::pct(p))),
                Style::new().fg(ctx.map_or(t.dim, |p| t.level(p))),
            ));
            let model = s.model.as_deref().map(|m| st.pricing.label(m)).unwrap_or_else(|| "—".into());
            let pid = s.proc.as_ref().map_or("—".into(), |p| p.pid.to_string());
            let name_w = if medium { 18 } else { 14 };
            let mut cells = vec![
                Cell::from(Span::styled(spinner(&s.state, frame), Style::new().fg(sc))),
                Cell::from(Span::styled(pid, Style::new().fg(if s.alive() { t.secondary } else { t.dim }))),
                Cell::from(fmt::fit(&s.display_name(), name_w)).style(base.add_modifier(Modifier::BOLD)),
                Cell::from(model).style(Style::new().fg(t.primary)),
            ];
            if wide {
                cells.push(Cell::from(effort_short(s.effort.as_deref())).style(Style::new().fg(t.muted)));
            }
            let since = fmt::ago(s.state_ts.max(s.first_ts), now);
            let show_since = medium && (s.state.is_busy() || s.state == SessionState::Waiting);
            cells.push(Cell::from(Line::from(vec![
                Span::styled(fmt::fit(&s.state.label(), if medium { 10 } else { 11 }), Style::new().fg(sc)),
                Span::styled(if show_since { format!(" {since}") } else { String::new() }, Style::new().fg(t.dim)),
            ])));
            cells.push(Cell::from(Line::from(ctx_spans)));
            let req_line = if medium && s.subagent_requests > 0 {
                Line::from(vec![
                    Span::styled(s.requests.to_string(), base),
                    Span::styled(format!("+{}", s.subagent_requests), Style::new().fg(t.dim)),
                ])
            } else {
                Line::from(Span::styled(s.requests.to_string(), base))
            };
            if medium {
                cells.push(Cell::from(s.turns.to_string()).style(base));
                cells.push(Cell::from(req_line));
                cells.push(Cell::from(fmt::si(s.usage.total())).style(base));
            } else {
                cells.push(Cell::from(req_line));
            }
            if wide {
                cells.push(Cell::from(s.cache_hit().map_or("—".into(), |h| format!("{:.0}%", h * 100.0))).style(Style::new().fg(t.muted)));
            }
            cells.push(Cell::from(fmt::money(s.cost)).style(Style::new().fg(t.tertiary)));
            if wide {
                let cpu = s.proc.as_ref().and_then(|p| p.cpu_pct);
                cells.push(Cell::from(cpu.map_or("—".into(), |c| format!("{c:.1}"))).style(Style::new().fg(match cpu {
                    Some(c) if c > 50.0 => t.warn,
                    Some(_) => t.text,
                    None => t.dim,
                })));
                cells.push(
                    Cell::from(s.proc.as_ref().and_then(|p| p.rss_kb).map_or("—".into(), |k| fmt::si(k * 1024)))
                        .style(Style::new().fg(t.muted)),
                );
            }
            cells.push(Cell::from(fmt::ago(s.last_ts, now)).style(Style::new().fg(t.muted)));
            if medium {
                let mut proj = vec![Span::styled(tilde(s.cwd.as_deref().unwrap_or("")), Style::new().fg(t.muted))];
                if let Some(b) = &s.branch {
                    proj.push(Span::styled(format!(" ⎇ {b}"), Style::new().fg(t.dim)));
                }
                cells.push(Cell::from(Line::from(proj)));
            }
            Row::new(cells)
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(
            Row::new(header.into_iter().map(Cell::from))
                .style(Style::new().fg(t.bg).bg(t.secondary).add_modifier(Modifier::BOLD)),
        )
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(lerp(t.bg, t.primary, 0.22)))
        .style(Style::new().bg(t.bg));
    let mut ts = TableState::default().with_selected(Some(app.sel_proc));
    StatefulWidget::render(table, inner, buf, &mut ts);
}

// ---------------------------------------------------------------------------
// WIRE
// ---------------------------------------------------------------------------

/// Which request-table columns fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cols {
    Narrow,
    Normal,
    WithKey,
}

impl Cols {
    pub fn for_width(w: u16, allow_key: bool) -> Cols {
        if w < 100 {
            Cols::Narrow
        } else if allow_key && w >= 170 {
            Cols::WithKey
        } else {
            Cols::Normal
        }
    }
}

pub fn request_row<'a>(r: &Request, st: &State, t: &Theme, now: DateTime<Utc>, cols: Cols) -> Row<'a> {
    let session = r
        .session_id
        .as_ref()
        .and_then(|id| st.sessions.get(id))
        .map(|s| s.display_name())
        .unwrap_or_else(|| "—".into());
    let fresh = (now - r.ts).num_milliseconds().abs() < 2500;
    let base = if r.is_error() {
        Style::new().fg(t.danger)
    } else if fresh {
        Style::new().fg(t.bright)
    } else {
        Style::new().fg(t.text)
    };
    let lat = match (r.ttft_ms, r.duration_ms) {
        (Some(tt), _) => fmt::ms(tt),
        (None, Some(d)) => fmt::ms(d),
        _ => "—".into(),
    };
    let stop = match (&r.error, &r.stop_reason) {
        (Some(e), _) => fmt::fit(e, 12),
        (None, Some(s)) => s.replace('_', " "),
        _ => "…".into(),
    };
    let model = if r.model.is_empty() { "—".into() } else { st.pricing.label(&r.model) };
    let cost = format!("{}{}", if r.cost_estimated { "~" } else { "" }, fmt::money(r.cost_usd));
    let mut cells = vec![
        Cell::from(fmt::clock(r.ts)).style(Style::new().fg(t.muted)),
        Cell::from(fmt::fit(&session, 14)).style(base),
        Cell::from(format!("{}{}", if r.sidechain { "↳" } else { "" }, model)).style(Style::new().fg(t.primary)),
    ];
    if cols == Cols::Narrow {
        cells.extend([
            Cell::from(fmt::si(r.usage.prompt_total())).style(Style::new().fg(t.l_input)),
            Cell::from(fmt::si(r.usage.output)).style(Style::new().fg(t.l_output)),
            Cell::from(cost).style(Style::new().fg(t.tertiary)),
            Cell::from(stop).style(base),
            Cell::from(lat).style(Style::new().fg(t.text)),
        ]);
    } else {
        cells.extend([
            Cell::from(fmt::si(r.usage.input)).style(Style::new().fg(t.l_input)),
            Cell::from(fmt::si(r.usage.cache_read)).style(Style::new().fg(t.muted)),
            Cell::from(fmt::si(r.usage.cache_write())).style(Style::new().fg(t.l_cache_write)),
            Cell::from(fmt::si(r.usage.output)).style(Style::new().fg(t.l_output)),
            Cell::from(cost).style(Style::new().fg(t.tertiary)),
            Cell::from(stop).style(base),
            Cell::from(lat).style(Style::new().fg(t.text)),
            Cell::from(r.sources.tag()).style(Style::new().fg(t.dim)),
            Cell::from(r.tools.iter().map(|x| crate::model::short_tool(x)).collect::<Vec<_>>().join(" ")).style(Style::new().fg(t.tertiary)),
        ]);
        if cols == Cols::WithKey {
            cells.push(Cell::from(r.key.clone()).style(Style::new().fg(t.dim)));
        }
    }
    Row::new(cells).style(if fresh { Style::new().bg(lerp(t.bg, t.primary, 0.12)) } else { Style::new() })
}

pub fn request_widths(cols: Cols) -> Vec<Constraint> {
    let mut w = vec![Constraint::Length(8), Constraint::Length(14), Constraint::Length(11)];
    match cols {
        Cols::Narrow => w.extend([
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Min(8),
            Constraint::Length(6),
        ]),
        Cols::Normal | Cols::WithKey => {
            w.extend([
                Constraint::Length(6),
                Constraint::Length(6),
                Constraint::Length(6),
                Constraint::Length(6),
                Constraint::Length(8),
                Constraint::Length(11),
                Constraint::Length(7),
                Constraint::Length(3),
                Constraint::Min(8),
            ]);
            if cols == Cols::WithKey {
                w.push(Constraint::Length(30));
            }
        }
    }
    w
}

pub fn request_header<'a>(t: &Theme, cols: Cols) -> Row<'a> {
    let h: Vec<&str> = match cols {
        Cols::Narrow => vec!["TIME", "SESSION", "MODEL", "PROMPT", "OUT", "COST", "STOP", "LAT"],
        Cols::Normal => vec!["TIME", "SESSION", "MODEL", "IN", "C.RD", "C.WR", "OUT", "COST", "STOP", "LAT", "SRC", "TOOLS"],
        Cols::WithKey => vec!["TIME", "SESSION", "MODEL", "IN", "C.RD", "C.WR", "OUT", "COST", "STOP", "LAT", "SRC", "TOOLS", "REQUEST ID"],
    };
    Row::new(h.into_iter().map(Cell::from)).style(Style::new().fg(t.bg).bg(t.primary).add_modifier(Modifier::BOLD))
}

fn wire(buf: &mut Buffer, area: Rect, app: &mut App, st: &State) {
    let t = app.theme.clone();
    let now = st.now();
    let note = Line::from(Span::styled(format!(" {} requests loaded ", st.requests.len()), Style::new().fg(t.dim)));
    let inner = panel(buf, area, &t, "WIRE", Some(note), false);
    app.areas.wire = inner;
    if inner.height < 2 {
        return;
    }
    app.areas.wire_first_row = Rect::new(inner.x, inner.y + 1, inner.width, 1);
    let recent = st.recent(inner.height as usize - 1);
    if recent.is_empty() {
        line(
            buf,
            inner,
            0,
            &Line::from(Span::styled(
                if st.scan.finished { "no API traffic recorded yet" } else { "scanning transcripts…" },
                Style::new().fg(t.muted),
            )),
        );
        return;
    }
    let cols = Cols::for_width(inner.width, false);
    let rows: Vec<Row> = recent.iter().map(|r| request_row(r, st, &t, now, cols)).collect();
    let table = Table::new(rows, request_widths(cols))
        .header(request_header(&t, cols))
        .column_spacing(1)
        .style(Style::new().bg(t.bg));
    ratatui::widgets::Widget::render(table, inner, buf);
}

// ---------------------------------------------------------------------------
// MODELS / TOOLS
// ---------------------------------------------------------------------------

fn models_tools(buf: &mut Buffer, area: Rect, app: &App, st: &State) {
    let t = &app.theme;
    let inner = panel(buf, area, t, "MODELS · TOOLS", None, false);
    if inner.height == 0 {
        return;
    }
    let today = st.today();
    let mut models: Vec<(&String, &(u64, f64))> = today.by_model.iter().filter(|(_, v)| v.0 > 0).collect();
    models.sort_by(|a, b| b.1.1.total_cmp(&a.1.1).then(a.0.cmp(b.0)));
    let maxc = models.first().map_or(0.0, |m| m.1.1);
    let mut y = 0;
    line(buf, inner, y, &Line::from(Span::styled("TODAY BY MODEL", Style::new().fg(t.secondary).add_modifier(Modifier::BOLD))));
    y += 1;
    if models.is_empty() {
        line(buf, inner, y, &Line::from(Span::styled("no requests today", Style::new().fg(t.dim))));
        y += 1;
    }
    let bar_w = inner.width.saturating_sub(11 + 8 + 2) as usize;
    let half = inner.height / 2;
    for (name, (tok, cost)) in models.iter().take((half.saturating_sub(1)) as usize) {
        let l = Line::from(vec![
            Span::styled(format!("{:<11}", fmt::fit(name, 10)), Style::new().fg(t.primary)),
            hbar(*cost, maxc, bar_w, t.primary),
            Span::styled(format!(" {:>7}", fmt::money(*cost)), Style::new().fg(t.tertiary)),
        ]);
        line(buf, inner, y, &l);
        let _ = tok;
        y += 1;
    }
    y = y.max(half);
    line(buf, inner, y, &Line::from(Span::styled("TOOLS", Style::new().fg(t.secondary).add_modifier(Modifier::BOLD))));
    y += 1;
    let mut tools: Vec<_> = st.tools.iter().collect();
    tools.sort_by(|a, b| b.1.calls.cmp(&a.1.calls).then(a.0.cmp(b.0)));
    let maxt = tools.first().map_or(0, |x| x.1.calls) as f64;
    let bar_w = inner.width.saturating_sub(12 + 6 + 5 + 2) as usize;
    for (name, stat) in tools.iter().take(inner.height.saturating_sub(y) as usize) {
        let fail = if stat.calls > 0 { stat.failures as f64 / stat.calls as f64 * 100.0 } else { 0.0 };
        let l = Line::from(vec![
            Span::styled(format!("{:<12}", fmt::fit(&crate::model::short_tool(name), 12)), Style::new().fg(t.text)),
            hbar(stat.calls as f64, maxt, bar_w, t.secondary),
            Span::styled(format!(" {:>5}", fmt::si(stat.calls)), Style::new().fg(t.muted)),
            Span::styled(
                if stat.failures > 0 { format!(" {fail:>3.0}%") } else { "     ".into() },
                Style::new().fg(if fail > 20.0 { t.danger } else { t.warn }),
            ),
        ]);
        line(buf, inner, y, &l);
        y += 1;
    }
    let _ = widgets::clear;
}
