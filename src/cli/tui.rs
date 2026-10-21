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
