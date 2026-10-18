//! Collect local information without looking up unrelated sessions.
use crate::appearance::{self, Models};
use crate::config::{Config, SegmentId};
use crate::payload::Payload;
use crate::render::Ctx;
use crate::{paths, probe, session};
use std::time::{Duration, Instant};

/// Every optional probe shares this deadline, so their individual timeouts
/// can't add up to a visibly late refresh.
const DEADLINE: Duration = Duration::from_millis(300);

pub fn collect(mut payload: Payload, config: Config, started: Instant) -> Ctx {
    let deadline = started + DEADLINE;
    let palette = appearance::palette(
        (!config.style.palette.is_empty()).then_some(config.style.palette.as_str()),
    );
    let wants = |id| config.segment(id).is_some_and(|s| s.enabled);
    let now = paths::now_secs();
    let stats = if wants(SegmentId::Usage)
        || wants(SegmentId::Cache)
        || wants(SegmentId::Subagent)
        || wants(SegmentId::Session)
        || wants(SegmentId::Tps)
    {
        session::collect(&payload, deadline, now)
    } else {
        None
    };
    let session_created = payload
        .duration_ms
        .map(|ms| now - ms as f64 / 1000.0)
        .or_else(|| stats.as_ref().and_then(|s| s.created));
    let git_seg = config.segment(SegmentId::Git).filter(|s| s.enabled);
    if git_seg.is_some() && !payload.cwd.is_empty() {
        let git = |args: &[&str]| {
            let left = deadline.checked_duration_since(Instant::now())?;
            probe::run_with_timeout(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&payload.cwd),
                left.min(Duration::from_millis(150)),
            )
        };
        payload.git_branch = probe::git_head(&payload.cwd)
            .or_else(|| git(&["symbolic-ref", "--quiet", "--short", "HEAD"]))
            .or_else(|| git(&["rev-parse", "--short", "HEAD"]))
            .map(|s| crate::payload::label(s.trim()))
            .filter(|s| !s.is_empty());
    }
    let git = git_seg
        .filter(|s| s.opt_bool("status", true) && payload.git_branch.is_some())
        .and_then(|_| probe::git_status(&payload.cwd, deadline));
    let pr = git_seg.filter(|s| s.opt_bool("pr", true)).and_then(|_| {
        payload.pr.clone().or_else(|| {
            payload
                .git_branch
                .as_ref()
                .and_then(|b| probe::pull_request(&payload.cwd, b))
        })
    });
    let quota = if wants(SegmentId::Quota) {
        payload.quota.clone().or_else(|| {
            config
                .segment(SegmentId::Quota)
                .filter(|s| s.opt_bool("oauth_fallback", false))
                .and_then(|s| crate::quota::get(s.opt_int("refresh_secs", 120).max(30) as f64))
        })
    } else {
        None
    };
    let effort = payload.effort.clone();
    let tps = stats.as_ref().and_then(|stats| stats.tps);
    Ctx {
        payload,
        config,
        palette,
        models: Models::load(),
        stats,
        tps,
        effort,
        session_created,
        git,
        pr,
        quota,
        now,
        color: std::env::var_os("CC_STATUSLINE_NO_COLOR").is_none()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var("TERM").map_or(true, |t| t != "dumb"),
    }
}

/// Explicit preview uses the last payload observed in this directory.
pub fn sample_payload(cwd: &str, session_id: Option<String>) -> Payload {
    let cached = paths::cache_dir().join(format!("preview-{}.json", paths::short_hash(cwd)));
    let mut p = std::fs::read(cached)
        .map(|b| crate::payload::parse(&b))
        .unwrap_or_default();
    if session_id.as_ref().is_some_and(|id| *id != p.session_id) {
        p = Payload::default();
    }
    p.cwd = cwd.to_string();
    p.is_preview = true;
    if p.model.is_empty() {
        p.model = "Claude".into();
    }
    p
}
