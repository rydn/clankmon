//! Boot the TUI: load config, start every enabled collector, run the UI.

use std::sync::Arc;

use chrono::Utc;
use color_eyre::eyre::Result;
use tokio::sync::mpsc::unbounded_channel;

use crate::cli::Cli;
use crate::collect::{jsonl::Tailer, oauth, otel, procs, statscache};
use crate::config::{Config, Paths};
use crate::model::{AppEvent, Collector, CollectorHealth};
use crate::pricing::Pricing;
use crate::state::State;
use crate::ui::theme::{self, Theme};
use crate::ui::{self, App};
use crate::{demo, ipc, proxy, store};

pub async fn run(cli: Cli, paths: Paths, mut cfg: Config) -> Result<()> {
    // CLI flags override the config file.
    if let Some(t) = &cli.theme {
        cfg.theme = t.clone();
    }
    if let Some(d) = cli.days {
        cfg.history_days = d;
    }
    cfg.no_fx |= cli.no_fx;
    if let Some(p) = cli.proxy {
        cfg.proxy.enabled = true;
        if p != 0 {
            cfg.proxy.port = p;
        }
    }
    if let Some(p) = cli.otel {
        cfg.otel.enabled = true;
        if p != 0 {
            cfg.otel.port = p;
        }
    }
    cfg.oauth.enabled |= cli.oauth;

    let pricing = Arc::new(Pricing::with_overrides(&cfg.pricing_overrides));
    let mut st = State::new(pricing.clone());
    let (tx, rx) = unbounded_channel::<AppEvent>();

    let no_color = theme::no_color();
    let theme = if no_color { Theme::mono() } else { Theme::by_name(&cfg.theme) };
    let app = App::new(theme, !cfg.no_fx && !no_color);

    if cli.demo {
        st.plan = Some("max".into());
        st.tier = Some("default_claude_max_20x".into());
        for e in demo::history(Utc::now(), 7) {
            st.apply(e);
        }
        tokio::spawn(demo::live(tx.clone(), 7));
        let _keep = tx;
        return ui::run(app, st, rx, None, cfg.idle_fps).await;
    }

    // Plan badge from Claude Code's credentials (no network).
    let cred_file = paths.credentials_file();
    if let Ok(Some(c)) = tokio::task::spawn_blocking({
        let f = cred_file.clone();
        move || oauth::read_credentials(&f)
    })
    .await
    {
        st.plan = c.subscription;
        st.tier = c.tier;
    }

    // Persisted proxy/OTel/limit data.
    let store = match store::open(&paths.db()) {
        Ok(conn) => {
            match store::load_recent(&conn, i64::from(cfg.history_days.min(35)), Utc::now()) {
                Ok(events) => {
                    for e in events {
                        st.apply(e);
                    }
                }
                Err(e) => tracing::warn!("loading store: {e}"),
            }
            st.persist_requests.clear();
            st.persist_limits.clear();
            Some(store::Store::spawn(conn))
        }
        Err(e) => {
            tracing::warn!("store unavailable: {e}");
            None
        }
    };

    // Frames that arrived while the TUI was not running.
    for e in ipc::drain_spool(&paths.spool()) {
        st.apply(e);
    }
    st.persist_limits.clear();

    if let Some(sc) = statscache::load(&paths.stats_cache()) {
        st.apply(AppEvent::StatsCache(sc));
    }

    // Transcripts: initial scan then tail, on a dedicated thread.
    let tailer = Tailer::new(paths.projects(), cfg.history_days, pricing.clone(), tx.clone());
    std::thread::Builder::new().name("clankmon-jsonl".into()).spawn(move || tailer.run())?;

    tokio::spawn(procs::run(paths.sessions(), tx.clone()));

    match ipc::bind(&paths.socket()).await {
        Ok(listener) => {
            tokio::spawn(ipc::serve(listener, tx.clone()));
        }
        Err(e) => {
            st.apply(AppEvent::Collector(Collector::Statusline, CollectorHealth::Error(e.to_string())));
        }
    }

    if cfg.proxy.enabled {
        let capture = cfg.proxy.capture_bodies.then(|| paths.home.join("captures"));
        let px = proxy::Proxy::new(&cfg.proxy.upstream, proxy::Sink::Channel(tx.clone()), capture);
        let port = cfg.proxy.port;
        tokio::spawn(async move {
            let _ = proxy::serve(port, px).await;
        });
    }
    if cfg.otel.enabled {
        tokio::spawn(otel::run(cfg.otel.port, tx.clone()));
    }
    if cfg.oauth.enabled {
        tokio::spawn(oauth::run(cred_file, cfg.oauth.interval_secs, tx.clone()));
    }
    drop(tx);

    ui::run(app, st, rx, store, cfg.idle_fps).await
}

/// Output format for `snapshot`.
pub enum SnapFormat {
    Text,
    Ansi,
    /// SVG, with an optional window title.
    Svg(Option<String>),
}

