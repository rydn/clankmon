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
    let mut st = state();
    let mut app = App::new(Theme::neon(), false);
    app.truecolor = true;
    app.view = view;
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
fn tiny_terminal_does_not_panic() {
    for (w, h) in [(1, 1), (10, 3), (40, 10), (60, 15)] {
        for v in View::ALL {
            let _ = render(v, w, h);
        }
    }
}
