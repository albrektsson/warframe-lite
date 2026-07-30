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
pub mod worldstate;

use std::time::Duration;

/// Build a shared [`reqwest::Client`] with a descriptive user agent and sane
/// timeouts. warframe.market in particular expects a real user agent.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("warframe-lite/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()
        .expect("failed to build HTTP client")
}
