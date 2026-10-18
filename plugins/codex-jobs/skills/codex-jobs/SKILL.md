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

Report the returned job ID and continue independent work. Do not poll: the plugin monitor notifies this Claude session when a job changes state. When notified of completion, read its `last.md` path or run `codex-jobs show <id>` and incorporate the result. Failed or lost jobs also need `show` for diagnostics. Use `codex-jobs cancel <id>` when the delegated work is no longer needed.

The monitor requires an interactive Claude Code session. In non-interactive sessions, explicitly use `codex-jobs log <id> -f` if waiting is needed.
