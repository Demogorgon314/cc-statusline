//! Normalize Claude Code's statusLine JSON into rendering data.
use serde_json::Value;
use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct Payload {
    pub model: String,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub context_usage: f64,
    pub context_tokens: u64,
    pub max_context_tokens: u64,
    pub session_id: String,
    pub transcript_path: String,
    pub effort: Option<Value>,
    pub mode: String,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    /// Preview snapshots must not advance the live throughput sampler.
    pub is_preview: bool,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub output_style: String,
    pub quota: Option<crate::quota::Quota>,
    pub pr: Option<crate::probe::PullRequest>,
}

pub fn read_bytes(timeout: Duration) -> Vec<u8> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::stdin()
            .lock()
            .take(1024 * 1024)
            .read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx.recv_timeout(timeout).unwrap_or_default()
}

pub fn read_stdin(timeout: Duration) -> Payload {
    let buf = read_bytes(timeout);
    let payload = parse(&buf);
    if !payload.cwd.is_empty() && !payload.session_id.is_empty() {
        let cache = crate::paths::cache_dir().join(format!(
            "preview-{}.json",
            crate::paths::short_hash(&payload.cwd)
        ));
        if std::fs::read(&cache).ok().as_deref() != Some(&buf[..]) {
            let _ = crate::paths::write_atomic(&cache, &buf);
        }
    }
    payload
}

/// Strip terminal controls from external labels; only the renderer emits ANSI.
pub fn label(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

pub fn parse(buf: &[u8]) -> Payload {
    let Ok(v) = serde_json::from_slice::<Value>(buf) else {
        return Payload::default();
    };
    let s = |ptr: &str| {
        v.pointer(ptr)
            .and_then(Value::as_str)
            .map(label)
            .unwrap_or_default()
    };
    let n = |ptr: &str| v.pointer(ptr).and_then(Value::as_u64).unwrap_or(0);
    let context_tokens = n("/context_window/current_usage/input_tokens")
        .saturating_add(n(
            "/context_window/current_usage/cache_creation_input_tokens",
        ))
        .saturating_add(n("/context_window/current_usage/cache_read_input_tokens"));
    let max_context_tokens = n("/context_window/context_window_size");
    let context_usage = v
        .pointer("/context_window/used_percentage")
        .and_then(Value::as_f64)
        .map(|p| p / 100.0)
        .unwrap_or_else(|| context_tokens as f64 / max_context_tokens.max(1) as f64);
    let mut modes = Vec::new();
    for ptr in ["/vim/mode", "/agent/name"] {
        let value = s(ptr);
        if !value.is_empty() {
            modes.push(value);
        }
    }
    if v.get("fast_mode").and_then(Value::as_bool) == Some(true) {
        modes.push("fast".into());
    }
    let raw_path = |ptr: &str| {
        v.pointer(ptr)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let cwd = raw_path("/workspace/current_dir");
    let model = s("/model/display_name");
    Payload {
        model: if model.is_empty() {
            s("/model/id")
        } else {
            model
        },
        cwd: if cwd.is_empty() {
            raw_path("/cwd")
        } else {
            cwd
        },
        session_id: s("/session_id"),
        transcript_path: raw_path("/transcript_path"),
        git_branch: None,
        context_tokens,
        max_context_tokens,
        context_usage,
        effort: v
            .pointer("/effort/level")
            .and_then(Value::as_str)
            .map(|s| Value::String(label(s)))
            .or_else(|| v.pointer("/thinking/enabled").cloned()),
        mode: modes.join(" "),
        cost_usd: v
            .pointer("/cost/total_cost_usd")
            .and_then(Value::as_f64)
            .filter(|v| *v >= 0.0),
        duration_ms: v.pointer("/cost/total_duration_ms").and_then(Value::as_u64),
        is_preview: false,
        lines_added: n("/cost/total_lines_added"),
        lines_removed: n("/cost/total_lines_removed"),
        output_style: s("/output_style/name"),
        quota: v
            .get("rate_limits")
            .filter(|r| r.is_object())
            .map(crate::quota::from_payload),
        pr: v.get("pr").and_then(|pr| {
            Some(crate::probe::PullRequest {
                number: pr.get("number")?.as_u64()?,
                url: pr.get("url")?.as_str().map(label)?,
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nullable_context_and_preferred_workspace() {
        let p = parse(br#"{"cwd":"/old","workspace":{"current_dir":"/new"},"model":{"id":"claude-test"},"context_window":{"context_window_size":1000000,"current_usage":null,"used_percentage":null}}"#);
        assert_eq!(p.cwd, "/new");
        assert_eq!(p.model, "claude-test");
        assert_eq!(p.context_usage, 0.0);
        let p = parse(br#"{"context_window":{"context_window_size":200000,"current_usage":{"input_tokens":1000,"cache_read_input_tokens":18000,"cache_creation_input_tokens":1000,"output_tokens":9000}}}"#);
        assert_eq!(p.context_tokens, 20000);
        assert_eq!(p.context_usage, 0.1);
    }
}
