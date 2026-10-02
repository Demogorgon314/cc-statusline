//! Incremental Claude JSONL accounting, isolated by transcript path and session ID.
//! Split assistant messages share message.id; retain the largest usage snapshot
//! for each ID instead of charging every content block as another API request.
use crate::{paths, payload::Payload, tps};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;
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
    pub state: CollectionState,
    pub tps: Option<tps::Estimate>,
    /// Logs (main or agent) with output inside the throughput window.
    pub active_logs: usize,
    /// Throughput per agent ID, from `agent-<id>.jsonl`.
    pub agent_tps: BTreeMap<String, tps::Estimate>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionState {
    #[default]
    Complete,
    CatchingUp,
    Partial,
}

#[derive(Serialize, Deserialize)]
struct Message {
    model: String,
    usage: Usage,
    /// Previous user/assistant record: the request cannot have started earlier.
    start: Option<f64>,
    /// Last logged content block.
    end: Option<f64>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    offset: u64,
    skipping: bool,
    identity: String,
    /// Unchanged size and mtime at EOF skip opening the file again.
    modified: Option<SystemTime>,
    prefix: Vec<u8>,
    created: Option<f64>,
    /// Latest user or assistant record time in this log.
    boundary: Option<f64>,
    messages: BTreeMap<String, Message>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    files: BTreeMap<String, Cursor>,
    next_file: Option<String>,
    state: CollectionState,
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
const READ_LIMIT: usize = 4 * 1024 * 1024;
/// Nested agent directories below `subagents/` to scan.
const MAX_AGENT_DEPTH: usize = 2;

/// `.../agent-<id>.jsonl` → `<id>`, the task ID in subagentStatusLine input.
fn agent_id(file: &str) -> Option<&str> {
    Path::new(file)
        .file_stem()?
        .to_str()?
        .strip_prefix("agent-")
}

fn advance(path: &Path, cursor: &mut Cursor, session_id: &str) -> CollectionState {
    // Finished agents stay in the directory; most refreshes need only a stat.
    let unchanged = |meta: &std::fs::Metadata| {
        meta.is_file()
            && !cursor.skipping
            && meta.len() == cursor.offset
            && meta.modified().ok() == cursor.modified
            && identity(meta) == cursor.identity
    };
    if std::fs::metadata(path).is_ok_and(|meta| unchanged(&meta)) {
        return CollectionState::Complete;
    }
    let Ok(mut f) = File::open(path) else {
        return CollectionState::Partial;
    };
    let Ok(meta) = f.metadata() else {
        return CollectionState::Partial;
    };
    if !meta.is_file() {
        return CollectionState::Partial;
    }
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
        return CollectionState::Partial;
    }
    let mut buf = Vec::new();
    let remaining = meta.len().saturating_sub(cursor.offset);
    if f.take(remaining.min(READ_LIMIT as u64))
        .read_to_end(&mut buf)
        .is_err()
    {
        return CollectionState::Partial;
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
    if consumed == 0 && buf.len() == READ_LIMIT {
        cursor.skipping = true;
        consumed = buf.len();
    }
    cursor.offset += consumed as u64;
    cursor.modified = meta.modified().ok();
    if cursor.offset == meta.len() && !cursor.skipping {
        CollectionState::Complete
    } else if remaining > READ_LIMIT as u64 || cursor.skipping {
        CollectionState::CatchingUp
    } else {
        CollectionState::Partial
    }
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
    let time = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|time| time.timestamp_millis() as f64 / 1000.0);
    if let Some(time) = time {
        cursor.created = Some(cursor.created.map_or(time, |old| old.min(time)));
    }
    let kind = v.get("type").and_then(Value::as_str);
    if kind == Some("user") {
        cursor.boundary = time.or(cursor.boundary);
    }
    if kind != Some("assistant") {
        return;
    }
    let previous = cursor.boundary;
    cursor.boundary = time.or(cursor.boundary);
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
        .or_insert_with(|| Message {
            model: crate::payload::label(model),
            usage: Usage::default(),
            start: previous.or(time),
            end: time,
        });
    entry.usage.merge(usage);
    entry.end = match (entry.end, time) {
        (Some(end), Some(time)) => Some(end.max(time)),
        (end, time) => end.or(time),
    };
}

