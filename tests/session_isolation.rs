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
        // Commands that ignore stdin (install, uninstall) may exit first.
        if let Err(e) = write!(child.stdin.take().unwrap(), "{payload}") {
            assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe, "{e}");
        }
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
fn subagent_rows_keep_individual_context_and_fit_the_panel() {
    let f = Fixture::new(&[]);
    let input = json!({"columns":120,"tasks":[
        {"id":"a","name":"Search\n\u{001b}","model":"claude-sonnet-4-6","status":"running","tokenCount":180000,"contextWindowSize":200000},
        {"id":"b","name":"测试","model":"claude-opus-4-6","status":"completed","tokenCount":20000,"contextWindowSize":1000000},
        {"id":"c","name":"Unresolved","status":"pending","tokenCount":0},
        {"name":"missing id"}
    ]});
    let rows: Vec<Value> = f
        .text(&["subagents"], &input)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["id"], "a");
    assert!(rows[0]["content"]
        .as_str()
        .unwrap()
        .contains("Sonnet 4.6 · running · ctx 90%"));
    assert!(rows[1]["content"]
        .as_str()
        .unwrap()
        .contains("completed · ctx 2%"));
    assert!(rows[2]["content"].as_str().unwrap().contains("ctx ?"));
    std::fs::write(
        f.0.join("cc-statusline/config.toml"),
        "[style]\nwidth=200\n",
    )
    .unwrap();
    let mut narrow = input.clone();
    narrow["columns"] = json!(12);
    for row in f.text(&["subagents"], &narrow).lines() {
        let row: Value = serde_json::from_str(row).unwrap();
        assert!(unicode_width::UnicodeWidthStr::width(row["content"].as_str().unwrap()) <= 12);
    }
    for width in [0, 1, 8, 24] {
        for line in f
            .text(&["subagents", "--width", &width.to_string()], &input)
            .lines()
        {
            let row: Value = serde_json::from_str(line).unwrap();
            let content = row["content"].as_str().unwrap();
            assert!(!content.chars().any(char::is_control));
            assert!(unicode_width::UnicodeWidthStr::width(content) <= width);
        }
    }
    assert_eq!(f.text(&["subagents"], &json!({"tasks":null})), "");
}

