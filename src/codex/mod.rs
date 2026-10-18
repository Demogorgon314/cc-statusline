//! Background Codex jobs, independent of Claude session caches.

mod events;
#[cfg(unix)]
mod runner;
mod store;
mod watch;

use crate::{paths, render};
use clap::{Args, Subcommand};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
pub use store::{Job, Scan, Status, Store};

#[derive(Args)]
pub struct RunArgs {
    #[arg(short, long)]
    model: String,
    #[arg(short, long)]
    effort: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(short = 'C')]
    cwd: Option<PathBuf>,
    #[arg(short, long, default_value = "workspace-write")]
    sandbox: String,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    max: Option<u32>,
    prompt_file: PathBuf,
    #[arg(last = true)]
    extra_args: Vec<String>,
}

#[derive(Subcommand)]
pub enum CodexCmd {
    /// Queue a prompt for a detached Codex process
    Run(RunArgs),
    #[command(name = "__supervise", hide = true)]
    Supervise {
        id: String,
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..))]
        max: u32,
    },
    /// List active jobs and jobs ended in the last day
    Ls {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        here: bool,
    },
    /// Show a job and its result or error logs
    Show { id: String },
    /// Tail human-readable Codex events
    Log {
        id: String,
        #[arg(short = 'n', default_value_t = 20)]
        lines: usize,
        #[arg(short = 'f')]
        follow: bool,
    },
    /// Cancel a queued or running job
    Cancel { id: String },
    /// Delete old ended jobs and clean up lost jobs
    Prune {
        #[arg(long, default_value = "7d", value_parser = parse_age)]
        older_than: u64,
    },
    /// Notify on status transitions (silent initial snapshot)
    Watch {
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..))]
        interval: u64,
    },
}

pub fn dispatch(cmd: CodexCmd) -> Result<(), String> {
    execute(cmd).map_err(|e| e.to_string())
}

fn execute(cmd: CodexCmd) -> io::Result<()> {
    let store = Store::new();
    match cmd {
        #[cfg(unix)]
        CodexCmd::Run(args) => runner::run(&store, args),
        #[cfg(unix)]
        CodexCmd::Supervise { id, max } => runner::supervise(&store, &id, max),
        #[cfg(unix)]
        CodexCmd::Cancel { id } => runner::cancel(&store, &id),
        #[cfg(not(unix))]
        CodexCmd::Run(_) | CodexCmd::Supervise { .. } | CodexCmd::Cancel { .. } => {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Codex job execution is unsupported on this platform",
            ))
        }
        CodexCmd::Ls { all, json, here } => list(&store, all, json, here),
        CodexCmd::Show { id } => show(&store, &id),
        CodexCmd::Log { id, lines, follow } => events::log(&store, &id, lines, follow),
        CodexCmd::Prune { older_than } => prune(&store, older_than),
        CodexCmd::Watch { all, interval } => watch::watch(&store, all, interval),
    }
}

pub fn display_path(path: &Path) -> String {
    crate::payload::label(&render::shorten_cwd(&path.to_string_lossy(), 0))
}

fn list(store: &Store, all: bool, json: bool, here: bool) -> io::Result<()> {
    let cwd = here.then(std::env::current_dir).transpose()?;
    let now = paths::now_secs();
    let mut jobs: Vec<_> = store
        .all()
        .into_iter()
        .map(|mut j| {
            j.status = j.effective_status();
            j
        })
        .filter(|j| {
            j.in_scope(cwd.as_deref())
                && (all
                    || j.status.active()
                    || j.status == Status::Lost
                    || j.ended_at.is_some_and(|t| now - t <= 86400.0))
        })
        .collect();
    jobs.sort_by(|a, b| {
        b.created_at
            .unwrap_or(0.0)
            .total_cmp(&a.created_at.unwrap_or(0.0))
            .then_with(|| b.id.cmp(&a.id))
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&jobs)?);
        return Ok(());
    }
    if jobs.is_empty() {
        println!("no codex jobs");
        return Ok(());
    }
    println!("ID      STATUS     MODEL/EFFORT          ELAPSED    TITLE  DIR");
    for j in jobs {
        let status = format!("{:<10}", j.status.name());
        let status = if io::stdout().is_terminal() {
            let c = match j.status {
                Status::Done => 32,
                Status::Failed | Status::Lost => 31,
                Status::Queued => 33,
                Status::Running => 36,
                Status::Cancelled => 2,
            };
            format!("\x1b[{c}m{status}\x1b[0m")
        } else {
            status
        };
        let elapsed = render::fmt_duration(j.elapsed(now));
        let elapsed = if j.status == Status::Queued {
            format!("queued {elapsed}")
        } else {
            elapsed
        };
        println!(
            "{:<6}  {status} {:<21} {:<10} {}  {}",
            j.id,
            j.model_effort(),
            elapsed,
            j.title(),
            j.origin_cwd
                .as_deref()
                .map(display_path)
                .unwrap_or_else(|| "?".into())
        );
    }
    Ok(())
}

fn show(store: &Store, id: &str) -> io::Result<()> {
    let mut job = store.read(id)?;
    job.status = job.effective_status();
    if let serde_json::Value::Object(fields) = serde_json::to_value(&job)? {
        for (key, value) in fields {
            println!(
                "{key:16} {}",
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            );
        }
    }
    let dir = store.dir(id);
    for name in [
        "job.json",
        "prompt.md",
        "events.jsonl",
        "stderr.log",
        "last.md",
        "supervisor.log",
    ] {
        println!("{name:16} {}", display_path(&dir.join(name)));
    }
    if !job.status.active() {
        println!(
            "\n{}",
            std::fs::read_to_string(dir.join("last.md"))
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "(no final message)".into())
        );
    }
    if matches!(job.status, Status::Failed | Status::Lost) {
        for name in ["stderr.log", "supervisor.log"] {
            println!("\n--- {name} ---");
            for line in events::tail(&dir.join(name), 20)? {
                println!("{line}");
            }
        }
    }
    Ok(())
}

fn parse_age(text: &str) -> Result<u64, String> {
    let (n, multiplier) = if let Some(n) = text.strip_suffix('d') {
        (n, 86400)
    } else if let Some(n) = text.strip_suffix('h') {
        (n, 3600)
    } else {
        return Err("expected Nd or Nh".into());
    };
    n.parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .ok_or_else(|| "expected Nd or Nh without overflow".into())
}

fn prune(store: &Store, age: u64) -> io::Result<()> {
    #[cfg(unix)]
    for job in store
        .all()
        .into_iter()
        .filter(|j| j.effective_status() == Status::Lost)
    {
        runner::cancel(store, &job.id)?;
    }
    let _lock = store.lock()?;
    let cutoff = paths::now_secs() - age as f64;
    let mut count = 0;
    for job in store.all() {
        if !job.status.active() && job.ended_at.is_some_and(|t| t < cutoff) {
            store.remove_active(&job.id)?;
            std::fs::remove_dir_all(store.dir(&job.id))?;
            count += 1;
        }
    }
    println!("pruned {count} codex jobs");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prune_duration() {
        assert_eq!(parse_age("7d"), Ok(604800));
        assert_eq!(parse_age("2h"), Ok(7200));
        for bad in ["-1d", "2m", "d", "1.5h", "18446744073709551615d"] {
            assert!(parse_age(bad).is_err());
        }
    }
}
