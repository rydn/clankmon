# clankmon

**htop and bmon for Claude Code.** A cyberpunk terminal monitor for live sessions, token throughput, subscription rate limits and cost. It's a single Rust binary that reads what Claude Code already writes, plus optional deeper feeds.

```text
 ▗▛▜▖▜▛  ▗█▖ █▖▐▌▜▌▐▌█▖▟▌▗▛▙ █▖▐▌   PLAN  MAX ▸ theme neon                                                                 20:24:39
 █   ▐▌  █ █ █▜▟▌▐▙▛ ███▌█ ▐▌█▜▟▌   LIVE  4 sessions ▸ 2 busy ▸ 1 waiting
 ▜▖▗▖▐▌▗▌█▀█ █ ▜▌▐▌▜▖█▝▐▌▜▖▟▘█ ▜▌   TODAY  $23.93 ▸ 25M tok ▸ 225 req ▸ $11.26/h
  ▀▀ ▀▀▀▘▀ ▀ ▀ ▝▘▀▘▝▘▀ ▝▘ ▀▘ ▀ ▝▘   FEED  ●jsonl ●procs ●status ●hooks ●proxy ○otel ○oauth
╭◢ QUOTA ◣───────────────────────────────── statusline 0s ago ╮╭◢ THROUGHPUT ◣───────────────────────────────────────────── col=1m ╮
│5H  ▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▮▰▰▰▰▰▱▱▱▱▱▱▱▱   81%  ⟳1h36m ││▲ PROMPT 205k/m pk 370k/m  ■new ■write ■read                       │
│                  ⢠⣤⣤⣤⣴⣶⣶ ALLOWED pace 68% ▸ 100% in 39m     ││▃ ▂  █▇▅    ▇ ▅▃▇ ▆  ▁ ▃               ▁▃▇  ▆▆     ▁ ▃ ▃  ▃▇  █ ▄  │
│                                                             ││█ █▃ ███▄▄ ▄█ ███▄█▇██ █ ▂ ▄▄▄ ▂▅▆▃ ▇  ███▄ ██ ▃▄▇▄█ █ █  ██▃▂█ █▅ │
│7D  ▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▮▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱   70%  ⟳2d12h ││█▇██▃████████▃████████▆█·█▃███▄████▄█▇▃████▃██▃█████▃█·█·▃█████▇██▃│
│                        ⢰ ALLOWED pace 64% ▸ 100% in 4h59m   ││▼ OUTPUT 2.2k/m pk 5.6k/m  ■text ■thinking                         │
│                                                             ││ ▂▁▄ ███   ▂█  ▁█ █▂ █▂  ▆ ▁▅  ▄ ▁▃  ▁▁▁ █  █▂ ▂ ▇ ▄▂█ ▃  ▅█▇ █ ▂  │
│BURN  $10.53/h ▸ 193k tok/m ▸ block ends near $50.16         ││▇███▂███▃▆▂██▂▄█████▇██▄·█▅██▂█████▁▅███▇█████▁█▆█▁███·█·▆███▆█▃█▁▁│
│BLOCK 18:00→23:00 ▸ $22.89 ▸ 24M tok ▸ 212 req ▸ ends 2h35m  ││req 2.0/m ▸ $13.40/h ▸ hit 94% ▸ ttft 1.0s ▸ p50 12.1s ▸ err 0     │
╰─────────────────────────────────────────────────────────────╯╰───────────────────────────────────────────────────────────────────╯
╭◢ PROCESSES ◣─────────────────────────────────────────────────────────────────────────────────── 4 live · 4 shown · sort activity ╮
│  PID    SESSION            MODEL      STATE           CONTEXT    TURN REQ     TOKENS COST    LAST  PROJECT                       │
│⏸ 41111  deck-firmware      Sonnet 5.5 permission 1m   ▱▱▱▱▱ 4.1% 13   187+30  10M    $9.57   10s   /home/neo/src/netrunner/deck ⎇│
│⚙ 41037  night-city         Opus 5.5   Bash 5s         ▰▱▱▱▱  18% 7    192+32  37M    $20.28  5s    /home/neo/src/night-city/api ⎇│
│◐ 41074  ice-breaker        Fable 5.1  thinking 45s    ▱▱▱▱▱ 9.6% 25   181+28  20M    $40.47  10s   /home/neo/src/arasaka/ice-brea│
│○ 41148  bar-tab            Haiku 4.5  idle            ▱▱▱▱▱ 6.0% 12   140+22  3.2M   $2.67   10s   /home/neo/src/afterlife/bar-ta│
╰──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
```

