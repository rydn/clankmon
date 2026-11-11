//! Snapshot renders of every view at three terminal sizes, from deterministic demo data.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use clankmon::demo;
use clankmon::pricing::Pricing;
use clankmon::state::State;
use clankmon::ui::theme::Theme;
use clankmon::ui::{self, App, View};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-21T19:30:00Z").unwrap().with_timezone(&Utc)
}

fn state() -> State {
    // Times render in local time; pin it so snapshots match everywhere.
    // SAFETY: every test sets the same value before reading the clock.
    unsafe { std::env::set_var("TZ", "UTC") };
    let mut st = State::new(Arc::new(Pricing::default()));
    st.clock = Some(now());
    st.plan = Some("max".into());
    st.tier = Some("default_claude_max_20x".into());
    for e in demo::history(now(), 7) {
        st.apply(e);
    }
    st.ensure_sorted();
    st.fx.clear();
    st.toasts.clear();
    st
}

fn render(view: View, w: u16, h: u16) -> String {
    render_with(view, w, h, false)
}

fn render_with(view: View, w: u16, h: u16, help: bool) -> String {
    let mut st = state();
    let mut app = App::new(Theme::neon(), false);
    app.truecolor = true;
    app.view = view;
    app.help = help;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::render(f, &mut app, &st)).unwrap();
    st.ensure_sorted();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn overview_large() {
    insta::assert_snapshot!(render(View::Overview, 200, 50));
}

#[test]
fn overview_medium() {
    insta::assert_snapshot!(render(View::Overview, 120, 40));
}

#[test]
fn overview_small() {
    insta::assert_snapshot!(render(View::Overview, 80, 24));
}

#[test]
fn sessions_view() {
    insta::assert_snapshot!(render(View::Sessions, 160, 45));
}

#[test]
fn requests_view() {
    insta::assert_snapshot!(render(View::Requests, 160, 30));
}

#[test]
fn history_view() {
    insta::assert_snapshot!(render(View::History, 160, 45));
}

#[test]
fn limits_view() {
    insta::assert_snapshot!(render(View::Limits, 160, 45));
}

#[test]
fn help_popup() {
    insta::assert_snapshot!(render_with(View::Overview, 160, 45, true));
}

#[test]
fn help_popup_energy_lines_not_clipped() {
    let out = render_with(View::Overview, 160, 45, true);
    for needle in [
        "ENERGY",
        "tokens × assumed Wh per million tokens by model tier",
        clankmon::energy::SOURCE,
        "Anthropic publishes no per-token energy; order of magnitude only",
    ] {
        assert!(out.contains(needle), "help popup clips or lacks: {needle}");
    }
}

#[test]
fn sessions_detail_shows_estimated_energy() {
    let out = render(View::Sessions, 160, 45);
    let line = out.lines().find(|l| l.contains("energy ")).expect("energy line in session detail");
    assert!(line.contains("≈"), "{line}");
    assert!(line.contains("est. · assumed rates, ? for method"), "clipped: {line}");
}

#[test]
fn overview_labels_energy_as_estimate() {
    let big = render(View::Overview, 200, 50);
    assert!(big.contains("W est."), "burn power missing");
    assert!(big.contains("kWh") || big.contains("Wh"), "block energy missing");
    assert!(big.contains(" est."), "header estimate marker missing");
}

#[test]
fn overview_small_has_no_energy_in_compact_header() {
    let out = render(View::Overview, 80, 24);
    let header: Vec<&str> = out.lines().take(3).collect();
    assert!(header.iter().all(|l| !l.contains("est.")), "{header:?}");
}

#[test]
fn tiny_terminal_does_not_panic() {
    for (w, h) in [(1, 1), (10, 3), (40, 10), (60, 15)] {
        for v in View::ALL {
            let _ = render(v, w, h);
            let _ = render_with(v, w, h, true);
        }
    }
}
