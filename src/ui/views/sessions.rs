//! Every session in the loaded history, with a detail pane.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, StatefulWidget, Table, TableState};

use crate::state::{Session, State};
use crate::ui::App;
use crate::ui::fmt;
use crate::ui::theme::lerp;
use crate::ui::widgets::{braille_area, hbar, line, mini_bar, panel};

pub fn all_sessions(st: &State) -> Vec<&Session> {
    let mut v: Vec<&Session> = st.sessions.values().filter(|s| s.requests > 0 || s.alive()).collect();
    v.sort_by(|a, b| b.alive().cmp(&a.alive()).then(b.last_ts.cmp(&a.last_ts)).then(a.id.cmp(&b.id)));
    v
}

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App, st: &State) {
    let t = app.theme.clone();
    let now = st.now();
    let sessions = all_sessions(st);
    if let Some(id) = app.focus_session.take()
        && let Some(i) = sessions.iter().position(|s| s.id == id)
    {
        app.sel_session = i;
    }
    let [list, detail] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);

    let note = Line::from(Span::styled(format!(" {} sessions ", sessions.len()), Style::new().fg(t.dim)));
    let inner = panel(buf, list, &t, "SESSIONS", Some(note), true);
    if sessions.is_empty() {
        line(buf, inner, 0, &Line::from(Span::styled("no sessions in the loaded history", Style::new().fg(t.muted))));
        return;
    }
    app.sel_session = app.sel_session.min(sessions.len() - 1);
    let rows: Vec<Row> = sessions
        .iter()
        .map(|s| {
            let style = if s.alive() { Style::new().fg(t.bright) } else { Style::new().fg(t.text) };
            Row::new(vec![
                Cell::from(Span::styled(if s.alive() { "●" } else { "○" }, Style::new().fg(if s.alive() { t.ok } else { t.dim }))),
                Cell::from(fmt::fit(&s.display_name(), 20)).style(style),
                Cell::from(s.model.as_deref().map(|m| st.pricing.label(m)).unwrap_or_default()).style(Style::new().fg(t.primary)),
                Cell::from(s.requests.to_string()).style(style),
                Cell::from(fmt::money(s.cost)).style(Style::new().fg(t.tertiary)),
                Cell::from(fmt::ago(s.last_ts, now)).style(Style::new().fg(t.muted)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(10),
            Constraint::Length(5),
            Constraint::Length(8),
            Constraint::Length(6),
        ],
    )
    .header(
        Row::new(["", "SESSION", "MODEL", "REQ", "COST", "LAST"].map(Cell::from))
            .style(Style::new().fg(t.bg).bg(t.secondary).add_modifier(Modifier::BOLD)),
    )
    .column_spacing(1)
    .row_highlight_style(Style::new().bg(lerp(t.bg, t.primary, 0.22)))
    .highlight_symbol("▶");
    let mut ts = TableState::default().with_selected(Some(app.sel_session));
    StatefulWidget::render(table, inner, buf, &mut ts);

    let s = sessions[app.sel_session];
    let inner = panel(buf, detail, &t, &format!("SESSION {}", s.id.chars().take(8).collect::<String>()), None, false);
    if inner.height == 0 {
        return;
    }
    let kv = |k: &str, v: String| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("{k:<11}"), Style::new().fg(t.muted)),
            Span::styled(v, Style::new().fg(t.text)),
        ])
    };
    let mut lines = vec![
        Line::from(Span::styled(s.display_name(), Style::new().fg(t.primary).add_modifier(Modifier::BOLD))),
        kv("cwd", s.cwd.clone().unwrap_or_default()),
        kv("branch", s.branch.clone().unwrap_or_else(|| "—".into())),
        kv(
            "model",
            format!(
                "{} {}",
                s.model.as_deref().map(|m| st.pricing.label(m)).unwrap_or_default(),
                s.effort.as_deref().map(|e| format!("· effort {e}")).unwrap_or_default()
            ),
        ),
        kv(
            "process",
            match &s.proc {
                Some(p) => format!(
                    "pid {} · {} · cpu {} · mem {}",
                    p.pid,
                    p.entrypoint.clone().unwrap_or_else(|| "cli".into()),
                    p.cpu_pct.map_or("—".into(), |c| format!("{c:.1}%")),
                    p.rss_kb.map_or("—".into(), |k| fmt::si(k * 1024))
                ),
                None => "not running".into(),
            },
        ),
        kv("state", format!("{} {} for {}", s.state.glyph(), s.state.label(), fmt::ago(s.state_ts.max(s.first_ts), now))),
        kv("active", format!("{} → {} ({})", fmt::hm(s.first_ts), fmt::hm(s.last_ts), fmt::dur_secs((s.last_ts - s.first_ts).num_seconds()))),
        kv("turns", format!("{} · {} requests · {} subagent · {} errors · {} compactions", s.turns, s.requests, s.subagent_requests, s.errors, s.compactions)),
        kv(
            "tokens",
            format!(
                "in {} · read {} · write {} · out {} · think {}",
                fmt::si(s.usage.input),
                fmt::si(s.usage.cache_read),
                fmt::si(s.usage.cache_write()),
                fmt::si(s.usage.output),
                fmt::si(s.usage.thinking)
            ),
        ),
    ];
    let mut cost = vec![
        Span::styled(format!("{:<11}", "cost"), Style::new().fg(t.muted)),
        Span::styled(fmt::money(s.cost), Style::new().fg(t.tertiary).add_modifier(Modifier::BOLD)),
        Span::styled(" list price", Style::new().fg(t.dim)),
    ];
    if let Some(c) = s.status.as_ref().and_then(|x| x.cost_usd) {
        cost.push(Span::styled(format!(" · Claude Code says {}", fmt::money(c)), Style::new().fg(t.muted)));
    }
    lines.push(Line::from(cost));
    let mut ctx = vec![Span::styled(format!("{:<11}", "context"), Style::new().fg(t.muted))];
    match s.context_pct(&st.pricing, now) {
        Some(p) => {
            ctx.extend(mini_bar(p, 20, &t));
            ctx.push(Span::styled(format!(" {} ({} tok)", fmt::pct(p), fmt::si(s.context_tokens)), Style::new().fg(t.level(p))));
        }
        None => ctx.push(Span::styled("—", Style::new().fg(t.dim))),
    }
    lines.push(Line::from(ctx));
    lines.push(kv("cache hit", s.cache_hit().map_or("—".into(), |h| fmt::pct(h * 100.0))));
    if let Some(sl) = &s.status
        && (sl.lines_added.is_some() || sl.lines_removed.is_some())
    {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<11}", "lines"), Style::new().fg(t.muted)),
            Span::styled(format!("+{}", sl.lines_added.unwrap_or(0)), Style::new().fg(t.ok)),
            Span::styled(format!(" -{}", sl.lines_removed.unwrap_or(0)), Style::new().fg(t.danger)),
        ]));
    }
    for (i, l) in lines.iter().enumerate() {
        line(buf, inner, i as u16, l);
    }
    let mut y = lines.len() as u16 + 1;

    // Context growth over the session's main-loop requests.
    let series: Vec<f64> = st
        .timeline()
        .filter(|r| r.session_id.as_deref() == Some(s.id.as_str()) && !r.sidechain)
        .map(|r| r.usage.prompt_total() as f64)
        .collect();
    if y + 4 < inner.height && !series.is_empty() {
        let max = series.iter().cloned().fold(0.0, f64::max);
        line(
            buf,
            inner,
            y,
            &Line::from(vec![
                Span::styled("CONTEXT PER REQUEST ", Style::new().fg(t.secondary).add_modifier(Modifier::BOLD)),
                Span::styled(format!("peak {} · {} requests", fmt::si_f(max), series.len()), Style::new().fg(t.dim)),
            ]),
        );
        let h = 4u16.min(inner.height - y - 1);
        let r = Rect::new(inner.x, inner.y + y + 1, inner.width, h);
        let tt = t.clone();
        braille_area(buf, r, &series, max, move |v| lerp(tt.secondary, tt.primary, v), t.bg);
        y += h + 2;
    }

    // Tool breakdown.
    if y < inner.height {
        let mut tools: Vec<_> = s.tools.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        line(buf, inner, y, &Line::from(Span::styled("TOOLS", Style::new().fg(t.secondary).add_modifier(Modifier::BOLD))));
        y += 1;
        let max = tools.first().map_or(0, |x| *x.1) as f64;
        let bar_w = inner.width.saturating_sub(16 + 7) as usize;
        for (name, n) in tools.into_iter().take(inner.height.saturating_sub(y) as usize) {
            line(
                buf,
                inner,
                y,
                &Line::from(vec![
                    Span::styled(format!("{:<16}", fmt::fit(&crate::model::short_tool(name), 16)), Style::new().fg(t.text)),
                    hbar(*n as f64, max, bar_w, t.tertiary),
                    Span::styled(format!(" {n:>5}"), Style::new().fg(t.muted)),
                ]),
            );
            y += 1;
        }
    }
}
