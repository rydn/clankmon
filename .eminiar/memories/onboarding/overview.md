---
name: onboarding/overview
kind: onboarding
tags: []
related: []
created: 2026-10-21T20:32:41.813Z
updated: 2026-10-21T20:32:41.813Z
---

clankmon is a single-binary Rust TUI that monitors Claude Code like htop/bmon: live sessions (processes), token throughput, subscription rate limits (5h/7d), cost at list price, tool usage and 429 incidents, with a cyberpunk console look (ratatui + tachyonfx). It's for Claude Code users on Pro/Max subscriptions or API keys who want to watch usage live.

Runtime pieces, all in one process (`clankmon` with no subcommand):
- Collectors (src/collect): transcript tailer (`~/.claude/projects/**/*.jsonl`, parallel scan then notify tail), session registry poller (`~/.claude/sessions/<pid>.json` plus `ps`), a Unix-socket IPC server receiving frames from `clankmon statusline` / `clankmon hook` (Claude Code runs these), and optionally the API proxy (`--proxy`, for ANTHROPIC_BASE_URL), the OTLP http/json receiver (`--otel`) and the slow OAuth `/api/oauth/usage` poller (`--oauth`).
- The reducer (src/state.rs) merges every event into one record per API request, keyed by request ID: the SRC column shows J/P/O for transcript, proxy and OTel.
- SQLite store (~/.clankmon/clankmon.db) persists proxy/OTel requests, limit snapshots and incidents. The spool (~/.clankmon/spool.jsonl) buffers frames while the TUI is down.
- UI (src/ui): five views (overview, sessions, requests, history, limits), four themes, effects that run only while animating.

Other subcommands: `proxy` (standalone, forwards to the TUI over IPC), `install`/`uninstall` (edit ~/.claude/settings.json with a backup), `doctor`, `snapshot` (render one frame to stdout), `export`, `config`.
