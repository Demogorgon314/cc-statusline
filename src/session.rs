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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionState {
    #[default]
    Complete,
    CatchingUp,
    Partial,
}

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    offset: u64,
    skipping: bool,
    identity: String,
    /// Bytes present on discovery/replacement are history, not fresh output.
    live_from: Option<u64>,
    prefix: Vec<u8>,
    created: Option<f64>,
    messages: BTreeMap<String, (String, Usage)>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    files: BTreeMap<String, Cursor>,
    next_file: Option<String>,
    state: CollectionState,
    // Persist sampler and cursors together: a cancelled process or preview must
    // not consume transcript increments without recording their throughput.
    sampler: tps::Sampler,
    persisted_at: f64,
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

fn advance(
    path: &Path,
    cursor: &mut Cursor,
    session_id: &str,
    sampler: &mut tps::Sampler,
    now: f64,
) -> CollectionState {
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
    let live_from = *cursor.live_from.get_or_insert(meta.len());
    if cursor.prefix.is_empty() {
        let _ = f.seek(SeekFrom::Start(0));
        let _ = (&mut f).take(256).read_to_end(&mut cursor.prefix);
    }
    if f.seek(SeekFrom::Start(cursor.offset)).is_err() {
        return CollectionState::Partial;
    }
    let mut buf = Vec::new();
    let remaining = meta.len().saturating_sub(cursor.offset);
    if remaining > READ_LIMIT as u64 {
        // A backlog cannot be assigned to the current observation window.
        cursor.live_from = Some(meta.len());
    }
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
            let live =
                cursor.offset + consumed as u64 >= live_from && remaining <= READ_LIMIT as u64;
            if let Some((id, output)) = record(line, cursor, session_id) {
                sampler.record(id, output, live, now);
            }
        }
        cursor.skipping = false;
        consumed = at + 1;
    }
    if consumed == 0 && buf.len() == READ_LIMIT {
        cursor.skipping = true;
        consumed = buf.len();
    }
    cursor.offset += consumed as u64;
    if cursor.offset == meta.len() && !cursor.skipping {
        CollectionState::Complete
    } else if remaining > READ_LIMIT as u64 || cursor.skipping {
        CollectionState::CatchingUp
    } else {
        CollectionState::Partial
    }
}

fn record(line: &[u8], cursor: &mut Cursor, session_id: &str) -> Option<(String, u64)> {
    let v = serde_json::from_slice::<Value>(line).ok()?;
    if v.get("sessionId")
        .and_then(Value::as_str)
        .is_some_and(|id| id != session_id)
    {
        return None;
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
        return None;
    }
    let m = v.get("message")?;
    let u = m.get("usage").filter(|u| u.is_object())?;
    let id = m
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| v.get("uuid").and_then(Value::as_str))?;
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
    Some((id.to_string(), entry.1.output))
}

pub fn collect(payload: &Payload, deadline: Instant, now: f64) -> Option<SessionStats> {
    let transcript = &payload.transcript_path;
    let session_id = &payload.session_id;
    if transcript.is_empty() || session_id.is_empty() {
        return None;
    }
    let key = paths::short_hash(&format!("{transcript}\0{session_id}"));
    let cache_path = paths::cache_dir().join(format!("session-v2-{key}.json"));
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
    if payload.is_preview || lock.is_none() {
        let mut stats = cache.stats(&payload.transcript_path);
        if lock.is_none() && !payload.is_preview {
            stats.state = CollectionState::Partial;
        }
        return Some(stats);
    }
    let started = Instant::now();
    let changed = cache.refresh(path, &payload.session_id, now, || {
        started.elapsed() < Duration::from_millis(100) && Instant::now() < deadline
    });
    if changed {
        cache.persisted_at = now;
        if let Ok(data) = serde_json::to_vec(&cache) {
            let _ = paths::write_atomic(cache_path, &data);
        }
    }
    Some(cache.stats(&payload.transcript_path))
}

