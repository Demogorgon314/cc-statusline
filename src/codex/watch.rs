//! Quiet, scoped notifications for Claude Code monitors.

use super::{display_path, Job, Scan, Status, Store};
use crate::{paths, render::fmt_duration};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Duration;

#[derive(Default)]
struct Transitions {
    previous: Option<BTreeMap<String, Status>>,
}

impl Transitions {
    fn update<'a>(&mut self, jobs: &'a [Job]) -> Vec<&'a Job> {
        let changes = self.previous.as_ref().map_or_else(Vec::new, |previous| {
            jobs.iter()
                .filter(|j| previous.get(&j.id) != Some(&j.status))
                .collect()
        });
        self.previous = Some(jobs.iter().map(|j| (j.id.clone(), j.status)).collect());
        changes
    }
}

fn notification(store: &Store, job: &Job, running: usize) -> String {
    let elapsed = fmt_duration(job.elapsed(paths::now_secs()));
    let title = job.title();
    let details = format!("details: cc-statusline codex show {}", job.id);
    let message = match job.status {
        Status::Queued => format!(
            "queued · {} · {title} ({running} running)",
            job.model_effort()
        ),
        Status::Running => format!("running · {} · {title}", job.model_effort()),
        Status::Done => format!(
            "done ✓ {elapsed} · {title} · result: {}",
            display_path(&store.dir(&job.id).join("last.md"))
        ),
        Status::Failed => format!(
            "failed ✗ exit {} after {elapsed} · {title} · {details}",
            job.exit_code.map_or_else(|| "?".into(), |c| c.to_string())
        ),
        Status::Lost => format!("lost ✗ supervisor died after {elapsed} · {details}"),
        Status::Cancelled => format!("cancelled after {elapsed} · {title}"),
    };
    format!("[codex {}] {message}", job.id)
}

pub(super) fn watch(store: &Store, all: bool, interval: u64) -> io::Result<()> {
    let cwd = (!all).then(std::env::current_dir).transpose()?;
    let mut scan = Scan::default();
    let mut transitions = Transitions::default();
    let mut out = io::stdout().lock();
    loop {
        let jobs: Vec<_> = scan
            .poll(store, None, None)
            .into_iter()
            .filter(|j| j.in_scope(cwd.as_deref()))
            .collect();
        let running = jobs.iter().filter(|j| j.status == Status::Running).count();
        for job in transitions.update(&jobs) {
            if writeln!(out, "{}", notification(store, job, running))
                .and_then(|_| out.flush())
                .is_err()
            {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_secs(interval));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silent_baseline_and_collapsed_transitions() {
        let mut diff = Transitions::default();
        let mut job: Job = serde_json::from_str(r#"{"id":"abcd","status":"queued"}"#).unwrap();
        assert!(diff.update(&[job.clone()]).is_empty());
        assert!(diff.update(&[job.clone()]).is_empty());
        job.status = Status::Done;
        assert_eq!(diff.update(&[job.clone()])[0].status, Status::Done);
        assert!(diff.update(&[job.clone()]).is_empty());
        job.id = "dcba".into();
        assert_eq!(diff.update(&[job])[0].id, "dcba");
        let mut empty = Transitions::default();
        assert!(empty.update(&[]).is_empty());
        let job: Job = serde_json::from_str(r#"{"id":"1234","status":"done"}"#).unwrap();
        assert_eq!(empty.update(&[job])[0].status, Status::Done);
    }
}
