//! Reads Warframe account inventory state from the live game process's
//! memory, via the token-relay technique validated in issue #52: scan
//! `Warframe.x64.exe`'s `/proc/[pid]/mem` for the `?accountId=...&nonce=...`
//! session marker, then echo it once to DE's own `inventory.php` endpoint.
//!
//! Read-only per [ADR-0001](../../../docs/adr/0001-observe-only-never-touch-game-process.md);
//! the nonce is never held as a credential
//! ([ADR-0013](../../../docs/adr/0013-token-relay-session-nonce-is-not-a-credential.md)).
//! Ephemeral: [`scan_and_fetch`] never caches or persists the *result* —
//! every call that actually reaches the network re-scans process memory and
//! re-fetches from scratch. Single attempt throughout — a failed scan or a
//! failed HTTP call is a plain error, never retried automatically.
//!
//! What *is* persisted is a lightweight [`SCAN_COOLDOWN`] timestamp: DE's
//! `inventory.php` is a real endpoint on DE's own domain, not a community
//! API, and a runaway click loop (or a scripted `mem-scan` invocation)
//! hammering it risks tripping an IP-level flag on infrastructure that may
//! be shared with login — see the login-troubleshooting writeup this was
//! added from. The cooldown arms the moment a request actually goes out to
//! DE's server — on a 2xx, on a non-2xx (a `403`/`429` is the clearest sign
//! we're already being flagged, so it must block retries at least as hard
//! as success does), and on a network-level failure alike. Only a scan that
//! never reaches the network at all (game not running, no session marker
//! found yet in memory) skips it, so retrying while waiting for the game to
//! finish loading isn't penalized.
//!
//! Only linked into `wf-lite` behind its `mem-scan` cargo feature (see the
//! workspace `Cargo.toml`); invoking the `mem-scan` subcommand at all **is**
//! the explicit in-the-moment consent this crate's design assumes — it never
//! prompts for confirmation itself.

mod equipment;
mod foundry;
mod inventory;
mod level_keys;
mod owned_parts;
mod persist;
mod process;
mod relics;
mod riven;

pub use equipment::{parse_owned_equipment, EquipmentCategory, OwnedEquipment, OwnedItem};
pub use foundry::{parse_foundry, FoundryState, OwnedRecipe, PendingBuild};
pub use inventory::fetch_inventory;
pub use level_keys::{parse_level_keys, LevelKey, LevelKeyState};
pub use owned_parts::{parse_owned_parts, OwnedPartRaw, OwnedPartsState};
pub use persist::{
    write_owned_parts, write_owned_relics, write_owned_rivens, PartsWriteReport, RelicsWriteReport,
    RivensWriteReport,
};
pub use process::{find_pids, scan_authz, Authz};
pub use relics::{parse_owned_relics, OwnedRelic, OwnedRelicState};
pub use riven::{parse_rivens, Riven, RivenState, RivenStat};

use std::time::Duration;

use anyhow::Result;

/// Minimum time between two calls that actually reach DE's `inventory.php`.
/// See the module docs for why this exists.
pub const SCAN_COOLDOWN: Duration = Duration::from_secs(30 * 60);

const COOLDOWN_MARKER_FILE: &str = "mem-scan-last.json";

/// `Some(time left)` if a scan within [`SCAN_COOLDOWN`] already landed,
/// `None` if a scan may proceed now (including when no marker exists yet).
fn cooldown_remaining(marker_file: &str) -> Option<Duration> {
    let cached = wf_cache::load_blob::<()>(marker_file)?;
    SCAN_COOLDOWN.checked_sub(cached.age())
}

fn record_scan(marker_file: &str) {
    let _ = wf_cache::save_blob(marker_file, &());
}

/// End-to-end: find the running game, scan its memory for the session authz,
/// and fetch the raw inventory JSON body. Field parsing is a separate,
/// later ticket (#57) — this only proves the plumbing works.
pub async fn scan_and_fetch(client: &reqwest::Client) -> Result<String> {
    if let Some(remaining) = cooldown_remaining(COOLDOWN_MARKER_FILE) {
        let mins = remaining.as_secs().div_ceil(60);
        anyhow::bail!(
            "Scan Memory has a {}-minute cooldown, to avoid hammering DE's own inventory \
             endpoint — try again in about {mins} more minute{}",
            SCAN_COOLDOWN.as_secs() / 60,
            if mins == 1 { "" } else { "s" }
        );
    }

    let pids = process::find_pids()?;
    let mut last_err: Option<anyhow::Error> = None;
    for pid in pids {
        match process::scan_authz(pid) {
            Ok(authz) => {
                // Arm the cooldown the moment a request actually goes out to
                // DE's server, regardless of outcome — a 403/429 is the
                // clearest possible sign we're already being rate-limited or
                // flagged, so it must block retries at least as hard as a
                // plain success does, not less.
                let result = inventory::fetch_inventory(client, &authz).await;
                record_scan(COOLDOWN_MARKER_FILE);
                return result;
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no candidate process yielded a session marker")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Uses a per-process-id marker filename (mirrors `wf-cache`'s own tests)
    // so this never touches a real user's `mem-scan-last.json`.
    fn test_marker_file() -> String {
        format!("wf-mem-test-cooldown-{}.json", std::process::id())
    }

    fn cleanup(marker_file: &str) {
        if let Ok(dir) = wf_cache::cache_dir() {
            let _ = std::fs::remove_file(dir.join(marker_file));
        }
    }

    #[test]
    fn no_marker_means_no_cooldown() {
        let marker_file = format!("{}-none", test_marker_file());
        cleanup(&marker_file);
        assert_eq!(cooldown_remaining(&marker_file), None);
    }

    #[test]
    fn a_fresh_scan_arms_the_cooldown() {
        let marker_file = format!("{}-fresh", test_marker_file());
        cleanup(&marker_file);
        record_scan(&marker_file);
        let remaining = cooldown_remaining(&marker_file).expect("cooldown should be active");
        assert!(remaining <= SCAN_COOLDOWN && remaining > SCAN_COOLDOWN - Duration::from_secs(5));
        cleanup(&marker_file);
    }
}
