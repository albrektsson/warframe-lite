//! Reads Warframe account inventory state from the live game process's
//! memory, via the token-relay technique validated in issue #52: scan
//! `Warframe.x64.exe`'s `/proc/[pid]/mem` for the `?accountId=...&nonce=...`
//! session marker, then echo it once to DE's own `inventory.php` endpoint.
//!
//! Read-only per [ADR-0001](../../../docs/adr/0001-observe-only-never-touch-game-process.md);
//! the nonce is never held as a credential
//! ([ADR-0013](../../../docs/adr/0013-token-relay-session-nonce-is-not-a-credential.md)).
//! Ephemeral: [`scan_and_fetch`] never caches or persists anything to disk —
//! every call re-scans process memory and re-fetches from scratch. Single
//! attempt throughout — a failed scan or a failed HTTP call is a plain
//! error, never retried automatically.
//!
//! Only linked into `wf-lite` behind its `mem-scan` cargo feature (see the
//! workspace `Cargo.toml`); invoking the `mem-scan` subcommand at all **is**
//! the explicit in-the-moment consent this crate's design assumes — it never
//! prompts for confirmation itself.

mod foundry;
mod inventory;
mod process;
mod riven;

pub use foundry::{parse_foundry, FoundryState, OwnedRecipe, PendingBuild};
pub use inventory::fetch_inventory;
pub use process::{find_pids, scan_authz, Authz};
pub use riven::{parse_rivens, Riven, RivenState, RivenStat};

use anyhow::Result;

/// End-to-end: find the running game, scan its memory for the session authz,
/// and fetch the raw inventory JSON body. Field parsing is a separate,
/// later ticket (#57) — this only proves the plumbing works.
pub async fn scan_and_fetch(client: &reqwest::Client) -> Result<String> {
    let pids = process::find_pids()?;
    let mut last_err: Option<anyhow::Error> = None;
    for pid in pids {
        match process::scan_authz(pid) {
            Ok(authz) => return inventory::fetch_inventory(client, &authz).await,
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no candidate process yielded a session marker")))
}
