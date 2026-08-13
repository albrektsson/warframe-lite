//! Live Void Fissures from the warframestat.us API.
//!
//! General world state (Void Trader, open-world cycles) is out of scope — see
//! ADR-0007. This module keeps only the Fissure feed, which is a relic feature:
//! the mastery plan uses the active-fissure tiers to flag which owned relics are
//! crackable right now.
//!
//! We deserialize only the subset of fields warframe-lite currently uses;
//! serde ignores the (very large) remainder of the payload.
//!
//! Note: warframestat does not send pre-formatted `eta` strings on these
//! objects — only an `expiry` timestamp — so we compute remaining time
//! ourselves from `expiry`.

use serde::{Deserialize, Serialize};
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

/// A fissure-panel filter: OR within each field, AND across fields. An empty
/// set on any field means "no filter on that field" (matches the convention
/// wf-browse's `tier_matches()` already uses for relic filters).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FissureFilter {
    pub tiers: std::collections::HashSet<String>,
    pub mission_types: std::collections::HashSet<String>,
    /// "Normal" | "SteelPath" | "VoidStorm" — no spaces, so these tokens
    /// round-trip safely through the control-socket wire format unchanged.
    pub kinds: std::collections::HashSet<String>,
}

impl FissureFilter {
    /// Whether any field constrains the filter at all.
    pub fn is_active(&self) -> bool {
        !self.tiers.is_empty() || !self.mission_types.is_empty() || !self.kinds.is_empty()
    }

    pub fn matches(&self, f: &Fissure) -> bool {
        let kind = if f.is_storm {
            "VoidStorm"
        } else if f.is_hard {
            "SteelPath"
        } else {
            "Normal"
        };
        (self.tiers.is_empty() || self.tiers.contains(&f.tier))
            && (self.mission_types.is_empty() || self.mission_types.contains(&f.mission_type))
            && (self.kinds.is_empty() || self.kinds.contains(kind))
    }
}

/// The slice of world-state warframe-lite consumes: just the live Fissures
/// (see ADR-0007 — general world state is out of scope).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorldState {
    pub fissures: Vec<Fissure>,
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

    fn fissure(tier: &str, mission_type: &str, is_hard: bool, is_storm: bool) -> Fissure {
        Fissure {
            node: "Hepit (Void)".to_string(),
            mission_type: mission_type.to_string(),
            tier: tier.to_string(),
            is_hard,
            is_storm,
            expiry: String::new(),
        }
    }

    #[test]
    fn empty_filter_matches_everything() {
        let filter = FissureFilter::default();
        assert!(!filter.is_active());
        assert!(filter.matches(&fissure("Axi", "Capture", false, false)));
        assert!(filter.matches(&fissure("Lith", "Survival", true, false)));
    }

    #[test]
    fn tier_only_filter() {
        let filter = FissureFilter {
            tiers: ["Axi".to_string()].into_iter().collect(),
            ..Default::default()
        };
        assert!(filter.is_active());
        assert!(filter.matches(&fissure("Axi", "Capture", false, false)));
        assert!(!filter.matches(&fissure("Neo", "Capture", false, false)));
    }

    #[test]
    fn mission_type_only_filter() {
        let filter = FissureFilter {
            mission_types: ["Capture".to_string()].into_iter().collect(),
            ..Default::default()
        };
        assert!(filter.matches(&fissure("Axi", "Capture", false, false)));
        assert!(!filter.matches(&fissure("Axi", "Survival", false, false)));
    }

    #[test]
    fn kind_only_filter() {
        let steel_path = FissureFilter {
            kinds: ["SteelPath".to_string()].into_iter().collect(),
            ..Default::default()
        };
        assert!(steel_path.matches(&fissure("Axi", "Capture", true, false)));
        assert!(!steel_path.matches(&fissure("Axi", "Capture", false, false)));
        assert!(!steel_path.matches(&fissure("Axi", "Capture", false, true)));

        let normal = FissureFilter {
            kinds: ["Normal".to_string()].into_iter().collect(),
            ..Default::default()
        };
        assert!(normal.matches(&fissure("Axi", "Capture", false, false)));
        assert!(!normal.matches(&fissure("Axi", "Capture", true, false)));

        let void_storm = FissureFilter {
            kinds: ["VoidStorm".to_string()].into_iter().collect(),
            ..Default::default()
        };
        assert!(void_storm.matches(&fissure("Axi", "Capture", false, true)));
        assert!(!void_storm.matches(&fissure("Axi", "Capture", false, false)));
    }

    #[test]
    fn combined_filter_ors_within_field_and_ands_across_fields() {
        let filter = FissureFilter {
            tiers: ["Axi".to_string(), "Neo".to_string()].into_iter().collect(),
            mission_types: ["Capture".to_string()].into_iter().collect(),
            kinds: std::collections::HashSet::new(),
        };
        assert!(filter.matches(&fissure("Axi", "Capture", false, false)));
        assert!(filter.matches(&fissure("Neo", "Capture", false, false)));
        assert!(!filter.matches(&fissure("Axi", "Survival", false, false)));
        assert!(!filter.matches(&fissure("Lith", "Capture", false, false)));
    }
}
