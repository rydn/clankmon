---
name: perf/scan-memory-buffer-capacity
kind: fact
tags: [performance, memory, gotcha]
related: [file:src/collect/jsonl.rs, file:src/mem.rs]
created: 2026-10-21T20:32:15.697Z
updated: 2026-10-21T20:32:15.697Z
---

Gotcha fixed in clankmon's transcript scanner (src/collect/jsonl.rs, read_new_lines): storing the reusable line buffer into each FileCursor.partial kept its full capacity (the file's longest line, up to 4.5 MB), which held ~85 MB across ~400 transcripts and made the 30-day scan peak at 112 MB live. Cursors must keep only the unfinished-line bytes (`buf[..].to_vec()` or an empty Vec). Diagnosed with a counting GlobalAlloc (peak live bytes) after vmmap/heap showed 74% malloc fragmentation; malloc_zone_pressure_relief and mimalloc did NOT help, and rebuilding state made it worse. Measured after the fix: 36 MB peak live, ~45 MB RSS for a 560 MB / 30-day scan in <1 s; TUI true idle ~0.4% CPU, 39 MB RSS.
