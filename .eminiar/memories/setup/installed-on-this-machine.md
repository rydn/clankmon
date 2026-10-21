---
name: setup/installed-on-this-machine
kind: fact
tags: [setup, environment]
related: []
created: 2026-10-21T20:32:16.614Z
updated: 2026-10-21T20:32:16.614Z
---

On Ryan's machine (2026-10-21) clankmon is installed at ~/.cargo/bin/clankmon and `clankmon install` was applied to ~/.claude/settings.json (backup settings.json.clankmon-bak-20261021-222405). The pre-existing status line was eminiar's (`"/Users/ryandick/.eminiar/plugin/node_modules/eminiar/bin/eminiar" statusline`); clankmon wraps it with `clankmon statusline --wrap '…'` so eminiar's output still shows, and refreshInterval is 60. Nine async exec-form hooks (SessionStart, SessionEnd, UserPromptSubmit, PostToolUse, PostToolUseFailure, Notification, PreCompact, PostCompact, Stop) were appended; the existing iTerm2 cc-status Notification hook was kept. If eminiar reinstalls its status line it will overwrite the wrap; `clankmon doctor` shows this. `clankmon uninstall` restores the eminiar command. Rust toolchain: Homebrew keg-only rustup, binaries in /opt/homebrew/opt/rustup/bin (not on PATH by default).
