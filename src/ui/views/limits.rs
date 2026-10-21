//! Rate-limit deep dive: utilisation history, sources, extra windows, incidents.

use chrono::Duration;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Cell, Chart, Dataset, GraphType, Row, Table, Widget};

use crate::model::Collector;
use crate::model::CollectorHealth;
use crate::state::State;
use crate::ui::App;
use crate::ui::fmt;
use crate::ui::widgets::{line, mini_bar, panel};

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App, st: &State) {
    let t = app.theme.clone();
    let now = st.now();
    let [chart_area, mid, bottom] =
        Layout::vertical([Constraint::Percentage(42), Constraint::Length(10), Constraint::Min(5)]).areas(area);

    // Utilisation history.
    let span = Duration::hours(app.limit_span_hours);
    let start = now - span;
    let note = Line::from(Span::styled(
        format!(
            " last {} · +/- span ",
            if app.limit_span_hours >= 168 { "7d".into() } else { format!("{}h", app.limit_span_hours) }
        ),
        Style::new().fg(t.dim),
    ));
    let inner = panel(buf, chart_area, &t, "UTILISATION", Some(note), false);
    let to_pts = |h: &std::collections::VecDeque<(chrono::DateTime<chrono::Utc>, f64)>| -> Vec<(f64, f64)> {
        h.iter().filter(|(ts, _)| *ts >= start).map(|(ts, p)| ((*ts - start).num_seconds() as f64, *p)).collect()
    };
    let five = to_pts(&st.limits.five_history);
    let seven = to_pts(&st.limits.seven_history);
    let xmax = span.num_seconds() as f64;
    let warn80 = vec![(0.0, 80.0), (xmax, 80.0)];
    let warn90 = vec![(0.0, 90.0), (xmax, 90.0)];
    if five.is_empty() && seven.is_empty() {
        line(buf, inner, 0, &Line::from(Span::styled("no limit readings in this span", Style::new().fg(t.muted))));
    } else {
        let ds = vec![
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::new().fg(t.dim))
                .data(&warn80),
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::new().fg(t.grid))
                .data(&warn90),
            Dataset::default()
                .name("7d")
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::new().fg(t.primary))
                .data(&seven),
            Dataset::default()
                .name("5h")
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::new().fg(t.secondary))
                .data(&five),
        ];
        let labels_x = vec![
            Span::styled(fmt::hm(start), Style::new().fg(t.dim)),
            Span::styled(fmt::hm(start + span / 2), Style::new().fg(t.dim)),
            Span::styled("now", Style::new().fg(t.dim)),
        ];
        Chart::new(ds)
            .style(Style::new().bg(t.bg))
            .x_axis(Axis::default().bounds([0.0, xmax]).labels(labels_x).style(Style::new().fg(t.grid)))
            .y_axis(
                Axis::default()
                    .bounds([0.0, 100.0])
                    .labels(["0", "50", "100"].map(|s| Span::styled(s, Style::new().fg(t.dim))))
                    .style(Style::new().fg(t.grid)),
            )
            .render(inner, buf);
    }

    // Sources and extra windows.
    let [src_area, extra_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(mid);
    let inner = panel(buf, src_area, &t, "SOURCES", None, false);
    let mut rows = Vec::new();
    for (src, snap) in &st.limits.by_source {
        let f = snap.five_hour.as_ref().map_or("—".into(), |w| fmt::pct(w.used_pct));
        let s = snap.seven_day.as_ref().map_or("—".into(), |w| fmt::pct(w.used_pct));
        rows.push(Row::new(vec![
            Cell::from(src.label()).style(Style::new().fg(t.secondary)),
            Cell::from(f).style(Style::new().fg(t.text)),
            Cell::from(s).style(Style::new().fg(t.text)),
            Cell::from(snap.overall_status.clone().unwrap_or_else(|| "—".into())).style(Style::new().fg(t.muted)),
            Cell::from(fmt::ago(snap.ts, now)).style(Style::new().fg(t.dim)),
        ]));
    }
    for c in [Collector::Statusline, Collector::Proxy, Collector::OAuth] {
        if let Some((CollectorHealth::Error(e), _)) = st.collectors.get(&c) {
            rows.push(Row::new(vec![
                Cell::from(c.label()).style(Style::new().fg(t.danger)),
                Cell::from(fmt::fit(e, 40)).style(Style::new().fg(t.danger)),
            ]));
        }
    }
    if rows.is_empty() {
        line(
            buf,
            inner,
            0,
            &Line::from(Span::styled("no sources reporting; run `clankmon doctor`", Style::new().fg(t.muted))),
        );
    } else {
        Table::new(
            rows,
            [
                Constraint::Length(11),
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Min(8),
                Constraint::Length(6),
            ],
        )
        .header(
            Row::new(["SOURCE", "5H", "7D", "STATUS", "AGE"].map(Cell::from))
                .style(Style::new().fg(t.bg).bg(t.secondary).add_modifier(Modifier::BOLD)),
        )
        .column_spacing(1)
        .render(inner, buf);
    }

    let inner = panel(buf, extra_area, &t, "WINDOWS", None, false);
    let mut y = 0;
    let mut add = |name: &str, pct: f64, reset: Option<chrono::DateTime<chrono::Utc>>, status: Option<&str>| {
        if y >= inner.height {
            return;
        }
        let mut spans = vec![Span::styled(format!("{:<18}", fmt::fit(name, 18)), Style::new().fg(t.text))];
        spans.extend(mini_bar(pct, 12, &t));
        spans.push(Span::styled(format!(" {:>6}", fmt::pct(pct)), Style::new().fg(t.level(pct))));
        if let Some(r) = reset {
            spans.push(Span::styled(format!(" ⟳{}", fmt::dur_secs((r - now).num_seconds())), Style::new().fg(t.dim)));
        }
        if let Some(s) = status {
            spans.push(Span::styled(format!(" {s}"), Style::new().fg(t.muted)));
        }
        line(buf, inner, y, &Line::from(spans));
        y += 1;
    };
    if let Some((w, _, _)) = st.window(true) {
        add("five_hour", w.used_pct, w.resets_at, w.status.as_deref());
    }
    if let Some((w, _, _)) = st.window(false) {
        add("seven_day", w.used_pct, w.resets_at, w.status.as_deref());
    }
    for (k, (w, _, _)) in &st.limits.extra {
        add(k, w.used_pct, w.resets_at, w.status.as_deref());
    }
    if let Some(c) = &st.limits.representative_claim
        && y < inner.height
    {
        line(
            buf,
            inner,
            y,
            &Line::from(vec![
                Span::styled("binding window    ", Style::new().fg(t.muted)),
                Span::styled(c.clone(), Style::new().fg(t.primary).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("  overall {}", st.limits.overall_status.clone().unwrap_or_else(|| "—".into())),
                    Style::new().fg(t.dim),
                ),
            ]),
        );
        y += 1;
    }
    if let Some(e) = &st.oauth.error {
        line(
            buf,
            inner,
            y.min(inner.height.saturating_sub(1)),
            &Line::from(Span::styled(format!("oauth: {e}"), Style::new().fg(t.danger))),
        );
    }

    // Incidents.
    let inner = panel(
        buf,
        bottom,
        &t,
        "INCIDENTS",
        Some(Line::from(Span::styled(format!(" {} recorded ", st.incidents.len()), Style::new().fg(t.dim)))),
        false,
    );
    if st.incidents.is_empty() {
        line(
            buf,
            inner,
            0,
            &Line::from(Span::styled("no 429s or API errors in the loaded history", Style::new().fg(t.ok))),
        );
        return;
    }
    let rows: Vec<Row> = st
        .incidents
        .iter()
        .rev()
        .take(inner.height as usize)
        .map(|i| {
            Row::new(vec![
                Cell::from(i.ts.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string())
                    .style(Style::new().fg(t.muted)),
                Cell::from(i.kind.clone()).style(Style::new().fg(t.danger)),
                Cell::from(i.http_status.map_or("".into(), |s| s.to_string())).style(Style::new().fg(t.warn)),
                Cell::from(i.resets_at.map_or("".into(), |r| format!("resets {}", fmt::hm(r))))
                    .style(Style::new().fg(t.dim)),
                Cell::from(i.message.clone()).style(Style::new().fg(t.text)),
            ])
        })
        .collect();
    Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Length(14),
            Constraint::Length(4),
            Constraint::Length(13),
            Constraint::Min(10),
        ],
    )
    .column_spacing(1)
    .render(inner, buf);
}