<sub>`clankmon --demo snapshot`, so every name and number above is synthetic. In the terminal it's neon on near-black, with tachyonfx effects.</sub>

## Features

- **Live processes, htop style.** Every running Claude Code session shows its PID, model, effort, state (`◐ thinking`, `⚙ Bash`, `⏸ permission`, `◇ compacting`), context fill, turns, requests (plus subagent requests), tokens, cache hit rate, cost, CPU, memory and project. Sessions waiting for permission sort to the top.
- **Quota gauges.** The 5-hour and 7-day subscription windows come with a pace marker, reset countdown, trend sparkline and a least-squares projection of when you'll hit 100%. Per-model weekly windows and API-key buckets appear when a source provides them.
- **bmon-style throughput.** Stacked graphs show prompt tokens (cache read, cache write, fresh) and output tokens (text, thinking) in tokens per minute, with a smooth "now" rate, request rate, cost burn, cache hit, TTFT and latency. Columns are aligned to the wall clock and never change once they've passed. The live column is tinted, and the axis ignores rare spikes, which clip with a `▲` cap. Zoom from 1-second to 15-minute columns; `g` switches to log scale.
- **The wire.** A live feed of every API turn with tokens, cost, stop reason, latency, tools called and which sources saw it.
- **Five views.** Overview, Sessions (per-session detail with a context-growth chart), Requests (filterable log with a detail popup), History (30-day cost and token bars, an hour×weekday heatmap, model mix) and Limits (utilisation chart, per-source readings, 429 incidents).
- **Cost at list price.** Every request is priced from an embedded, overridable table that's aware of the 5-minute and 1-hour cache-write TTLs. This is API-equivalent spend; subscription usage isn't billed per token.
- **Cyberpunk console.** Four themes (`neon`, `synthwave`, `matrix`, `mono`), pure Unicode glyphs (no Nerd Font needed), true colour with an automatic 256-colour fallback, `NO_COLOR` support, and effects that only run while they're visible.
- **Light.** One 9 MB binary. A 30-day backfill of 560 MB of transcripts scans in under a second and peaks under 50 MB RSS. The status line and hook clients run in about 4 ms.

## Install

```sh
cargo install --path .        # from a clone; Rust 1.90+
clankmon install              # wire Claude Code's status line and hooks (asks first, backs up settings.json)
clankmon                      # run the monitor
```

`clankmon doctor` shows which data sources are live and how to turn on the rest.

## Data sources

clankmon merges every source into one record per API request, keyed by Anthropic's request ID, so nothing is double counted. The `SRC` column shows which sources saw each request: `J` for transcript, `P` for proxy, `O` for OTel.

| Source | Setup | What it adds |
|---|---|---|
| **Transcripts** `~/.claude/projects/**/*.jsonl` | none | every API turn: tokens by kind, model, stop reason, tools, subagents, effort, 429s and quota rejections |
| **Session registry** `~/.claude/sessions/*.json` + `ps` | none | the live process list, CPU and memory |
| **Status line** | `clankmon install` | 5h/7d utilisation and resets (Pro/Max), context %, Claude Code's own cost figure, prompt-cache stats |
| **Hooks** (async, never block) | `clankmon install` | permission waits, compaction, session start/end, tool failures |
| **Proxy** | `clankmon proxy` + `ANTHROPIC_BASE_URL` | exact server-side limits on every response (`anthropic-ratelimit-unified-*`), API-key buckets, TTFT, request shape, 429 details |
| **OpenTelemetry** | `clankmon install --otel` + `clankmon --otel` | Claude Code's `api_request`, `tool_result` and `api_error` events |
| **OAuth usage** | `clankmon --oauth` | per-model weekly limits and extra usage from the endpoint behind `/usage`. It's undocumented and rate limited, so clankmon polls it every 10 minutes or less often and backs off. |

clankmon wraps an existing status line instead of replacing it, and its output is shown unchanged. Without one, clankmon prints its own neon status line with model, context, quota and cost.

### Proxy

