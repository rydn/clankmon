---
name: architecture/single-binary-collectors-in-tui
kind: decision
tags: [architecture, stack]
related: []
created: 2026-10-21T19:24:59.559Z
updated: 2026-10-21T19:24:59.559Z
---

clankmon is a single Rust binary. The TUI process hosts every collector (JSONL tailer, process registry poller, Unix-socket IPC server for statusline/hook frames, optional proxy, optional OTLP http/json receiver, optional OAuth poller) and a SQLite store at ~/.clankmon/clankmon.db. `clankmon statusline` and `clankmon hook` are short-lived clients that write one JSON frame to ~/.clankmon/ctl.sock and fall back to a spool directory when the TUI is not running; they must exit in under 50 ms and never block Claude Code.

Why: Claude Code already pushes rate limits, context, cost and cache stats through the statusline command, so a proxy is not required for the core feature set. Keeping collectors in-process avoids a daemon lifecycle. The proxy is opt-in (`clankmon proxy`, ANTHROPIC_BASE_URL) because it requires exporting an env var in every shell; it adds exact server-side utilization headers, TTFT and request shape. The OAuth usage endpoint is opt-in and polled at >= 10 min because it 429s aggressively.

Rejected: proxy-first design (too much setup for the common case); separate daemon + thin TUI (lifecycle complexity for v1); OTLP protobuf (http/json avoids the prost dependency).

Stack: ratatui 0.30, crossterm 0.29, tokio 1.54, tachyonfx 0.25, tui-big-text 0.8, hyper 1.12 + hyper-util + rustls for the proxy, axum 0.8 only for the OTel receiver, reqwest 0.13 for the OAuth poller, notify 8, sysinfo 0.39, rusqlite 0.40 (bundled), clap 4.6, insta for snapshot tests.
