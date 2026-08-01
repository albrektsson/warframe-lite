//! Live world-state from the warframestat.us API.
//!
//! We deserialize only the subset of fields warframe-lite currently uses;
//! serde ignores the (very large) remainder of the payload.
//!
//! Note: warframestat does not send pre-formatted `eta`/`timeLeft` strings on
//! these objects — only an `expiry` timestamp — so we compute remaining time
//! ourselves from `expiry`.

use serde::Deserialize;
use time::OffsetDateTime;

const BASE: &str = "https://api.warframestat.us";

/// A single active Void Fissure.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Fissure {
    /// Human-readable node, e.g. "Hepit (Void)".
    pub node: String,
    /// Mission type, e.g. "Capture".
    pub mission_type: String,
    /// Relic tier, e.g. "Lith", "Meso", "Neo", "Axi", "Requiem".
    pub tier: String,
    /// Whether this is a Steel Path fissure.
    pub is_hard: bool,
    /// Whether this is a Void Storm (Railjack) fissure.
    pub is_storm: bool,
    /// RFC3339 expiry timestamp.
    pub expiry: String,
}

impl Fissure {
    /// Time remaining until expiry, pre-formatted (e.g. "42m 10s").
    pub fn eta(&self) -> String {
        format_eta(&self.expiry)
    }

    /// Whether the fissure has not yet expired.
    pub fn active(&self) -> bool {
        !is_expired(&self.expiry)
    }
}

/// The Void Trader (Baro Ki'Teer).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VoidTrader {
    /// Trader name, e.g. "Baro Ki'Teer".
    pub character: String,
    /// Relay where he is / will be.
    pub location: String,
    /// Whether he is currently present.
    pub active: bool,
    /// RFC3339 arrival time (relevant while away).
    pub activation: String,
    /// RFC3339 departure time (relevant while present).
    pub expiry: String,
}

impl VoidTrader {
    /// Formatted time until Baro arrives.
    pub fn arrives_in(&self) -> String {
        format_eta(&self.activation)
    }

    /// Formatted time until Baro leaves.
    pub fn leaves_in(&self) -> String {
        format_eta(&self.expiry)
    }
}

/// A simple day/night-style cycle (Cetus, Vallis, Cambion, ...).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Cycle {
    /// Current state, e.g. "day"/"night", "warm"/"cold", "fass"/"vome".
    pub state: String,
    /// RFC3339 expiry of the current state.
    pub expiry: String,
}

impl Cycle {
    /// Formatted time until the next state.
    pub fn time_left(&self) -> String {
        format_eta(&self.expiry)
    }
}

/// The slice of world-state warframe-lite currently consumes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorldState {
    pub fissures: Vec<Fissure>,
    pub void_trader: VoidTrader,
    pub cetus_cycle: Cycle,
    pub vallis_cycle: Cycle,
    pub cambion_cycle: Cycle,
}

impl WorldState {
    /// The relic tiers (e.g. "Axi", "Requiem") with at least one currently
    /// active Fissure — used to flag which owned relics are crackable right
    /// now.
    pub fn active_fissure_tiers(&self) -> std::collections::HashSet<String> {
        self.fissures.iter().filter(|f| f.active()).map(|f| f.tier.clone()).collect()
    }
}

/// Fetch the current world-state for `platform` (e.g. "pc").
pub async fn fetch(client: &reqwest::Client, platform: &str) -> anyhow::Result<WorldState> {
    let url = format!("{BASE}/{platform}?language=en");
    tracing::debug!("GET {url}");
    let ws = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json::<WorldState>()
        .await?;
    Ok(ws)
}

/// Parse an RFC3339 timestamp, returning `None` on failure or empty input.
fn parse_ts(ts: &str) -> Option<OffsetDateTime> {
    if ts.is_empty() {
        return None;
    }
    OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339).ok()
}

fn is_expired(ts: &str) -> bool {
    match parse_ts(ts) {
        Some(t) => t <= OffsetDateTime::now_utc(),
        None => false,
    }
}

/// Format the time remaining until `ts` as e.g. "1h 4m", "42m 10s", or "now".
fn format_eta(ts: &str) -> String {
    let Some(target) = parse_ts(ts) else {
        return String::new();
    };
    let remaining = target - OffsetDateTime::now_utc();
    format_duration(remaining)
}

/// Human-friendly duration formatting, showing the two most significant units.
fn format_duration(d: time::Duration) -> String {
    let secs = d.whole_seconds();
    if secs <= 0 {
        return "now".to_string();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    let s = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(time::Duration::seconds(0)), "now");
        assert_eq!(format_duration(time::Duration::seconds(-5)), "now");
        assert_eq!(format_duration(time::Duration::seconds(42)), "42s");
        assert_eq!(format_duration(time::Duration::seconds(130)), "2m 10s");
        assert_eq!(format_duration(time::Duration::seconds(3700)), "1h 1m");
        assert_eq!(format_duration(time::Duration::seconds(90_000)), "1d 1h");
    }

    #[test]
    fn empty_timestamp_is_not_expired_and_has_blank_eta() {
        assert!(!is_expired(""));
        assert_eq!(format_eta(""), "");
    }
}
