---
name: energy/token-energy-estimate-path
kind: decision
status: proposed
tags: [energy, estimate, reducer, design]
related: [file:src/energy.rs, file:src/state.rs, file:src/model.rs, file:src/windows.rs, file:src/pricing.rs, file:src/ui/fmt.rs]
created: 2026-11-11T18:32:53.307Z
updated: 2026-11-11T18:32:53.307Z
---

Token energy estimate (Wh) design (swarm run 20261111-bddd47, task #1). Rates live in a static const table in src/energy.rs: Wh per million tokens by token class (input/cache_write/cache_read/output), keyed by model family prefix (haiku/sonnet/opus/fable/mythos), longest prefix on pricing::normalize_model. Unknown models fall back to the opus row; empty or <synthetic> gives 0. The values are clankmon's order-of-magnitude assumption, anchored on Epoch AI 2025 (~0.3 Wh per GPT-4o query); Anthropic publishes no per-token energy. The UI must prefix values with ≈, say "est.", and explain energy::SOURCE in Help. Energy follows the cost path exactly: Request.energy_wh is recomputed in State::on_request from the merged usage (both branches, next to cost_usd) and added only in account()/unaccount() (DayAgg.energy_wh, Session.energy_wh). windows::Block.energy_wh and Burn.energy_wh_per_hour (mean watts) are derived from requests. Never add energy in on_stream, Bucket or Series, or a stream would be counted twice. thinking is a subset of output, so never add it.
