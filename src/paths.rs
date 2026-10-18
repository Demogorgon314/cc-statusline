//! Locations under the Claude Code home directory, plus the debug log.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_home() -> PathBuf {
    if let Some(p) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    home_dir()
        .map(|h| h.join(".claude"))
        .unwrap_or_else(|| PathBuf::from(".claude"))
}

pub fn cache_dir() -> PathBuf {
    claude_home().join("cc-statusline-cache")
}

/// Stable short hash for cache file names (FNV-1a 64; std's hasher is not
/// stable across releases).
pub fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Append to `<home>/cc-statusline-debug.log` when `CC_STATUSLINE_DEBUG`
/// is set or the flag file `<home>/cc-statusline-debug` exists (the latter
/// needs no TUI restart).
pub fn debug(msg: &str) {
    let home = claude_home();
    if std::env::var_os("CC_STATUSLINE_DEBUG").is_none()
        && !home.join("cc-statusline-debug").exists()
    {
        return;
    }
    log(msg);
}

/// Append to the debug log unconditionally (for failures worth keeping).
pub fn log(msg: &str) {
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(claude_home().join("cc-statusline-debug.log"))
    {
        let _ = writeln!(f, "{:.3} {msg}", now_secs());
    }
}

/// Per-session and per-directory cache files nobody has touched for this
/// long belong to finished sessions or deleted checkouts.
const CACHE_MAX_AGE_SECS: u64 = 14 * 86_400;
const SWEEP_EVERY_SECS: u64 = 86_400;

/// Delete abandoned cache files, at most once a day. Shared files
/// (`quota.json`, `update.json`, locks) are never touched.
pub fn sweep_cache() {
    let dir = cache_dir();
    let stamp = dir.join("sweep.stamp");
    let age = |p: &std::path::Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs())
    };
    if age(&stamp).is_some_and(|a| a < SWEEP_EVERY_SECS) {
        return;
    }
    let _ = std::fs::write(&stamp, b"");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Removing a locked inode would let a second process lock a new file
        // at the same path while the first still owns the old inode.
        if name.ends_with(".lock") {
            continue;
        }
        let max_age = if name.ends_with(".tmp") {
            // orphaned by a write_atomic killed between create and rename
            3600
        } else if ["session-", "tps-", "preview-", "git-", "pr-"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            CACHE_MAX_AGE_SECS
        } else {
            continue;
        };
        if age(&entry.path()).is_some_and(|a| a > max_age) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Write via a temp file + rename, so a run killed at the render deadline can never
/// leave a half-written cache behind.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Replace a symlink's target, not the link: dotfile managers symlink
    // settings.json and renaming over it would detach their copy.
    let resolved;
    let path = if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        // a dangling link is replaced like a missing file
        resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        resolved.as_path()
    } else {
        path
    };
    let tmp = path.with_extension(format!("{}.{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        // settings.json can contain secrets in env; an atomic replacement
        // must not broaden the original file's access permissions.
        if let Ok(metadata) = std::fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(data)?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    result.inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn write_atomic_keeps_symlinks() {
        let dir = std::env::temp_dir().join(format!("cc-statusline-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.json");
        let link = dir.join("settings.json");
        std::fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        super::write_atomic(&link, b"new").unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
