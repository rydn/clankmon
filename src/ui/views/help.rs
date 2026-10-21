//! Help overlay: keys, glyphs and data sources.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};

use crate::ui::{App, popup};

pub fn render(buf: &mut Buffer, area: Rect, app: &App) {
    let t = &app.theme;
    let h = |s: &str| Line::from(Span::styled(s.to_string(), Style::new().fg(t.primary).add_modifier(Modifier::BOLD)));
    let k = |key: &str, desc: &str| {
        Line::from(vec![
            Span::styled(format!("  {key:<12}"), Style::new().fg(t.secondary).add_modifier(Modifier::BOLD)),
            Span::styled(desc.to_string(), Style::new().fg(t.text)),
        ])
    };
    let lines = vec![
        h("KEYS"),
        k("1-5 tab", "overview · sessions · requests · history · limits"),
        k("↑↓ jk pgup", "move selection"),
        k("enter", "open session (overview) · request detail (requests)"),
        k("/", "filter requests by session, model, stop, tool or 'error'"),
        k("s", "cycle process sort: activity cost tokens cpu name"),
        k("+ -", "graph column width · limits chart span"),
        k("g", "toggle log scale on throughput"),
        k("t f", "cycle theme · toggle effects"),
        k("p space", "pause the feed (events are buffered)"),
        k("q esc", "quit"),
        Line::raw(""),
        h("STATES"),
        k("◐ thinking", "waiting on the model"),
        k("⚙ tool", "running a tool"),
        k("⏸ permission", "blocked on a permission prompt (needs hooks)"),
        k("◇ compacting", "conversation compaction"),
        k("○ idle  ✕ ended", "turn done · process exited"),
        Line::raw(""),
        h("SOURCES (SRC column: J transcript · P proxy · O otel)"),
        k("jsonl", "~/.claude/projects transcripts, always on"),
        k("procs", "~/.claude/sessions registry + ps"),
        k("status hooks", "clankmon install wires Claude Code's statusline and hooks"),
        k("proxy", "clankmon proxy + ANTHROPIC_BASE_URL: exact limit headers, TTFT"),
        k("otel", "clankmon --otel + OTLP http/json env vars"),
        k("oauth", "clankmon --oauth: /api/oauth/usage, polled slowly"),
        Line::raw(""),
        Line::from(Span::styled(
            "  costs are list-price equivalents; subscription usage is not billed per token",
            Style::new().fg(t.dim),
        )),
    ];
    let r = popup(area, 84, lines.len() as u16 + 2);
    Clear.render(r, buf);
    Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::new().fg(t.secondary))
                .title(Line::from(Span::styled(
                    " ◢ CLANKMON HELP ◣ ",
                    Style::new().fg(t.bright).add_modifier(Modifier::BOLD),
                )))
                .title_bottom(Line::from(Span::styled(" any key to close ", Style::new().fg(t.dim))).right_aligned())
                .style(Style::new().bg(t.panel)),
        )
        .render(r, buf);
}
