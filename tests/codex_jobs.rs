#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("ccs-codex-{name}-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("prompt.md"), "# Integration prompt\nDo the job").unwrap();
        let script = root.join("fake-codex");
        fs::write(
            &script,
            r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
    if [ "$1" = '-o' ]; then shift; output=$1; fi
    shift
done
echo '{"type":"thread.started","thread_id":"t-1"}'
sleep "${CXJ_SLEEP:-0.05}"
while [ -n "${CXJ_GATE:-}" ] && [ ! -f "$CXJ_GATE" ]; do sleep 0.05; done
echo OK > "$output"
echo '{"type":"item.completed","item":{"type":"agent_message","text":"OK"}}'
exit "${CXJ_EXIT:-0}"
"#,
        )
        .unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
        Self(root)
    }

    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cc-statusline"));
        c.arg("codex")
            .env("CODEX_JOBS_HOME", self.0.join("store"))
            .env("CODEX_JOBS_CODEX_BIN", self.0.join("fake-codex"))
            .env_remove("CODEX_JOBS_MAX")
            .current_dir(&self.0);
        c
    }

    fn run(&self, args: &[&str]) -> String {
        let out = self.command().args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn submit(&self, gate: bool, code: i32) -> String {
        let mut command = self.command();
        command
            .args([
                "run",
                "-m",
                "gpt-6.1-sol",
                "-e",
                "high",
                "--max",
                "2",
                "prompt.md",
            ])
            .env("CXJ_EXIT", code.to_string());
        if gate {
            command.env("CXJ_GATE", self.0.join("release"));
        }
        let out = command.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .split_whitespace()
            .nth(2)
            .unwrap()
            .into()
    }

    fn job(&self, id: &str) -> Value {
        serde_json::from_slice(
            &fs::read(self.0.join("store/jobs").join(id).join("job.json")).unwrap(),
        )
        .unwrap()
    }

    fn wait(&self, id: &str, status: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            let job = self.job(id);
            if job["status"] == status && (status != "running" || job["codex_pid"].is_number()) {
                return job;
            }
            assert!(Instant::now() < deadline, "wanted {status}, got {job}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(entries) = fs::read_dir(self.0.join("store/jobs")) {
            for entry in entries.flatten() {
                if let Ok(bytes) = fs::read(entry.path().join("job.json")) {
                    if let Ok(job) = serde_json::from_slice::<Value>(&bytes) {
                        if matches!(job["status"].as_str(), Some("running" | "queued")) {
                            if let Some(pid) = job["codex_pid"].as_i64() {
                                unsafe {
                                    libc::kill(-(pid as i32), libc::SIGKILL);
                                }
                            }
                            if let Some(pid) = job["supervisor_pid"].as_i64() {
                                unsafe {
                                    libc::kill(pid as i32, libc::SIGKILL);
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn global_queue_limits_slots_and_preserves_results() {
    let f = Fixture::new("queue");
    let first = f.submit(true, 0);
    f.wait(&first, "running");
    let second = f.submit(true, 0);
    f.wait(&second, "running");
    let third = f.submit(true, 0);
    assert_eq!(f.job(&third)["status"], "queued");
    assert!(f.job(&third)["codex_pid"].is_null());
    // Jobs cannot finish until the fixture releases the shared gate.
    fs::write(f.0.join("release"), "").unwrap();
    for id in [&first, &second, &third] {
        let job = f.wait(id, "done");
        assert_eq!(job["thread_id"], "t-1");
        assert_eq!(job["exit_code"], 0);
        assert!(!f.0.join("store/active").join(id).exists());
        assert!(f.run(&["show", id]).contains("OK"));
        assert!(f.run(&["log", id]).contains("item.completed OK"));
    }
    let jobs: Vec<Value> = serde_json::from_str(&f.run(&["ls", "--json", "--here"])).unwrap();
    assert_eq!(jobs.len(), 3);
}

#[test]
fn failed_exit_is_preserved() {
    let f = Fixture::new("failed");
    let id = f.submit(false, 23);
    assert_eq!(f.wait(&id, "failed")["exit_code"], 23);
}

#[test]
fn cancel_terminates_process_group() {
    let f = Fixture::new("cancel");
    let id = f.submit(true, 0);
    let pid = f.wait(&id, "running")["codex_pid"].as_i64().unwrap() as i32;
    f.run(&["cancel", &id]);
    assert_eq!(f.job(&id)["status"], "cancelled");
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[test]
fn dead_supervisor_is_lost_without_reader_writes() {
    let f = Fixture::new("lost");
    let id = f.submit(true, 0);
    let job = f.wait(&id, "running");
    let pid = job["supervisor_pid"].as_i64().unwrap() as i32;
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(Instant::now() < deadline, "supervisor not reaped");
        std::thread::sleep(Duration::from_millis(20));
    }
    let jobs: Vec<Value> = serde_json::from_str(&f.run(&["ls", "--json"])).unwrap();
    assert_eq!(jobs[0]["status"], "lost");
    assert_eq!(f.job(&id)["status"], "running");
    f.run(&["cancel", &id]);
    assert_eq!(f.job(&id)["status"], "cancelled");
}
