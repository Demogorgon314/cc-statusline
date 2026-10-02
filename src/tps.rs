//! Recent session output per wall-clock second, including parallel agents.
//! Usage snapshots and API timers have no shared request boundary. Never divide
//! their independent deltas; timestamp deduplicated output when it is observed.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

const WINDOW_SECS: f64 = 30.0;
const MAX_SAMPLE_GAP_SECS: f64 = 1800.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Estimate {
    pub tokens_per_sec: f64,
    /// Time of the last new output, not the last idle refresh.
    pub measured_at: f64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Sampler {
    started_at: Option<f64>,
    observed_at: f64,
    /// High-water marks survive file replacement/removal and forked copies.
    seen: BTreeMap<String, u64>,
    buckets: VecDeque<(i64, u64)>,
    last_output_at: Option<f64>,
    last: Option<Estimate>,
}

impl Sampler {
    /// Returns true when existing files need a fresh historical baseline.
    pub fn begin(&mut self, now: f64) -> bool {
        let reset = self.started_at.is_none()
            || now < self.observed_at
            || now - self.observed_at > MAX_SAMPLE_GAP_SECS;
        if reset {
            self.started_at = Some(now);
            self.buckets.clear();
            self.last_output_at = None;
            self.last = None;
        }
        self.observed_at = now;
        // One-second buckets bound cache size even under frequent refreshes.
        while self
            .buckets
            .front()
            .is_some_and(|(at, _)| *at as f64 <= now - WINDOW_SECS)
        {
            self.buckets.pop_front();
        }
        reset
    }

    pub fn record(&mut self, id: String, output: u64, live: bool, now: f64) {
        let previous = self.seen.entry(id).or_default();
        let delta = output.saturating_sub(*previous);
        *previous = (*previous).max(output);
        if !live || delta == 0 {
            return;
        }
        let second = now.floor() as i64;
        if let Some((_, tokens)) = self.buckets.back_mut().filter(|(at, _)| *at == second) {
            *tokens = tokens.saturating_add(delta);
        } else {
            self.buckets.push_back((second, delta));
        }
        self.last_output_at = Some(now);
    }

    pub fn finish(&mut self, now: f64, complete: bool) -> Option<Estimate> {
        // Keep the previous trustworthy estimate during partial collection.
        // Healthy files still contribute buckets, without making old data fresh.
        if complete {
            let elapsed = now - self.started_at?;
            if elapsed >= 1.0 {
                if let Some(measured_at) = self.last_output_at {
                    let output = self
                        .buckets
                        .iter()
                        .fold(0u64, |n, (_, v)| n.saturating_add(*v));
                    self.last = Some(Estimate {
                        tokens_per_sec: output as f64 / elapsed.min(WINDOW_SECS),
                        measured_at,
                    });
                }
            }
        }
        self.last
    }

    pub fn estimate(&self) -> Option<Estimate> {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaved_agents_share_a_wall_clock_window() {
        let mut s = Sampler::default();
        s.begin(0.0);
        s.begin(10.0);
        s.record("a".into(), 420, true, 10.0);
        assert_eq!(s.finish(10.0, true).unwrap().tokens_per_sec, 42.0);
        s.begin(20.0);
        s.record("b".into(), 420, true, 20.0);
        assert_eq!(s.finish(20.0, true).unwrap().tokens_per_sec, 42.0);
        s.begin(21.0);
        s.record("c".into(), 42, true, 21.0);
        assert_eq!(s.finish(21.0, true).unwrap().tokens_per_sec, 42.0);
        s.record("a".into(), 420, true, 21.0);
        assert_eq!(s.finish(21.0, true).unwrap().tokens_per_sec, 42.0);
    }

    #[test]
    fn history_partial_reads_expiry_and_resume() {
        let mut s = Sampler::default();
        s.begin(0.0);
        s.record("history".into(), 9000, false, 0.0);
        s.begin(10.0);
        s.record("live".into(), 420, true, 10.0);
        let last = s.finish(10.0, true).unwrap();
        s.begin(11.0);
        s.record("other".into(), 42, true, 11.0);
        assert_eq!(s.finish(11.0, false), Some(last));
        assert_eq!(s.finish(11.0, true).unwrap().tokens_per_sec, 42.0);
        s.begin(41.0);
        let idle = s.finish(41.0, true).unwrap();
        assert_eq!(idle.tokens_per_sec, 0.0);
        assert_eq!(idle.measured_at, 11.0);
        assert!(s.begin(2000.0));
        assert!(s.finish(2000.0, true).is_none());
        s.record("live".into(), 420, true, 2000.0);
        assert!(s.finish(2002.0, true).is_none());
        assert!(s.begin(1000.0), "clock rollback rebaselines");
    }

    #[test]
    fn warmup_and_subsecond_updates_are_bounded() {
        let mut s = Sampler::default();
        s.begin(0.0);
        for i in 1..100 {
            let now = i as f64 / 100.0;
            s.begin(now);
            s.record("stream".into(), i, true, now);
            assert!(s.finish(now, true).is_none());
        }
        assert_eq!(s.buckets.len(), 1);
        assert_eq!(s.finish(1.0, true).unwrap().tokens_per_sec, 99.0);
    }
}
