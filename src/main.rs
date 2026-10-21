use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::Result;

use clankmon::cli::{self, Cli, Cmd};
use clankmon::config::{Config, Paths};
use clankmon::pricing::Pricing;
use clankmon::{ipc, net, proxy};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let paths = Paths::resolve(cli.claude_dir.clone());

    // Hot paths Claude Code runs on every update: no runtime, no logging, never fail loudly.
    match &cli.cmd {
        Some(Cmd::Statusline { wrap, quiet }) => {
            return ExitCode::from(cli::push::statusline(&paths, wrap.clone(), *quiet) as u8);
        }
        Some(Cmd::Hook) => return ExitCode::from(cli::push::hook(&paths) as u8),
        _ => {}
    }

    let _ = color_eyre::install();
    match run(cli, paths) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("clankmon: {e:?}");
            ExitCode::FAILURE
        }
    }
}

fn init_logging(paths: &Paths) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    paths.ensure_home().ok()?;
    let appender = tracing_appender::rolling::never(paths.log_dir(), "clankmon.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = tracing_subscriber::EnvFilter::try_from_env("CLANKMON_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,clankmon=info"));
    tracing_subscriber::fmt().with_writer(writer).with_ansi(false).with_env_filter(filter).init();
    Some(guard)
}

fn run(cli: Cli, paths: Paths) -> Result<()> {
    let cfg = Config::load(&paths.config())?;
    match cli.cmd {
        Some(Cmd::Doctor) => {
            cli::doctor::run(&paths, &cfg);
            Ok(())
        }
        Some(Cmd::Config) => {
            print!("{}", Config::example());
            Ok(())
        }
        Some(Cmd::Install { statusline, hooks, otel, yes, dry_run, settings, bin }) => {
            // No component flags means statusline + hooks.
            let all = !statusline && !hooks;
            let bin = match bin {
                Some(b) => b,
                None => std::env::current_exe()?.canonicalize()?,
            };
            cli::install::install(&cli::install::Options {
                statusline: statusline || all,
                hooks: hooks || all,
                otel,
                yes,
                dry_run,
                settings: settings.unwrap_or_else(|| paths.claude_settings()),
                bin,
            })
        }
        Some(Cmd::Uninstall { yes, dry_run, settings }) => {
            cli::install::uninstall(&settings.unwrap_or_else(|| paths.claude_settings()), yes, dry_run)
        }
        Some(Cmd::Export { format, since, output }) => {
            let pricing = Arc::new(Pricing::with_overrides(&cfg.pricing_overrides));
            cli::export::run(&paths, pricing, format, &since, output.as_deref())
        }
        Some(Cmd::Proxy { port, upstream, print_env, capture_bodies, quiet }) => {
            let port = port.unwrap_or(cfg.proxy.port);
            if print_env {
                println!("export ANTHROPIC_BASE_URL=http://127.0.0.1:{port}");
                return Ok(());
            }
            let _guard = init_logging(&paths);
            let upstream = upstream.unwrap_or_else(|| cfg.proxy.upstream.clone());
            let capture = (capture_bodies || cfg.proxy.capture_bodies).then(|| paths.home.join("captures"));
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move {
                net::init_crypto();
                let sink = Arc::new(ipc::AsyncSink::new(paths.socket(), paths.spool()));
                let px = proxy::Proxy::new(&upstream, proxy::Sink::Ipc { sink, print: !quiet }, capture);
                println!(
                    "\x1b[38;2;255;43;214m◢ clankmon proxy ◣\x1b[0m 127.0.0.1:{port} → {upstream}\n  export ANTHROPIC_BASE_URL=http://127.0.0.1:{port}\n"
                );
                proxy::serve(port, px).await
            })?;
            Ok(())
        }
        Some(Cmd::Snapshot { ref view, width, height, ansi, svg, ref title }) => {
            let format = if svg {
                cli::tui::SnapFormat::Svg(title.clone())
            } else if ansi {
                cli::tui::SnapFormat::Ansi
            } else {
                cli::tui::SnapFormat::Text
            };
            print!("{}", cli::tui::snapshot(&cli, &paths, &cfg, view, width, height, format)?);
            Ok(())
        }
        Some(Cmd::Statusline { .. }) | Some(Cmd::Hook) => unreachable!("handled before the runtime starts"),
        None => {
            let _guard = init_logging(&paths);
            net::init_crypto();
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
            rt.block_on(cli::tui::run(cli, paths, cfg))
        }
    }
}
