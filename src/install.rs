//! Install the selected status line hook, preserving all other JSON keys.
use crate::paths;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct Outcome {
    pub changed: bool,
    pub command: String,
}

fn settings_path() -> PathBuf {
    paths::claude_home().join("settings.json")
}
fn receipt_path(hook: &str) -> PathBuf {
    crate::config::config_dir().join(if hook == "subagentStatusLine" {
        "installed-subagent-command.json"
    } else {
        "installed-command.json"
    })
}

fn load() -> Result<(Value, Option<Vec<u8>>), String> {
    let path = settings_path();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((json!({}), None)),
        Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
    };
    let doc: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid settings.json: {e}"))?;
    if !doc.is_object() {
        return Err("settings.json must contain a JSON object".into());
    }
    Ok((doc, Some(bytes)))
}

fn save(doc: &Value, old: Option<&[u8]>) -> Result<(), String> {
    let path = settings_path();
    if let Some(old) = old {
        paths::write_atomic(&path.with_extension("json.bak"), old).map_err(|e| e.to_string())?;
    }
    let mut data = serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())?;
    data.push(b'\n');
    paths::write_atomic(&path, &data).map_err(|e| e.to_string())
}

fn is_ours(cmd: &str, hook: &str) -> bool {
    if std::fs::read(receipt_path(hook))
        .ok()
        .and_then(|b| serde_json::from_slice::<String>(&b).ok())
        .as_deref()
        == Some(cmd)
    {
        return true;
    }
    if hook == "subagentStatusLine" {
        matches!(
            cmd,
            "cc-statusline subagents" | "cc-statusline.exe subagents"
        )
    } else {
        matches!(cmd, "cc-statusline" | "cc-statusline.exe")
    }
}

fn quote_command(exe: &str) -> String {
    // Claude executes statusLine through a shell, including Git Bash on Windows.
    format!("'{}'", exe.replace('\\', "/").replace('\'', "'\\''"))
}

pub fn install(command: Option<String>, force: bool) -> Result<Outcome, String> {
    install_hook("statusLine", command, force)
}

pub fn install_subagents(command: Option<String>, force: bool) -> Result<Outcome, String> {
    install_hook("subagentStatusLine", command, force)
}

fn install_hook(hook: &str, command: Option<String>, force: bool) -> Result<Outcome, String> {
    let command = match command {
        Some(c) if !c.trim().is_empty() => c,
        Some(_) => return Err("The status line command cannot be empty".into()),
        None => {
            quote_command(
                &std::env::current_exe()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy(),
            ) + if hook == "subagentStatusLine" {
                " subagents"
            } else {
                ""
            }
        }
    };
    let (mut doc, old) = load()?;
    if let Some(existing) = doc.get(hook).filter(|v| !v.is_null()) {
        let existing_command = existing.get("command").and_then(Value::as_str);
        if existing_command == Some(&command)
            && existing.get("type").and_then(Value::as_str) == Some("command")
            && (hook != "statusLine" || existing.get("refreshInterval").is_some())
        {
            paths::write_atomic(&receipt_path(hook), &serde_json::to_vec(&command).unwrap())
                .map_err(|e| e.to_string())?;
            return Ok(Outcome {
                changed: false,
                command,
            });
        }
        if !force
            && existing_command != Some(&command)
            && !existing_command.is_some_and(|cmd| is_ours(cmd, hook))
        {
            return Err(format!(
                "settings.json already has another {hook}; use install {}--force to replace it",
                if hook == "subagentStatusLine" {
                    "--subagents "
                } else {
                    ""
                }
            ));
        }
    }
    let mut settings = doc
        .get(hook)
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    settings["type"] = json!("command");
    settings["command"] = json!(command);
    if hook == "statusLine" {
        let object = settings.as_object_mut().unwrap();
        object.entry("padding").or_insert(json!(0));
        object.entry("refreshInterval").or_insert(json!(1));
    }
    doc[hook] = settings;
    save(&doc, old.as_deref())?;
    paths::write_atomic(&receipt_path(hook), &serde_json::to_vec(&command).unwrap())
        .map_err(|e| e.to_string())?;
    Ok(Outcome {
        changed: true,
        command,
    })
}

pub fn uninstall() -> Result<bool, String> {
    uninstall_hook("statusLine")
}

pub fn uninstall_subagents() -> Result<bool, String> {
    uninstall_hook("subagentStatusLine")
}

fn uninstall_hook(hook: &str) -> Result<bool, String> {
    let (mut doc, old) = load()?;
    if !doc
        .get(hook)
        .and_then(|value| value.get("command"))
        .and_then(Value::as_str)
        .is_some_and(|cmd| is_ours(cmd, hook))
    {
        return Ok(false);
    }
    doc.as_object_mut().unwrap().remove(hook);
    save(&doc, old.as_deref())?;
    let _ = std::fs::remove_file(receipt_path(hook));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn executable_paths_are_shell_quoted() {
        assert_eq!(
            quote_command("/tmp/a $b/'cc-statusline"),
            "'/tmp/a $b/'\\''cc-statusline'"
        );
        assert_eq!(
            quote_command("C:\\Users\\A B\\cc-statusline.exe"),
            "'C:/Users/A B/cc-statusline.exe'"
        );
    }
}
