//! Slow or file-heavy probes, each backed by a small TTL file cache so the
//! foreground budget only pays for them occasionally: git working-tree status,
//! PR information and terminal width.

use crate::paths;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
struct Cached<T> {
    t: f64,
    v: T,
}

/// Fresh cached value, or (stale value, needs refresh).
fn cached<T: DeserializeOwned>(name: &str, ttl: f64) -> (Option<T>, bool) {
    let path = paths::cache_dir().join(name);
    let Ok(bytes) = std::fs::read(path) else {
        return (None, true);
    };
    match serde_json::from_slice::<Cached<T>>(&bytes) {
        Ok(c) => {
            let stale = paths::now_secs() - c.t >= ttl;
            (Some(c.v), stale)
        }
        Err(_) => (None, true),
    }
}

fn store<T: Serialize>(name: &str, v: &T) {
    if let Ok(data) = serde_json::to_vec(&Cached {
        t: paths::now_secs(),
        v,
    }) {
        let _ = paths::write_atomic(&paths::cache_dir().join(name), &data);
    }
}

/// Run a command with a hard deadline; None on failure, nonzero exit or
/// timeout (the child is killed).
pub fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct GitStatus {
    pub dirty: bool,
    pub conflicts: bool,
    pub ahead: u32,
    pub behind: u32,
    pub added: u64,
    pub deleted: u64,
}

const GIT_TTL: f64 = 15.0; // upstream STATUS_TTL_MS

fn git(cwd: &str, args: &[&str]) -> Option<String> {
    run_with_timeout(
        Command::new("git")
            .arg("--no-optional-locks")
            .args(args)
            .current_dir(cwd),
        Duration::from_millis(150),
    )
}

fn probe_git(cwd: &str) -> Option<GitStatus> {
    let out = git(cwd, &["status", "--porcelain=v1", "--branch"])?;
    let mut st = GitStatus::default();
    for line in out.lines() {
        if let Some(head) = line.strip_prefix("##") {
            let num = |key: &str| {
                head.find(key).and_then(|i| {
                    head[i + key.len()..]
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                        .parse()
                        .ok()
                })
            };
            st.ahead = num("ahead ").unwrap_or(0);
            st.behind = num("behind ").unwrap_or(0);
        } else if !line.is_empty() {
            st.dirty = true;
            if matches!(
                line.get(..2),
                Some("UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD")
            ) {
                st.conflicts = true;
            }
        }
    }
    if st.dirty {
        if let Some(ns) = git(cwd, &["diff", "--numstat", "HEAD"]) {
            for line in ns.lines() {
                let mut parts = line.split('\t');
                st.added += parts
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0);
                st.deleted += parts
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0);
            }
        }
    }
    Some(st)
}

/// Working-tree status for `cwd`, refreshed at most every 15s. When the run
/// is already over budget the stale value is kept instead of spawning git.
pub fn git_status(cwd: &str, over_budget: bool) -> Option<GitStatus> {
    let name = format!("git-{}.json", paths::short_hash(cwd));
    let (prev, stale) = cached::<Option<GitStatus>>(&name, GIT_TTL);
    if !stale || over_budget {
        return prev.flatten();
    }
    match probe_git(cwd) {
        Some(v) => {
            store(&name, &Some(v));
            Some(v)
        }
        None => {
            let prev = prev.flatten();
            store(&name, &prev);
            prev
        }
    }
}

// ---------------------------------------------------------------------------
// terminal width
// ---------------------------------------------------------------------------

/// Best-effort terminal width, including shells without a controlling TTY.
pub fn terminal_width() -> Option<usize> {
    #[cfg(unix)]
    {
        if let Some(w) = tty_columns("/dev/tty") {
            return Some(w);
        }
        if let Some(w) = ancestor_tty_columns() {
            return Some(w);
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .filter(|&w| w > 0)
}

#[cfg(unix)]
fn tty_columns(dev: &str) -> Option<usize> {
    use std::os::fd::AsRawFd;
    let f = std::fs::File::open(dev).ok()?;
    // SAFETY: TIOCGWINSZ writes a winsize into the zeroed struct we own.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(f.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
    (rc == 0 && ws.ws_col > 0).then_some(ws.ws_col as usize)
}

/// (parent pid, tty device path) of `pid`.
#[cfg(target_os = "macos")]
fn proc_info(pid: u32) -> Option<(u32, Option<String>)> {
    // SAFETY: proc_pidinfo fills at most `size` bytes of the zeroed struct;
    // devname returns a pointer into a static buffer we copy out at once.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let n = libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        if n != size {
            return None;
        }
        let tdev = info.e_tdev as libc::dev_t;
        let tty = (tdev != libc::dev_t::MAX && tdev != 0)
            .then(|| libc::devname(tdev, libc::S_IFCHR))
            .filter(|p| !p.is_null())
            .map(|p| format!("/dev/{}", std::ffi::CStr::from_ptr(p).to_string_lossy()));
        Some((info.pbi_ppid, tty))
    }
}

