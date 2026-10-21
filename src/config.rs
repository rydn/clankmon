//! Paths and the user config at `~/.clankmon/config.toml`.

use std::path::{Path, PathBuf};

use color_eyre::eyre::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::pricing::ModelPrice;

/// Filesystem locations clankmon reads and writes.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Claude Code's config dir (`CLAUDE_CONFIG_DIR` or `~/.claude`).
    pub claude_dir: PathBuf,
    /// clankmon's own dir (`CLANKMON_HOME` or `~/.clankmon`).
    pub home: PathBuf,
}

impl Paths {
    pub fn resolve(claude_dir: Option<PathBuf>) -> Self {
        let user_home =
            directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
        let claude_dir = claude_dir
            .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
            .unwrap_or_else(|| user_home.join(".claude"));
        let home = std::env::var_os("CLANKMON_HOME").map(PathBuf::from).unwrap_or_else(|| user_home.join(".clankmon"));
        Paths { claude_dir, home }
    }

    pub fn projects(&self) -> PathBuf {
        self.claude_dir.join("projects")
    }
    pub fn sessions(&self) -> PathBuf {
        self.claude_dir.join("sessions")
    }
    pub fn stats_cache(&self) -> PathBuf {
        self.claude_dir.join("stats-cache.json")
    }
    pub fn claude_settings(&self) -> PathBuf {
        self.claude_dir.join("settings.json")
    }
    pub fn credentials_file(&self) -> PathBuf {
        self.claude_dir.join(".credentials.json")
    }
    pub fn socket(&self) -> PathBuf {
        self.home.join("ctl.sock")
    }
    pub fn spool(&self) -> PathBuf {
        self.home.join("spool.jsonl")
    }
    pub fn db(&self) -> PathBuf {
        self.home.join("clankmon.db")
    }
    pub fn config(&self) -> PathBuf {
        self.home.join("config.toml")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.home.clone()
    }
    /// Last statusline frame, for `clankmon doctor` and the fallback status line.
    pub fn last_statusline(&self) -> PathBuf {
        self.home.join("last-statusline.json")
    }

    pub fn ensure_home(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home).wrap_err_with(|| format!("creating {}", self.home.display()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    /// Disable tachyonfx animations.
    pub no_fx: bool,
    /// Days of transcripts to load on startup.
    pub history_days: u32,
    /// UI redraw rate when nothing animates, in Hz.
    pub idle_fps: u32,
    pub proxy: ProxyConfig,
    pub otel: OtelConfig,
    pub oauth: OauthConfig,
    /// Daily budget in USD for the cost gauge; 0 hides it.
    pub daily_budget_usd: f64,
    #[serde(rename = "pricing")]
    pub pricing_overrides: Vec<ModelPrice>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "neon".into(),
            no_fx: false,
            history_days: 30,
            idle_fps: 4,
            proxy: ProxyConfig::default(),
            otel: OtelConfig::default(),
            oauth: OauthConfig::default(),
            daily_budget_usd: 0.0,
            pricing_overrides: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    /// Host the proxy inside the TUI process.
    pub enabled: bool,
    pub port: u16,
    pub upstream: String,
    /// Persist request and response bodies (off by default: they contain your code).
    pub capture_bodies: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        ProxyConfig { enabled: false, port: 8484, upstream: "https://api.anthropic.com".into(), capture_bodies: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OtelConfig {
    pub enabled: bool,
    pub port: u16,
}

impl Default for OtelConfig {
    fn default() -> Self {
        OtelConfig { enabled: false, port: 4318 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OauthConfig {
    pub enabled: bool,
    /// Minimum seconds between polls. The endpoint 429s aggressively.
    pub interval_secs: u64,
}

impl Default for OauthConfig {
    fn default() -> Self {
        OauthConfig { enabled: false, interval_secs: 600 }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).wrap_err_with(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).wrap_err_with(|| format!("reading {}", path.display())),
        }
    }

    pub fn example() -> String {
        let mut s = String::from("# clankmon configuration\n");
        s.push_str(&toml::to_string_pretty(&Config::default()).unwrap_or_default());
        s.push_str(
            "\n# Override or add a model price (USD per million tokens):\n\
             # [[pricing]]\n# match = \"claude-opus-5-5\"\n# label = \"Opus 5.5\"\n\
             # input = 4.0\n# output = 20.0\n# cache_read = 0.20\n# context = 1000000\n",
        );
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_round_trips() {
        let cfg: Config = toml::from_str(&Config::example()).unwrap();
        assert_eq!(cfg.proxy.port, 8484);
        assert_eq!(cfg.oauth.interval_secs, 600);
    }

    #[test]
    fn partial_config_uses_defaults() {
        let cfg: Config = toml::from_str("theme = \"matrix\"\n[proxy]\nenabled = true\n").unwrap();
        assert_eq!(cfg.theme, "matrix");
        assert!(cfg.proxy.enabled);
        assert_eq!(cfg.proxy.port, 8484);
        assert_eq!(cfg.history_days, 30);
    }

    #[test]
    fn pricing_override_parses() {
        let cfg: Config =
            toml::from_str("[[pricing]]\nmatch = \"claude-x\"\ninput = 1.0\noutput = 2.0\ncache_read = 0.1\n").unwrap();
        assert_eq!(cfg.pricing_overrides[0].matcher, "claude-x");
    }
}
