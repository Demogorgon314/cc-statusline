//! Collect local information without looking up unrelated sessions.
use crate::appearance::{self, Models};
use crate::config::{Config, SegmentId};
use crate::payload::Payload;
use crate::render::Ctx;
use crate::{paths, probe, session};
use std::time::{Duration, Instant};

pub fn collect(mut payload: Payload, config: Config, started: Instant) -> Ctx {
    let palette = appearance::palette(
        (!config.style.palette.is_empty()).then_some(config.style.palette.as_str()),
    );
    let wants = |id| config.segment(id).is_some_and(|s| s.enabled);
    let now = paths::now_secs();
    let stats =
        if wants(SegmentId::Usage) || wants(SegmentId::Subagent) || wants(SegmentId::Session) {
            session::collect(&payload.transcript_path, &payload.session_id)
        } else {
            None
        };
    let session_created = payload
        .duration_ms
        .map(|ms| now - ms as f64 / 1000.0)
        .or_else(|| stats.as_ref().and_then(|s| s.created));
    let git_seg = config.segment(SegmentId::Git).filter(|s| s.enabled);
    if git_seg.is_some() && !payload.cwd.is_empty() {
        payload.git_branch = probe::run_with_timeout(
            std::process::Command::new("git")
                .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
                .current_dir(&payload.cwd),
            Duration::from_millis(150),
        )
        .or_else(|| {
            probe::run_with_timeout(
                std::process::Command::new("git")
                    .args(["rev-parse", "--short", "HEAD"])
                    .current_dir(&payload.cwd),
                Duration::from_millis(150),
            )
        })
        .map(|s| crate::payload::label(s.trim()))
        .filter(|s| !s.is_empty());
    }
    let git = git_seg
        .filter(|s| s.opt_bool("status", true) && payload.git_branch.is_some())
        .and_then(|_| {
            probe::git_status(&payload.cwd, started.elapsed() > Duration::from_millis(180))
        });
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
    Ctx {
        payload,
        config,
        palette,
        models: Models::load(),
        stats,
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
    if p.model.is_empty() {
        p.model = "Claude".into();
    }
    p
}
