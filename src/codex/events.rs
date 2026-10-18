//! Tolerant event tails and incremental JSONL reads.

use super::Store;
use serde_json::Value;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;
use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub(super) struct Cursor {
    offset: u64,
    pending: Vec<u8>,
}

impl Cursor {
    pub fn read(&mut self, path: &Path, ended: bool) -> io::Result<Vec<String>> {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        if file.metadata()?.len() < self.offset {
            self.offset = 0;
            self.pending.clear();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        self.offset += file.read_to_end(&mut self.pending)? as u64;
        let end = if ended {
            self.pending.len()
        } else {
            self.pending
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |n| n + 1)
        };
        let lines = String::from_utf8_lossy(&self.pending[..end])
            .lines()
            .map(str::to_owned)
            .collect();
        self.pending.drain(..end);
        Ok(lines)
    }
}

pub(super) fn tail(path: &Path, n: usize) -> io::Result<Vec<String>> {
    tail_snapshot(path, n, false).map(|(lines, _)| lines)
}

fn tail_snapshot(path: &Path, n: usize, follow: bool) -> io::Result<(Vec<String>, Cursor)> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Cursor::default()))
        }
        Err(e) => return Err(e),
    };
    let mut start = file.metadata()?.len();
    let offset = start;
    let mut bytes = Vec::new();
    let mut newlines = 0;
    while start > 0 && newlines <= n {
        let length = start.min(8192);
        start -= length;
        file.seek(SeekFrom::Start(start))?;
        let mut chunk = vec![0; length as usize];
        file.read_exact(&mut chunk)?;
        newlines += chunk.iter().filter(|b| **b == b'\n').count();
        chunk.extend(bytes);
        bytes = chunk;
    }
    let end = if follow {
        bytes.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1)
    } else {
        bytes.len()
    };
    let text = String::from_utf8_lossy(&bytes[..end]);
    let lines: Vec<_> = text.lines().collect();
    let lines = lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|s| (*s).into())
        .collect();
    Ok((
        lines,
        Cursor {
            offset,
            pending: bytes[end..].to_vec(),
        },
    ))
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(items) => {
            let s = items.iter().filter_map(text).collect::<Vec<_>>().join(" ");
            (!s.is_empty()).then_some(s)
        }
        Value::Object(_) => [
            "message",
            "text",
            "command",
            "content",
            "aggregated_output",
            "item",
            "error",
        ]
        .into_iter()
        .find_map(|key| value.get(key).and_then(text)),
        _ => None,
    }
}

fn format_event(line: &str, width: usize) -> String {
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        return line.to_owned();
    };
    // Codex JSONL currently has no timestamps; use observation time as a fallback.
    let time = event
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| chrono::Local::now().format("%H:%M:%S").to_string());
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("event");
    let detail = text(&event).unwrap_or_default();
    let line = format!(
        "{time} {kind} {}",
        detail.split_whitespace().collect::<Vec<_>>().join(" ")
    );
    let mut used = 0;
    line.chars()
        .filter(|c| !c.is_control())
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= width
        })
        .collect()
}

pub(super) fn log(store: &Store, id: &str, n: usize, follow: bool) -> io::Result<()> {
    store.read(id)?;
    let path = store.dir(id).join("events.jsonl");
    let width = crate::probe::terminal_width().unwrap_or(160);
    let (lines, mut cursor) = tail_snapshot(&path, n, follow)?;
    let mut out = io::stdout().lock();
    for line in lines {
        writeln!(out, "{}", format_event(&line, width))?;
    }
    out.flush()?;
    if follow {
        loop {
            let ended = !store.read(id)?.effective_status().active();
            for line in cursor.read(&path, ended)? {
                writeln!(out, "{}", format_event(&line, width))?;
            }
            out.flush()?;
            if ended {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn follow_keeps_partial_lines_and_does_not_replay_the_tail() {
        let path = std::env::temp_dir().join(format!("cxj-events-{}", std::process::id()));
        std::fs::write(&path, "old\none\ntw").unwrap();
        let (lines, mut cursor) = tail_snapshot(&path, 1, true).unwrap();
        assert_eq!(lines, ["one"]);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"o\nthree\n").unwrap();
        assert_eq!(cursor.read(&path, false).unwrap(), ["two", "three"]);
        assert!(cursor.read(&path, false).unwrap().is_empty());
        std::fs::write(&path, "reset").unwrap();
        assert_eq!(cursor.read(&path, true).unwrap(), ["reset"]);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn real_event_shapes_and_unknown_lines() {
        let s = format_event(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"OK\nnext"}}"#,
            160,
        );
        assert!(s.ends_with("item.completed OK next"));
        let s = format_event(
            r#"{"type":"item.started","item":{"type":"command_execution","command":"date"}}"#,
            160,
        );
        assert!(s.ends_with("item.started date"));
        assert_eq!(format_event("raw line", 160), "raw line");
        assert!(
            format_event(r#"{"type":"unknown","content":"界界界"}"#, 12)
                .chars()
                .count()
                <= 12
        );
    }
}
