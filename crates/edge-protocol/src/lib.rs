//! Fleet gRPC protocol bindings and reconnect helpers.
//!
//! The generated protobuf code lives in `fleet` module. This crate adds the
//! small pieces every edge binary needs: jittered exponential backoff for
//! outbound-only reconnection (the cloud never dials into the edge).

#![allow(clippy::result_large_err)]

pub mod fleet {
    tonic::include_proto!("edgewarden.fleet.v1");
}

pub use fleet::*;

use rand::Rng;
use std::time::Duration;

/// Jittered exponential backoff for control-plane reconnection.
///
/// The edge always initiates the connection. On failure it waits
/// `min(cap, base * 2^attempt)` plus full jitter, so a fleet-wide outage
/// does not produce a thundering herd when the cloud returns.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    cap: Duration,
    attempt: u32,
}

impl Backoff {
    pub fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            attempt: 0,
        }
    }

    /// Compute the next delay and advance the attempt counter.
    pub fn next_delay(&mut self) -> Duration {
        let exp = self
            .base
            .as_millis()
            .saturating_mul(1u128 << self.attempt.min(10));
        let capped = exp.min(self.cap.as_millis());
        self.attempt = self.attempt.saturating_add(1);
        let jitter: u128 = rand::thread_rng().gen_range(0..=capped.max(1));
        Duration::from_millis((capped / 2 + jitter / 2).min(capped) as u64)
    }

    /// Deterministic variant for tests.
    pub fn delay_for_attempt(base: Duration, cap: Duration, attempt: u32) -> Duration {
        let exp = base.as_millis().saturating_mul(1u128 << attempt.min(10));
        Duration::from_millis(exp.min(cap.as_millis()) as u64)
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    pub fn attempts(&self) -> u32 {
        self.attempt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps() {
        let base = Duration::from_millis(100);
        let cap = Duration::from_secs(5);
        assert_eq!(
            Backoff::delay_for_attempt(base, cap, 0),
            Duration::from_millis(100)
        );
        assert_eq!(
            Backoff::delay_for_attempt(base, cap, 1),
            Duration::from_millis(200)
        );
        assert_eq!(
            Backoff::delay_for_attempt(base, cap, 10),
            Duration::from_secs(5)
        );
        assert_eq!(
            Backoff::delay_for_attempt(base, cap, 20),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn backoff_jitter_stays_within_cap() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(2));
        for _ in 0..100 {
            let d = b.next_delay();
            assert!(d <= Duration::from_secs(2));
        }
    }

    #[test]
    fn backoff_reset_clears_attempts() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(2));
        b.next_delay();
        b.next_delay();
        assert_eq!(b.attempts(), 2);
        b.reset();
        assert_eq!(b.attempts(), 0);
    }
}
