---
name: onboarding/conventions
kind: onboarding
tags: []
related: []
created: 2026-10-21T20:33:02.155Z
updated: 2026-10-21T20:33:02.155Z
---

- Commits: one per logical phase that compiles and passes tests, authored as the repo-local git identity (Ryan Dick). No AI attribution or Co-Authored-By lines.
- Data flow: collectors only emit `AppEvent`s. All merging and counting happens in `State::apply`. Merge by request key (`requestId`), use `Usage::merge_max`, and recompute cost after a merge. Never add a second counter elsewhere, or you'll double count across transcript, proxy and OTel.
- Rendering takes `&State`. Call `st.ensure_sorted()` before `timeline()`, `since()` or `recent()`. Time comes from `st.now()` (tests pin `st.clock`) so snapshots are deterministic, and every sort needs a stable tie-breaker (id/name).
- Hot paths: `statusline`/`hook` must stay runtime-free (~4 ms), silent on failure, and always exit 0. Installed hooks use `async: true` and exec form (`command` + `args`).
- Memory: coalesce during the initial scan (`ScanAcc`), and never stash large-capacity buffers in long-lived structs. Idle budget: about 0.4% CPU and under 60 MB RSS.
- Privacy: never persist prompt or completion text unless `--capture-bodies` is set. The OAuth token is read per call and never stored or refreshed.
- UI: pure Unicode glyphs (no Nerd Font), theme colours from `Theme`, layout tiers by width (narrow < 100 ≤ medium < 150 ≤ wide). Test 80×24 and tiny sizes; `tiny_terminal_does_not_panic` guards this.
- Tests: unit tests live next to the code (fixtures are real-shaped transcript lines). Integration tests go in tests/. Use `tempfile` and CLANKMON_HOME for isolation.
