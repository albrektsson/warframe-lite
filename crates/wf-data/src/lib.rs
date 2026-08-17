//! External data sources for warframe-lite.
//!
//! * [`worldstate`] — live game state (fissures, Void Trader, cycles) from the
//!   community [warframestat.us](https://docs.warframestat.us/) API.
//! * [`market`] — trade prices from [warframe.market](https://warframe.market).
//!
//! Both are read-only public HTTP APIs, so nothing here requires an account or
//! touches the game process.

pub mod items;
pub mod market;
pub mod poll;
pub mod polarity;
pub mod rate_limit;
pub mod riven_market;
pub mod worldstate;

pub use polarity::Polarity;

use std::time::Duration;

/// Build a shared [`reqwest::Client`] with a descriptive user agent and sane
/// timeouts. warframe.market in particular expects a real, identifying user
/// agent (`Name/version (+contact-url)`, per its anti-impersonation rules —
/// see `docs.warframe.market/docs/rules/overview`), not just a bare name.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!(
            "warframe-lite/",
            env!("CARGO_PKG_VERSION"),
            " (+",
            env!("CARGO_PKG_REPOSITORY"),
            ")"
        ))
        .timeout(Duration::from_secs(15))
        .build()
        .expect("failed to build HTTP client")
}

/// User-Agent to send on calls to Digital Extremes' own `api.warframe.com`
/// endpoints (`wf_relic::mastery`'s profile fetch, `wf-mem`'s `inventory.php`
/// token-relay) — deliberately *not* [`http_client`]'s honest, self-
/// identifying default. Two community tools that have hit these same
/// endpoints for years without issue — WFHelper's `apiHelperRunner.ts`, and
/// the `calamity-inc/Soup` HTTP library behind the original
/// `Sainan/warframe-api-helper` PoC — both independently send a spoofed
/// browser-style UA specifically here (and only here: WFHelper still uses
/// its own honest `"WFHelper"` UA against GitHub). This matches that prior
/// art instead of being the one non-browser-looking UA on DE's own domain.
/// `warframe.market` is unaffected — its documented contract wants the
/// opposite (an honest, contactable UA) and still gets [`http_client`]'s
/// default.
///
/// Set this via `.header("User-Agent", DE_USER_AGENT)` on the individual
/// request, not on the shared client: reqwest only fills in a client-level
/// default header when the request doesn't already set that header itself
/// (see `reqwest::Client::execute_request`), so a per-request override like
/// this replaces the default cleanly rather than sending both.
pub const DE_USER_AGENT: &str = "Mozilla/5.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_matches_the_documented_contact_format() {
        // reqwest only merges the client's default headers (incl. `user_agent`)
        // in at send time, not into a `.build()`'d `Request` — so this checks
        // the client's `Debug` output, which does include them, instead of
        // requiring a live send.
        let debug = format!("{:?}", http_client());
        let expected_ua = concat!(
            "warframe-lite/",
            env!("CARGO_PKG_VERSION"),
            " (+",
            env!("CARGO_PKG_REPOSITORY"),
            ")"
        );
        assert!(
            debug.contains(expected_ua),
            "expected User-Agent {expected_ua:?} in client debug output: {debug}"
        );
    }
}
