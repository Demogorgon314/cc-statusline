# Codex background jobs

Delegate work to `codex exec` and receive status changes in your Claude Code session. Jobs survive the invoking shell and share a global FIFO queue. The footer's `codex` segment shows running, queued, and recent results.

Requires cc-statusline **0.7.3 or later** on PATH, in `~/.local/bin` or `~/.cargo/bin`, or in `$CC_STATUSLINE_BIN_DIR`; also requires an authenticated `codex` executable. Running and cancelling jobs requires Unix. Plugin monitors run in interactive Claude Code sessions with Monitor support.

```sh
curl -fsSL https://raw.githubusercontent.com/Demogorgon314/cc-statusline/main/install.sh | sh
```

In Claude Code:

```text
/plugin marketplace add Demogorgon314/cc-statusline
/plugin install codex-jobs@cc-statusline
```

For local development:

```sh
claude --plugin-dir ./plugins/codex-jobs
```

Enable **Codex** in `cc-statusline config` for existing configurations. New configurations enable it automatically; with no jobs it renders nothing. Options: `scope = "project"` (or `"all"`), `recent_secs = 600`, and `max_jobs = 2`. Project scope includes jobs submitted from the current directory or its descendants, regardless of the execution directory (`-C`).

Ask Claude to delegate a task, or submit it directly:

```sh
cc-statusline codex run -m gpt-6.1-sol -e high --title "Research auth" -s read-only prompt.md
cc-statusline codex ls --here
cc-statusline codex show 3f2a
cc-statusline codex log 3f2a -n 40
cc-statusline codex cancel 3f2a
cc-statusline codex prune --older-than 7d
```

Claude commands: `/codex-jobs:status` (optional `all`), `/codex-jobs:log <id>`, `/codex-jobs:result <id>`, and `/codex-jobs:cancel <id>`. The plugin's `codex-jobs` wrapper is on Claude's Bash PATH. Argument-bearing commands validate IDs before running Bash rather than interpolating arbitrary arguments into shell injections.

State lives in `$CODEX_JOBS_HOME` (default `~/.codex-jobs`). `--max` overrides `$CODEX_JOBS_MAX` (default 2) for a submission; each supervisor compares its limit against running jobs across the entire store. Use one consistent limit for predictable capacity. `$CODEX_JOBS_CODEX_BIN` can select another Codex executable. Prompt, JSON events, stderr, final message, and supervisor diagnostics remain in `jobs/<id>/` until pruned.

The monitor silently snapshots existing jobs on startup and emits only subsequent transitions. Changes between polls collapse to the last observed state. A dead supervisor is reported as `lost`; cancel or prune cleans it up. Lost jobs remain visible as errors until cleaned up. Log timestamps use observation time when Codex emits no timestamp. Durations use the footer's existing minute-resolution formatter.
