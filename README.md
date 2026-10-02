<div align="center">

# cc-statusline

**See what your Claude Code session is really costing you — tokens, cache hits and plan quota — right in the footer.**

[![Release](https://img.shields.io/github/v/release/Demogorgon314/cc-statusline?style=flat-square)](https://github.com/Demogorgon314/cc-statusline/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/Demogorgon314/cc-statusline/ci.yml?branch=main&style=flat-square&label=ci)](https://github.com/Demogorgon314/cc-statusline/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
![Rust](https://img.shields.io/badge/rust-%E2%9C%93-orange?style=flat-square&logo=rust)

English | [中文](README.zh.md)

![cc-statusline with a demo Claude Code session](assets/hero.png)

</div>

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Demogorgon314/cc-statusline/main/install.sh | sh
```

Restart Claude Code to load the status line.

<details>
<summary>Windows, manual configuration, or from source</summary>

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/Demogorgon314/cc-statusline/main/install.ps1 | iex
```

**From source**

```bash
cargo install --git https://github.com/Demogorgon314/cc-statusline --locked
cc-statusline install
```

From a local checkout, use `cargo install --path . --locked`, then `cc-statusline install`.

**Manual configuration** in `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "cc-statusline",
    "padding": 0
  }
}
```

Merge this object into your existing settings. The automatic installer uses a quoted absolute executable path, so it does not depend on PATH. It preserves unrelated settings, backs up to `settings.json.bak`, and refuses to replace another status line unless you pass `install --force`. `CLAUDE_CONFIG_DIR` overrides `~/.claude`.

Requires Claude Code with command status line support. On Windows, Claude runs the command through Git Bash. Nerd Font and powerline styles need a Nerd Font in your terminal; the default plain style does not.

</details>

## Why

Keep the details of your Claude Code session visible without interrupting your work:

- 📊 **Whole-session token usage** across the main agent and visible subagents, plus **cache hit rate**, colored from red to green
- ⏳ **Five-hour and seven-day quota usage** and reset times (for example, `5h 42% ↻1h20m`)
- 🧩 **Subagent usage grouped by model**, alongside Claude's estimated session cost
- 🌿 **Git at a glance**: changed lines, ahead/behind, and clickable `[PR#42]` links

Model and live reasoning effort, context usage, Vim/agent/fast-mode badges, and estimated output throughput fit alongside them. Long model context labels stay compact: `Opus 5.5 [1M]`.

## Make it yours

![All ten built-in themes](assets/themes.png)

10 built-in themes, from the Claude-colored default to full powerline. Run `cc-statusline` and choose the configurator, or open it directly with `cc-statusline config`:

![Interactive TUI configurator](assets/configurator.png)

Toggle and reorder segments, choose colors and icons, switch themes, and preview with your session's data. Keyboard and mouse both work: click to select, click again to toggle or edit, drag to reorder, and scroll to navigate. Inspired by [CCometixLine](https://github.com/Haleclipse/CCometixLine).

In any TUI screen, press **Ctrl+C twice within 1.5 seconds** to exit. The first press shows a confirmation hint; exiting discards unsaved changes.

## Fast by design

cc-statusline is a single Rust binary. Work stays bounded as your session grows:

- **Incremental transcript reads** resume from cached cursors, with a 4 MiB per-file read limit and a 100 ms parsing budget per refresh; older history catches up over successive refreshes
- Optional OAuth quota refreshes and `gh pr view` run **in the background**; native quota needs no extra request
- **Adaptive width** compacts the line and removes lower-priority segments as the terminal narrows

![Adaptive status line at four terminal widths](assets/adaptive.png)

## Reference

<details>
<summary>Segments</summary>

| ID | Content |
| --- | --- |
| `mode` | Vim mode, agent name, fast-mode badges |
| `cost` | Claude's estimated session cost in USD |
| `model` | Model and live reasoning effort; compact `[1M]` suffix |
| `output_style` | Output style (disabled by default) |
| `directory` | Working directory |
| `git` | Branch, diff, conflicts, ahead/behind, open PR (fallback lookup needs `gh`) |
| `context` | Main conversation context percentage and input tokens / capacity |
| `usage` | Whole-session input ↑ / output ↓ / cache hit rate, including subagents |
| `subagent` | Cumulative subagent usage: top two models by input, plus `+N` for the rest |
| `session` | Accumulated session duration (disabled by default) |
| `quota` | Five-hour / seven-day quota, optional gateway spend limit, reset times |
| `changes` | Session lines added/removed (disabled by default) |
| `tps` | Recent session output per wall-clock second: `≈42 tok/s` (compact: `≈42 t/s`) |

When space runs out, segments drop in this order: session → tps → changes → git → directory → subagent → output_style → cost → context → quota → mode.

`main ctx` counts the main conversation's input plus cache reads and writes, excluding output; Claude's native percentage takes precedence. Subagents have independent contexts and are not added to this percentage. Session totals come from the exact supplied transcript and its adjacent subagent logs, deduplicated by message ID. The `sub` segment includes finished tasks and groups them by model, not by agent; compact mode shows the largest model and a count of the rest. Incomplete totals are marked `≈` and dimmed. Missing sessions never inherit another session's statistics. Quota belongs to the account and can persist across sessions.

Preview and the configurator use the last observed payload for the current directory. `preview --session ID` only accepts a matching cached session. Before one is available, preview shows a generic Claude label; the configurator fills missing values with demo data.

**How TPS works**

```text
new deduplicated output observed in the last 30 seconds / elapsed window seconds
```

This measures the main conversation and visible parallel subagents together, including waiting time. It replaces the older API-time-normalized estimate: independent transcript and API timer updates cannot reliably be paired. API timing fields and model changes no longer affect TPS. Logs can arrive in batches, so this is observed output throughput, not model decode speed. Output is grouped into one-second buckets over a fixed 30-second window; during startup the denominator is the observed duration, with a one-second minimum warmup.

Each log gets its own baseline. Contents already present when a file is discovered or replaced, and large backlogs being caught up, contribute to usage totals but not recent throughput. This conservatively excludes the first output of an agent whose nonempty log is discovered late. New empty logs do not reset the session. Partial or unavailable logs retain the previous trustworthy estimate, dimmed, while healthy logs continue collecting. Preview reads the saved snapshot without advancing cursors. Collection rotates through files under a time budget, and a nonblocking session lock prevents overlapping refreshes from overwriting progress. Clock rollback or a sampling gap over 30 minutes starts a fresh window.

New configurations enable TPS. In existing configurations, enable **TPS** in `cc-statusline config`. Idle output leaves the window and the rate falls to zero; after five minutes without new output it dims by default. `install` adds `statusLine.refreshInterval: 1` when absent so background agent output and idle decay refresh even while the main agent waits; an explicitly configured interval is preserved. Re-run `cc-statusline install` for an existing installation, or add that field manually. Old TPS caches are ignored on upgrade.

**Per-agent context rows**

Run `cc-statusline install --subagents` to install the separate agent-panel renderer. It preserves the main status line and refuses to replace another renderer unless `--force` is supplied. `cc-statusline uninstall --subagents` removes only this hook. No settings are changed by merely building or running the renderer.

The hook invokes `cc-statusline subagents`, reads Claude's `tasks` array and prints one JSON row per task with its name, model, status, and individual context percentage. Missing context fields show `ctx ?`, never a fabricated zero. Rows fit the provided `columns` width; `--width` overrides it. Context fields require Claude Code v2.1.205 or later; see the [subagent status line protocol](https://code.claude.com/docs/en/statusline#subagent-status-lines).

</details>

<details>
<summary>Configuration file</summary>

`~/.claude/cc-statusline/config.toml` (custom themes go in `themes/<name>.toml`), following the segment-based format of CCometixLine:

```toml
theme = "claude"

[style]
mode = "plain"          # plain | nerd_font | powerline
separator = "  "        # "" for powerline arrows
lang = "en"             # or "zh"
palette = "dark"        # dark | light | custom palette name
width = 0              # auto-detect; --width overrides this

[[segments]]
id = "quota"
enabled = true
colors = { text = "text_dim" }
options = { show_5h = true, show_7d = true, show_spend = true, show_reset = true, bar = false, oauth_fallback = false, refresh_secs = 120 }

[[segments]]
id = "tps"
enabled = true
options = { stale_secs = 300, hide_when_stale = false }
```

Segments appear in configuration order; omitted segments are appended disabled. Use `cc-statusline init` for a complete starting point. TPS can hide stale or incomplete estimates with `hide_when_stale = true`; `stale_secs = 0` disables age-based dimming, but incomplete data remains dimmed.

Colors accept palette names (`primary`, `accent`, `text_dim`, `success`, `warning`, `error`), `"#rrggbb"`, `{ c16 = 14 }`, `{ c256 = 208 }`, or `{ r = 1, g = 2, b = 3 }`. Custom palettes live in `palettes/<name>.json`, with a `base` (`dark` / `light`) and a `colors` object using camelCase names such as `textDim`.

Built-in themes: `claude`, `cometix`, `default`, `minimal`, `gruvbox`, `nord`, `powerline-dark`, `powerline-light`, `powerline-rose-pine`, `powerline-tokyo-night`.

Optional `~/.claude/cc-statusline/models.toml` maps exact incoming model display names or IDs to aliases:

```toml
"claude-sonnet-custom" = "Team Sonnet"
```

</details>

<details>
<summary>How quota works</summary>

Claude's native `rate_limits` takes precedence, without extra requests or credential reads. Older clients can enable `oauth_fallback = true`; background refreshes run no more often than `refresh_secs` (minimum 30 seconds). `cc-statusline quota` explicitly fetches the OAuth usage endpoint.

The fallback uses Claude's existing OAuth credentials from macOS Keychain or `.credentials.json` and contacts `https://api.anthropic.com/api/oauth/usage`. It never refreshes or writes login tokens. Failed refreshes keep the last cached value; API-key and third-party accounts can leave fallback disabled.

PR lookup also runs in the background when Claude has not supplied a PR. Set the git segment's `pr = false` to disable lookup. Caches live in `~/.claude/cc-statusline-cache/`.

</details>

<details>
<summary>Commands, debugging, uninstall</summary>

```bash
cc-statusline                     # menu: configure, install, quota, update…
cc-statusline config              # configurator
cc-statusline init                # write default config; --force to replace
cc-statusline themes              # list themes
cc-statusline -t nord preview     # preview a theme
cc-statusline preview --cwd /path/to/project --width 100
cc-statusline quota               # fetch quota now
cc-statusline update              # latest release; --check to check only
cc-statusline uninstall           # remove our statusLine setting, then restart Claude
```

- Debug logs: `touch ~/.claude/cc-statusline-debug`, then read `~/.claude/cc-statusline-debug.log`. Alternatively set `CC_STATUSLINE_DEBUG=1`.
- Disable colors with `NO_COLOR=1` or `CC_STATUSLINE_NO_COLOR=1`.
- Try the example payload: `cargo run -- --theme claude --width 160 < examples/claude.json`.
- Updates verify SHA256 checksums. Uninstall preserves unrelated settings and another application's status line.

</details>

## Contributing

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
uv run --with pyte python scripts/screenshots.py
```

The screenshot generator needs Chrome, ImageMagick and a Nerd Font, and runs on macOS/Linux with a PTY. Set `SHOT_CHROME` for another Chrome executable and `SHOT_FONT` for another font. With `pyte` installed, `python3 scripts/screenshots.py` also works. Images use synthetic data rendered by the actual binary and configurator, inside an HTML terminal frame.

To release: update `Cargo.toml` and `Cargo.lock`, then push a matching `vX.Y.Z` tag. CI checks Linux, macOS and Windows; the release workflow builds six platform archives and checksums.

Thanks to [kimi-statusline](https://github.com/Demogorgon314/kimi-statusline) (project foundation and README/screenshot design) and [CCometixLine](https://github.com/Haleclipse/CCometixLine) (configurator and themes). Protocol reference: [Claude Code status line documentation](https://code.claude.com/docs/en/statusline).

## License

MIT
