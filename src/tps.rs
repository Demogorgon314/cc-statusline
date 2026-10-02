//! Recent output per second of generation, including parallel agents.
//! Claude writes each content block only after it finishes, so a request's
//! output is spread over its transcript interval (preceding user/tool record to
//! last block) instead of the moment a refresh happened to observe it. Only
//! time with a request in flight is counted: waiting on tools or the person
//! does not drag the rate toward zero, it keeps the last value (dimmed by the
//! renderer once stale). The result is a pure function of the logs: late,
//! contended or resumed reads land in the same place.
use serde::{Deserialize, Serialize};

/// Seconds of generation, most recent first, that the rate averages over.
const WINDOW_SECS: f64 = 30.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Estimate {
    pub tokens_per_sec: f64,
    /// Time of the last logged output, not the last refresh.
    pub measured_at: f64,
}

/// One deduplicated API request: output tokens over `[start, end]` seconds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    pub output: u64,
}

pub fn estimate(spans: impl IntoIterator<Item = Span>, now: f64) -> Option<Estimate> {
    let mut spans: Vec<Span> = spans
        .into_iter()
        .filter(|s| s.output > 0 && s.end <= now)
        .map(|s| Span {
            start: s.start.min(s.end),
            ..s
        })
        .collect();
    let last = spans
        .iter()
        .map(|s| s.end)
        .fold(f64::NEG_INFINITY, f64::max);
    if !last.is_finite() {
        return None;
    }
    // Union of in-flight time, newest first; parallel requests share it.
    spans.sort_by(|a, b| b.end.total_cmp(&a.end));
    let mut busy: Vec<(f64, f64)> = Vec::new();
    for s in &spans {
        match busy.last_mut() {
            Some(seg) if s.end >= seg.0 => seg.0 = seg.0.min(s.start),
            _ => busy.push((s.start, s.end)),
        }
    }
    // Walk back until WINDOW_SECS of generation is covered.
    let mut cutoff = f64::NEG_INFINITY;
    let mut covered = 0.0;
    for &(start, end) in &busy {
        if covered + (end - start) >= WINDOW_SECS {
            cutoff = end - (WINDOW_SECS - covered);
            covered = WINDOW_SECS;
            break;
        }
        covered += end - start;
    }
    let mut tokens = 0.0;
    for s in &spans {
        if s.end < cutoff {
            continue;
        }
        tokens += if s.start >= cutoff || s.end <= s.start {
            s.output as f64
        } else {
            s.output as f64 * (s.end - cutoff) / (s.end - s.start)
        };
    }
    Some(Estimate {
        // Instant records alone carry no duration; a second bounds the spike.
        tokens_per_sec: tokens / covered.max(1.0),
        measured_at: last,
    })
}

/// Output inside the last window of wall-clock time, so the log counts as
/// working in parallel right now.
pub fn is_active(estimate: &Estimate, now: f64) -> bool {
    now - estimate.measured_at < WINDOW_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f64, end: f64, output: u64) -> Span {
        Span { start, end, output }
    }

    #[test]
    fn rate_is_per_second_of_generation() {
        // A 60s request logged at once: its last 30s of generation.
        let e = estimate([span(0.0, 60.0, 3000)], 60.0).unwrap();
        assert_eq!(e.tokens_per_sec, 50.0);
        assert_eq!(e.measured_at, 60.0);
        let e = estimate([span(0.0, 10.0, 420)], 10.0).unwrap();
        assert_eq!(e.tokens_per_sec, 42.0);
    }

    #[test]
    fn idle_time_keeps_the_last_rate() {
        // Tool runs and waiting between requests are not generation.
        let spans = [span(0.0, 10.0, 500), span(100.0, 110.0, 300)];
        assert_eq!(estimate(spans, 110.0).unwrap().tokens_per_sec, 40.0);
        let idle = estimate(spans, 400.0).unwrap();
        assert_eq!(idle.tokens_per_sec, 40.0);
        assert_eq!(idle.measured_at, 110.0);
        assert_eq!(estimate(spans, 1e6).unwrap().tokens_per_sec, 40.0);
        assert!(estimate([], 1.0).is_none());
    }

    #[test]
    fn parallel_agents_add_over_shared_time() {
        let spans = [span(100.0, 130.0, 900), span(110.0, 130.0, 600)];
        assert_eq!(estimate(spans, 130.0).unwrap().tokens_per_sec, 50.0);
        assert_eq!(estimate(spans, 500.0).unwrap().tokens_per_sec, 50.0);
    }

    #[test]
    fn window_covers_recent_generation_only() {
        // An old slow request is outside the last 30s of generation.
        let spans = [span(0.0, 30.0, 300), span(50.0, 80.0, 1500)];
        assert_eq!(estimate(spans, 90.0).unwrap().tokens_per_sec, 50.0);
        // Partly covered requests count their covered share.
        let spans = [span(0.0, 20.0, 400), span(50.0, 70.0, 1000)];
        assert_eq!(estimate(spans, 70.0).unwrap().tokens_per_sec, 40.0);
    }

    #[test]
    fn instant_records_and_unfinished_clocks_are_bounded() {
        let e = estimate([span(5.0, 5.0, 100), span(0.0, 5.0, 0)], 10.0).unwrap();
        assert_eq!(e.tokens_per_sec, 100.0);
        // Records stamped after `now` (clock skew) are not yet counted.
        assert!(estimate([span(0.0, 20.0, 400)], 10.0).is_none());
    }
}
