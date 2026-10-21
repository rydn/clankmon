---
name: onboarding/commands
kind: onboarding
tags: []
related: []
created: 2026-10-21T20:32:55.029Z
updated: 2026-10-21T20:32:55.029Z
---

Toolchain: stable Rust 1.90+ (developed on 1.99). On Ryan's Mac, rustup is Homebrew keg-only: prefix commands with `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`.

- Build: `cargo build` / `cargo build --release` (about 9 MB binary)
- Install locally: `cargo install --path . --locked`, which puts the binary at ~/.cargo/bin/clankmon. The installed status line and hooks reference this path, so reinstall after changes.
- Test: `cargo test --locked` (85 tests: unit, insta render snapshots, proxy and OTLP integration). Update snapshots after an intentional UI change with `INSTA_UPDATE=always cargo test --test render`, then read the `.snap` diffs.
- Lint/format: `cargo clippy --all-targets --locked -- -D warnings` and `cargo fmt --check` (rustfmt.toml: max_width 120). CI (.github/workflows/ci.yml) runs fmt, clippy and test on Ubuntu and macOS.
- Run: `clankmon` (real data), `clankmon --demo` (synthetic), `clankmon --proxy --otel --oauth` for extra feeds, `clankmon proxy` standalone plus `export ANTHROPIC_BASE_URL=http://127.0.0.1:8484`.
- Headless inspection: `clankmon snapshot --width 200 --height 50 [--view limits] [--ansi]`. Run `clankmon doctor` for source health.
- Env vars: CLAUDE_CONFIG_DIR (Claude dir), CLANKMON_HOME (state dir; use a temp dir in tests so the real spool/db stay clean), CLANKMON_LOG (tracing filter; log at ~/.clankmon/clankmon.log), CLANKMON_SCAN_THREADS, NO_COLOR, COLORTERM.
- No release pipeline yet.
