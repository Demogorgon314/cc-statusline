//! Recent session output per wall-clock second, including parallel agents.
//! Claude writes each content block only after it finishes, so a request's
//! output is spread over its transcript interval (preceding user/tool record to
//! last block) instead of the moment a refresh happened to observe it. The
//! result is a pure function of the logs: late, contended or resumed reads
//! land in the same place.
use serde::{Deserialize, Serialize};

const WINDOW_SECS: f64 = 30.0;
/// Older output belongs to a previous working period; hide instead of `≈0`.
const MAX_IDLE_SECS: f64 = 1800.0;

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
    let from = now - WINDOW_SECS;
    let mut first = f64::INFINITY;
    let mut last = f64::NEG_INFINITY;
    let mut tokens = 0.0;
    for span in spans {
        if span.output == 0 {
            continue;
        }
        first = first.min(span.start);
        last = last.max(span.end);
        let (start, end) = (span.start.max(from), span.end.min(now));
        if span.end <= span.start {
            if span.end > from && span.end <= now {
                tokens += span.output as f64;
            }
        } else if end > start {
            tokens += span.output as f64 * (end - start) / (span.end - span.start);
        }
    }
    if !last.is_finite() || now - last > MAX_IDLE_SECS {
        return None;
    }
    // A young session has not been observable for a whole window yet.
    let elapsed = (now - first).clamp(1.0, WINDOW_SECS);
    Some(Estimate {
        tokens_per_sec: tokens / elapsed,
        measured_at: last,
    })
}

/// Output inside the current window, so the log counts as working in parallel.
pub fn is_active(estimate: &Estimate, now: f64) -> bool {
    estimate.tokens_per_sec > 0.0 && now - estimate.measured_at < WINDOW_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f64, end: f64, output: u64) -> Span {
        Span { start, end, output }
    }

    #[test]
    fn block_flushes_are_spread_over_their_request() {
        // A 60s request logged at once contributes only its last 30s.
        let e = estimate([span(0.0, 60.0, 3000)], 60.0).unwrap();
        assert_eq!(e.tokens_per_sec, 50.0);
        assert_eq!(e.measured_at, 60.0);
        // Warmup divides by observed time, not a one-second sliver.
        let e = estimate([span(0.0, 10.0, 420)], 10.0).unwrap();
        assert_eq!(e.tokens_per_sec, 42.0);
    }

    #[test]
    fn parallel_agents_add_and_idle_decays() {
        let spans = [span(100.0, 130.0, 900), span(110.0, 130.0, 600)];
        assert_eq!(estimate(spans, 130.0).unwrap().tokens_per_sec, 50.0);
        let idle = estimate(spans, 200.0).unwrap();
        assert_eq!(idle.tokens_per_sec, 0.0);
        assert_eq!(idle.measured_at, 130.0);
        assert!(estimate(spans, 130.0 + MAX_IDLE_SECS + 1.0).is_none());
        assert!(estimate([], 1.0).is_none());
    }

    #[test]
    fn instant_records_and_future_clocks_are_bounded() {
        let e = estimate([span(5.0, 5.0, 100), span(0.0, 5.0, 0)], 10.0).unwrap();
        assert_eq!(e.tokens_per_sec, 20.0);
        // Clock skew past `now` only counts the elapsed share.
        let e = estimate([span(0.0, 20.0, 400)], 10.0).unwrap();
        assert_eq!(e.tokens_per_sec, 20.0);
    }
}
