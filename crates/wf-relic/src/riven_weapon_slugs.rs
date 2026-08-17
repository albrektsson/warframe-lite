//! Disk-cached wrapper around [`wf_data::riven_market::weapon_catalogue`] —
//! `GET /v2/riven/weapons`, whose `game_ref -> slug` map is the join key
//! `wf-browse` needs to fire the Rivens tab's lazy per-weapon Verdict fetch
//! (ADR-0020, see `riven_verdict_targets`). Originally fetched fresh on
//! every launch with no cache of its own — the reasoning being that it isn't
//! the endpoint ADR-0020's rate limiter scopes to, and is small (~400
//! entries). That reasoning missed that the endpoint can still 429 on its
//! own (confirmed live, 2026-08-17), and with no cache to fall back on, a
//! single rate-limited launch left `riven_slug_by_weapon` empty for the
//! *whole session* — starving every weapon's live Verdict fetch, since
//! `riven_verdict_targets` has nothing to query without this map. Mirrors
//! [`crate::riven_catalogue::RivenCatalogue::load_cached`]'s weekly TTL /
//! stale-served-on-failure pattern exactly.

use std::collections::HashMap;
use std::time::Duration;

const CACHE_FILE: &str = "riven-market-weapon-slugs-v1.json";

async fn fetch(client: &reqwest::Client, platform: &str) -> anyhow::Result<HashMap<String, String>> {
    let weapons = wf_data::riven_market::weapon_catalogue(client, platform).await?;
    Ok(weapons.into_iter().map(|w| (w.game_ref, w.slug)).collect())
}

/// Fetch + cache (weekly TTL, stale-served on failure), mirroring
/// [`crate::riven_catalogue::RivenCatalogue::load_cached`]. Returns the
/// `game_ref -> slug` map directly (rather than the raw
/// [`wf_data::riven_market::RivenWeapon`] list) since that's the only shape
/// any caller needs, and `RivenWeapon` itself isn't `Serialize`.
pub async fn load_cached(
    client: &reqwest::Client,
    platform: &str,
    ttl: Duration,
) -> anyhow::Result<HashMap<String, String>> {
    if let Some(cached) = wf_cache::load_blob::<HashMap<String, String>>(CACHE_FILE) {
        if cached.age() < ttl {
            tracing::info!("riven market weapon slugs from cache ({} weapons)", cached.value.len());
            return Ok(cached.value);
        }
        match fetch(client, platform).await {
            Ok(map) => {
                let _ = wf_cache::save_blob(CACHE_FILE, &map);
                return Ok(map);
            }
            Err(e) => {
                tracing::warn!("riven market weapon slug refresh failed ({e}); using stale cache");
                return Ok(cached.value);
            }
        }
    }
    let map = fetch(client, platform).await?;
    let _ = wf_cache::save_blob(CACHE_FILE, &map);
    Ok(map)
}
