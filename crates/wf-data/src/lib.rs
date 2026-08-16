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
