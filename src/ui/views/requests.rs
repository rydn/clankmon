//! Scrollable, filterable request log with a detail popup.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Row, StatefulWidget, Table, TableState, Widget, Wrap};

use super::overview::{Cols, request_header, request_row, request_widths};
use crate::model::Request;
use crate::state::State;
use crate::ui::fmt;
use crate::ui::theme::lerp;
use crate::ui::widgets::panel;
use crate::ui::{App, popup};

fn matches(r: &Request, st: &State, f: &str) -> bool {
    if f.is_empty() {
        return true;
    }
    let f = f.to_lowercase();
    let session = r
        .session_id
        .as_ref()
        .and_then(|id| st.sessions.get(id))
        .map(|s| s.display_name())
        .unwrap_or_default();
    [
        session.as_str(),
        r.model.as_str(),
        r.key.as_str(),
        r.stop_reason.as_deref().unwrap_or(""),
        r.error.as_deref().unwrap_or(""),
        &r.tools.join(" "),
        if r.is_error() { "error" } else { "" },
    ]
    .iter()
    .any(|s| s.to_lowercase().contains(&f))
}

pub fn filtered<'a>(st: &'a State, f: &str) -> Vec<&'a Request> {
    st.timeline().rev().filter(|r| matches(r, st, f)).take(5000).collect()
}

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App, st: &State) {
    let t = app.theme.clone();
    let now = st.now();
    let reqs = filtered(st, &app.filter);
    let filter_note = if app.filter_edit {
        format!(" filter: {}▏ ", app.filter)
    } else if app.filter.is_empty() {
        " / filter · enter details ".to_string()
    } else {
        format!(" filter: {} ", app.filter)
    };
    let note = Line::from(vec![
        Span::styled(filter_note, Style::new().fg(if app.filter_edit { t.tertiary } else { t.dim })),
        Span::styled(format!("{} shown ", reqs.len()), Style::new().fg(t.dim)),
    ]);
    let inner = panel(buf, area, &t, "REQUESTS", Some(note), true);
    if inner.height < 2 {
        return;
    }
    if reqs.is_empty() {
        buf.set_string(inner.x, inner.y, "no matching requests", Style::new().fg(t.muted));
        return;
    }
    app.sel_req = app.sel_req.min(reqs.len() - 1);
    let cols = Cols::for_width(inner.width, true);
    let rows: Vec<Row> = reqs.iter().map(|r| request_row(r, st, &t, now, cols)).collect();
    let table = Table::new(rows, request_widths(cols))
        .header(request_header(&t, cols))
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(lerp(t.bg, t.primary, 0.25)))
        .style(Style::new().bg(t.bg));
    let mut ts = TableState::default().with_selected(Some(app.sel_req));
    StatefulWidget::render(table, inner, buf, &mut ts);
}

pub fn render_detail(buf: &mut Buffer, area: Rect, app: &App, st: &State) {
    let t = &app.theme;
    let reqs = filtered(st, &app.filter);
    let Some(r) = reqs.get(app.sel_req) else { return };
    let r = *r;
    let kv = |k: &str, v: String| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("{k:<14}"), Style::new().fg(t.muted)),
            Span::styled(v, Style::new().fg(t.text)),
        ])
    };
    let session = r.session_id.as_ref().and_then(|id| st.sessions.get(id)).map(|s| s.display_name());
    let mut lines = vec![
        kv("request id", r.request_id.clone().unwrap_or_else(|| r.key.clone())),
        kv("message id", r.message_id.clone().unwrap_or_else(|| "—".into())),
        kv("time", r.ts.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S%.3f").to_string()),
        kv("session", format!("{} {}", session.unwrap_or_default(), r.session_id.clone().unwrap_or_default())),
        kv("model", format!("{} ({})", st.pricing.label(&r.model), r.model)),
        kv("effort / tier", format!("{} · {} · {}", r.effort.clone().unwrap_or_else(|| "—".into()), r.service_tier.clone().unwrap_or_else(|| "—".into()), r.speed.clone().unwrap_or_else(|| "normal".into()))),
        kv("subagent", if r.sidechain { "yes".into() } else { "no".into() }),
        kv("stop", r.stop_reason.clone().unwrap_or_else(|| "—".into())),
        kv("tools called", if r.tools.is_empty() { "—".into() } else { r.tools.join(", ") }),
        Line::raw(""),
        kv("input", fmt::si(r.usage.input)),
        kv("cache read", fmt::si(r.usage.cache_read)),
        kv("cache write", format!("{} (5m {} · 1h {})", fmt::si(r.usage.cache_write()), fmt::si(r.usage.cache_write_5m), fmt::si(r.usage.cache_write_1h))),
        kv("output", format!("{} (thinking {})", fmt::si(r.usage.output), fmt::si(r.usage.thinking))),
        kv("cost", format!("{}{}", fmt::money(r.cost_usd), if r.cost_estimated { " (estimated: unknown model)" } else { "" })),
        kv("latency", format!("ttft {} · total {}", r.ttft_ms.map_or("—".into(), fmt::ms), r.duration_ms.map_or("—".into(), fmt::ms))),
        kv("http", r.http_status.map_or("—".into(), |s| s.to_string())),
        kv("sources", r.sources.tag()),
    ];
    if let Some(e) = &r.error {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<14}", "error"), Style::new().fg(t.muted)),
            Span::styled(e.clone(), Style::new().fg(t.danger)),
        ]));
    }
    if let Some(s) = &r.shape {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("REQUEST SHAPE (proxy)", Style::new().fg(t.secondary).add_modifier(Modifier::BOLD))));
        lines.push(kv("body", format!("{} · {} messages · {} tools · system {}", fmt::si(s.body_bytes as u64), s.message_count, s.tool_count, fmt::si(s.system_bytes as u64))));
        lines.push(kv("max_tokens", s.max_tokens.map_or("—".into(), |m| m.to_string())));
        lines.push(kv("thinking", s.thinking.clone().unwrap_or_else(|| "—".into())));
        lines.push(kv("betas", if s.betas.is_empty() { "—".into() } else { s.betas.join(", ") }));
        if !s.limit_headers.is_empty() {
            lines.push(Line::from(Span::styled("RESPONSE HEADERS", Style::new().fg(t.secondary).add_modifier(Modifier::BOLD))));
            for (k, v) in &s.limit_headers {
                lines.push(Line::from(vec![
                    Span::styled(format!("{k}: "), Style::new().fg(t.dim)),
                    Span::styled(v.clone(), Style::new().fg(t.text)),
                ]));
            }
        }
    }
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let r = popup(area, 96, h);
    Clear.render(r, buf);
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::new().fg(t.primary))
                .title(Line::from(Span::styled(" ◢ REQUEST ◣ ", Style::new().fg(t.bright).add_modifier(Modifier::BOLD))))
                .title_bottom(Line::from(Span::styled(" esc close ", Style::new().fg(t.dim))).right_aligned())
                .style(Style::new().bg(t.panel)),
        )
        .render(r, buf);
}
