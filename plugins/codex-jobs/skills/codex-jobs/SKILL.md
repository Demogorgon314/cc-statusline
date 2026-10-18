---
name: codex-jobs
description: Delegate research or implementation to Codex in the background, with queued execution and final results delivered through native task notifications.
---

Resolve the installed `cc-statusline` executable: check `command -v cc-statusline`, then executable files at `$CC_STATUSLINE_BIN_DIR/cc-statusline` (when set), `$HOME/.local/bin/cc-statusline`, and `$HOME/.cargo/bin/cc-statusline`. Verify it supports `codex wait --help` and `codex run --help` with `--wait`. If not, report that a newer build is needed before submitting work. Use the verified, quoted absolute path in every Bash call; do not assume `codex-jobs` is on PATH or that shell variables persist between calls.

Write a prompt file describing the task, working directory, constraints, and expected deliverable. Submit and wait in a single Claude Code Bash tool call, replacing the paths below with actual paths:

```json
{
  "command": "\"/absolute/path/cc-statusline\" codex run --wait -m gpt-6.1-sol -e high --title \"Describe the task\" -C /absolute/project/path /absolute/path/prompt.md",
  "run_in_background": true,
  "description": "Codex: describe the task"
}
```

Always specify model and effort. Use `gpt-6.1-sol` (cheaper, usually `high`) for routine work; use `gpt-6-astra` (stronger, pricier, usually `medium`) for hard work. Use `-s read-only` for research that needs no writes; the default is `workspace-write`. Extra Codex flags go after `--`.

The global queue defaults to two concurrent jobs across projects. Keep that limit consistent across submissions and coordinate file ownership for tasks sharing a checkout. `run --wait` prints the job ID immediately, then waits through queueing and execution. Bash's `run_in_background` registers the native task; shell `&` does not. Continue independent work without repeated polling or sleep loops. Do not start a second waiter for the same job.

When the native completion notification arrives, read its `output-file`. The notification itself need not contain the result. The output contains the job ID, final status, original Codex exit code, absolute `last.md` path, and final response. A zero command exit means a successful job with a readable, nonempty response. Other outcomes return nonzero (currently 1); inspect the report rather than treating that as the original Codex exit code. Failed, cancelled, or lost jobs may have a partial response explicitly labelled as incomplete. Incorporate the final result into your work; use `show <id>` for further diagnostics and read the reported `last.md` if the task output is truncated.

If the native task is interrupted while Codex is still active, recover the job ID from its initial output (or `codex ls --here`) and attach using another Bash call with `run_in_background: true`:

```sh
"/absolute/path/cc-statusline" codex wait <id>
```

`wait` attaches to the existing job and immediately reports an already-finished job; do not resubmit the prompt. Stopping a waiter does not cancel Codex. Use `codex cancel <id>` to stop work, and `codex log <id>` to inspect progress. The optional monitor is not the completion guarantee. Handle duplicate notifications only once. Without native background notifications, run `run --wait` or `wait <id>` in the foreground; do not promise asynchronous delivery.
