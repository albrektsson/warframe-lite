//! A narrow token-bucket rate limiter — built for
//! [`crate::riven_market::RivenMarketClient::auctions_for`] (ADR-0020), not
//! as a general-purpose primitive. warframe.market's `/v1/auctions/search`
//! carries a much tighter budget than the API's general 3 req/s, and the
//! caller's own concurrency cap (`wf-browse`'s `PRICE_FETCH_CONCURRENCY`)
//! only bounds how many requests run *at once* — it says nothing about
//! *rate*, so a single tab-open across many distinct weapons could still
//! burst well past that budget. This sits underneath that concurrency cap,
//! gating the requests themselves regardless of how many callers are
//! contending for a token at once.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Continuously-refilling token bucket: up to `capacity` calls proceed
/// immediately (a fresh bucket, or one that's had time to fully refill),
/// then each further call waits for its own token to accrue at a steady
/// `capacity` tokens per `refill_window`, rather than being rejected
/// outright — callers here have nowhere else to go but the same endpoint,
/// so queuing is the right behavior, not shedding.
pub struct TokenBucket {
    capacity: f64,
    tokens_per_sec: f64,
    state: Mutex<State>,
}

struct State {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    pub fn new(capacity: u32, refill_window: Duration) -> Self {
        Self {
            capacity: capacity as f64,
            tokens_per_sec: capacity as f64 / refill_window.as_secs_f64(),
            state: Mutex::new(State { tokens: capacity as f64, last_refill: Instant::now() }),
        }
    }

    /// Waits, if necessary, until a token is available, then consumes it.
    /// Re-checks after every wait rather than trusting a single computed
    /// delay, so a bucket contended by several concurrent callers (see this
    /// module's doc) never lets two of them consume the same freshly-refilled
    /// token.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                let now = Instant::now();
                refill(&mut state, now, self.capacity, self.tokens_per_sec);
                if state.tokens >= 1.0 {
                    state.tokens -= 1.0;
                    None
                } else {
                    Some(wait_for_token(state.tokens, self.tokens_per_sec))
                }
            };
            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }
}

/// Add whatever's accrued since `state.last_refill` at `tokens_per_sec`,
/// capped at `capacity` — pulled out of [`TokenBucket::acquire`] as a pure
/// function so it's testable without real sleeps (mirrors
/// [`crate::poll::backoff_interval`]'s style).
fn refill(state: &mut State, now: Instant, capacity: f64, tokens_per_sec: f64) {
    let elapsed = now.duration_since(state.last_refill).as_secs_f64();
    state.tokens = (state.tokens + elapsed * tokens_per_sec).min(capacity);
    state.last_refill = now;
}

/// How long until `tokens` (already refilled as of "now") reaches 1.0 at
/// `tokens_per_sec` — zero if a token is already available.
fn wait_for_token(tokens: f64, tokens_per_sec: f64) -> Duration {
    Duration::from_secs_f64(((1.0 - tokens) / tokens_per_sec).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refill_adds_no_tokens_when_no_time_has_passed() {
        let now = Instant::now();
        let mut state = State { tokens: 3.0, last_refill: now };
        refill(&mut state, now, 15.0, 0.25);
        assert_eq!(state.tokens, 3.0);
    }

    #[test]
    fn refill_adds_tokens_proportional_to_elapsed_time() {
        let start = Instant::now();
        let mut state = State { tokens: 0.0, last_refill: start };
        // 0.25 tokens/sec (15 per 60s) over 8s = 2 tokens.
        refill(&mut state, start + Duration::from_secs(8), 15.0, 0.25);
        assert!((state.tokens - 2.0).abs() < 1e-9, "{}", state.tokens);
    }

    #[test]
    fn refill_caps_at_capacity() {
        let start = Instant::now();
        let mut state = State { tokens: 14.0, last_refill: start };
        refill(&mut state, start + Duration::from_secs(600), 15.0, 0.25);
        assert_eq!(state.tokens, 15.0);
    }

    #[test]
    fn wait_for_token_is_zero_when_a_token_is_already_available() {
        assert_eq!(wait_for_token(1.0, 0.25), Duration::ZERO);
        assert_eq!(wait_for_token(3.0, 0.25), Duration::ZERO);
    }

    #[test]
    fn wait_for_token_scales_with_the_deficit() {
        // Empty bucket at 0.25 tokens/sec needs 4s for the next token.
        assert_eq!(wait_for_token(0.0, 0.25), Duration::from_secs(4));
        // Half a token already accrued needs half that wait.
        assert_eq!(wait_for_token(0.5, 0.25), Duration::from_secs(2));
    }
}