pub fn collect(payload: &Payload, deadline: Instant, now: f64) -> Option<SessionStats> {
    let transcript = &payload.transcript_path;
    let session_id = &payload.session_id;
    if transcript.is_empty() || session_id.is_empty() {
        return None;
    }
    let key = paths::short_hash(&format!("{transcript}\0{session_id}"));
    let cache_path = paths::cache_dir().join(format!("session-v3-{key}.json"));
    collect_cached(payload, &cache_path, deadline, now)
}

fn collect_cached(
    payload: &Payload,
    cache_path: &Path,
    deadline: Instant,
    now: f64,
) -> Option<SessionStats> {
    let path = Path::new(&payload.transcript_path);
    if !path.is_file() && !cache_path.is_file() {
        return None;
    }
    // The OS releases the lock even if Claude cancels the process. A competing
    // renderer uses the last snapshot instead of racing a read-modify-write.
    let lock = if payload.is_preview {
        None
    } else {
        cache_path
            .parent()
            .and_then(|dir| std::fs::create_dir_all(dir).ok())
            .and_then(|_| {
                File::options()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(cache_path.with_extension("lock"))
                    .ok()
            })
            .filter(|file| file.try_lock().is_ok())
    };
    let mut cache: Cache = std::fs::read(cache_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // The lock holder persists its collection state with the snapshot.
    if payload.is_preview || lock.is_none() {
        return Some(cache.stats(&payload.transcript_path, now));
    }
    let started = Instant::now();
    let changed = cache.refresh(path, &payload.session_id, || {
        started.elapsed() < Duration::from_millis(100) && Instant::now() < deadline
    });
    if changed {
        if let Ok(data) = serde_json::to_vec(&cache) {
            let _ = paths::write_atomic(cache_path, &data);
        }
    }
    Some(cache.stats(&payload.transcript_path, now))
}

impl Cache {
    fn refresh(
        &mut self,
        path: &Path,
        session_id: &str,
        mut can_read: impl FnMut() -> bool,
    ) -> bool {
        let before = (self.next_file.clone(), self.state);
        let mut changed = false;
        let mut files = BTreeSet::from([path.to_string_lossy().into_owned()]);
        // The subagent directory belongs to this transcript, never the latest session.
        // Workflow agents may write into nested per-run subdirectories.
        let mut dirs = vec![(path.with_extension("").join("subagents"), 0)];
        let mut directory_complete = true;
        while let Some((dir, depth)) = dirs.pop() {
            match std::fs::read_dir(&dir) {
                Ok(entries) => {
                    for entry in entries {
                        let Ok(e) = entry else {
                            directory_complete = false;
                            continue;
                        };
                        let path = e.path();
                        if path.extension().is_some_and(|e| e == "jsonl") {
                            files.insert(path.to_string_lossy().into_owned());
                        } else if depth < MAX_AGENT_DEPTH && e.file_type().is_ok_and(|t| t.is_dir())
                        {
                            dirs.push((path, depth + 1));
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => directory_complete = false,
            }
        }
        let known = self.files.len();
        if directory_complete {
            self.files.retain(|name, _| files.contains(name));
        } else {
            // A failed directory listing does not mean those agents disappeared.
            files.extend(self.files.keys().cloned());
        }
        changed |= known != self.files.len();
        let mut files: Vec<_> = files.into_iter().collect();
        let start = self
            .next_file
            .as_ref()
            .map_or(0, |next| files.partition_point(|name| name < next));
        let count = files.len();
        files.rotate_left(start % count);
        self.state = if directory_complete {
            CollectionState::Complete
        } else {
            CollectionState::Partial
        };
        for file in &files {
            if !can_read() {
                self.next_file = Some(file.clone());
                if self.state == CollectionState::Complete {
                    self.state = CollectionState::CatchingUp;
                }
                break;
            }
            let cursor = self.files.entry(file.clone()).or_default();
            let before = (
                cursor.offset,
                cursor.identity.clone(),
                cursor.prefix.len(),
                cursor.modified,
            );
            let state = advance(Path::new(file), cursor, session_id);
            changed |= before
                != (
                    cursor.offset,
                    cursor.identity.clone(),
                    cursor.prefix.len(),
                    cursor.modified,
                );
            match state {
                CollectionState::Partial => self.state = state,
                CollectionState::CatchingUp if self.state == CollectionState::Complete => {
                    self.state = state
                }
                _ => {}
            }
        }
        changed || before != (self.next_file.clone(), self.state)
    }

    fn stats(&self, transcript: &str, now: f64) -> SessionStats {
        let mut stats = SessionStats {
            state: self.state,
            ..Default::default()
        };
        struct Request<'a> {
            subagent: bool,
            model: &'a str,
            usage: Usage,
            interval: Option<(f64, f64)>,
            /// Log holding the earliest copy, which made the request.
            owner: &'a str,
        }
        let mut requests: BTreeMap<&str, Request> = BTreeMap::new();
        for (file, cursor) in &self.files {
            if file == transcript {
                stats.created = cursor.created;
            }
            for (id, message) in &cursor.messages {
                // Forked agents may carry copies of parent conversation messages.
                // A shared API message ID still represents only one billed request.
                let entry = requests.entry(id).or_insert(Request {
                    subagent: file != transcript,
                    model: &message.model,
                    usage: Usage::default(),
                    interval: None,
                    owner: file,
                });
                entry.usage.merge(message.usage);
                if file == transcript {
                    entry.subagent = false;
                }
                // The earliest copy is the original request.
                if let (Some(start), Some(end)) = (message.start, message.end) {
                    if entry.interval.is_none_or(|(_, old)| end < old) {
                        entry.interval = Some((start.min(end), end));
                        entry.owner = file;
                    }
                }
            }
        }
        let mut by_log: BTreeMap<&str, Vec<tps::Span>> = BTreeMap::new();
        for r in requests.values() {
            if let Some((start, end)) = r.interval {
                by_log.entry(r.owner).or_default().push(tps::Span {
                    start,
                    end,
                    output: r.usage.output,
                });
            }
        }
        stats.tps = tps::estimate(by_log.values().flatten().copied(), now);
        for (file, spans) in &by_log {
            let Some(estimate) = tps::estimate(spans.iter().copied(), now) else {
                continue;
            };
            if tps::is_active(&estimate, now) {
                stats.active_logs += 1;
            }
            if let Some(id) = agent_id(file).filter(|_| *file != transcript) {
                stats.agent_tps.insert(id.to_string(), estimate);
            }
        }
        for r in requests.values() {
            stats.total.add(&r.usage);
            if r.subagent {
                stats
                    .sub_by_model
                    .entry(r.model.to_string())
                    .or_default()
                    .add(&r.usage);
            }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        main: PathBuf,
        agents: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "cc-sampling-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let main = root.join("one.jsonl");
            let agents = root.join("one/subagents");
            std::fs::create_dir_all(&agents).unwrap();
            std::fs::write(&main, "").unwrap();
            Self { root, main, agents }
        }

        fn refresh(&self, cache: &mut Cache, now: f64) -> SessionStats {
            cache.refresh(&self.main, "one", || true);
            cache.stats(self.main.to_str().unwrap(), now)
        }

        fn payload(&self) -> Payload {
            Payload {
                session_id: "one".into(),
                transcript_path: self.main.to_string_lossy().into_owned(),
                ..Default::default()
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Seconds since the epoch as Claude's RFC 3339 timestamp.
    fn at(secs: f64) -> String {
        chrono::DateTime::from_timestamp_millis((secs * 1000.0) as i64)
            .unwrap()
            .to_rfc3339()
    }

    fn user(secs: f64) -> String {
        format!(
            "{}\n",
            serde_json::json!({"type":"user","sessionId":"one","timestamp":at(secs)})
        )
    }

    fn message(id: &str, output: u64, secs: f64) -> String {
        format!(
            "{}\n",
            serde_json::json!({"type":"assistant","sessionId":"one","timestamp":at(secs),"message":{"id":id,"model":"Sonnet","usage":{"input_tokens":100,"output_tokens":output}}})
        )
    }

    fn append(path: &Path, text: &str) {
        File::options()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    fn rate(stats: &SessionStats) -> f64 {
        stats.tps.unwrap().tokens_per_sec
    }

    #[test]
    fn parallel_agents_are_timed_by_their_own_records() {
        let f = Fixture::new();
        let a = f.agents.join("a.jsonl");
        let b = f.agents.join("b.jsonl");
        let mut cache = Cache::default();
        assert!(f.refresh(&mut cache, 100.0).tps.is_none());
        // Agents discovered after they already logged output still count.
        std::fs::write(&a, user(100.0) + &message("a1", 300, 110.0)).unwrap();
        std::fs::write(&b, user(100.0) + &message("b1", 600, 120.0)).unwrap();
        // 900 tokens over the 20 s either agent was generating
        let stats = f.refresh(&mut cache, 130.0);
        assert_eq!(rate(&stats), 45.0);
        assert_eq!(stats.tps.unwrap().measured_at, 120.0);
        // Observing the same logs later never adds output.
        assert_eq!(rate(&f.refresh(&mut cache, 130.0)), 45.0);
        // Split blocks extend the request instead of counting as new output.
        append(&a, &message("a1", 330, 130.0));
        assert_eq!(rate(&f.refresh(&mut cache, 130.0)), 31.0);
        // Old history is outside the window; idle keeps the last rate.
        let old = f.agents.join("old.jsonl");
        std::fs::write(&old, user(0.0) + &message("old", 9000, 10.0)).unwrap();
        assert_eq!(rate(&f.refresh(&mut cache, 130.0)), 31.0);
        assert_eq!(rate(&f.refresh(&mut cache, 200.0)), 31.0);
        assert_eq!(rate(&f.refresh(&mut cache, 2000.0)), 31.0);
    }

    #[test]
    fn nested_agents_report_their_own_rates_and_parallelism() {
        let f = Fixture::new();
        let nested = f.agents.join("wf_run");
        std::fs::create_dir_all(&nested).unwrap();
        append(&f.main, &(user(100.0) + &message("m1", 300, 110.0)));
        std::fs::write(
            f.agents.join("agent-aa.jsonl"),
            user(100.0) + &message("a1", 600, 120.0),
        )
        .unwrap();
        std::fs::write(
            nested.join("agent-bb.jsonl"),
            user(0.0) + &message("b1", 900, 10.0),
        )
        .unwrap();
        let mut cache = Cache::default();
        let stats = f.refresh(&mut cache, 130.0);
        assert_eq!(stats.total.output, 1800);
        assert_eq!(stats.agent_tps["aa"].tokens_per_sec, 30.0);
        assert_eq!(stats.agent_tps["bb"].tokens_per_sec, 90.0);
        assert!(!stats.agent_tps.contains_key("one"));
        // main and aa produced output inside the window; bb finished long ago
        assert_eq!(stats.active_logs, 2);
    }

    #[test]
    fn forked_copies_keep_the_original_request_time() {
        let f = Fixture::new();
        append(&f.main, &(user(100.0) + &message("m1", 300, 110.0)));
        let fork = f.agents.join("fork.jsonl");
        std::fs::write(&fork, user(125.0) + &message("m1", 300, 126.0)).unwrap();
        let mut cache = Cache::default();
        let stats = f.refresh(&mut cache, 130.0);
        assert_eq!(stats.total.output, 300);
        assert_eq!(rate(&stats), 30.0);
        assert_eq!(stats.tps.unwrap().measured_at, 110.0);
    }

    #[test]
    fn partial_agent_keeps_complete_usage_from_other_logs() {
        let f = Fixture::new();
        let a = f.agents.join("a.jsonl");
        std::fs::write(&a, "").unwrap();
        let mut cache = Cache::default();
        append(&f.main, &message("main1", 420, 10.0));
        append(&a, message("a1", 42, 10.0).trim_end());
        append(&f.main, &message("main2", 42, 10.0));
        let partial = f.refresh(&mut cache, 11.0);
        assert_eq!(partial.state, CollectionState::Partial);
        assert_eq!(partial.total.output, 462);
        append(&a, "\n");
        let complete = f.refresh(&mut cache, 12.0);
        assert_eq!(complete.total.output, 504);
        assert_eq!(complete.state, CollectionState::Complete);
        // A temporarily unreadable file retains its usage.
        std::fs::remove_file(&a).unwrap();
        std::fs::create_dir(&a).unwrap();
        let unavailable = f.refresh(&mut cache, 13.0);
        assert_eq!(unavailable.state, CollectionState::Partial);
        assert_eq!(unavailable.total.output, 504);
    }

    #[test]
    fn backlog_catches_up_and_replacement_rebuilds() {
        let f = Fixture::new();
        let a = f.agents.join("history.jsonl");
        std::fs::write(
            &a,
            "x".repeat(READ_LIMIT) + "\n" + &message("old", 9000, 0.0),
        )
        .unwrap();
        let mut cache = Cache::default();
        let partial = f.refresh(&mut cache, 10.0);
        assert_eq!(partial.state, CollectionState::CatchingUp);
        let caught_up = f.refresh(&mut cache, 10.0);
        assert_eq!(caught_up.state, CollectionState::Complete);
        assert_eq!(caught_up.total.output, 9000);
        std::fs::write(&a, message("new", 42, 10.0)).unwrap();
        assert_eq!(f.refresh(&mut cache, 10.0).total.output, 42);
        std::fs::remove_file(&a).unwrap();
        assert_eq!(f.refresh(&mut cache, 10.0).total.output, 0);
    }

    #[test]
    fn scan_budget_rotates_across_all_logs() {
        let f = Fixture::new();
        for id in ["a", "b", "c"] {
            std::fs::write(f.agents.join(format!("{id}.jsonl")), message(id, 10, 0.0)).unwrap();
        }
        let mut cache = Cache::default();
        for _ in 0..4 {
            let mut remaining = 1;
            cache.refresh(&f.main, "one", || {
                let allowed = remaining > 0;
                remaining = 0;
                allowed
            });
        }
        assert_eq!(cache.files.len(), 4);
        assert_eq!(cache.stats(f.main.to_str().unwrap(), 0.0).total.output, 30);
        assert_eq!(f.refresh(&mut cache, 4.0).state, CollectionState::Complete);
    }

    #[test]
    fn preview_and_lock_contention_read_the_last_snapshot() {
        let f = Fixture::new();
        let path = f.root.join("session.json");
        let mut payload = f.payload();
        let read = |p: &Payload, now| {
            collect_cached(p, &path, Instant::now() + Duration::from_secs(1), now).unwrap()
        };
        assert!(read(&payload, 0.0).tps.is_none());
        append(&f.main, &(user(0.0) + &message("m1", 420, 10.0)));
        assert_eq!(rate(&read(&payload, 10.0)), 42.0);
        let bytes = std::fs::read(&path).unwrap();
        append(&f.main, &message("m2", 42, 11.0));
        payload.is_preview = true;
        assert_eq!(read(&payload, 11.0).total.output, 420);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        payload.is_preview = false;
        let lock = File::options()
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .unwrap();
        lock.try_lock().unwrap();
        let contended = read(&payload, 11.0);
        assert_eq!(contended.state, CollectionState::Complete, "no flicker");
        assert_eq!(contended.total.output, 420);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(lock);
        assert_eq!(read(&payload, 11.0).total.output, 462);
        // Idle refreshes neither reopen logs nor rewrite the cache.
        let bytes = std::fs::read(&path).unwrap();
        read(&payload, 42.0);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn split_messages_and_updates_are_not_double_counted() {
        let mut c = Cursor::default();
        for output in [10, 20, 10] {
            let line = serde_json::json!({"type":"assistant","sessionId":"one",
                "message":{"id":"msg-1","model":"Sonnet","usage":{"input_tokens":100,"output_tokens":output}}});
            record(line.to_string().as_bytes(), &mut c, "one");
        }
        assert_eq!(c.messages.len(), 1);
        assert_eq!(c.messages["msg-1"].usage.output, 20);
        assert_eq!(c.messages["msg-1"].usage.input(), 100);
        record(br#"{"type":"assistant","sessionId":"other","message":{"id":"msg-2","usage":{"input_tokens":999}}}"#, &mut c, "one");
        assert_eq!(c.messages.len(), 1);
    }
}
