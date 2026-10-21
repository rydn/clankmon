---
name: onboarding/structure
kind: onboarding
tags: []
related: [file:src/state.rs, file:src/collect/jsonl.rs, file:src/ui/views/overview.rs, file:src/ui/mod.rs, file:src/proxy/mod.rs, file:src/cli/tui.rs, file:src/main.rs]
created: 2026-10-21T20:32:49.087Z
updated: 2026-10-21T20:32:49.087Z
---

Layout (one crate, lib + bin; about 11k lines):
- `src/main.rs`: clap dispatch. `statusline` and `hook` are handled before any runtime/logging starts (hot path).
- `src/cli/`: `mod.rs` (clap definitions), `tui.rs` (boots every collector plus the UI; also `snapshot`), `push.rs` (statusline and hook clients, neon status line), `install.rs` (settings.json merge/unmerge), `doctor.rs`, `export.rs`.
- `src/collect/`: `jsonl.rs` (transcript schema with borrowed Cow fields, `parse_line`, `ScanAcc` per-file coalescing, `Tailer`), `procs.rs`, `statscache.rs`, `push.rs` (statusline/hook JSON to events), `otel.rs` (OTLP JSON receiver), `oauth.rs` (Keychain via the `security` CLI, usage poller).
- `src/proxy/`: `mod.rs` (hyper streaming passthrough + tee), `sse.rs` (incremental SSE parser), `headers.rs` (unified and API-key rate-limit headers).
- `src/state.rs`: reducer, sessions, limits, toasts and fx cues. `src/model.rs`: shared types and `AppEvent`. `src/windows.rs`: blocks, burn, buckets, projection. `src/pricing.rs` + `assets/pricing.toml`. `src/ipc.rs`, `src/store.rs`, `src/net.rs`, `src/mem.rs`, `src/config.rs`, `src/demo.rs`.
- `src/ui/`: `mod.rs` (App, event loop, header/footer/toasts), `views/*.rs`, `widgets.rs` (neon gauge, braille area, stacked bars), `theme.rs`, `fx.rs`, `fmt.rs`.
- `tests/`: `render.rs` + `snapshots/` (insta), `proxy.rs` (mock upstream), `otel.rs`.

Hotspots by size and churn: state.rs, collect/jsonl.rs, ui/views/overview.rs, ui/mod.rs, proxy/mod.rs.
