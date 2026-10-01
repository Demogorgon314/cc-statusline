//! Native rate limits, with an opt-in background OAuth fallback for older clients.
use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Don't spawn another fetch while one may still be running.
const FETCH_LOCK_S: f64 = 20.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub used_ratio: f64,
    pub reset_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Quota {
    pub limit_5h: Option<Entry>,
    pub limit_7d: Option<Entry>,
    pub spend: Option<Entry>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    /// last fetch attempt
    t: f64,
    /// when `v` was fetched
    fetched_at: f64,
    v: Option<Quota>,
    error: Option<String>,
}

fn cache_path() -> PathBuf {
    paths::cache_dir().join("quota.json")
}

fn lock_path() -> PathBuf {
    paths::cache_dir().join("quota.lock")
}

fn read_cache() -> Cache {
    std::fs::read(cache_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// After a failed fetch, retry this soon rather than waiting a full `ttl`.
const RETRY_AFTER_ERROR_S: f64 = 15.0;

fn mtime_secs(path: &std::path::Path) -> Option<f64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64())
}

/// Cached quota; kicks off a background refresh when it is due:
/// - the cache is older than `ttl`, or
/// - the last attempt failed and either `RETRY_AFTER_ERROR_S` passed or
///   Claude Code has since rewritten the credentials (a refreshed token
///   usually fixes an "expired" failure immediately).
pub fn get(ttl: f64) -> Option<Quota> {
    let cache = read_cache();
    let now = paths::now_secs();
    let failed = cache.error.is_some() || cache.v.is_none();
    let creds_changed = failed
        && mtime_secs(&paths::claude_home().join(".credentials.json")).is_some_and(|m| m > cache.t);
    let due =
        now - cache.t >= ttl || (failed && (creds_changed || now - cache.t >= RETRY_AFTER_ERROR_S));
    if due {
        let lock = lock_path();
        if mtime_secs(&lock).is_some_and(|t| now - t > FETCH_LOCK_S) {
            let _ = std::fs::remove_file(&lock);
        }
        let _ = std::fs::create_dir_all(paths::cache_dir());
        // Exclusive creation prevents simultaneous status-line processes
        // from launching duplicate requests for the same account.
        if std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock)
            .is_ok()
        {
            spawn_fetch();
        }
    }
    cache.v
}

fn spawn_fetch() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.arg("fetch-quota")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // own process group: foreground rendering may exit first
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    let _ = cmd.spawn();
}

/// `fetch-quota`: one request, result into the cache. Errors are recorded
/// next to the last good value instead of replacing it.
pub fn fetch_and_store() -> Result<Quota, String> {
    let result = fetch();
    let mut cache = read_cache();
    cache.t = paths::now_secs();
    match &result {
        Ok(q) => {
            cache.v = Some(q.clone());
            cache.fetched_at = cache.t;
            cache.error = None;
        }
        Err(e) => cache.error = Some(e.clone()),
    }
    if let Ok(data) = serde_json::to_vec(&cache) {
        let _ = paths::write_atomic(&cache_path(), &data);
    }
    let _ = std::fs::remove_file(lock_path());
    paths::debug(&format!("fetch-quota: {:?}", result.as_ref().map(|_| "ok")));
    result
}

/// Last fetch error, for `cc-statusline quota` diagnostics.
pub fn last_error() -> Option<String> {
    read_cache().error
}

fn access_token() -> Result<String, String> {
    let file = paths::claude_home().join(".credentials.json");
    let raw = std::fs::read_to_string(file).ok();
    #[cfg(target_os = "macos")]
    let raw = {
        use sha2::{Digest, Sha256};
        let suffix = std::env::var("CLAUDE_CONFIG_DIR")
            .ok()
            .filter(|dir| !dir.is_empty())
            .map(|dir| format!("-{:x}", Sha256::digest(dir.as_bytes()))[..9].to_string())
            .unwrap_or_default();
        let service = format!("Claude Code-credentials{suffix}");
        let mut cmd = Command::new("security");
        cmd.args(["find-generic-password", "-w", "-s", &service]);
        if let Ok(user) = std::env::var("USER") {
            cmd.args(["-a", &user]);
        }
        crate::probe::run_with_timeout(&mut cmd, std::time::Duration::from_secs(2)).or(raw)
    };
    let value: serde_json::Value = raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .ok_or("No Claude Code OAuth login found; sign in with /login")?;
    let oauth = value
        .get("claudeAiOauth")
        .ok_or("No Claude Code OAuth token")?;
    if oauth
        .get("expiresAt")
        .and_then(|v| v.as_f64())
        .is_some_and(|t| t <= paths::now_secs() * 1000.0)
    {
        return Err("OAuth token expired; let Claude Code refresh your login".into());
    }
    oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "No Claude Code OAuth access token".into())
}

fn fetch() -> Result<Quota, String> {
    let token = access_token()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(8)))
        .http_status_as_error(false)
        .build()
        .into();
    // Never send the login token to a user-configured API/proxy provider URL.
    let mut res = agent
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "cc-statusline")
        .call()
        .map_err(|_| "Quota request failed".to_string())?;
    if res.status().as_u16() != 200 {
        return Err(format!(
            "Quota request returned HTTP {}",
            res.status().as_u16()
        ));
    }
    let body: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|_| "Invalid quota response")?;
    Ok(parse(&body))
}

pub fn parse(body: &serde_json::Value) -> Quota {
    let entry = |key| -> Option<Entry> {
        let v = body.get(key)?;
        Some(Entry {
            used_ratio: v.get("utilization")?.as_f64()? / 100.0,
            reset_at: v
                .get("resets_at")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
    };
    Quota {
        limit_5h: entry("five_hour"),
        limit_7d: entry("seven_day"),
        spend: None,
    }
}

/// Native payload percentages use 0..100; reset timestamps are epoch seconds.
pub fn from_payload(body: &serde_json::Value) -> Quota {
    let entry = |key| -> Option<Entry> {
        let v = body.get(key)?;
        Some(Entry {
            used_ratio: v.get("used_percentage")?.as_f64()? / 100.0,
            reset_at: v
                .get("resets_at")
                .and_then(|v| v.as_i64())
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        })
    };
    Quota {
        limit_5h: entry("five_hour"),
        limit_7d: entry("seven_day"),
        spend: entry("spend_limit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_and_oauth_units_match() {
        let native = from_payload(
            &serde_json::json!({"five_hour":{"used_percentage":42.5,"resets_at":1800000000}}),
        );
        let api = parse(
            &serde_json::json!({"five_hour":{"utilization":42.5,"resets_at":"2027-01-15T08:00:00Z"}}),
        );
        assert_eq!(native.limit_5h, api.limit_5h);
        assert!(native.limit_7d.is_none());
    }
}
