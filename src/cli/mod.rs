//! Command-line interface.

pub mod doctor;
pub mod export;
pub mod install;
pub mod push;
pub mod tui;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "clankmon",
    version,
    about = "A cyberpunk htop/bmon-style monitor for Claude Code: live sessions, token throughput, rate limits and cost",
    after_help = "Run `clankmon install` once to feed live quota data from Claude Code, then `clankmon`."
)]
pub struct Cli {
    /// Claude Code config dir (default: $CLAUDE_CONFIG_DIR or ~/.claude)
    #[arg(long, global = true, env = "CLAUDE_CONFIG_DIR")]
    pub claude_dir: Option<PathBuf>,

    /// Colour theme: neon, synthwave, matrix, mono
    #[arg(long)]
    pub theme: Option<String>,

    /// Disable animations
    #[arg(long)]
    pub no_fx: bool,

    /// Days of transcripts to load
    #[arg(long)]
    pub days: Option<u32>,

    /// Host the API proxy inside the TUI (optionally on PORT)
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "0")]
    pub proxy: Option<u16>,

    /// Receive OpenTelemetry (OTLP http/json) from Claude Code (optionally on PORT)
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "0")]
    pub otel: Option<u16>,

    /// Poll the undocumented /api/oauth/usage endpoint (slowly)
    #[arg(long)]
    pub oauth: bool,

    /// Synthetic traffic, for screenshots and trying the UI
    #[arg(long)]
    pub demo: bool,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Claude Code statusLine command: forwards the frame, prints a neon status line
    Statusline {
        /// Run this command with the same JSON and show its output instead
        #[arg(long)]
        wrap: Option<String>,
        /// Print nothing
        #[arg(long)]
        quiet: bool,
    },
    /// Claude Code hook command: forwards the event, prints nothing
    Hook,
    /// Run the transparent API proxy (point ANTHROPIC_BASE_URL at it)
    Proxy {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        upstream: Option<String>,
        /// Print the export line for your shell and exit
        #[arg(long)]
        print_env: bool,
        /// Save request/response bodies under ~/.clankmon/captures (contains your code!)
        #[arg(long)]
        capture_bodies: bool,
        /// No per-request log lines
        #[arg(long)]
        quiet: bool,
    },
    /// Wire Claude Code's statusline and hooks (and optionally OTel) to clankmon
    Install {
        /// Only the status line
        #[arg(long)]
        statusline: bool,
        /// Only the hooks
        #[arg(long)]
        hooks: bool,
        /// Also export OpenTelemetry to clankmon
        #[arg(long)]
        otel: bool,
        /// Don't ask for confirmation
        #[arg(long, short)]
        yes: bool,
        /// Show the result without writing
        #[arg(long)]
        dry_run: bool,
        /// Settings file to edit (default: <claude dir>/settings.json)
        #[arg(long)]
        settings: Option<PathBuf>,
        /// Path to the clankmon binary to reference (default: this executable)
        #[arg(long)]
        bin: Option<PathBuf>,
    },
    /// Remove everything `install` added
    Uninstall {
        #[arg(long, short)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        settings: Option<PathBuf>,
    },
    /// Check which data sources are live
    Doctor,
    /// Export requests or daily totals
    Export {
        #[arg(long, value_enum, default_value = "csv")]
        format: export::Format,
        /// How far back: 30m, 12h, 7d, 2w
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Print an example config file
    Config,
}
