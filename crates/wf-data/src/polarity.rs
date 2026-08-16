//! A Riven's polarity, unifying two upstream spellings into one type.
//!
//! The mobile inventory API's `Upgrades[]` fingerprint (`pol`, see
//! `wf_mem::riven::Riven`) carries DE's own internal AP codes
//! (`AP_ATTACK`, `AP_DEFENSE`, `AP_TACTIC`, `AP_POWER`, `AP_WARD`), while
//! warframe.market's auction listings (`item.polarity`, see
//! `crate::riven_market::AuctionItem`) spell the same five values as plain
//! lowercase names (`"madurai"`, `"vazarin"`, ...). Both parse into this one
//! canonical type at the `wf-data` boundary — see
//! `docs/adr/0018-riven-polarity-is-a-typed-value.md`.

use serde::{Deserialize, Serialize};

/// The slot shape a Riven carries, one of five known polarities — or
/// [`Polarity::Unknown`], preserving whatever raw string didn't match
/// either upstream spelling (a new polarity DE added, or a malformed
/// value), rather than losing the source data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Polarity {
    Madurai,
    Vazarin,
    Naramon,
    Zenurik,
    Unairu,
    Unknown(String),
}

impl Polarity {
    /// Parse DE's internal AP code (`AP_ATTACK`, ...) from the mobile
    /// inventory API's riven fingerprint `pol` field.
    pub fn from_ap_code(code: &str) -> Self {
        match code {
            "AP_ATTACK" => Self::Madurai,
            "AP_DEFENSE" => Self::Vazarin,
            "AP_TACTIC" => Self::Naramon,
            "AP_POWER" => Self::Zenurik,
            "AP_WARD" => Self::Unairu,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// Parse warframe.market's plain-name spelling (`"madurai"`, ...) from
    /// an auction listing's `item.polarity` field. Matched
    /// case-insensitively — warframe.market's own casing isn't documented
    /// anywhere and isn't worth depending on.
    pub fn from_market_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "madurai" => Self::Madurai,
            "vazarin" => Self::Vazarin,
            "naramon" => Self::Naramon,
            "zenurik" => Self::Zenurik,
            "unairu" => Self::Unairu,
            _ => Self::Unknown(name.to_string()),
        }
    }

    /// The display name players know — the raw source string for
    /// [`Polarity::Unknown`], since there's no known display name to fall
    /// back to.
    pub fn display_name(&self) -> &str {
        match self {
            Self::Madurai => "Madurai",
            Self::Vazarin => "Vazarin",
            Self::Naramon => "Naramon",
            Self::Zenurik => "Zenurik",
            Self::Unairu => "Unairu",
            Self::Unknown(raw) => raw,
        }
    }
}

impl std::fmt::Display for Polarity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_known_ap_code() {
        assert_eq!(Polarity::from_ap_code("AP_ATTACK"), Polarity::Madurai);
        assert_eq!(Polarity::from_ap_code("AP_DEFENSE"), Polarity::Vazarin);
        assert_eq!(Polarity::from_ap_code("AP_TACTIC"), Polarity::Naramon);
        assert_eq!(Polarity::from_ap_code("AP_POWER"), Polarity::Zenurik);
        assert_eq!(Polarity::from_ap_code("AP_WARD"), Polarity::Unairu);
    }

    #[test]
    fn unrecognized_ap_code_falls_back_to_unknown() {
        assert_eq!(
            Polarity::from_ap_code("AP_UMBRA"),
            Polarity::Unknown("AP_UMBRA".to_string())
        );
    }

    #[test]
    fn parses_every_known_market_name_case_insensitively() {
        assert_eq!(Polarity::from_market_name("madurai"), Polarity::Madurai);
        assert_eq!(Polarity::from_market_name("Vazarin"), Polarity::Vazarin);
        assert_eq!(Polarity::from_market_name("NARAMON"), Polarity::Naramon);
        assert_eq!(Polarity::from_market_name("zenurik"), Polarity::Zenurik);
        assert_eq!(Polarity::from_market_name("unairu"), Polarity::Unairu);
    }

    #[test]
    fn unrecognized_market_name_falls_back_to_unknown_preserving_original_casing() {
        assert_eq!(
            Polarity::from_market_name("Penjaga"),
            Polarity::Unknown("Penjaga".to_string())
        );
    }

    #[test]
    fn display_name_is_the_player_facing_name() {
        assert_eq!(Polarity::Madurai.display_name(), "Madurai");
        assert_eq!(Polarity::Unairu.to_string(), "Unairu");
    }

    #[test]
    fn unknown_displays_its_raw_source_string() {
        assert_eq!(Polarity::Unknown("AP_UMBRA".to_string()).display_name(), "AP_UMBRA");
    }

    #[test]
    fn roundtrips_through_json() {
        let json = serde_json::to_string(&Polarity::Vazarin).unwrap();
        let back: Polarity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Polarity::Vazarin);
    }
}
