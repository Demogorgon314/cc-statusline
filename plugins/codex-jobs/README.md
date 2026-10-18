# Codex background jobs

Delegate work to `codex exec` and receive status changes in your Claude Code session. Jobs survive the invoking shell and share a global FIFO queue. The footer's `codex` segment shows running, queued, and recent results.

Basic job commands require cc-statusline **0.8.0 or later** on PATH, in `~/.local/bin` or `~/.cargo/bin`, or in `$CC_STATUSLINE_BIN_DIR`; also requires an authenticated `codex` executable. Running and cancelling jobs requires Unix. Plugin monitors run in interactive Claude Code sessions with Monitor support.

The completion workflow below requires a build with `codex wait` and `codex run --wait` (not included in the v0.8.0 release). Until released, build this checkout with `cargo build --release --locked` and use the resulting binary. Check support with `cc-statusline codex wait --help`.

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

Resolve and verify the installed executable and use its quoted absolute path in each Bash call. The skill runs `"/absolute/path/cc-statusline" codex run --wait ...` with Bash\'s `run_in_background: true`: one native task submits the job, waits through queueing and execution, and prints its status, original Codex exit code, absolute result path, and final response. Claude must read the notification\'s output file to obtain the result; the notification itself need not contain it. Shell `&` does not register a native task. The monitor provides optional progress updates only.

`codex wait <id>` attaches to an existing job or immediately reports one that already ended. It never resubmits work. Both waiting commands return 0 only for a successful job with a readable, nonempty final response; unsuccessful jobs and missing results return 1. The original Codex exit code is printed separately. Partial responses from unsuccessful jobs are labelled as incomplete, with the last 20 lines of diagnostic logs. Stopping a waiter leaves the job running; use `codex cancel <id>` to cancel. To recover an interrupted waiter, find its job ID in the initial output or `codex ls --here`, then start `codex wait <id>` as a new native background task.

Ask Claude to delegate a task, or submit it directly:

```sh
cc-statusline codex run --wait -m gpt-6.1-sol -e high --title "Research auth" -s read-only prompt.md
cc-statusline codex wait 3f2a
cc-statusline codex ls --here
cc-statusline codex show 3f2a
cc-statusline codex log 3f2a -n 40
cc-statusline codex cancel 3f2a
cc-statusline codex prune --older-than 7d
```

Claude commands: `/codex-jobs:status` (optional `all`), `/codex-jobs:log <id>`, `/codex-jobs:result <id>`, and `/codex-jobs:cancel <id>`. The plugin's `codex-jobs` wrapper is on Claude's Bash PATH. Argument-bearing commands validate IDs before running Bash rather than interpolating arbitrary arguments into shell injections.

State lives in `$CODEX_JOBS_HOME` (default `~/.codex-jobs`). `--max` overrides `$CODEX_JOBS_MAX` (default 2) for a submission; each supervisor compares its limit against running jobs across the entire store. Use one consistent limit for predictable capacity. `$CODEX_JOBS_CODEX_BIN` can select another Codex executable. Prompt, JSON events, stderr, final message, and supervisor diagnostics remain in `jobs/<id>/` until pruned.

The monitor silently snapshots existing jobs on startup and emits only subsequent transitions. Changes between polls collapse to the last observed state. A dead supervisor is reported as `lost`; cancel or prune cleans it up. Lost jobs remain visible as errors until cleaned up. Log timestamps use observation time when Codex emits no timestamp. Durations use the footer's existing minute-resolution formatter.
