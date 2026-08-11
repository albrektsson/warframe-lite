//! Stand-in for `ocr_enabled.rs` when the `ocr` cargo feature is **not**
//! compiled in (see the root `Cargo.toml`'s `ocr` feature and README.md).
//! `main.rs` picks between the two via `#[cfg]`/`#[path]` on the `mod ocr;`
//! declaration, so every OCR-dependent CLI command and the live overlay's
//! reward-picker auto-detection route through this same public surface
//! either way — this variant just explains, clearly, why nothing happened,
//! instead of failing to compile or silently doing nothing.

use anyhow::Result;
use wf_config::Config;

use crate::{RelicScanStatus, RewardState};

const MSG: &str = "OCR isn't compiled into this build — rebuild with `--features ocr` (see README.md).";

/// Stand-in for the real `PrewarmCtx` — never constructed, since
/// [`start_relic_watch`] always returns `None` in this build.
#[derive(Clone)]
pub(crate) struct PrewarmCtx(());

/// Always reports relic auto-detection as off, with the reason, and does
/// nothing else — the OCR-feature-off counterpart of `ocr_enabled.rs`'s
/// `start_relic_watch`, called unconditionally from `run_overlay` so the
/// rest of the overlay (fissures, control socket, window placement) still
/// works normally without the `ocr` feature.
pub(crate) async fn start_relic_watch(
    _config: &Config,
    _client: &reqwest::Client,
    _reward: RewardState,
    _relic_scan_status: RelicScanStatus,
) -> Option<PrewarmCtx> {
    println!("  relic auto-detect: OFF ({MSG})");
    None
}

/// No-op: [`start_relic_watch`] never returns `Some`, so this is never
/// actually called with a real context — kept only so `run_overlay`'s
/// renderer loop doesn't need feature-specific branches of its own.
pub(crate) async fn prewarm_new_tiers(_ctx: PrewarmCtx, _tiers: std::collections::HashSet<String>) {}

pub(crate) fn ocr_test() -> Result<()> {
    println!("\n== OCR test ==\n  {MSG}");
    Ok(())
}

pub(crate) fn ocr_file() -> Result<()> {
    println!("{MSG}");
    Ok(())
}

pub(crate) async fn relic_file(_config: &Config) -> Result<()> {
    println!("\n== Relic file ==\n  {MSG}");
    Ok(())
}

pub(crate) async fn relic_scan(_config: &Config) -> Result<()> {
    println!("\n== Relic scan ==\n  {MSG}");
    Ok(())
}

pub(crate) async fn relic_grid_file() -> Result<()> {
    println!("\n== Relic grid file ==\n  {MSG}");
    Ok(())
}

pub(crate) async fn inventory_grid_file() -> Result<()> {
    println!("\n== Inventory grid file ==\n  {MSG}");
    Ok(())
}