/// Load everything the TUI would show, render a single frame, return it.
pub fn snapshot(
    cli: &Cli,
    paths: &Paths,
    cfg: &Config,
    view: &str,
    w: u16,
    h: u16,
    format: SnapFormat,
) -> Result<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let view = match view {
        "sessions" => ui::View::Sessions,
        "requests" => ui::View::Requests,
        "history" => ui::View::History,
        "limits" => ui::View::Limits,
        _ => ui::View::Overview,
    };
    let pricing = Arc::new(Pricing::with_overrides(&cfg.pricing_overrides));
    let theme = Theme::by_name(cli.theme.as_deref().unwrap_or(&cfg.theme));
    let mut st = if cli.demo {
        let mut st = State::new(pricing);
        st.plan = Some("max".into());
        for e in demo::history(Utc::now(), 7) {
            st.apply(e);
        }
        st
    } else {
        let mut st = crate::cli::export::load(paths, cli.days.unwrap_or(cfg.history_days), pricing.clone());
        if let Some(c) = oauth::read_credentials(&paths.credentials_file()) {
            st.plan = c.subscription;
            st.tier = c.tier;
        }
        if let Ok(conn) = store::open(&paths.db())
            && let Ok(events) = store::load_recent(&conn, 35, Utc::now())
        {
            for e in events {
                st.apply(e);
            }
        }
        for e in ipc::peek_spool(&paths.spool()) {
            st.apply(e);
        }
        if let Some(sc) = statscache::load(&paths.stats_cache()) {
            st.apply(AppEvent::StatsCache(sc));
        }
        let mut procs = procs::read_sessions(&paths.sessions());
        let stats = procs::ps_stats(&procs.iter().map(|p| p.pid).collect::<Vec<_>>());
        for p in &mut procs {
            if let Some((c, r)) = stats.get(&p.pid) {
                p.cpu_pct = Some(*c);
                p.rss_kb = Some(*r);
            }
        }
        st.apply(AppEvent::Procs(procs));
        st.apply(AppEvent::Collector(Collector::Jsonl, CollectorHealth::Ok));
        st
    };
    st.ensure_sorted();
    st.toasts.clear();
    st.fx.clear();
    let mut app = App::new(theme, false);
    app.truecolor = true;
    app.view = view;
    let mut term = Terminal::new(TestBackend::new(w, h))?;
    term.draw(|f| ui::render(f, &mut app, &st))?;
    let buf = term.backend().buffer();
    Ok(match format {
        SnapFormat::Text => buffer_to_text(buf),
        SnapFormat::Ansi => buffer_to_ansi(buf),
        SnapFormat::Svg(title) => crate::ui::svg::render(buf, &svg_options(&app.theme, title)),
    })
}

/// SVG options matching a theme.
pub fn svg_options(t: &Theme, title: Option<String>) -> crate::ui::svg::SvgOptions {
    crate::ui::svg::SvgOptions {
        chrome: title,
        fg: t.text,
        bg: if t.bg == ratatui::style::Color::Reset { ratatui::style::Color::Rgb(0, 0, 0) } else { t.bg },
        accent: t.primary,
        accent2: t.secondary,
        ..Default::default()
    }
}

fn buffer_to_text(buf: &ratatui::buffer::Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

fn buffer_to_ansi(buf: &ratatui::buffer::Buffer) -> String {
    use ratatui::style::{Color, Modifier};
    let sgr = |c: Color, fg: bool| -> String {
        let base = if fg { 38 } else { 48 };
        match c {
            Color::Rgb(r, g, b) => format!("\x1b[{base};2;{r};{g};{b}m"),
            Color::Indexed(i) => format!("\x1b[{base};5;{i}m"),
            Color::Reset => format!("\x1b[{}m", if fg { 39 } else { 49 }),
            other => {
                // Named colours: map through the 16-colour indexes.
                let idx = match other {
                    Color::Black => 0,
                    Color::Red => 1,
                    Color::Green => 2,
                    Color::Yellow => 3,
                    Color::Blue => 4,
                    Color::Magenta => 5,
                    Color::Cyan => 6,
                    Color::Gray => 7,
                    Color::DarkGray => 8,
                    Color::LightRed => 9,
                    Color::LightGreen => 10,
                    Color::LightYellow => 11,
                    Color::LightBlue => 12,
                    Color::LightMagenta => 13,
                    Color::LightCyan => 14,
                    _ => 15,
                };
                format!("\x1b[{base};5;{idx}m")
            }
        }
    };
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut last: Option<(Color, Color, Modifier)> = None;
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            let key = (c.fg, c.bg, c.modifier);
            if last != Some(key) {
                out.push_str("\x1b[0m");
                out.push_str(&sgr(c.fg, true));
                out.push_str(&sgr(c.bg, false));
                if c.modifier.contains(Modifier::BOLD) {
                    out.push_str("\x1b[1m");
                }
                last = Some(key);
            }
            out.push_str(c.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    out
}
