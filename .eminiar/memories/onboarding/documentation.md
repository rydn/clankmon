---
name: onboarding/documentation
kind: onboarding
tags: []
related: []
created: 2026-10-21T20:33:08.544Z
updated: 2026-10-21T20:33:08.544Z
---

Registered documentation sources (manifest .eminiar/docs.json):
- ratatui: concepts, recipes, widget/layout/style API
- tachyonfx: effects API (fx::*, EffectManager, patterns; the Glitch builder uses bon)
- tui-big-text: BigText builder, PixelSize
- crossterm: events and EventStream
- hyper: hyper 1.x guides plus hyper-util, hyper-rustls, http-body-util
- rusqlite, notify
- claude-code-monitoring: Claude Code statusline JSON fields (rate_limits, context_window, prompt_cache), hooks reference (events, async, exec form, matchers), monitoring/OTel (api_request event attributes, OTEL_* env), costs, settings, API rate-limit headers, streaming SSE events
- anthropic, github (Actions): from discovery; readme, docs, deploy-config: local files

Integrated services and APIs:
- api.anthropic.com `/v1/messages`, through the proxy: documented `anthropic-ratelimit-*` headers, plus the undocumented `anthropic-ratelimit-unified-{5h,7d}-{utilization,reset,status}` headers on OAuth traffic
- `/api/oauth/usage`: undocumented, 429s aggressively (anthropics/claude-code#31637)
- Claude Code local files, recorded in memory `sources/claude-code-data-sources`

Local docs: README.md, CLAUDE.md, docs/overview.txt (demo frame), docs/demo.tape (VHS). There are no API specs.

Rule: run `em_docs {action: "lookup", query}` before WebFetch/WebSearch for anything the project depends on. Read crate sources under ~/.cargo/registry/src when exact signatures matter (tachyonfx 0.25 and ratatui 0.30 differ from older docs).
