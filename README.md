# cc-statusline

A Rust status line for Claude Code, built from [kimi-statusline](https://github.com/Demogorgon314/kimi-statusline) and informed by [CCometixLine](https://github.com/Haleclipse/CCometixLine).

[中文](README.zh.md)

```text
$1.23  Opus 4.6 high  …/code/project  main [+42 -7]  ctx 38% (76.0k/200.0k)  │ total ↑ 1.26M · ↓ 21.4k cache 96.2%  5h 42% ↻2h00m
```

## Install from this checkout

```sh
cargo install --path . --locked
cc-statusline install
```

Restart Claude Code. Installation updates only `statusLine` in `~/.claude/settings.json`, keeps the previous file as `settings.json.bak`, and refuses to replace another command unless you pass `--force`. `CLAUDE_CONFIG_DIR` overrides the Claude home directory.

To configure it manually:

```json
{
  "statusLine": {
    "type": "command",
    "command": "cc-statusline",
    "padding": 0
  }
}
```

The automatic installer uses the quoted absolute executable path, so PATH configuration is unnecessary. On Windows, Claude Code executes the command through Git Bash.

`install.sh`, `install.ps1`, and `update` target this project's GitHub release assets; they require a published release. Source installation works before the first release.

## Features

- Native Claude JSON input: model, reasoning effort, context, cost, duration, output style, code changes, Vim/agent/fast-mode badges, PR links, and rate limits.
- Incremental transcript accounting: cumulative input/output tokens, cache hit rate, and the busiest subagent model. Split assistant messages are deduplicated by `message.id`.
- Estimated API output throughput (`≈42 tok/s`), sampled from token and API-time deltas.
- Git branch, detached HEAD, working-tree changes, conflicts, ahead/behind, and optional open PR lookup through `gh`.
- Ten themes, plain/Nerd Font/powerline styles, custom colors and icons, English/Chinese labels, width fitting, and a keyboard/mouse configurator.
- Safe install/uninstall, configuration backups, and checksum-verified binary updates.

Claude-specific data is optional. Missing fields disappear instead of breaking the line. The program does not modify Claude's executable.

## Commands

```sh
cc-statusline                     # interactive menu when stdin is a terminal
cc-statusline config              # reorder/toggle segments, edit colors, live preview
cc-statusline init                # write the default config; --force to replace
cc-statusline themes
cc-statusline -t nord preview
cc-statusline preview --cwd /path/to/project --width 100
cc-statusline quota               # explicitly query the Claude OAuth usage endpoint
cc-statusline update --check
cc-statusline update
cc-statusline uninstall
```

With piped stdin, the default command renders one line from Claude's JSON. Press Ctrl+C twice within 1.5 seconds to exit the configurator and discard unsaved edits.

Try the bundled payload: `cargo run -- --theme claude --width 160 < examples/claude.json`.

Preview reuses the last payload observed for the requested directory. `--session ID` only accepts that cached session; it never substitutes a different session. Before any payload has been observed, preview shows the directory and a generic Claude label. The configurator adds demonstration values for missing data.

## Segments

| ID | Data |
| --- | --- |
| `mode` | Vim mode, agent name, fast mode |
| `cost` | Estimated session cost in USD from Claude |
| `model` | Display name and live reasoning effort |
| `output_style` | Output style (disabled by default) |
| `directory` | Working directory, configurable path depth |
| `git` | Branch, diff, conflicts, upstream tracking, PR |
| `context` | Context percentage and input tokens / capacity |
| `usage` | Cumulative session input/output and cache hit rate, including subagents |
| `subagent` | Usage of the subagent model with the most input |
| `session` | Claude's accumulated duration, falling back to transcript age (disabled by default) |
| `quota` | Five-hour, seven-day and gateway spend limits, with reset times |
| `changes` | Session lines added/removed (disabled by default) |
| `tps` | Approximate output tokens per second of API request time |

Context tokens count uncached input plus cache reads and writes, excluding output. Native `used_percentage` takes precedence. Session totals come from the transcript, because the meaning of `context_window.total_*_tokens` differs between Claude versions. Cost is Claude's estimate, not a separately calculated invoice.

The live renderer reads only `transcript_path` and its `<session>/subagents/*.jsonl` directory. Empty, missing, or mismatched sessions never inherit another session's statistics. First-time reads are bounded and catch up over successive refreshes; incomplete trailing records are retried later. Transcript truncation/replacement resets the affected cursor.

### Estimated TPS

`tps` displays `≈42 tok/s` (`≈42 t/s` in compact mode):

```text
Δ deduplicated transcript output tokens × 1000 / Δ cost.total_api_duration_ms
```

It includes visible subagent output, while shared parent messages count once. The denominator is Claude's accumulated API request time, including first-token waits and retries. Parallel requests contribute their individual durations; this measures request-time-normalized throughput, not parallel wall-clock throughput or pure decode speed. Independently updated logs/timers and API calls absent from transcripts make it an estimate.

The first complete observation establishes a baseline. A value appears after both counters advance; idle refreshes retain the last value. Missing timing, partial logs, historical log catch-up, model changes, counter rollback, replaced logs, a changed set of subagent logs, or a sampling gap over 30 minutes require a new baseline. Preview reads the cached estimate without advancing the sampler.

New default configurations enable TPS. For existing configurations, enable **TPS** in `cc-statusline config` or add:

```toml
[[segments]]
id = "tps"
enabled = true
options = { stale_secs = 300, hide_when_stale = false }
```

After five minutes without a new measurement the estimate dims. Set `hide_when_stale = true` to hide it instead, or `stale_secs = 0` to disable dimming. Narrow layouts may drop TPS to keep the model and token usage visible.

## Configuration

`~/.claude/cc-statusline/config.toml`:

```toml
theme = "claude"

[style]
mode = "plain"          # plain | nerd_font | powerline
separator = "  "
lang = "en"             # en | zh
palette = "dark"        # dark | light | custom palette name
width = 0              # 0: auto-detect; --width overrides it

[[segments]]
id = "model"
enabled = true
colors = { text = "primary" }

[[segments]]
id = "context"
enabled = true
options = { show_tokens = true, colorful = true }

[[segments]]
id = "quota"
enabled = true
options = { show_5h = true, show_7d = true, show_spend = true, show_reset = true, bar = false, oauth_fallback = false, refresh_secs = 120 }
```

Segments appear in configuration order. Omitted segments are appended disabled. Generate a complete starting point with `cc-statusline init`.

Themes: `claude`, `cometix`, `default`, `minimal`, `gruvbox`, `nord`, `powerline-dark`, `powerline-light`, `powerline-rose-pine`, `powerline-tokyo-night`. Save custom themes in `cc-statusline/themes/<name>.toml`.

Colors accept palette tokens (`text`, `primary`, `accent`, `text_dim`, `text_muted`, `success`, `warning`, `error`), `"#rrggbb"`, `{ c16 = 14 }`, `{ c256 = 208 }`, or `{ r = 1, g = 2, b = 3 }`. Custom palettes live in `cc-statusline/palettes/<name>.json`, with `base` (`dark`/`light`) and a `colors` object using camelCase tokens such as `textDim`.

Optional model aliases in `cc-statusline/models.toml` map exact incoming display names or IDs to labels:

```toml
"claude-sonnet-custom" = "Team Sonnet"
```

## Quota and offline operation

Native `rate_limits` is preferred and needs no extra request or credential read. Older clients can opt into `oauth_fallback = true`; `cc-statusline quota` also fetches explicitly. The fallback reads Claude's existing OAuth credentials (macOS Keychain, otherwise `.credentials.json`), contacts only `https://api.anthropic.com/api/oauth/usage`, and never rotates or writes login tokens. Failed refreshes retain cached data. API-key/third-party accounts can leave fallback disabled.

GitHub PR lookup uses `gh` in the background when Claude has not supplied `pr`; set the git segment's `pr = false` to disable it. Quota fallback also runs in the background. Set `NO_COLOR=1` or `CC_STATUSLINE_NO_COLOR=1` for plain output.

Caches and last-observed preview payloads live in `~/.claude/cc-statusline-cache/`. Enable diagnostics with `CC_STATUSLINE_DEBUG=1`; logs go to `~/.claude/cc-statusline-debug.log`.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

CI runs formatting, Clippy, and tests on Linux, macOS, and Windows. Tag releases as `vX.Y.Z` matching Cargo.toml; the release workflow produces six platform archives and SHA256SUMS.

Protocol reference: [Claude Code status line documentation](https://code.claude.com/docs/en/statusline). Transcript deduplication and subagent layout were also checked against local Claude Code sources.

MIT. The original kimi-statusline copyright notice is retained in [LICENSE](LICENSE).
