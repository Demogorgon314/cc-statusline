//! Request-time-normalized output throughput, not streaming decode speed.
//!
//! Transcript output and Claude's API timer are independently updated. Keep
//! a baseline until both counters advance; never divide new tokens by an
//! unchanged timer or count historical transcript catch-up as fresh output.
use crate::{paths, payload::Payload, session::SessionStats};
use serde::{Deserialize, Serialize};

/// A long gap can mean a resumed Claude process with different counter scope.
const MAX_SAMPLE_GAP_SECS: f64 = 1800.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Estimate {
    pub tokens_per_sec: f64,
    pub measured_at: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Counters {
    output: u64,
    api_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct Sampler {
    source: String,
    model: String,
    baseline: Counters,
    observed: Counters,
    observed_at: f64,
    last: Option<Estimate>,
}

impl Sampler {
    fn new(source: &str, model: &str, counters: Counters, now: f64) -> Self {
        Self {
            source: source.into(),
            model: model.into(),
            baseline: counters,
            observed: counters,
            observed_at: now,
            last: None,
        }
    }

    fn observe(&mut self, source: &str, model: &str, counters: Counters, now: f64) {
        if self.source != source
            || self.model != model
            || counters.output < self.observed.output
            || counters.api_ms < self.observed.api_ms
            || now < self.observed_at
            || now - self.observed_at > MAX_SAMPLE_GAP_SECS
        {
            *self = Self::new(source, model, counters, now);
            return;
        }
        let output = counters.output - self.baseline.output;
        let api_ms = counters.api_ms - self.baseline.api_ms;
        if output > 0 && api_ms > 0 {
            self.last = Some(Estimate {
                tokens_per_sec: output as f64 * 1000.0 / api_ms as f64,
                measured_at: now,
            });
            self.baseline = counters;
        }
        self.observed = counters;
        self.observed_at = now;
    }
}

pub fn get(payload: &Payload, stats: Option<&SessionStats>, now: f64) -> Option<Estimate> {
    if payload.session_id.is_empty() || payload.transcript_path.is_empty() {
        return None;
    }
    let key = paths::short_hash(&format!(
        "{}\0{}",
        payload.transcript_path, payload.session_id
    ));
    let path = paths::cache_dir().join(format!("tps-v1-{key}.json"));
    let previous: Option<Sampler> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let valid = stats
        .filter(|stats| stats.complete)
        .zip(payload.api_duration_ms);
    let Some((stats, api_ms)) = valid else {
        // Drop the baseline as well: the next complete read may include old
        // output whose API time was already accounted for.
        if !payload.is_preview {
            let _ = std::fs::remove_file(path);
        }
        return None;
    };
    if payload.is_preview {
        return previous
            .filter(|s| s.source == stats.source && s.model == payload.model)
            .and_then(|s| s.last);
    }
    let counters = Counters {
        output: stats.total.output,
        api_ms,
    };
    let mut sampler =
        previous.unwrap_or_else(|| Sampler::new(&stats.source, &payload.model, counters, now));
    sampler.observe(&stats.source, &payload.model, counters, now);
    if let Ok(bytes) = serde_json::to_vec(&sampler) {
        let _ = paths::write_atomic(&path, &bytes);
    }
    sampler.last
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(output: u64, api_ms: u64) -> Counters {
        Counters { output, api_ms }
    }

    #[test]
    fn staggered_counters_and_idle_refreshes_do_not_distort_rate() {
        let mut s = Sampler::new("log", "Opus", counts(100, 1000), 0.0);
        assert!(s.last.is_none());
        s.observe("log", "Opus", counts(520, 1000), 1.0);
        assert!(s.last.is_none(), "tokens arrived before the API timer");
        s.observe("log", "Opus", counts(520, 11000), 2.0);
        assert_eq!(s.last.unwrap().tokens_per_sec, 42.0);
        s.observe("log", "Opus", counts(520, 11000), 60.0);
        assert_eq!(
            s.last.unwrap().measured_at,
            2.0,
            "idle does not refresh freshness"
        );
        s.observe("log", "Opus", counts(520, 21000), 61.0);
        s.observe("log", "Opus", counts(820, 21000), 62.0);
        assert_eq!(
            s.last.unwrap().tokens_per_sec,
            30.0,
            "timer may also arrive first"
        );
    }

    #[test]
    fn counter_rollback_source_model_and_resume_rebaseline() {
        for (source, model, counters, now) in [
            ("log", "Opus", counts(50, 12000), 3.0),
            ("log", "Opus", counts(900, 500), 3.0),
            ("replaced-log", "Opus", counts(900, 12000), 3.0),
            ("log", "Sonnet", counts(900, 12000), 3.0),
            ("log", "Opus", counts(900, 12000), MAX_SAMPLE_GAP_SECS + 3.0),
        ] {
            let mut s = Sampler::new("log", "Opus", counts(0, 0), 0.0);
            s.observe("log", "Opus", counts(420, 10000), 1.0);
            s.observe(source, model, counters, now);
            assert!(s.last.is_none());
            s.observe(
                source,
                model,
                counts(counters.output + 100, counters.api_ms + 2000),
                now + 1.0,
            );
            assert_eq!(s.last.unwrap().tokens_per_sec, 50.0);
        }
    }
}
