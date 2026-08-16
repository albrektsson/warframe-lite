//! Shared polling helpers: failure backoff and startup/interval jitter.
//!
//! Originally lived only in the overlay's worldstate refresh loop
//! (`worldstate_retry_interval` in `src/main.rs`); lifted here so wf-browse's
//! own poll loops can reuse the same behavior instead of reimplementing it
//! (issue #100).

use std::time::Duration;

/// Retry interval given how many fetches have failed in a row: `base` on a
/// clean run, doubling per consecutive failure and capped at `cap` — a
/// struggling/erroring API shouldn't be polled at the steady-state cadence
/// forever. Resets to `base` the moment a fetch succeeds
/// (`consecutive_failures == 0`).
pub fn backoff_interval(base: Duration, consecutive_failures: u32, cap: Duration) -> Duration {
    if consecutive_failures == 0 {
        return base;
    }
    // Cap the shift, not just the result: `1u32 << 32` panics, and this many
    // consecutive failures already blows past the cap many times over.
    let multiplier = 1u32 << consecutive_failures.min(16);
    base.checked_mul(multiplier).unwrap_or(cap).min(cap)
}

/// `base` randomized by up to `fraction` in either direction (e.g.
/// `fraction: 0.2` returns somewhere in `[0.8 * base, 1.2 * base]`) — so
/// independent installs polling on the same nominal cadence, or all waking
/// up together after an update invalidates every cache at once, don't line
/// up into a synchronized burst against the same API. `fraction` is clamped
/// to `[0.0, 1.0]`.
pub fn jitter(base: Duration, fraction: f64) -> Duration {
    let fraction = fraction.clamp(0.0, 1.0);
    let factor = 1.0 + (fastrand::f64() * 2.0 - 1.0) * fraction;
    base.mul_f64(factor.max(0.0))
}

/// A uniformly random delay in `[Duration::ZERO, max]` — meant to be
/// `sleep`'d once before a process's very first network fetch. Unlike
/// [`jitter`] (which scales a nonzero recurring interval), this covers the
/// "first fetch after startup" case: many independent installs relaunching
/// around the same real-world moment (a patch drop, a scheduled restart) or
/// waking up to a cache-format bump that invalidates every cache at once
/// would otherwise all fire their first request in the same instant.
pub fn startup_delay(max: Duration) -> Duration {
    max.mul_f64(fastrand::f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_stays_at_base_with_no_failures() {
        assert_eq!(
            backoff_interval(Duration::from_secs(60), 0, Duration::from_secs(600)),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn backoff_doubles_per_consecutive_failure() {
        let base = Duration::from_secs(60);
        let cap = Duration::from_secs(600);
        assert_eq!(backoff_interval(base, 1, cap), Duration::from_secs(120));
        assert_eq!(backoff_interval(base, 2, cap), Duration::from_secs(240));
        assert_eq!(backoff_interval(base, 3, cap), Duration::from_secs(480));
    }

    #[test]
    fn backoff_caps_and_never_overflows() {
        let base = Duration::from_secs(60);
        let cap = Duration::from_secs(600);
        assert_eq!(backoff_interval(base, 4, cap), cap);
        assert_eq!(backoff_interval(base, 30, cap), cap);
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let base = Duration::from_secs(100);
        for _ in 0..1000 {
            let j = jitter(base, 0.2);
            assert!(j >= Duration::from_secs(80) && j <= Duration::from_secs(120), "{j:?}");
        }
    }

    #[test]
    fn zero_fraction_jitter_is_a_no_op() {
        assert_eq!(jitter(Duration::from_secs(45), 0.0), Duration::from_secs(45));
    }

    #[test]
    fn startup_delay_stays_within_bounds() {
        let max = Duration::from_secs(5);
        for _ in 0..1000 {
            let d = startup_delay(max);
            assert!(d <= max, "{d:?}");
        }
    }

    #[test]
    fn zero_max_startup_delay_is_a_no_op() {
        assert_eq!(startup_delay(Duration::ZERO), Duration::ZERO);
    }
}