```sh
clankmon proxy                                  # 127.0.0.1:8484 → api.anthropic.com
export ANTHROPIC_BASE_URL=http://127.0.0.1:8484 # in the shell where you run claude
```

The proxy forwards requests untouched, including auth, and streams responses back chunk by chunk while a copy feeds the SSE parser. It works with subscription (OAuth) and API-key auth. It records token counts, timings and headers, never prompt or completion text unless you pass `--capture-bodies`. You can also host it inside the TUI with `clankmon --proxy`. The standalone proxy forwards to a running TUI, or spools records for the next start.

## Keys

| Key | Action |
|---|---|
| `1`–`5`, `Tab` | overview · sessions · requests · history · limits |
| `↑↓` `jk` `PgUp` `PgDn` | select |
| `Enter` | open the session (overview) or request detail (requests) |
| `/` | filter requests by session, model, stop reason, tool or `error` |
| `s` | sort processes: activity, cost, tokens, cpu, name |
| `+` `-` | graph column width; chart span in Limits |
| `g` `t` `f` | log scale · theme · effects |
| `p` / `space` | pause the feed (events are buffered) |
| `?` `q` | help · quit |

## Commands

```text
clankmon [--theme T] [--no-fx] [--days N] [--proxy[=PORT]] [--otel[=PORT]] [--oauth] [--demo]
clankmon install [--statusline] [--hooks] [--otel] [--dry-run] [--yes]
clankmon uninstall [--dry-run] [--yes]        # restores a wrapped status line
clankmon proxy [--port P] [--upstream URL] [--print-env] [--capture-bodies] [--quiet]
clankmon doctor
clankmon snapshot [--view V] [--width W] [--height H] [--ansi]   # one frame to stdout
clankmon export [--format csv|jsonl|daily] [--since 7d] [-o FILE]
clankmon config                               # print an example ~/.clankmon/config.toml
```

## Configuration

`~/.clankmon/config.toml` is optional. Run `clankmon config` for a commented example. You can set the theme, history days, frame rate, a daily budget, proxy/OTel/OAuth defaults, and price overrides:

```toml
theme = "synthwave"
history_days = 30

[proxy]
enabled = true          # host the proxy in the TUI

[[pricing]]             # add or override a model (USD per million tokens)
match = "claude-opus-5-5"
input = 4.0
output = 20.0
cache_read = 0.20
```

`CLAUDE_CONFIG_DIR` (or `--claude-dir`) points at a non-default Claude Code directory. `CLANKMON_HOME` moves clankmon's own state, and `CLANKMON_LOG` sets the log filter (the log is at `~/.clankmon/clankmon.log`).

## Privacy

Everything stays on your machine. clankmon reads Claude Code's local files. It talks to the network only through the opt-in proxy (which forwards your own requests) and the opt-in OAuth poller (one read-only call, using Claude Code's token, which clankmon never stores or refreshes). Telemetry from Claude Code goes only to the loopback receiver you start.

## How it's built

```text
 transcripts ─┐  notify tail, parallel scan
 sessions/   ─┤  1 s poll + ps
 statusline  ─┤                                   ┌─ store (SQLite, ~/.clankmon)
 hooks       ─┼─ Unix socket ── AppEvent channel ─┼─ reducer (one record per request)
 proxy       ─┤  (spool if TUI is down)           └─ ratatui + tachyonfx, 4 Hz idle / 30 fps animating
 otel        ─┤
 oauth       ─┘
```

Built with ratatui, tachyonfx, tui-big-text, tokio, hyper and rustls, and rusqlite. The data layer lives in `src/collect`, `src/state.rs` and `src/windows.rs`; the UI lives in `src/ui`.

```sh
cargo test                    # unit, snapshot (insta) and proxy/OTLP integration tests
cargo clippy --all-targets -- -D warnings
clankmon --demo               # try the UI on synthetic traffic
vhs docs/demo.tape            # record docs/demo.gif (needs charmbracelet/vhs)
```

## Prior art

Inspired by [ccusage](https://github.com/ryoppippi/ccusage) (transcript parsing, 5-hour blocks), [Claude-Code-Usage-Monitor](https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor) (forecasting), [claudeMon](https://github.com/woffko/claudeMon) (status-line limits feed), [claude-rate-monitor](https://github.com/nsanden/claude-rate-monitor) (unified limit headers), and of course htop and bmon.

## License

MIT
