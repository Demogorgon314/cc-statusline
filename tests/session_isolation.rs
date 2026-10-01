use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(segments: &[&str]) -> Self {
        let home = std::env::temp_dir().join(format!(
            "cc-statusline-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(home.join("cc-statusline")).unwrap();
        let config = segments
            .iter()
            .map(|id| format!("[[segments]]\nid = '{id}'\n"))
            .collect::<String>();
        std::fs::write(home.join("cc-statusline/config.toml"), config).unwrap();
        Self(home)
    }
    fn run(&self, args: &[&str], payload: &Value) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cc-statusline"))
            .args(args)
            .env("CLAUDE_CONFIG_DIR", &self.0)
            .env("CC_STATUSLINE_NO_COLOR", "1")
            .env("COLUMNS", "1000")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        write!(child.stdin.take().unwrap(), "{payload}").unwrap();
        child.wait_with_output().unwrap()
    }
    fn text(&self, args: &[&str], payload: &Value) -> String {
        let out = self.run(args, payload);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }
    fn transcript(&self, id: &str) -> PathBuf {
        let dir = self.0.join("projects/demo");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{id}.jsonl"))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn message(session: &str, id: &str, output: u64) -> String {
    format!(
        "{}\n",
        json!({"type":"assistant","sessionId":session,"timestamp":"2026-01-01T00:00:00Z",
        "message":{"id":id,"model":"Sonnet","usage":{"input_tokens":100,"output_tokens":output,"cache_read_input_tokens":900}}})
    )
}

fn append(path: &std::path::Path, text: &str) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[test]
fn tps_can_measure_the_first_request_after_an_empty_transcript() {
    let f = Fixture::new(&["tps"]);
    let path = f.transcript("new");
    std::fs::write(&path, "").unwrap();
    let mut p = json!({"session_id":"new","transcript_path":path,
        "cost":{"total_api_duration_ms":0}});
    assert_eq!(f.text(&[], &p), "");
    append(&path, &message("new", "first", 420));
    p["cost"]["total_api_duration_ms"] = json!(10000);
    assert_eq!(f.text(&[], &p), "≈42 tok/s");
}

#[test]
fn tps_uses_session_deltas_including_deduplicated_subagents() {
    let f = Fixture::new(&["tps"]);
    let path = f.transcript("one");
    std::fs::write(&path, message("one", "main-1", 100)).unwrap();
    let agents = path.with_extension("").join("subagents");
    std::fs::create_dir_all(&agents).unwrap();
    let agent = agents.join("agent-a.jsonl");
    std::fs::write(
        &agent,
        message("one", "main-1", 100) + &message("one", "sub-1", 50),
    )
    .unwrap();
    let mut p = json!({"session_id":"one","transcript_path":path,"cwd":"/work/tps",
        "model":{"display_name":"Opus"},
        "cost":{"total_api_duration_ms":1000},
        "context_window":{"total_output_tokens":999999}});
    assert_eq!(
        f.text(&[], &p),
        "",
        "first complete observation is a baseline"
    );
    append(&path, &message("one", "main-2", 220));
    append(&agent, &message("one", "sub-2", 200));
    assert_eq!(
        f.text(&[], &p),
        "",
        "wait for the matching API-time increment"
    );
    p["cost"]["total_api_duration_ms"] = json!(11000);
    assert_eq!(f.text(&[], &p), "≈42 tok/s");
    append(&path, &message("one", "main-2", 220));
    assert_eq!(
        f.text(&[], &p),
        "≈42 tok/s",
        "duplicate records and idle keep the last sample"
    );
    assert_eq!(
        f.text(&["preview", "--cwd", "/work/tps"], &json!({})),
        "≈42 tok/s"
    );

    let other = json!({"session_id":"other","transcript_path":f.transcript("other"),
        "cost":{"total_api_duration_ms":5000}});
    assert_eq!(
        f.text(&[], &other),
        "",
        "new sessions cannot inherit a cached speed"
    );
    p["cost"]["total_api_duration_ms"] = json!(100);
    assert_eq!(f.text(&[], &p), "", "a resumed/reset timer rebaselines");
    append(&path, &message("one", "main-3", 100));
    p["cost"]["total_api_duration_ms"] = json!(2100);
    assert_eq!(f.text(&[], &p), "≈50 tok/s");

    p["cost"] = json!({});
    assert_eq!(
        f.text(&[], &p),
        "",
        "missing timing data hides the estimate"
    );
    p["cost"]["total_api_duration_ms"] = json!(2100);
    assert_eq!(f.text(&[], &p), "", "restored timing starts a new baseline");
}

#[test]
fn tps_rebaselines_after_partial_records_and_historical_catchup() {
    let f = Fixture::new(&["tps"]);
    let path = f.transcript("one");
    std::fs::write(&path, message("one", "m1", 100)).unwrap();
    let mut p =
        json!({"session_id":"one","transcript_path":path,"cost":{"total_api_duration_ms":1000}});
    assert_eq!(f.text(&[], &p), "");
    let next = message("one", "m2", 420);
    append(&path, next.trim_end());
    p["cost"]["total_api_duration_ms"] = json!(11000);
    assert_eq!(
        f.text(&[], &p),
        "",
        "partially written transcripts are not synchronized samples"
    );
    append(&path, "\n");
    assert_eq!(f.text(&[], &p), "", "EOF catch-up establishes a baseline");
    append(&path, &message("one", "m3", 420));
    p["cost"]["total_api_duration_ms"] = json!(21000);
    assert_eq!(f.text(&[], &p), "≈42 tok/s");

    let oversized = format!(
        "{{\"type\":\"user\",\"content\":\"{}\"}}\n",
        "x".repeat(4 * 1024 * 1024)
    );
    append(&path, &oversized);
    append(&path, &message("one", "m4", 9000));
    p["cost"]["total_api_duration_ms"] = json!(22000);
    for _ in 0..5 {
        assert_eq!(
            f.text(&[], &p),
            "",
            "historical catch-up cannot become a TPS spike"
        );
    }
    append(&path, &message("one", "m5", 100));
    p["cost"]["total_api_duration_ms"] = json!(24000);
    assert_eq!(f.text(&[], &p), "≈50 tok/s");

    // Even a replacement with a higher token total must not be treated as new output.
    std::fs::write(&path, message("one", "replacement", 50000)).unwrap();
    p["cost"]["total_api_duration_ms"] = json!(25000);
    assert_eq!(f.text(&[], &p), "");
}
#[test]
fn session_isolation_resume_and_preview() {
    let f = Fixture::new(&["usage", "subagent"]);
    let path = f.transcript("old");
    std::fs::write(&path, message("old", "m1", 420)).unwrap();
    let p = json!({"session_id":"old","transcript_path":path,"cwd":"/work/demo"});
    let old = f.text(&[], &p);
    assert!(old.contains("420") && old.contains("90%"), "{old}");
    assert_eq!(f.text(&[], &p), old);
    for id in ["", "new"] {
        let p = json!({"session_id":id,"transcript_path":f.transcript(id)});
        assert_eq!(f.text(&[], &p), "");
    }
    assert_eq!(f.text(&["preview", "--cwd", "/work/demo"], &json!({})), old);
    assert_eq!(
        f.text(
            &["preview", "--cwd", "/work/demo", "--session", "missing"],
            &json!({})
        ),
        ""
    );
}
#[test]
fn partial_records_duplicate_messages_and_truncation() {
    let f = Fixture::new(&["usage"]);
    let path = f.transcript("one");
    let p = json!({"session_id":"one","transcript_path":path});
    std::fs::write(&path, message("one", "m1", 10)).unwrap();
    assert!(f.text(&[], &p).contains("↓ 10"));
    let line = message("one", "m1", 20);
    let cut = line.len() / 2;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(&line.as_bytes()[..cut]).unwrap();
    assert!(f.text(&[], &p).contains("↓ 10"));
    file.write_all(&line.as_bytes()[cut..]).unwrap();
    let updated = f.text(&[], &p);
    assert!(
        updated.contains("↓ 20") && updated.contains("↑ 1.0k"),
        "{updated}"
    );
    drop(file);
    std::fs::write(&path, message("one", "m2", 7)).unwrap();
    let reset = f.text(&[], &p);
    assert!(reset.contains("↓ 7") && !reset.contains("↓ 20"), "{reset}");
}
#[test]
fn subagents_are_scoped_to_parent_transcript() {
    let f = Fixture::new(&["usage", "subagent"]);
    let path = f.transcript("one");
    std::fs::write(&path, message("one", "main", 100)).unwrap();
    let agents = path.with_extension("").join("subagents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("agent-a.jsonl"),
        message("one", "main", 100) + &message("one", "sub", 200),
    )
    .unwrap();
    std::fs::write(
        f.transcript("unrelated"),
        message("unrelated", "other", 999),
    )
    .unwrap();
    let text = f.text(&[], &json!({"session_id":"one","transcript_path":path}));
    assert!(
        text.contains("↓ 300") && text.contains("Sonnet ↑ 1.0k") && !text.contains("999"),
        "{text}"
    );
}
#[test]
fn native_fields_render_without_credentials_or_transcript() {
    let f = Fixture::new(&[
        "model",
        "context",
        "cost",
        "session",
        "quota",
        "output_style",
        "changes",
        "mode",
    ]);
    let text = f.text(&[], &json!({
        "model":{"display_name":"Opus 5.5 (1M context)"},"effort":{"level":"high"},
        "context_window":{"context_window_size":1000000,"used_percentage":12.5,
            "current_usage":{"input_tokens":25000,"cache_read_input_tokens":100000,"output_tokens":10000}},
        "cost":{"total_cost_usd":1.23,"total_duration_ms":65000,"total_lines_added":10,"total_lines_removed":2},
        "rate_limits":{"five_hour":{"used_percentage":42},"seven_day":{"used_percentage":13},"spend_limit":{"used_percentage":123}},
        "output_style":{"name":"Explanatory"},"vim":{"mode":"NORMAL"},"fast_mode":true
    }));
    for expected in [
        "Opus 5.5 [1M] high",
        "ctx 13% (125.0k/1.00M)",
        "$1.23",
        "1m",
        "5h 42%",
        "7d 13%",
        "spend 123%",
        "Explanatory",
        "+10 -2",
        "NORMAL fast",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(!f.0.join("cc-statusline-cache/quota.lock").exists());
}

#[test]
fn git_reports_changes_and_detached_head() {
    let f = Fixture::new(&["git"]);
    std::fs::write(
        f.0.join("cc-statusline/config.toml"),
        "[[segments]]\nid = 'git'\noptions = { pr = false }\n",
    )
    .unwrap();
    let repo = f.0.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&repo)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("file.txt"), "old\n").unwrap();
    git(&["add", "file.txt"]);
    git(&["commit", "-qm", "initial"]);
    std::fs::write(repo.join("file.txt"), "new\nextra\n").unwrap();
    let p = json!({"workspace":{"current_dir":repo}});
    let text = f.text(&[], &p);
    assert!(text.contains("main [+2 -1]"), "{text}");
    git(&["checkout", "-q", "--detach"]);
    let head = git(&["rev-parse", "--short", "HEAD"]);
    let text = f.text(&[], &p);
    assert!(text.starts_with(head.trim()), "{text}");
}

