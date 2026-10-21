//! Regenerate the README images in `docs/assets/` from the deterministic demo:
//!
//! ```sh
//! cargo run --release --example docs
//! ```

use std::fmt::Write as _;

use chrono::{DateTime, Duration, Utc};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;
use tui_big_text::{BigText, PixelSize};

use clankmon::cli::tui::svg_options;
use clankmon::demo::{self, LiveSim};
use clankmon::model::AppEvent;
use clankmon::pricing::Pricing;
use clankmon::state::State;
use clankmon::ui::svg::{self, SvgOptions, hex};
use clankmon::ui::theme::Theme;
use clankmon::ui::{self, App, View};

const OUT: &str = "docs/assets";
const SEED: u64 = 7;

fn t0() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-21T19:30:00Z").unwrap().with_timezone(&Utc)
}

fn state(now: DateTime<Utc>) -> State {
    let mut st = State::new(std::sync::Arc::new(Pricing::default()));
    st.clock = Some(now);
    st.plan = Some("max".into());
    st.tier = Some("default_claude_max_20x".into());
    for e in demo::history(now, SEED) {
        st.apply(e);
    }
    st.ensure_sorted();
    st.fx.clear();
    st.toasts.clear();
    st
}

fn frame(st: &State, app: &mut App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::render(f, app, st)).unwrap();
    term.backend().buffer().clone()
}

fn app(theme: Theme, view: View) -> App {
    let mut a = App::new(theme, false);
    a.truecolor = true;
    a.view = view;
    a
}

fn opts(theme: &Theme, title: Option<&str>, id: &str) -> SvgOptions {
    SvgOptions { id: id.into(), ..svg_options(theme, title.map(str::to_string)) }
}

fn crop(b: &Buffer, rows: u16) -> Buffer {
    let area = Rect::new(0, 0, b.area.width, rows.min(b.area.height));
    let mut out = Buffer::empty(area);
    for y in 0..area.height {
        for x in 0..area.width {
            out[(x, y)] = b[(x, y)].clone();
        }
    }
    out
}

fn write(name: &str, svg: &str) {
    let path = format!("{OUT}/{name}");
    std::fs::write(&path, svg).unwrap();
    println!("{path:<32} {:>7.1} KB", svg.len() as f64 / 1024.0);
}

/// The wordmark: tui-big-text pixels as vector squares, with a chromatic
/// split, a neon glow, scanlines and a grid backdrop.
fn banner() -> String {
    let t = Theme::neon();
    let mut logo = Buffer::empty(Rect::new(0, 0, 64, 8));
    BigText::builder().pixel_size(PixelSize::Full).lines(vec!["CLANKMON".into()]).build().render(logo.area, &mut logo);
    let (w, h) = (1000.0, 250.0);
    let px = 11.0;
    let (lx, ly) = ((w - 64.0 * px) / 2.0, 46.0);
    let mut pixels = String::new();
    for y in 0..8u16 {
        let mut x = 0u16;
        while x < 64 {
            if logo[(x, y)].symbol() == " " {
                x += 1;
                continue;
            }
            let start = x;
            while x < 64 && logo[(x, y)].symbol() != " " {
                x += 1;
            }
            let _ = write!(
                pixels,
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/>"#,
                lx + start as f64 * px,
                ly + y as f64 * px,
                (x - start) as f64 * px - 1.5,
                px - 1.5
            );
        }
    }
    let (m, c, y_, bg, grid, muted) = (
        hex(t.primary, t.bg),
        hex(t.secondary, t.bg),
        hex(t.tertiary, t.bg),
        hex(t.bg, t.bg),
        hex(t.grid, t.bg),
        hex(t.muted, t.bg),
    );
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="'JetBrains Mono','SF Mono',Menlo,Consolas,monospace">
<defs>
<linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="{m}"/><stop offset=".55" stop-color="#9b5cff"/><stop offset="1" stop-color="{c}"/></linearGradient>
<linearGradient id="edge" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{m}"/><stop offset="1" stop-color="{c}"/></linearGradient>
<radialGradient id="halo" cx=".5" cy=".42" r=".55"><stop offset="0" stop-color="#9b5cff" stop-opacity=".28"/><stop offset="1" stop-color="{bg}" stop-opacity="0"/></radialGradient>
<pattern id="grid" width="24" height="24" patternUnits="userSpaceOnUse"><path d="M24 0H0V24" fill="none" stroke="{grid}" stroke-width="1"/></pattern>
<pattern id="scan" width="4" height="3" patternUnits="userSpaceOnUse"><rect width="4" height="1" fill="#000" fill-opacity=".35"/></pattern>
<filter id="glow" x="-10%" y="-30%" width="120%" height="160%"><feGaussianBlur stdDeviation="5" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
</defs>
<rect x="1" y="1" width="{w1}" height="{h1}" rx="14" fill="{bg}" stroke="url(#edge)" stroke-width="1.5"/>
<rect x="1" y="1" width="{w1}" height="{h1}" rx="14" fill="url(#grid)" opacity=".55"/>
<rect x="1" y="1" width="{w1}" height="{h1}" rx="14" fill="url(#halo)"/>
<g fill="{c}" opacity=".55" transform="translate(3.5 0)">{pixels}</g>
<g fill="{m}" opacity=".55" transform="translate(-3.5 0)">{pixels}</g>
<g fill="url(#g)" filter="url(#glow)">{pixels}</g>
<rect x="{lx}" y="{ly}" width="{lw}" height="{lh}" fill="url(#scan)"/>
<text x="{cx}" y="172" fill="{c}" text-anchor="middle" font-size="19" font-weight="700" letter-spacing="7">HTOP + BMON FOR CLAUDE CODE</text>
<text x="{cx}" y="203" fill="{muted}" text-anchor="middle" font-size="14" letter-spacing="1.5">sessions <tspan fill="{m}">▸</tspan> tokens <tspan fill="{m}">▸</tspan> rate limits <tspan fill="{m}">▸</tspan> cost <tspan fill="{m}">▸</tspan> live proxy streams</text>
<path d="M22 22h34M22 22v34M{rx} 22h-34M{rx} 22v34M22 {by}h34M22 {by}v-34M{rx} {by}h-34M{rx} {by}v-34" stroke="{m}" stroke-width="2" fill="none"/>
<text x="36" y="{ty}" fill="{y_}" font-size="11" letter-spacing="2">◢ SYS.ONLINE</text>
<text x="{rx2}" y="{ty}" fill="{muted}" font-size="11" text-anchor="end" letter-spacing="2">RUST · RATATUI · v{ver}</text>
</svg>
"##,
        w1 = w - 2.0,
        h1 = h - 2.0,
        lw = 64.0 * px,
        lh = 8.0 * px,
        cx = w / 2.0,
        rx = w - 22.0,
        by = h - 22.0,
        ty = h - 32.0,
        rx2 = w - 36.0,
        ver = env!("CARGO_PKG_VERSION"),
    )
}