impl Cache {
    fn refresh(
        &mut self,
        path: &Path,
        session_id: &str,
        now: f64,
        mut can_read: impl FnMut() -> bool,
    ) -> bool {
        let before = (self.next_file.clone(), self.state, self.sampler.estimate());
        let mut changed = self.sampler.begin(now);
        if changed {
            for cursor in self.files.values_mut() {
                cursor.live_from = None;
            }
        }
        let mut files = BTreeSet::from([path.to_string_lossy().into_owned()]);
        // The subagent directory belongs to this transcript, never the latest session.
        let subdir = path.with_extension("").join("subagents");
        let mut directory_complete = true;
        match std::fs::read_dir(subdir) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(e) if e.path().extension().is_some_and(|e| e == "jsonl") => {
                            files.insert(e.path().to_string_lossy().into_owned());
                        }
                        Ok(_) => {}
                        Err(_) => directory_complete = false,
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => directory_complete = false,
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
                cursor.live_from,
            );
            let state = advance(Path::new(file), cursor, session_id, &mut self.sampler, now);
            changed |= before
                != (
                    cursor.offset,
                    cursor.identity.clone(),
                    cursor.prefix.len(),
                    cursor.live_from,
                );
            match state {
                CollectionState::Partial => self.state = state,
                CollectionState::CatchingUp if self.state == CollectionState::Complete => {
                    self.state = state
                }
                _ => {}
            }
        }
        self.sampler
            .finish(now, self.state == CollectionState::Complete);
        changed
            || before != (self.next_file.clone(), self.state, self.sampler.estimate())
            || now - self.persisted_at >= 60.0
    }

    fn stats(&self, transcript: &str) -> SessionStats {
        let mut stats = SessionStats {
            state: self.state,
            tps: self.sampler.estimate(),
            ..Default::default()
        };
        let mut requests: BTreeMap<&str, (bool, &str, Usage)> = BTreeMap::new();
        for (file, cursor) in &self.files {
            if file == transcript {
                stats.created = cursor.created;
            }
            for (id, (model, usage)) in &cursor.messages {
                // Forked agents may carry copies of parent conversation messages.
                // A shared API message ID still represents only one billed request.
                let entry =
                    requests
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
            cache.refresh(&self.main, "one", now, || true);
            cache.stats(self.main.to_str().unwrap())
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

    fn message(id: &str, output: u64) -> String {
        format!(
            "{}\n",
            serde_json::json!({"type":"assistant","sessionId":"one","message":{"id":id,"model":"Sonnet","usage":{"input_tokens":100,"output_tokens":output}}})
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

    #[test]
    fn interleaved_logs_and_new_agents_do_not_reset_or_spike_tps() {
        let f = Fixture::new();
        let a = f.agents.join("a.jsonl");
        let b = f.agents.join("b.jsonl");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        let mut cache = Cache::default();
        assert!(f.refresh(&mut cache, 0.0).tps.is_none());
        append(&a, &message("a1", 420));
        assert_eq!(
            f.refresh(&mut cache, 10.0).tps.unwrap().tokens_per_sec,
            42.0
        );
        append(&b, &message("b1", 420));
        assert_eq!(
            f.refresh(&mut cache, 20.0).tps.unwrap().tokens_per_sec,
            42.0
        );
        append(&a, &message("a2", 42));
        let last = f.refresh(&mut cache, 21.0).tps;
        assert_eq!(last.unwrap().tokens_per_sec, 42.0);
        std::fs::write(f.agents.join("empty.jsonl"), "").unwrap();
        assert_eq!(f.refresh(&mut cache, 21.0).tps, last);
        let history = f.agents.join("history.jsonl");
        std::fs::write(&history, message("a1", 420) + &message("old", 9000)).unwrap();
        assert_eq!(f.refresh(&mut cache, 21.0).tps, last);
        append(&history, &message("old", 9000));
        append(&b, &message("a1", 420));
        assert_eq!(f.refresh(&mut cache, 21.0).tps, last);
        std::fs::write(&a, message("replacement", 50000)).unwrap();
        assert_eq!(f.refresh(&mut cache, 21.0).tps, last);
        std::fs::remove_file(&history).unwrap();
        assert_eq!(f.refresh(&mut cache, 21.0).tps, last);
    }

    #[test]
    fn partial_agent_preserves_last_sample_and_other_agents_keep_progressing() {
        let f = Fixture::new();
        let a = f.agents.join("a.jsonl");
        std::fs::write(&a, "").unwrap();
        let mut cache = Cache::default();
        f.refresh(&mut cache, 0.0);
        append(&f.main, &message("main1", 420));
        let last = f.refresh(&mut cache, 10.0).tps;
        append(&a, message("a1", 42).trim_end());
        append(&f.main, &message("main2", 42));
        let partial = f.refresh(&mut cache, 11.0);
        assert_eq!(partial.state, CollectionState::Partial);
        assert_eq!(partial.total.output, 462);
        assert_eq!(partial.tps, last);
        append(&a, "\n");
        let complete = f.refresh(&mut cache, 12.0);
        assert_eq!(complete.total.output, 504);
        assert_eq!(complete.state, CollectionState::Complete);
        assert_eq!(complete.tps.unwrap().tokens_per_sec, 42.0);
        // A temporarily unreadable file retains its usage and old estimate.
        std::fs::remove_file(&a).unwrap();
        std::fs::create_dir(&a).unwrap();
        let unavailable = f.refresh(&mut cache, 13.0);
        assert_eq!(unavailable.state, CollectionState::Partial);
        assert_eq!(unavailable.total.output, 504);
        assert_eq!(unavailable.tps, complete.tps);
    }

    #[test]
    fn historical_backlog_is_not_fresh_throughput() {
        let f = Fixture::new();
        let mut cache = Cache::default();
        f.refresh(&mut cache, 0.0);
        append(&f.main, &message("main1", 420));
        let last = f.refresh(&mut cache, 10.0).tps;
        let a = f.agents.join("history.jsonl");
        std::fs::write(&a, "x".repeat(READ_LIMIT) + "\n" + &message("old", 9000)).unwrap();
        let partial = f.refresh(&mut cache, 10.0);
        assert_eq!(partial.state, CollectionState::CatchingUp);
        assert_eq!(partial.tps, last);
        let caught_up = f.refresh(&mut cache, 10.0);
        assert_eq!(caught_up.state, CollectionState::Complete);
        assert_eq!(caught_up.tps, last);
        assert_eq!(caught_up.total.output, 9420);
        append(&a, &message("new", 42));
        assert_eq!(
            f.refresh(&mut cache, 11.0).tps.unwrap().tokens_per_sec,
            42.0
        );
    }

    #[test]
    fn scan_budget_rotates_across_all_logs() {
        let f = Fixture::new();
        for id in ["a", "b", "c"] {
            std::fs::write(f.agents.join(format!("{id}.jsonl")), message(id, 10)).unwrap();
        }
        let mut cache = Cache::default();
        for now in 0..4 {
            let mut remaining = 1;
            cache.refresh(&f.main, "one", now as f64, || {
                let allowed = remaining > 0;
                remaining = 0;
                allowed
            });
        }
        assert_eq!(cache.files.len(), 4);
        assert_eq!(cache.stats(f.main.to_str().unwrap()).total.output, 30);
        assert_eq!(f.refresh(&mut cache, 4.0).state, CollectionState::Complete);
    }

    #[test]
    fn preview_and_lock_contention_never_consume_live_increments() {
        let f = Fixture::new();
        let path = f.root.join("session.json");
        let mut payload = f.payload();
        let read = |p: &Payload, now| {
            collect_cached(p, &path, Instant::now() + Duration::from_secs(1), now).unwrap()
        };
        assert!(read(&payload, 0.0).tps.is_none());
        append(&f.main, &message("m1", 420));
        let last = read(&payload, 10.0).tps;
        assert_eq!(last.unwrap().tokens_per_sec, 42.0);
        let bytes = std::fs::read(&path).unwrap();
        append(&f.main, &message("m2", 42));
        payload.is_preview = true;
        assert_eq!(read(&payload, 11.0).tps, last);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        payload.is_preview = false;
        let lock = File::options()
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .unwrap();
        lock.try_lock().unwrap();
        let contended = read(&payload, 11.0);
        assert_eq!(contended.state, CollectionState::Partial);
        assert_eq!(contended.tps, last);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(lock);
        assert_eq!(read(&payload, 11.0).tps.unwrap().tokens_per_sec, 42.0);
        assert_eq!(read(&payload, 41.0).tps.unwrap().tokens_per_sec, 0.0);
        // Once idle, avoid serializing the entire session on every timer tick.
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
        assert_eq!(c.messages["msg-1"].1.output, 20);
        assert_eq!(c.messages["msg-1"].1.input(), 100);
        record(br#"{"type":"assistant","sessionId":"other","message":{"id":"msg-2","usage":{"input_tokens":999}}}"#, &mut c, "one");
        assert_eq!(c.messages.len(), 1);
    }
}