#[cfg(unix)]
#[test]
fn install_preserves_private_settings_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(&[]);
    let path = f.0.join("settings.json");
    std::fs::write(&path, "{}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    f.text(&["install"], &json!({}));
    for file in [path.clone(), path.with_extension("json.bak")] {
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn install_preserves_settings_and_uninstall_owns_only_its_command() {
    let f = Fixture::new(&[]);
    let path = f.0.join("settings.json");
    let original = br#"{"env":{"A":"B"},"permissions":{"allow":["Read"]},"statusLine":{"type":"command","command":"echo cc-statusline-other"}}"#;
    std::fs::write(&path, original).unwrap();
    assert!(!f.run(&["install"], &json!({})).status.success());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    f.text(&["uninstall"], &json!({}));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    f.text(
        &[
            "install",
            "--force",
            "--command",
            "custom-status-command --render",
        ],
        &json!({}),
    );
    assert_eq!(
        std::fs::read(path.with_extension("json.bak")).unwrap(),
        original
    );
    let installed: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(installed["env"]["A"], "B");
    assert_eq!(installed["permissions"]["allow"][0], "Read");
    assert_eq!(
        installed["statusLine"]["command"],
        "custom-status-command --render"
    );
    f.text(
        &["install", "--command", "custom-status-command --render"],
        &json!({}),
    );
    assert_eq!(
        std::fs::read(path.with_extension("json.bak")).unwrap(),
        original,
        "idempotent install preserves backup"
    );
    f.text(&["uninstall"], &json!({}));
    let removed: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(removed.get("statusLine").is_none());
    assert_eq!(removed["env"], installed["env"]);
}
#[test]
fn malformed_settings_are_never_overwritten() {
    let f = Fixture::new(&[]);
    let path = f.0.join("settings.json");
    for bytes in ["{broken", "[]", "null"] {
        std::fs::write(&path, bytes).unwrap();
        assert!(!f.run(&["install", "--force"], &json!({})).status.success());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    }
}
#[test]
fn themes_fit_narrow_widths_and_sanitize_labels() {
    let f = Fixture::new(&[]);
    let p = json!({"model":{"display_name":"Claude\nInjected\u{001b}x"},"cwd":"/中文/project",
        "context_window":{"context_window_size":200000,"used_percentage":50},"cost":{"total_cost_usd":0.12}});
    for theme in [
        "claude",
        "minimal",
        "cometix",
        "default",
        "nord",
        "gruvbox",
        "powerline-dark",
        "powerline-light",
        "powerline-rose-pine",
        "powerline-tokyo-night",
    ] {
        for width in [0, 1, 12, 40, 160] {
            let text = f.text(&["--theme", theme, "--width", &width.to_string()], &p);
            assert!(!text.contains(['\n', '\r', '\x1b']), "{theme}: {text:?}");
            assert!(
                unicode_width::UnicodeWidthStr::width(text.as_str()) <= width,
                "{theme} {width}: {text}"
            );
        }
    }
}
