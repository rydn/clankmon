# clankmon

A Rust TUI that monitors Claude Code: sessions, token throughput, rate limits and cost.

## Commands

- Build: `cargo build` (release: `cargo build --release`; install: `cargo install --path . --locked`)
- Test: `cargo test`. This includes insta render snapshots in `tests/snapshots/`. After an intentional UI change, run `INSTA_UPDATE=always cargo test --test render`, then read the changed `.snap` files before committing.
- Lint: `cargo clippy --all-targets -- -D warnings` (must be clean)
- See the UI without a TTY: `cargo run -- snapshot --width 160 --height 45` (real data) or `cargo run -- --demo snapshot`

## Layout

- `src/collect/`: one module per data source (jsonl transcripts, procs, statscache, push = statusline/hook payloads, otel, oauth). Each emits `model::AppEvent`.
- `src/proxy/`: the streaming ANTHROPIC_BASE_URL proxy (`sse.rs` parses the stream; `headers.rs` parses rate-limit headers).
- `src/state.rs`: the reducer. Every event merges here, keyed by request ID, without double counting. Keep it free of UI code.
- `src/windows.rs`: 5h blocks, burn rates, the throughput `Series` (clock-aligned per-second/per-minute rings), quota projection, axis ceilings.
- `src/ui/`: ratatui views (`views/`), custom widgets, themes, tachyonfx effects.
- `src/cli/`: subcommands. `statusline` and `hook` are hot paths that Claude Code runs on every update, so they must stay runtime-free and fast, and must never print errors.

## Conventions

- Rendering reads `&State`; call `st.ensure_sorted()` before using `timeline`/`since`/`recent`.
- Every sort needs a stable tie-breaker, or snapshots and tables jitter.
- During the initial scan, coalesce per file (`ScanAcc` in `jsonl.rs`). Don't keep buffers with large capacity in long-lived structs (a past bug cost 85 MB).
- Throughput graphs read `State::series`, never the request list: live tokens are dated when observed, so a passed column never changes. Keep columns wall-clock aligned and UI ticks aligned to their period.
- Never log or persist prompt/completion text unless the user opted into `--capture-bodies`.
- Commit at each logical phase; no AI attribution lines in commits.
