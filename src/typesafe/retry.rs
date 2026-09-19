//! Retry policy for TypeSafe requests
//!
//! TypeSafe's own SDKs retry with backoff by default; the HTTP API does not do
//! it for you. Defaults here match the documented Python `RetryPolicy` so that
//! behaviour is the same whichever client a service uses.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How the client reacts to retryable failures.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Retries after the initial attempt. 0 disables retrying.
    pub max_retries: u32,
    /// Delay before the first retry; doubles each attempt.
    pub backoff_initial: Duration,
    /// Ceiling for the doubling.
    pub backoff_max: Duration,
    /// Fraction of the delay to randomize, spreading retries from concurrent
    /// callers so they do not land together. 0.25 means +/-25%.
    pub backoff_jitter: f64,
    /// Statuses worth another attempt.
    pub retry_statuses: Vec<u16>,
    /// Honour a `retry-after` header instead of the computed backoff.
    pub respect_retry_after: bool,
    /// Retry when the connection itself failed.
    pub retry_on_connection_error: bool,
    /// Retry when the request timed out.
    pub retry_on_timeout: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
            backoff_jitter: 0.25,
            // 408 and 429 plus every 5xx, which covers the documented 529.
            retry_statuses: {
                let mut statuses = vec![408, 429];
                statuses.extend(500u16..600);
                statuses
            },
            respect_retry_after: true,
            retry_on_connection_error: true,
            retry_on_timeout: true,
        }
    }
}

impl RetryPolicy {
    /// A policy that never retries. Use when the caller sits in a latency
    /// budget it cannot overrun, and would rather fail fast.
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Self::default()
        }
    }

    /// Whether this status is worth another attempt.
    pub fn retries_status(&self, status: u16) -> bool {
        self.retry_statuses.contains(&status)
    }

    /// How long to wait before attempt number `attempt`, where the first retry
    /// is attempt 1.
    ///
    /// A `retry_after` from the server wins over the computed backoff when the
    /// policy respects it, because the server knows when its own limit resets.
    pub fn delay_for(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if self.respect_retry_after {
            if let Some(after) = retry_after {
                return after.min(self.backoff_max.max(after));
            }
        }

        let exponent = attempt.saturating_sub(1).min(16);
        let base = self
            .backoff_initial
            .saturating_mul(2u32.saturating_pow(exponent))
            .min(self.backoff_max);

        self.apply_jitter(base)
    }

    fn apply_jitter(&self, base: Duration) -> Duration {
        if self.backoff_jitter <= 0.0 {
            return base;
        }
        // A fraction in 0.0..1.0 derived from the clock. Backoff only needs
        // retries to land apart, which does not justify a random-number
        // dependency.
        let entropy = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let unit = (entropy % 1_000_000) as f64 / 1_000_000.0;
        let swing = self.backoff_jitter.clamp(0.0, 1.0) * (2.0 * unit - 1.0);
        let scaled = base.as_secs_f64() * (1.0 + swing);
        Duration::from_secs_f64(scaled.max(0.0))
    }
}

/// Parse a `retry-after` header value.
///
/// Only the delay-seconds form is handled. The HTTP-date form returns `None`,
/// which falls back to exponential backoff rather than retrying immediately.
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_sdk_policy() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 2);
        assert_eq!(p.backoff_initial, Duration::from_millis(500));
        assert_eq!(p.backoff_max, Duration::from_secs(5));
        assert!(p.respect_retry_after);
        assert!(p.retries_status(429));
        assert!(p.retries_status(529));
        assert!(p.retries_status(408));
        assert!(p.retries_status(503));
        assert!(!p.retries_status(422));
        assert!(!p.retries_status(401));
        assert!(!p.retries_status(200));
    }

    #[test]
    fn backoff_grows_and_stays_under_the_ceiling() {
        let p = RetryPolicy {
            backoff_jitter: 0.0,
            ..RetryPolicy::default()
        };
        assert_eq!(p.delay_for(1, None), Duration::from_millis(500));
        assert_eq!(p.delay_for(2, None), Duration::from_secs(1));
        assert_eq!(p.delay_for(3, None), Duration::from_secs(2));
        assert_eq!(p.delay_for(9, None), Duration::from_secs(5));
    }

    #[test]
    fn retry_after_overrides_backoff() {
        let p = RetryPolicy::default();
        assert_eq!(
            p.delay_for(1, Some(Duration::from_secs(30))),
            Duration::from_secs(30)
        );

        let ignoring = RetryPolicy {
            respect_retry_after: false,
            backoff_jitter: 0.0,
            ..RetryPolicy::default()
        };
        assert_eq!(
            ignoring.delay_for(1, Some(Duration::from_secs(30))),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn jitter_stays_within_its_band() {
        let p = RetryPolicy::default();
        for _ in 0..200 {
            let d = p.delay_for(2, None).as_secs_f64();
            assert!((0.75..=1.25).contains(&d), "delay {d} outside jitter band");
        }
    }

    #[test]
    fn parses_only_the_seconds_form_of_retry_after() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("  7 "), Some(Duration::from_secs(7)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after(""), None);
    }

    #[test]
    fn none_policy_disables_retrying() {
        assert_eq!(RetryPolicy::none().max_retries, 0);
    }
}
