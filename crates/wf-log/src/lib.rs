//! EE.log parsing and tailing for warframe-lite.
//!
//! Warframe's `EE.log` lines look like:
//!
//! ```text
//! 20.268 Script [Info]: ThemedSquadOverlay.lua: OnLoginComplete - squad overlay
//! ```
//!
//! i.e. `<seconds> <Subsystem> [<Level>]: <message>`, where `<seconds>` is a
//! float count since process start. This crate turns that stream into typed
//! [`Event`]s and provides a rotation-aware [`LogTailer`] for following the file
//! while the game runs.

pub mod account;
pub mod tail;

pub use account::{scan_account, AccountScan};
pub use tail::LogTailer;

/// A parsed log line (borrows from the source string).
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine<'a> {
    /// Seconds since the game process started.
    pub secs: f64,
    /// Subsystem, e.g. "Sys", "Script", "Net", "Game".
    pub subsystem: &'a str,
    /// Level, e.g. "Info", "Error", "Warning", "Diag".
    pub level: &'a str,
    /// The message body (leading whitespace trimmed).
    pub message: &'a str,
}

/// Parse a single raw log line into its structured parts.
///
/// Returns `None` for lines that do not match the standard prefix shape
/// (e.g. multi-line continuations such as stack traces).
pub fn parse_line(line: &str) -> Option<LogLine<'_>> {
    let (ts, rest) = line.split_once(' ')?;
    let secs: f64 = ts.parse().ok()?;
    // rest = "Script [Info]: message..."
    let (subsystem, rest) = rest.split_once(' ')?;
    // rest = "[Info]: message..."
    let rest = rest.strip_prefix('[')?;
    let (level, message) = rest.split_once("]:")?;
    if level.is_empty() {
        return None;
    }
    Some(LogLine {
        secs,
        subsystem,
        level,
        message: message.trim_start(),
    })
}

/// A gameplay event of interest, derived from one log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Account logged in, carrying the display name.
    LoggedIn(String),
    /// A host migration occurred (squad session changed host).
    HostMigration,
    /// The Void Fissure four-choice reward **selection screen** is now on
    /// screen (it stays up for a ~15-second countdown).
    ///
    /// Confirmed marker: `ProjectionRewardChoice.lua` emits "Relic rewards
    /// initialized" / "Got rewards" exactly when the screen appears.
    RelicRewardScreen,

    /// A relic was cracked (crack-aftermath dialog). The reward screen follows
    /// within ~a minute — used to open a polling window, since the player can
    /// also bring the screen up manually (Tab / progress) and log lines flush
    /// in bursts, so we can't rely on one precise moment.
    RelicCrack,

    /// The Void **Relics inventory** screen was opened (the relic-management
    /// grid). Confirmed marker: `ThemedProjectionManager.lua` populates its
    /// inventory grid when the screen appears — used to start an owned-relic scan.
    RelicInventoryOpen,

    /// The **Inventory/Sell** screen was opened (or its grid repopulated by a
    /// category-tab switch). Confirmed marker (issue #37's region-calibration
    /// research, resolving #31): `InventoryTest.lua` populates its item grid
    /// on open and on each tab switch — used to start an owned-Prime-Part scan.
    InventorySellOpen,
}

/// Substrings that mark the reward **selection screen appearing**. Confirmed
/// against a live fissure: `ProjectionRewardChoice.lua` logs these when the
/// four-choice screen shows. Deliberately excludes the "Selection countdown
/// done" / "shut down" lines, which mean the screen has closed.
pub const RELIC_REWARD_MARKERS: &[&str] = &[
    "ProjectionRewardChoice.lua: Relic rewards initialized",
    "ProjectionRewardChoice.lua: Got rewards",
];

/// Substrings that mark a relic **crack** (reward screen imminent).
pub const RELIC_CRACK_MARKERS: &[&str] = &["DVRCAftermath"];

/// Substring that marks the Void **Relics inventory** screen opening. Fires when
/// `ThemedProjectionManager` populates its relic grid. (It also emits a
/// `…GridEnd` line ~0.1s later, which contains this substring too; the consumer
/// debounces, so a double-fire is harmless.)
pub const RELIC_INVENTORY_MARKER: &str = "ThemedProjectionManager.lua: PopulateInventoryGrid";

/// Substring that marks the **Inventory/Sell** screen's grid (re)populating —
/// on open, and again on every category-tab switch (e.g. to Prime Parts).
/// Empirically confirmed against a real `EE.log` capture (issue #37's
/// region-calibration research, resolving #31): the predicted line
/// `InventoryTest.lua: PopulateGrid()` fired exactly as expected. Deliberately
/// excludes the trailing `()` so this also matches the `PopulateGrid
/// complete` line `InventoryTest.lua` emits ~0.8s later — the same
/// consumer-debounces-a-double-fire shape [`RELIC_INVENTORY_MARKER`] has.
pub const INVENTORY_SELL_MARKER: &str = "InventoryTest.lua: PopulateGrid";

