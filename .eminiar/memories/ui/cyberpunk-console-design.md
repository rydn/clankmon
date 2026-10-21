---
name: ui/cyberpunk-console-design
kind: preference
tags: [ui, theme, preference]
related: []
created: 2026-10-21T19:25:00.029Z
updated: 2026-10-21T19:25:00.029Z
---

User preferences for clankmon's look and runtime environment (confirmed 2026-10-21): htop/bmon-style layout with a cyberpunk console aesthetic. Terminal is true-color (iTerm2) WITHOUT a Nerd Font, so use only pure Unicode glyphs: heavy/double box drawing, braille sparklines, block gauges (▰▱), status dots (●◐○), arrows (▲▼). Default theme `neon`: bg #07080d, grid #1a1d2b, magenta #ff2bd6, cyan #00e5ff, acid yellow #f4ff5a, danger #ff3b3b, muted #7c819a; alternates synthwave, matrix, mono; auto-degrade to 256 colors and honour NO_COLOR. tachyonfx effects (coalesce on start, sweep_in on view change, hsl_shift pulse at 80/90 % quota, glitch on 429, dissolve on quit) run at 30 fps only while active, otherwise 4 Hz or event-driven; `--no-fx` disables them. Views: 1 Overview (QUOTA gauges, THROUGHPUT stacked braille chart, PROCESSES table, WIRE log, MODELS/TOOLS panel), 2 Sessions, 3 Requests, 4 History, 5 Limits, ? Help. Footer uses an htop-style key legend.