#[cfg(target_os = "linux")]
fn proc_info(pid: u32) -> Option<(u32, Option<String>)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // fields after the parenthesised comm: state ppid ...
    let ppid = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let tty = (0..3).find_map(|fd| {
        let link = std::fs::read_link(format!("/proc/{pid}/fd/{fd}")).ok()?;
        let s = link.to_string_lossy().into_owned();
        (s.starts_with("/dev/pts/") || s.starts_with("/dev/tty")).then_some(s)
    });
    Some((ppid, tty))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn proc_info(pid: u32) -> Option<(u32, Option<String>)> {
    let out = run_with_timeout(
        Command::new("ps").args(["-o", "ppid=,tty=", "-p", &pid.to_string()]),
        Duration::from_millis(50),
    )?;
    let mut it = out.split_whitespace();
    let ppid = it.next()?.parse().ok()?;
    let tty = it.next().filter(|t| *t != "?" && *t != "??").map(|t| {
        if t.starts_with('/') {
            t.to_string()
        } else {
            format!("/dev/{t}")
        }
    });
    Some((ppid, tty))
}

/// Walk ancestors when this command has no controlling terminal.
fn ancestor_tty_columns() -> Option<usize> {
    // SAFETY: getppid has no preconditions.
    let mut pid = unsafe { libc::getppid() } as u32;
    for _ in 0..10 {
        if pid <= 1 {
            break;
        }
        let (ppid, tty) = proc_info(pid)?;
        if let Some(dev) = tty {
            // every higher ancestor sits on the same terminal
            return tty_columns(&dev);
        }
        pid = ppid;
    }
    None
}

// ---------------------------------------------------------------------------
// pull request (gh pr view, detached)
// ---------------------------------------------------------------------------

const PR_TTL: f64 = 60.0; // upstream PULL_REQUEST_TTL_MS

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
}

#[derive(Serialize, Deserialize)]
struct PrCache {
    t: f64,
    branch: String,
    v: Option<PullRequest>,
}

fn which(cmd: &str) -> Option<std::path::PathBuf> {
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ""]
    } else {
        &[""]
    };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| {
        exts.iter()
            .map(|e| dir.join(format!("{cmd}{e}")))
            .find(|p| p.is_file())
    })
}

/// The branch's open PR. `gh` needs a network round trip (upstream allows it
/// 5s), beyond our foreground budget, so it is spawned detached with stdout aimed at a
/// side file, and a later run adopts the answer. The stale value keeps
/// rendering meanwhile, and a value is only trusted for the branch it was
/// fetched on.
pub fn pull_request(cwd: &str, branch: &str) -> Option<PullRequest> {
    let dir = paths::cache_dir();
    let key = paths::short_hash(cwd);
    let path = dir.join(format!("pr-{key}.json"));
    let out = dir.join(format!("pr-{key}-{}.out", paths::short_hash(branch)));

    let cached: Option<PrCache> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|c: &PrCache| c.branch == branch);
    let mut value = cached.as_ref().and_then(|c| c.v.clone());
    let write = |v: &Option<PullRequest>| {
        let c = PrCache {
            t: paths::now_secs(),
            branch: branch.to_string(),
            v: v.clone(),
        };
        if let Ok(data) = serde_json::to_vec(&c) {
            let _ = paths::write_atomic(&path, &data);
        }
    };

    if let Ok(meta) = std::fs::metadata(&out) {
        let finished = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0.0, |d| d.as_secs_f64());
        let parsed = std::fs::read(&out)
            .ok()
            .and_then(|b| serde_json::from_slice::<PullRequest>(&b).ok());
        // gh writes its one-line JSON as it exits: parseable means done; an
        // old unparseable file means no PR / gh failed; a young one is still
        // in flight
        if parsed.is_some() || paths::now_secs() - finished > 30.0 {
            let _ = std::fs::remove_file(&out);
            if cached.as_ref().is_none_or(|c| finished >= c.t) {
                value = parsed;
                write(&value);
                return value;
            }
        }
    }
    if cached
        .as_ref()
        .is_some_and(|c| paths::now_secs() - c.t < PR_TTL)
    {
        return value;
    }
    // stamp first so concurrent runs don't all spawn gh
    write(&value);
    spawn_gh(cwd, &out);
    value
}

fn spawn_gh(cwd: &str, out: &Path) {
    let Some(gh) = which("gh") else { return };
    let Ok(file) = std::fs::File::create(out) else {
        return;
    };
    let mut cmd = Command::new(gh);
    cmd.args(["pr", "view", "--json", "number,url"])
        .current_dir(cwd)
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // own process group: foreground rendering may exit first, gh must survive
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    let _ = cmd.spawn();
}
