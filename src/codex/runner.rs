//! Unix supervisor lifecycle; each transition holds the store lock.

use super::{display_path, store::pid_alive, Job, RunArgs, Status, Store};
use crate::{paths, probe};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::time::{Duration, Instant};

fn title(prompt: &str, path: &Path) -> String {
    prompt
        .lines()
        .find_map(|l| l.trim().strip_prefix("# ").map(str::trim))
        .filter(|s| !s.is_empty())
        .or_else(|| path.file_stem().and_then(|s| s.to_str()))
        .unwrap_or("untitled")
        .chars()
        .take(60)
        .collect()
}

fn create_id(store: &Store) -> io::Result<String> {
    let mut random = File::open("/dev/urandom")?;
    reserve_id(store, |long| {
        let mut bytes = [0u8; 3];
        random.read_exact(&mut bytes)?;
        Ok(bytes[..if long { 3 } else { 2 }]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    })
}

fn reserve_id(
    store: &Store,
    mut random: impl FnMut(bool) -> io::Result<String>,
) -> io::Result<String> {
    for attempt in 0..1000 {
        let id = random(attempt >= 20)?;
        match fs::create_dir(store.dir(&id)) {
            Ok(()) => return Ok(id),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("could not allocate a unique job id"))
}

pub fn run(store: &Store, args: RunArgs) -> io::Result<String> {
    let max = args
        .max
        .map(Ok)
        .unwrap_or_else(|| match std::env::var("CODEX_JOBS_MAX") {
            Ok(s) => s
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| io::Error::other("CODEX_JOBS_MAX must be positive")),
            Err(_) => Ok(2),
        })?;
    let prompt = fs::read_to_string(&args.prompt_file)?;
    let origin = std::env::current_dir()?.canonicalize()?;
    let cwd = args.cwd.as_deref().unwrap_or(&origin).canonicalize()?;
    if !cwd.is_dir() {
        return Err(io::Error::other("-C must name a directory"));
    }
    let _lock = store.lock()?;
    let id = create_id(store)?;
    let dir = store.dir(&id);
    let mut job = Job {
        id,
        status: Status::Queued,
        title: Some(
            args.title
                .unwrap_or_else(|| title(&prompt, &args.prompt_file))
                .chars()
                .take(60)
                .collect(),
        ),
        model: Some(args.model),
        effort: Some(args.effort),
        sandbox: Some(args.sandbox),
        cwd: Some(cwd),
        origin_cwd: Some(origin),
        extra_args: args.extra_args,
        created_at: Some(paths::now_secs()),
        started_at: None,
        ended_at: None,
        exit_code: None,
        supervisor_pid: Some(std::process::id()),
        codex_pid: None,
        thread_id: None,
    };
    fs::write(dir.join("prompt.md"), prompt)?;
    store.save(&job)?;
    fs::write(store.root.join("active").join(&job.id), "")?;
    match probe::spawn_codex_supervisor(&store.root.canonicalize()?, &dir, &job.id, max) {
        Ok(child) => {
            job.supervisor_pid = Some(child.id());
            store.save(&job)?;
        }
        Err(e) => {
            finish(store, &mut job, 127)?;
            return Err(e);
        }
    }
    println!(
        "codex job {} queued · {} · {}\n  dir:    {}\n  result: {}",
        job.id,
        job.model_effort(),
        job.title(),
        display_path(&dir),
        display_path(&dir.join("last.md"))
    );
    io::stdout().flush()?;
    Ok(job.id)
}

pub fn supervise(store: &Store, id: &str, max: u32) -> io::Result<()> {
    loop {
        {
            let _lock = store.lock()?;
            let mut job = store.read(id)?;
            if job.status != Status::Queued {
                return Ok(());
            }
            let jobs = store.all();
            let running = jobs
                .iter()
                .filter(|j| j.effective_status() == Status::Running)
                .count();
            let oldest = jobs
                .iter()
                .filter(|j| j.effective_status() == Status::Queued)
                .min_by(|a, b| {
                    a.created_at
                        .unwrap_or(0.0)
                        .total_cmp(&b.created_at.unwrap_or(0.0))
                        .then_with(|| a.id.cmp(&b.id))
                });
            if running < max as usize && oldest.is_some_and(|j| j.id == id) {
                job.status = Status::Running;
                job.started_at = Some(paths::now_secs());
                job.supervisor_pid = Some(std::process::id());
                store.save(&job)?;
                break;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    // Spawn and record the group under the same lock as cancel, closing its PID race.
    let mut child = {
        let _lock = store.lock()?;
        let mut job = store.read(id)?;
        if job.status == Status::Cancelled {
            return Ok(());
        }
        match probe::spawn_codex_job(&job, &store.dir(id)) {
            Ok(child) => {
                job.codex_pid = Some(child.id());
                if let Err(e) = store.save(&job) {
                    signal_group(child.id(), libc::SIGKILL);
                    return Err(e);
                }
                child
            }
            Err(e) => {
                eprintln!("could not spawn codex: {e}");
                finish(store, &mut job, 127)?;
                return Ok(());
            }
        }
    };
    let mut events = super::events::Cursor::default();
    let mut scanned = Instant::now() - Duration::from_secs(2);
    loop {
        let exit = child.try_wait()?;
        if scanned.elapsed() >= Duration::from_secs(2) || exit.is_some() {
            let lines = events.read(&store.dir(id).join("events.jsonl"), exit.is_some())?;
            if let Some(thread_id) = lines.iter().find_map(|l| {
                serde_json::from_str::<serde_json::Value>(l)
                    .ok()?
                    .get("thread_id")?
                    .as_str()
                    .map(str::to_owned)
            }) {
                let _lock = store.lock()?;
                let mut job = store.read(id)?;
                if job.thread_id.is_none() {
                    job.thread_id = Some(thread_id);
                    store.save(&job)?;
                }
            }
            scanned = Instant::now();
        }
        if let Some(exit) = exit {
            let _lock = store.lock()?;
            let mut job = store.read(id)?;
            finish(
                store,
                &mut job,
                exit.code()
                    .unwrap_or_else(|| 128 + exit.signal().unwrap_or(0)),
            )?;
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn finish(store: &Store, job: &mut Job, code: i32) -> io::Result<()> {
    if job.status != Status::Cancelled {
        job.status = if code == 0 {
            Status::Done
        } else {
            Status::Failed
        };
        job.exit_code = Some(code);
    }
    job.ended_at = Some(paths::now_secs());
    store.save(job)?;
    store.remove_active(&job.id)?;
    // The atomic rename also updates the directory mtime for incremental readers.
    Ok(())
}

fn signal_group(pid: u32, signal: i32) {
    if pid > 1 && pid <= i32::MAX as u32 {
        unsafe {
            libc::kill(-(pid as i32), signal);
        }
    }
}

fn group_alive(pid: u32) -> bool {
    pid > 1
        && pid <= i32::MAX as u32
        && (unsafe { libc::kill(-(pid as i32), 0) == 0 }
            || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH))
}

pub fn cancel(store: &Store, id: &str) -> io::Result<()> {
    let pid = {
        let _lock = store.lock()?;
        let mut job = store.read(id)?;
        if !job.status.active() && job.status != Status::Lost {
            println!("codex job {id} already {}", job.status.name());
            return Ok(());
        }
        job.status = Status::Cancelled;
        job.ended_at = Some(paths::now_secs());
        store.save(&job)?;
        store.remove_active(id)?;
        if let Some(pid) = job.codex_pid {
            signal_group(pid, libc::SIGTERM);
        }
        job.codex_pid
    };
    if let Some(pid) = pid {
        let deadline = Instant::now() + Duration::from_secs(5);
        while group_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if group_alive(pid) {
            signal_group(pid, libc::SIGKILL);
        }
    }
    let _lock = store.lock()?;
    let mut job = store.read(id)?;
    if job.supervisor_pid.is_none_or(|p| !pid_alive(p)) {
        finish(store, &mut job, 143)?;
    }
    println!("codex job {id} cancelled");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn titles_and_collision_growth() {
        assert_eq!(
            title("intro\n# Research\n# Other", Path::new("fallback.md")),
            "Research"
        );
        assert_eq!(title("no heading", Path::new("fallback.md")), "fallback");
        assert_eq!(
            title(&format!("# {}", "界".repeat(80)), Path::new("x"))
                .chars()
                .count(),
            60
        );
        let store = Store {
            root: std::env::temp_dir().join(format!("cxj-ids-{}", std::process::id())),
        };
        let _lock = store.lock().unwrap();
        let a = create_id(&store).unwrap();
        let b = create_id(&store).unwrap();
        assert_ne!(a, b);
        assert!(super::super::store::valid_id(&a));
        fs::create_dir(store.dir("aaaa")).unwrap();
        let mut attempts = 0;
        let id = reserve_id(&store, |long| {
            attempts += 1;
            Ok(if long { "bbbbbb" } else { "aaaa" }.into())
        })
        .unwrap();
        assert_eq!(id, "bbbbbb");
        assert_eq!(attempts, 21);
        fs::remove_dir_all(store.root).unwrap();
    }
}
