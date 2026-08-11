//! Persist decoded owned-relic mem-scan results to `owned-relics.json`.
//!
//! Moved out of `wf-lite`'s `src/main.rs` (#72) so both the CLI (`wf-lite
//! mem-scan`) and the GUI (`wf-browse`'s Home-tab Scan Memory button) share
//! one implementation instead of two copies of the same decode+snapshot+
//! apply+save logic. Pure side-effecting logic with a typed result — no
//! stdout printing here; each caller formats [`RelicsWriteReport`] into its
//! own console text / status line.

use crate::relics::OwnedRelicState;

/// Outcome of [`write_owned_relics`]: how many owned-relic entries were
/// written to `owned-relics.json`, how many raw entries `relic_names`
/// couldn't decode (skipped, not applied — already logged via
/// `tracing::warn!` by this function so it isn't silently dropped from view),
/// and whether the write actually landed on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelicsWriteReport {
    /// Entries applied to `owned-relics.json`'s exact snapshot.
    pub written: usize,
    /// Raw entries `relic_names` had no `(code, refinement)` for, and so
    /// couldn't be included in the snapshot.
    pub undecoded: usize,
    /// Whether `owned-relics.json` was actually saved. `false` means
    /// [`wf_cache::save_blob`] failed — already logged via `tracing::warn!`
    /// by this function — so a caller printing a "wrote N entries" success
    /// line should skip it in that case.
    pub saved: bool,
}

/// Write every decoded owned-relic entry to `owned-relics.json` as an exact,
/// [`wf_relic::Source::MemScan`]-tagged snapshot (ADR-0009's
/// revision): the mem-scanned inventory becomes the new ground truth, and
/// any prior entry not covered by it is dropped as a confirmed zero (see
/// [`wf_relic::apply_exact_snapshot`]'s doc for why absence is authoritative
/// here). Running `mem-scan` (or clicking Scan Memory) at all is already the
/// map's required in-the-moment consent — no separate opt-in is needed to
/// let its findings take effect. An entry `relic_names` couldn't decode (no
/// `(code, refinement)` to key `owned-relics.json` by) is skipped here but
/// counted in the returned report's `undecoded` field.
pub fn write_owned_relics(
    state: &OwnedRelicState,
    relic_names: &wf_relic::RelicNameIndex,
) -> RelicsWriteReport {
    let mut snapshot: Vec<(String, wf_relic::Refinement, u32)> = Vec::new();
    let mut undecoded = 0usize;
    for r in &state.relics {
        let decoded = relic_names.lookup(&r.item_type).and_then(|id| {
            wf_relic::Refinement::from_label(&id.refinement).map(|refinement| (id.display(), refinement))
        });
        match decoded {
            Some((code, refinement)) => snapshot.push((code, refinement, r.item_count)),
            None => undecoded += 1,
        }
    }
    if undecoded > 0 {
        let (noun, verb) = if undecoded == 1 { ("entry", "was") } else { ("entries", "were") };
        tracing::warn!(
            "mem-scan: {undecoded} owned-relic {noun} could not be decoded and {verb} not applied to {}",
            wf_relic::OWNED_RELICS_FILE
        );
    }

    let mut owned: wf_relic::OwnedRelics = wf_cache::load_blob_or_reset(wf_relic::OWNED_RELICS_FILE);
    wf_relic::apply_exact_snapshot(&mut owned, &snapshot);
    let written = snapshot.len();
    let saved = match wf_cache::save_blob(wf_relic::OWNED_RELICS_FILE, &owned) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("failed to write {}: {e}", wf_relic::OWNED_RELICS_FILE);
            false
        }
    };

    RelicsWriteReport { written, undecoded, saved }
}
