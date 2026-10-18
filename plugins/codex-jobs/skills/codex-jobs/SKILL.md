---
name: codex-jobs
description: Delegate research or implementation to Codex in the background, with queued execution and completion notifications.
---

Write a prompt file describing the task, working directory, constraints, and expected deliverable. Resolve the installed `cc-statusline` executable first: check `command -v cc-statusline`, then executable files at `$CC_STATUSLINE_BIN_DIR/cc-statusline` (when set), `$HOME/.local/bin/cc-statusline`, and `$HOME/.cargo/bin/cc-statusline`. Verify the selected executable with `codex log --help`. If none works, report the missing prerequisite instead of submitting a job. Use its quoted absolute path for every command; do not assume `codex-jobs` is on PATH. Replace `/absolute/path/cc-statusline` below with that verified path in each Bash call (shell variables do not persist between calls):

```sh
"/absolute/path/cc-statusline" codex run -m gpt-6.1-sol -e high --title "Describe the task" -C /absolute/project/path prompt.md
```

Always specify both model and effort. Use `gpt-6.1-sol` (cheaper, usually `high`) for routine work; use `gpt-6-astra` (stronger, pricier, usually `medium`) for hard work. Use `-s read-only` for research that needs no writes; the default is `workspace-write`. Extra Codex flags go after `--`.

Keep running jobs within the configured maximum: the global queue does this automatically (`--max`, then `CODEX_JOBS_MAX`, then 2). Use a consistent limit across submissions. Jobs from all projects share slots. Coordinate file ownership for implementation jobs that share a checkout.

After submission, register one completion waiter for the returned job ID using Claude Code's Bash tool:

```json
{
  "command": "\"/absolute/path/cc-statusline\" codex log <id> -f",
  "run_in_background": true,
  "description": "Wait for Codex job <id> to finish"
}
```

Replace `<id>` with the returned job ID. Bash's `run_in_background` registers a native Claude Code task; shell `&` does not. Continue independent work without repeated polling or sleep loops. The follow command exits when the job ends, including when it finished before the waiter started, and Claude Code delivers a native task completion notification.

On notification, always run `"/absolute/path/cc-statusline" codex show <id>` to check the actual job status, then read its reported `last.md` path and incorporate the result. The waiter's exit code is not the Codex job's exit code: a successful waiter can represent a failed, cancelled, or lost job. If the waiter fails or stops while the job is active, resolve the cause and register a replacement. Stopping the waiter does not cancel Codex; use `"/absolute/path/cc-statusline" codex cancel <id>` to stop the work.

The plugin monitor provides optional progress updates; do not rely on it for completion delivery. It may be unavailable even when the plugin is enabled. Handle duplicate monitor/native notifications only once. Without native background notifications, run `"/absolute/path/cc-statusline" codex log <id> -f` in the foreground and check `show` afterwards; do not promise an asynchronous notification.
