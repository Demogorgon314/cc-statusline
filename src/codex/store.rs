//! Atomic job records and lock-free incremental readers.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
    Lost,
}

impl Status {
    pub fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Lost => "lost",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub status: Status,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub sandbox: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub origin_cwd: Option<PathBuf>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub created_at: Option<f64>,
    #[serde(default)]
    pub started_at: Option<f64>,
    #[serde(default)]
    pub ended_at: Option<f64>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub supervisor_pid: Option<u32>,
    #[serde(default)]
    pub codex_pid: Option<u32>,
    #[serde(default)]
    pub thread_id: Option<String>,
}

impl Job {
    pub fn effective_status(&self) -> Status {
        if self.status.active() && self.supervisor_pid.is_some_and(|p| !pid_alive(p)) {
            Status::Lost
        } else {
            self.status
        }
    }

    pub fn in_scope(&self, cwd: Option<&Path>) -> bool {
        cwd.is_none_or(|cwd| self.origin_cwd.as_ref().is_some_and(|p| p.starts_with(cwd)))
    }

    pub fn elapsed(&self, now: f64) -> u64 {
        (self.ended_at.unwrap_or(now) - self.started_at.or(self.created_at).unwrap_or(now)).max(0.0)
            as u64
    }

    pub fn title(&self) -> String {
        crate::payload::label(self.title.as_deref().unwrap_or("untitled"))
    }

    pub fn model_effort(&self) -> String {
        crate::payload::label(&format!(
            "{}/{}",
            self.model.as_deref().unwrap_or("?"),
            self.effort.as_deref().unwrap_or("?")
        ))
    }
}

pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid == 0 || pid > i32::MAX as u32 {
            return false;
        }
        // EPERM still means the process exists.
        (unsafe { libc::kill(pid as i32, 0) == 0 })
            || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

pub fn valid_id(id: &str) -> bool {
    matches!(id.len(), 4 | 6)
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn new() -> Self {
        Self {
            root: paths::codex_jobs_home(),
        }
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join("jobs").join(id)
    }

    pub fn lock(&self) -> io::Result<File> {
        fs::create_dir_all(self.root.join("jobs"))?;
        fs::create_dir_all(self.root.join("active"))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join("lock"))?;
        file.lock()?;
        Ok(file)
    }

    pub fn read(&self, id: &str) -> io::Result<Job> {
        if !valid_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid job id",
            ));
        }
        let job: Job = serde_json::from_slice(&fs::read(self.dir(id).join("job.json"))?)?;
        if job.id != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "job id mismatch",
            ));
        }
        Ok(job)
    }

    /// Caller owns the store lock.
    #[cfg(any(unix, test))]
    pub fn save(&self, job: &Job) -> io::Result<()> {
        paths::write_atomic(
            &self.dir(&job.id).join("job.json"),
            &serde_json::to_vec_pretty(job)?,
        )
    }

    pub fn remove_active(&self, id: &str) -> io::Result<()> {
        match fs::remove_file(self.root.join("active").join(id)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn all(&self) -> Vec<Job> {
        fs::read_dir(self.root.join("jobs"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| self.read(&e.file_name().to_string_lossy()).ok())
            .collect()
    }
}

/// Active records are refreshed every poll; ended records only on directory changes.
#[derive(Default)]
pub struct Scan {
    jobs: BTreeMap<String, Job>,
    modified: BTreeMap<String, SystemTime>,
}

impl Scan {
    pub fn poll(
        &mut self,
        store: &Store,
        recent_since: Option<f64>,
        deadline: Option<Instant>,
    ) -> Vec<Job> {
        let active: HashSet<String> = fs::read_dir(store.root.join("active"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        let mut seen = HashSet::new();
        let mut complete = true;
        for id in &active {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            if let Ok(job) = store.read(id) {
                self.jobs.insert(id.clone(), job);
            }
        }
        for entry in fs::read_dir(store.root.join("jobs"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                complete = false;
                break;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) {
                continue;
            }
            seen.insert(id.clone());
            let modified = entry.metadata().and_then(|m| m.modified()).ok();
            let recent = recent_since.is_none_or(|since| {
                modified
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .is_some_and(|d| d.as_secs_f64() >= since)
            });
            if !active.contains(&id) && recent && modified.as_ref() != self.modified.get(&id) {
                if let Ok(job) = store.read(&id) {
                    self.jobs.insert(id.clone(), job);
                    if let Some(t) = modified {
                        self.modified.insert(id, t);
                    }
                }
            }
        }
        if complete {
            self.jobs.retain(|id, _| seen.contains(id));
            self.modified.retain(|id, _| seen.contains(id));
        }
        self.jobs
            .values()
            .cloned()
            .map(|mut j| {
                j.status = j.effective_status();
                j
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_reads_active_and_changed_dirs_without_locking() {
        let store = Store {
            root: std::env::temp_dir().join(format!("cxj-scan-{}", std::process::id())),
        };
        let lock = store.lock().unwrap();
        let mut job: Job = serde_json::from_str(
            r#"{"id":"abcd","status":"queued","origin_cwd":"/project/child"}"#,
        )
        .unwrap();
        store.save(&job).unwrap();
        fs::write(store.root.join("active/abcd"), "").unwrap();
        let mut scan = Scan::default();
        assert_eq!(scan.poll(&store, None, None)[0].status, Status::Queued);
        assert!(job.in_scope(Some(Path::new("/project"))));
        assert!(!job.in_scope(Some(Path::new("/proj"))));
        job.status = Status::Running;
        store.save(&job).unwrap();
        assert_eq!(scan.poll(&store, None, None)[0].status, Status::Running);
        job.status = Status::Done;
        store.save(&job).unwrap();
        store.remove_active(&job.id).unwrap();
        assert_eq!(scan.poll(&store, None, None)[0].status, Status::Done);
        // Overwriting file contents doesn't change directory mtime: ended jobs stay cached.
        fs::write(store.dir(&job.id).join("job.json"), "partial").unwrap();
        assert_eq!(scan.poll(&store, None, None)[0].status, Status::Done);
        assert!(Scan::default().poll(&store, None, None).is_empty());
        fs::remove_dir_all(store.dir(&job.id)).unwrap();
        assert!(scan.poll(&store, None, None).is_empty());
        drop(lock);
        fs::remove_dir_all(store.root).unwrap();
    }

    #[test]
    fn tolerant_round_trip_and_lost() {
        let mut job: Job =
            serde_json::from_str(r#"{"id":"abcd","status":"running","future":true}"#).unwrap();
        assert_eq!(job.effective_status(), Status::Running);
        job.supervisor_pid = Some(std::process::id());
        assert_eq!(job.effective_status(), Status::Running);
        #[cfg(unix)]
        {
            job.supervisor_pid = Some(i32::MAX as u32);
            assert_eq!(job.effective_status(), Status::Lost);
        }
        let round: Job = serde_json::from_slice(&serde_json::to_vec(&job).unwrap()).unwrap();
        assert_eq!(round.id, job.id);
        assert_eq!(round.status, Status::Running);
        assert_eq!(round.supervisor_pid, job.supervisor_pid);
        assert!(serde_json::from_str::<Job>(r#"{"id":"abcd"}"#).is_err());
        assert!(serde_json::from_str::<Job>(r#"{"status":"done"}"#).is_err());
    }
}