#[test]
fn subagent_panel_shows_per_agent_rates_and_degrades_uniformly() {
    let f = Fixture::new(&[]);
    let path = f.transcript("one");
    std::fs::write(&path, "").unwrap();
    let dir = path.with_extension("").join("subagents");
    std::fs::create_dir_all(&dir).unwrap();
    let now = chrono::Utc::now();
    let at = |secs: i64| (now - chrono::Duration::seconds(secs)).to_rfc3339();
    let user = json!({"type":"user","sessionId":"one","timestamp":at(10)});
    let reply = json!({"type":"assistant","sessionId":"one","timestamp":at(0),
        "message":{"id":"x","model":"claude-sonnet-4-6","usage":{"output_tokens":500}}});
    std::fs::write(dir.join("agent-a.jsonl"), format!("{user}\n{reply}\n")).unwrap();
    let rows = |columns: u64| -> Vec<String> {
        let input = json!({"session_id":"one","transcript_path":path,"columns":columns,"tasks":[
            {"id":"a","name":"Search","model":"claude-sonnet-4-6","status":"running","tokenCount":40000,"contextWindowSize":200000},
            {"id":"b","name":"Review","model":"claude-opus-4-6","status":"completed","tokenCount":20000,"contextWindowSize":1000000}
        ]});
        f.text(&["subagents"], &input)
            .lines()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["content"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    // ≈50 t/s, less the time this test takes to reach the renderer
    let rate = |row: &str| -> u64 {
        row.split('≈')
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let wide = rows(120);
    assert!((40..=50).contains(&rate(&wide[0])), "{wide:?}");
    assert!(
        wide[0].ends_with(" t/s · ctx 20% (40.0k/200.0k)") && wide[0].contains("running · ≈"),
        "{wide:?}"
    );
    assert!(
        !wide[1].contains("t/s"),
        "finished agents have no live rate: {wide:?}"
    );
    // Every row uses the same level, and the share survives the narrowest one.
    let narrow = rows(28);
    assert!(
        narrow[0].starts_with("Search · ≈") && narrow[0].ends_with(" t/s · 20%"),
        "{narrow:?}"
    );
    assert_eq!(narrow[1], "Review · 2%");
    for row in rows(9) {
        assert!(
            row.ends_with('%') && unicode_width::UnicodeWidthStr::width(row.as_str()) <= 9,
            "{row}"
        );
    }
}

#[test]
fn subagent_hook_installation_preserves_the_main_hook_and_other_owners() {
    let f = Fixture::new(&[]);
    let path = f.0.join("settings.json");
    let original = json!({"statusLine":{"type":"command","command":"main-other","refreshInterval":5},
        "subagentStatusLine":{"type":"command","command":"sub-other"},"env":{"keep":"yes"}});
    std::fs::write(&path, original.to_string()).unwrap();
    assert!(!f
        .run(&["install", "--subagents"], &json!({}))
        .status
        .success());
    f.text(&["uninstall", "--subagents"], &json!({}));
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
        original
    );
    f.text(&["install", "--subagents", "--force"], &json!({}));
    let installed: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(installed["statusLine"], original["statusLine"]);
    assert!(installed["subagentStatusLine"]["command"]
        .as_str()
        .unwrap()
        .ends_with(" subagents"));
    f.text(&["uninstall", "--subagents"], &json!({}));
    let removed: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(removed.get("subagentStatusLine").is_none());
    assert_eq!(removed["statusLine"], original["statusLine"]);
    assert_eq!(removed["env"], original["env"]);
}

#[test]
fn installation_adds_background_refresh_without_overwriting_user_preferences() {
    let f = Fixture::new(&[]);
    let path = f.0.join("settings.json");
    std::fs::write(
        &path,
        json!({"statusLine":{"type":"command","command":"cc-statusline","padding":3}}).to_string(),
    )
    .unwrap();
    f.text(&["install", "--command", "cc-statusline"], &json!({}));
    let mut installed: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(installed["statusLine"]["refreshInterval"], 1);
    assert_eq!(installed["statusLine"]["padding"], 3);
    installed["statusLine"]["refreshInterval"] = json!(5);
    std::fs::write(&path, installed.to_string()).unwrap();
    f.text(&["install", "--command", "cc-statusline"], &json!({}));
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
        installed
    );
}

#[test]
fn model_summary_is_ranked_and_main_context_is_independent() {
    let f = Fixture::new(&["context", "subagent"]);
    let path = f.transcript("one");
    std::fs::write(&path, "").unwrap();
    let dir = path.with_extension("").join("subagents");
    std::fs::create_dir_all(&dir).unwrap();
    for (id, model, input) in [
        ("a", "Sonnet", 300),
        ("b", "Opus", 200),
        ("c", "Haiku", 100),
    ] {
        let line = json!({"type":"assistant","sessionId":"one","message":{"id":id,"model":model,"usage":{"input_tokens":input,"output_tokens":10}}});
        std::fs::write(dir.join(format!("agent-{id}.jsonl")), format!("{line}\n")).unwrap();
    }
    let p = json!({"session_id":"one","transcript_path":path,"context_window":{"context_window_size":200000,"used_percentage":20,"current_usage":{"input_tokens":40000}}});
    let text = f.text(&[], &p);
    assert!(text.contains("ctx 20% · 40k/200k"), "{text}");
    assert!(text.find("Sonnet").unwrap() < text.find("Opus").unwrap());
    assert!(text.contains("+1") && !text.contains("Haiku"), "{text}");
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
    // An explicit width: the real terminal outranks COLUMNS when one exists.
    let text = f.text(&["--width", "1000"], &json!({
        "model":{"display_name":"Opus 5.5 (1M context)"},"effort":{"level":"high"},
        "context_window":{"context_window_size":1000000,"used_percentage":12.5,
            "current_usage":{"input_tokens":25000,"cache_read_input_tokens":100000,"output_tokens":10000}},
        "cost":{"total_cost_usd":1.23,"total_duration_ms":65000,"total_lines_added":10,"total_lines_removed":2},
        "rate_limits":{"five_hour":{"used_percentage":42},"seven_day":{"used_percentage":13},"spend_limit":{"used_percentage":123}},
        "output_style":{"name":"Explanatory"},"vim":{"mode":"NORMAL"},"fast_mode":true
    }));
    for expected in [
        "Opus 5.5 [1M] high",
        "ctx 13% · 125k/1M",
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