/// Nest several SVG documents in one, each at its own offset.
fn compose(w: f64, h: f64, parts: &[(f64, f64, String)], bg: Color) -> String {
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}"><rect width="{w:.0}" height="{h:.0}" fill="none"/>"#
    );
    let _ = bg;
    for (x, y, svg) in parts {
        out.push_str(&svg.replacen("<svg ", &format!(r#"<svg x="{x:.0}" y="{y:.0}" "#), 1));
    }
    out.push_str("</svg>\n");
    out
}

fn main() {
    // Times in the UI are local; pin them so the images are reproducible.
    // SAFETY: single-threaded at this point.
    unsafe { std::env::set_var("TZ", "UTC") };
    std::fs::create_dir_all(OUT).unwrap();
    let neon = Theme::neon();

    write("banner.svg", &banner());

    // Static overview.
    let st = state(t0());
    let mut a = app(Theme::neon(), View::Overview);
    let buf = frame(&st, &mut a, 150, 46);
    write("overview.svg", &svg::render(&buf, &opts(&neon, Some("clankmon  ·  ~/src/night-city"), "ov")));

    // Animated overview: 12 s of demo traffic streaming through the proxy,
    // one frame every 600 ms, with 1 s graph columns so the motion is visible.
    let mut st = state(t0());
    let mut sim = LiveSim::new(SEED, t0());
    let mut a = app(Theme::neon(), View::Overview);
    a.bucket_secs = 1;
    let mut frames = Vec::new();
    for k in 0..20i64 {
        if k > 0 {
            for sub in [-300i64, 0] {
                let now = t0() + Duration::milliseconds(k * 600 + sub);
                st.clock = Some(now);
                st.apply(AppEvent::Batch(sim.step(now)));
            }
        }
        st.ensure_sorted();
        st.fx.clear();
        st.toasts.clear();
        a.frames = k as u64 * 2;
        frames.push(frame(&st, &mut a, 124, 36));
    }
    write("demo.svg", &svg::render_animated(&frames, 600, &opts(&neon, Some("clankmon --demo  ·  live"), "dm")));

    // The other views.
    let st = state(t0());
    for (name, view, h) in [
        ("sessions.svg", View::Sessions, 40),
        ("requests.svg", View::Requests, 30),
        ("history.svg", View::History, 44),
        ("limits.svg", View::Limits, 40),
    ] {
        let mut a = app(Theme::neon(), view);
        let buf = frame(&st, &mut a, 132, h);
        let title = format!("clankmon  ·  {}", view.label().to_lowercase());
        write(name, &svg::render(&buf, &opts(&neon, Some(&title), &name[..3])));
    }

    // Theme gallery: the top of the overview in each theme, two by two.
    let mut parts = Vec::new();
    let (cols, rows) = (112u16, 19u16);
    for (i, name) in ui::theme::NAMES.iter().enumerate() {
        let t = Theme::by_name(name);
        let mut a = app(t.clone(), View::Overview);
        let buf = crop(&frame(&st, &mut a, cols, 40), rows);
        let mut o = opts(&t, Some(&format!("theme  ·  {name}")), &format!("t{i}"));
        o.cell_w = 7.0;
        o.cell_h = 14.0;
        o.font_size = 11.4;
        let one = svg::render(&buf, &o);
        let (w, h) = (cols as f64 * 7.0 + 28.0, rows as f64 * 14.0 + 30.0 + 28.0);
        parts.push(((i % 2) as f64 * (w + 16.0), (i / 2) as f64 * (h + 16.0), one, w, h));
    }
    let (w, h) = (parts[0].3 * 2.0 + 16.0, parts[0].4 * 2.0 + 16.0);
    let parts: Vec<_> = parts.into_iter().map(|(x, y, s, _, _)| (x, y, s)).collect();
    write("themes.svg", &compose(w, h, &parts, neon.bg));
}