/// Classify a parsed line into an [`Event`], if it matches a known pattern.
pub fn classify(line: &LogLine<'_>) -> Option<Event> {
    // Relic markers appear on several subsystems (HudRedux/Transmission on
    // Script, GetQueuedTransmission on Game), so check them before subsystem-
    // specific rules and regardless of subsystem.
    if RELIC_REWARD_MARKERS.iter().any(|m| line.message.contains(m)) {
        return Some(Event::RelicRewardScreen);
    }
    if RELIC_CRACK_MARKERS.iter().any(|m| line.message.contains(m)) {
        return Some(Event::RelicCrack);
    }
    if line.message.contains(RELIC_INVENTORY_MARKER) {
        return Some(Event::RelicInventoryOpen);
    }
    if line.message.contains(INVENTORY_SELL_MARKER) {
        return Some(Event::InventorySellOpen);
    }
    match line.subsystem {
        "Sys" if line.message.starts_with("Logged in ") => {
            let name = line.message.trim_start_matches("Logged in ").trim();
            Some(Event::LoggedIn(name.to_string()))
        }
        "Game" if line.message.contains("HostMigration") => Some(Event::HostMigration),
        _ => None,
    }
}

/// Convenience: parse a raw line and classify it in one step.
pub fn event_from_line(line: &str) -> Option<Event> {
    classify(&parse_line(line)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_line() {
        let l = parse_line("20.268 Script [Info]: ThemedSquadOverlay.lua: OnLoginComplete")
            .unwrap();
        assert_eq!(l.secs, 20.268);
        assert_eq!(l.subsystem, "Script");
        assert_eq!(l.level, "Info");
        assert_eq!(l.message, "ThemedSquadOverlay.lua: OnLoginComplete");
    }

    #[test]
    fn parses_sys_error_line() {
        let l = parse_line("9.187 Sys [Error]: something had a NULL reward entry").unwrap();
        assert_eq!(l.subsystem, "Sys");
        assert_eq!(l.level, "Error");
        assert!(l.message.contains("NULL reward"));
    }

    #[test]
    fn rejects_non_log_lines() {
        assert!(parse_line("").is_none());
        assert!(parse_line("just some continuation text").is_none());
        assert!(parse_line("no-timestamp Script [Info]: x").is_none());
    }

    #[test]
    fn detects_login_event() {
        let ev = event_from_line("18.148 Sys [Info]: Logged in SpiroTris");
        assert_eq!(ev, Some(Event::LoggedIn("SpiroTris".to_string())));
    }

    #[test]
    fn detects_host_migration() {
        let ev = event_from_line("7.714 Game [Info]: HostMigration::ResetOldServerConnection()");
        assert_eq!(ev, Some(Event::HostMigration));
    }

    #[test]
    fn detects_relic_reward_marker() {
        // Real lines from a live fissure: the selection screen appearing.
        for line in [
            "6990.602 Script [Info]: ProjectionRewardChoice.lua: Relic rewards initialized",
            "6990.924 Script [Info]: ProjectionRewardChoice.lua: Got rewards",
        ] {
            assert_eq!(event_from_line(line), Some(Event::RelicRewardScreen));
        }
    }

    #[test]
    fn detects_relic_inventory_open() {
        // Real line: opening the Void Relics management screen.
        assert_eq!(
            event_from_line("49.227 Script [Info]: ThemedProjectionManager.lua: PopulateInventoryGrid"),
            Some(Event::RelicInventoryOpen)
        );
        // The ~0.1s-later GridEnd line also matches (consumer debounces).
        assert_eq!(
            event_from_line("49.306 Script [Info]: ThemedProjectionManager.lua: PopulateInventoryGridEnd"),
            Some(Event::RelicInventoryOpen)
        );
    }

    #[test]
    fn detects_inventory_sell_open() {
        // Real captured lines (issue #37): opening the Inventory/Sell screen.
        assert_eq!(
            event_from_line("289.679 Script [Info]: InventoryTest.lua: PopulateGrid()"),
            Some(Event::InventorySellOpen)
        );
        // The ~0.8s-later "complete" line also matches (consumer debounces),
        // same shape as the relic inventory marker's GridEnd double-fire.
        assert_eq!(
            event_from_line("290.477 Script [Info]: InventoryTest.lua: PopulateGrid complete"),
            Some(Event::InventorySellOpen)
        );
    }

    #[test]
    fn inventory_sell_debug_lines_are_not_events() {
        // Real captured lines that share the InventoryTest.lua source file but
        // aren't grid-population — must not spuriously arm a scan window.
        for line in [
            "281.706 Script [Info]: InventoryTest.lua: DBG: HudVis 0",
            "289.676 Script [Info]: InventoryTest.lua: InventoryTest - CurrMode: Inventory",
        ] {
            assert_eq!(event_from_line(line), None);
        }
    }

    #[test]
    fn reward_screen_close_is_not_an_event() {
        // Screen closing must NOT re-trigger a scan.
        assert_eq!(
            event_from_line(
                "7005.925 Script [Info]: ProjectionRewardChoice.lua: Selection countdown done"
            ),
            None
        );
    }

    #[test]
    fn crack_dialog_is_a_crack_event() {
        // The crack aftermath opens the polling window (reward screen ~1min out).
        assert_eq!(
            event_from_line(
                "6935.733 Script [Info]: HudRedux.lua: Queuing new transmission: DVRCAftermathLotus"
            ),
            Some(Event::RelicCrack)
        );
    }
}
