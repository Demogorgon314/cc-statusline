//! Read-only attachment to a job: waiting never owns or cancels its execution.

use super::{events, Status, Store};
use std::io::{self, Write};
use std::time::Duration;

pub(super) fn wait(store: &Store, id: &str) -> io::Result<()> {
    let job = loop {
        let mut job = store.read(id)?;
        job.status = job.effective_status();
        if !job.status.active() {
            break job;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let dir = store.dir(id).canonicalize()?;
    let result = dir.join("last.md");
    let mut out = io::stdout().lock();
    writeln!(out, "\nCodex job: {id}\nStatus: {}", job.status.name())?;
    writeln!(
        out,
        "Codex exit code: {}",
        job.exit_code
            .map_or_else(|| "unknown".into(), |c| c.to_string())
    )?;
    writeln!(out, "Result: {}", result.display())?;
    // A cancelled/lost process may still be writing a partial result. Only a successful
    // process exit and a nonempty final message constitute a delivered result.
    let message = std::fs::read_to_string(&result);
    let has_result = message.as_ref().is_ok_and(|s| !s.trim().is_empty());
    if has_result {
        let label = if job.status == Status::Done {
            "Final response"
        } else {
            "Available response (job unsuccessful; may be incomplete)"
        };
        writeln!(out, "\n--- {label} ---\n{}", message.as_ref().unwrap())?;
    } else {
        writeln!(
            out,
            "\nNo readable, nonempty final response at {}",
            result.display()
        )?;
    }
    if job.status != Status::Done || !has_result {
        for name in ["stderr.log", "supervisor.log"] {
            writeln!(out, "\n--- {name} (last 20 lines) ---")?;
            match events::tail(&dir.join(name), 20) {
                Ok(lines) => {
                    for line in lines {
                        writeln!(out, "{line}")?;
                    }
                }
                Err(e) => writeln!(out, "Could not read diagnostics: {e}")?,
            }
        }
    }
    out.flush()?;
    if job.status != Status::Done {
        return Err(io::Error::other(format!(
            "Codex job {id} {}",
            job.status.name()
        )));
    }
    if !has_result {
        return Err(io::Error::other(format!(
            "Codex job {id} exited successfully but its final response is missing, empty, or unreadable"
        )));
    }
    Ok(())
}
