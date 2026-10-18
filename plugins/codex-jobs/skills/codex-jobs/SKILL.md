---
name: codex-jobs
description: Delegate research or implementation to Codex in the background, with queued execution and completion notifications.
---

Write a prompt file describing the task, working directory, constraints, and expected deliverable. Then run the plugin's `codex-jobs` executable (available on Bash's PATH):

```sh
codex-jobs run -m gpt-6.1-sol -e high --title "Describe the task" -C /absolute/project/path prompt.md
```

Always specify both model and effort. Use `gpt-6.1-sol` (cheaper, usually `high`) for routine work; use `gpt-6-astra` (stronger, pricier, usually `medium`) for hard work. Use `-s read-only` for research that needs no writes; the default is `workspace-write`. Extra Codex flags go after `--`.

Keep running jobs within the configured maximum: the global queue does this automatically (`--max`, then `CODEX_JOBS_MAX`, then 2). Use a consistent limit across submissions. Jobs from all projects share slots. Coordinate file ownership for implementation jobs that share a checkout.

After submission, register one completion waiter for the returned job ID using Claude Code's Bash tool:

```json
{
  "command": "codex-jobs log <id> -f",
  "run_in_background": true,
  "description": "Wait for Codex job <id> to finish"
}
```

Replace `<id>` with the returned job ID. Use `cc-statusline codex` if the wrapper is unavailable. Bash's `run_in_background` registers a native Claude Code task; shell `&` does not. Continue independent work without repeated polling or sleep loops. The follow command exits when the job ends, including when it finished before the waiter started, and Claude Code delivers a native task completion notification.

On notification, always run `codex-jobs show <id>` to check the actual job status, then read its reported `last.md` path and incorporate the result. The waiter's exit code is not the Codex job's exit code: a successful waiter can represent a failed, cancelled, or lost job. If the waiter fails or stops while the job is active, resolve the cause and register a replacement. Stopping the waiter does not cancel Codex; use `codex-jobs cancel <id>` to stop the work.

The plugin monitor provides optional progress updates; do not rely on it for completion delivery. It may be unavailable even when the plugin is enabled. Handle duplicate monitor/native notifications only once. Without native background notifications, run `codex-jobs log <id> -f` in the foreground and check `show` afterwards; do not promise an asynchronous notification.
