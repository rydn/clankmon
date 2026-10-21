---
name: testing/live-end-to-end-recipe
kind: fact
tags: [testing, e2e, proxy, otel]
related: [file:tests/proxy.rs, file:tests/otel.rs, file:src/cli/tui.rs]
created: 2026-10-21T20:32:16.151Z
updated: 2026-10-21T20:32:16.151Z
---

Live end-to-end check for clankmon (verified 2026-10-21): spawn the TUI in a Python pty (pty.fork, TIOCSWINSZ 45x160) with CLANKMON_HOME set to a temp dir and `--proxy 18484 --otel 14318`; run `claude -p 'Reply with exactly the word: ok' --model haiku` from the scratchpad with CLAUDECODE unset and ANTHROPIC_BASE_URL=http://127.0.0.1:18484 plus CLAUDE_CODE_ENABLE_TELEMETRY=1, OTEL_LOGS_EXPORTER=otlp, OTEL_EXPORTER_OTLP_PROTOCOL=http/json, OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:14318, OTEL_LOGS_EXPORT_INTERVAL=1000; wait ~5 s, send 'q', then read the temp clankmon.db. Expected: one request row with sources=7 (JSONL|PROXY|OTEL merged by request id), ttft/duration set, and a proxy limit snapshot with real anthropic-ratelimit-unified-5h/7d values. Claude Code with Pro OAuth works through ANTHROPIC_BASE_URL unchanged. Headless UI inspection: `clankmon snapshot --width 200 --height 50` (or `--demo snapshot`).
