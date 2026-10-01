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
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("cc-statusline-debug.log"))
    {
        let _ = writeln!(f, "{:.3} {msg}", now_secs());
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
