//! Incremental Claude JSONL accounting, isolated by transcript path and session ID.
//! Split assistant messages share message.id; retain the largest usage snapshot
//! for each ID instead of charging every content block as another API request.
use crate::paths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    pub input_other: u64,
    pub output: u64,
    pub input_cache_read: u64,
    pub input_cache_creation: u64,
}
impl Usage {
    pub fn input(&self) -> u64 {
        self.input_other
            .saturating_add(self.input_cache_read)
            .saturating_add(self.input_cache_creation)
    }
    pub fn add(&mut self, other: &Self) {
        self.input_other = self.input_other.saturating_add(other.input_other);
        self.output = self.output.saturating_add(other.output);
        self.input_cache_read = self.input_cache_read.saturating_add(other.input_cache_read);
        self.input_cache_creation = self
            .input_cache_creation
            .saturating_add(other.input_cache_creation);
    }
    fn merge(&mut self, other: Self) {
        self.input_other = self.input_other.max(other.input_other);
        self.output = self.output.max(other.output);
        self.input_cache_read = self.input_cache_read.max(other.input_cache_read);
        self.input_cache_creation = self.input_cache_creation.max(other.input_cache_creation);
    }
    pub fn cache_rate(&self) -> Option<f64> {
        (self.input() > 0).then(|| self.input_cache_read as f64 / self.input() as f64 * 100.0)
    }
    pub fn is_empty(&self) -> bool {
        self.input() == 0 && self.output == 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub total: Usage,
    pub sub_by_model: BTreeMap<String, Usage>,
    pub created: Option<f64>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    offset: u64,
    skipping: bool,
    identity: String,
    prefix: Vec<u8>,
    created: Option<f64>,
    messages: BTreeMap<String, (String, Usage)>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    files: BTreeMap<String, Cursor>,
}

fn identity(meta: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        format!("{:?}", meta.created().ok())
    }
}

/// At most 4 MiB per file per refresh. Partial records remain unconsumed;
/// oversized tool-result records are skipped without allocating their full size.
fn advance(path: &Path, cursor: &mut Cursor, session_id: &str) {
    let Ok(mut f) = File::open(path) else { return };
    let Ok(meta) = f.metadata() else { return };
    let id = identity(&meta);
    let mut prefix = vec![0; cursor.prefix.len()];
    if f.read_exact(&mut prefix).is_err() {
        prefix.clear();
    }
    if id != cursor.identity || meta.len() < cursor.offset || prefix != cursor.prefix {
        *cursor = Cursor {
            identity: id,
            ..Default::default()
        };
    }
    if cursor.prefix.is_empty() {
        let _ = f.seek(SeekFrom::Start(0));
        let _ = (&mut f).take(256).read_to_end(&mut cursor.prefix);
    }
    if f.seek(SeekFrom::Start(cursor.offset)).is_err() {
        return;
    }
    let mut buf = Vec::new();
    if f.take(4 * 1024 * 1024).read_to_end(&mut buf).is_err() {
        return;
    }
    let mut consumed = 0;
    for (at, byte) in buf.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let line = &buf[consumed..at];
        if !cursor.skipping {
            record(line, cursor, session_id);
        }
        cursor.skipping = false;
        consumed = at + 1;
    }
    if consumed == 0 && buf.len() == 4 * 1024 * 1024 {
        cursor.skipping = true;
        consumed = buf.len();
    }
    cursor.offset += consumed as u64;
}

fn record(line: &[u8], cursor: &mut Cursor, session_id: &str) {
    let Ok(v) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    if v.get("sessionId")
        .and_then(Value::as_str)
        .is_some_and(|id| id != session_id)
    {
        return;
    }
    if let Some(time) = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    {
        let time = time.timestamp_millis() as f64 / 1000.0;
        cursor.created = Some(cursor.created.map_or(time, |old| old.min(time)));
    }
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let Some(m) = v.get("message") else { return };
    let Some(u) = m.get("usage").filter(|u| u.is_object()) else {
        return;
    };
    let Some(id) = m
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| v.get("uuid").and_then(Value::as_str))
    else {
        return;
    };
    let n = |key| u.get(key).and_then(Value::as_u64).unwrap_or(0);
    let usage = Usage {
        input_other: n("input_tokens"),
        output: n("output_tokens"),
        input_cache_read: n("cache_read_input_tokens"),
        input_cache_creation: n("cache_creation_input_tokens"),
    };
    let model = m.get("model").and_then(Value::as_str).unwrap_or("Claude");
    let entry = cursor
        .messages
        .entry(id.to_string())
        .or_insert_with(|| (crate::payload::label(model), Usage::default()));
    entry.1.merge(usage);
}

pub fn collect(transcript: &str, session_id: &str) -> Option<SessionStats> {
    if transcript.is_empty() || session_id.is_empty() {
        return None;
    }
    let path = Path::new(transcript);
    if !path.is_file() {
        return None;
    }
    let key = paths::short_hash(&format!("{transcript}\0{session_id}"));
    let cache_path = paths::cache_dir().join(format!("session-v1-{key}.json"));
    let mut cache: Cache = std::fs::read(&cache_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut files = vec![path.to_path_buf()];
    // The subagent directory belongs to this transcript, never the latest session.
    let subdir = path.with_extension("").join("subagents");
    if let Ok(entries) = std::fs::read_dir(subdir) {
        files.extend(
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "jsonl")),
        );
    }
    let start = Instant::now();
    for file in &files {
        if start.elapsed() > Duration::from_millis(100) {
            break;
        }
        advance(
            file,
            cache
                .files
                .entry(file.to_string_lossy().into_owned())
                .or_default(),
            session_id,
        );
    }
    cache
        .files
        .retain(|name, _| files.iter().any(|p| p.to_string_lossy() == *name));
    let mut stats = SessionStats::default();
    let mut requests: BTreeMap<&str, (bool, &str, Usage)> = BTreeMap::new();
    for (file, cursor) in &cache.files {
        if file == transcript {
            stats.created = cursor.created;
        }
        for (id, (model, usage)) in &cursor.messages {
            // Forked agents may carry copies of parent conversation messages.
            // A shared API message ID still represents only one billed request.
            let entry = requests
                .entry(id)
                .or_insert((file != transcript, model, Usage::default()));
            entry.2.merge(*usage);
            if file == transcript {
                entry.0 = false;
            }
        }
    }
    for (subagent, model, usage) in requests.values() {
        stats.total.add(usage);
        if *subagent {
            stats
                .sub_by_model
                .entry((*model).to_string())
                .or_default()
                .add(usage);
        }
    }
    if let Ok(data) = serde_json::to_vec(&cache) {
        let _ = paths::write_atomic(&cache_path, &data);
    }
    Some(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_messages_and_updates_are_not_double_counted() {
        let mut c = Cursor::default();
        for output in [10, 20, 10] {
            let line = serde_json::json!({"type":"assistant","sessionId":"one",
                "message":{"id":"msg-1","model":"Sonnet","usage":{"input_tokens":100,"output_tokens":output}}});
            record(line.to_string().as_bytes(), &mut c, "one");
        }
        assert_eq!(c.messages.len(), 1);
        assert_eq!(c.messages["msg-1"].1.output, 20);
        assert_eq!(c.messages["msg-1"].1.input(), 100);
        record(br#"{"type":"assistant","sessionId":"other","message":{"id":"msg-2","usage":{"input_tokens":999}}}"#, &mut c, "one");
        assert_eq!(c.messages.len(), 1);
    }
}
