//! Install only statusLine in Claude settings, preserving all other JSON keys.
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
fn receipt_path() -> PathBuf {
    crate::config::config_dir().join("installed-command.json")
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

fn is_ours(cmd: &str) -> bool {
    if std::fs::read(receipt_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<String>(&b).ok())
        .as_deref()
        == Some(cmd)
    {
        return true;
    }
    matches!(cmd, "cc-statusline" | "cc-statusline.exe")
}

fn quote_command(exe: &str) -> String {
    // Claude executes statusLine through a shell, including Git Bash on Windows.
    format!("'{}'", exe.replace('\\', "/").replace('\'', "'\\''"))
}

pub fn install(command: Option<String>, force: bool) -> Result<Outcome, String> {
    let command = match command {
        Some(c) if !c.trim().is_empty() => c,
        Some(_) => return Err("The status line command cannot be empty".into()),
        None => quote_command(
            &std::env::current_exe()
                .map_err(|e| e.to_string())?
                .to_string_lossy(),
        ),
    };
    let (mut doc, old) = load()?;
    if let Some(existing) = doc.get("statusLine").filter(|v| !v.is_null()) {
        let existing_command = existing.get("command").and_then(Value::as_str);
        if existing_command == Some(&command)
            && existing.get("type").and_then(Value::as_str) == Some("command")
        {
            paths::write_atomic(&receipt_path(), &serde_json::to_vec(&command).unwrap())
                .map_err(|e| e.to_string())?;
            return Ok(Outcome {
                changed: false,
                command,
            });
        }
        if !force && !existing_command.is_some_and(is_ours) {
            return Err(
                "settings.json already has another statusLine; use install --force to replace it"
                    .into(),
            );
        }
    }
    doc["statusLine"] = json!({"type":"command","command":command,"padding":0});
    save(&doc, old.as_deref())?;
    paths::write_atomic(&receipt_path(), &serde_json::to_vec(&command).unwrap())
        .map_err(|e| e.to_string())?;
    Ok(Outcome {
        changed: true,
        command,
    })
}

pub fn uninstall() -> Result<bool, String> {
    let (mut doc, old) = load()?;
    if !doc
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
        .is_some_and(is_ours)
    {
        return Ok(false);
    }
    doc.as_object_mut().unwrap().remove("statusLine");
    save(&doc, old.as_deref())?;
    let _ = std::fs::remove_file(receipt_path());
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
