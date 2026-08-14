//! Persist decoded owned-relic and owned-Prime-Part-component mem-scan
//! results to `owned-relics.json`/`owned-prime-parts.json`.
//!
//! Moved out of `wf-lite`'s `src/main.rs` (#72, extended to parts by #81) so
//! both the CLI (`wf-lite mem-scan`) and the GUI (`wf-browse`'s Home-tab
//! Scan Memory button) share one implementation instead of two copies of the
//! same decode+snapshot+apply+save logic. Pure side-effecting logic with a
//! typed result — no stdout printing here; each caller formats
//! [`RelicsWriteReport`]/[`PartsWriteReport`] into its own console text /
//! status line.

use crate::owned_parts::OwnedPartsState;
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

/// Outcome of [`write_owned_parts`]: how many owned-Prime-Part entries were
/// written to `owned-prime-parts.json`, how many raw entries were dropped
/// (either a non-Prime frame/weapon component, or a Prime `quantities`
/// doesn't recognize — the two aren't distinguished, since both are the
/// ordinary "not a trackable Prime Part" case rather than a data gap; see
/// [`write_owned_parts`]'s doc), and whether the write actually landed on
/// disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartsWriteReport {
    /// Entries applied to `owned-prime-parts.json`'s exact snapshot.
    pub written: usize,
    /// Raw entries that weren't a recognized Prime Part component — expected
    /// to be the majority of a typical account's `MiscItems[]` built
    /// components (most players own far more non-Prime gear), so this is
    /// reported for visibility only, not logged as a warning the way
    /// [`RelicsWriteReport::undecoded`] is (that count really is an
    /// unexpected name-index gap).
    pub skipped: usize,
    /// Whether `owned-prime-parts.json` was actually saved. `false` means
    /// [`wf_cache::save_blob`] failed — already logged via
    /// `tracing::warn!` by this function — so a caller printing a "wrote N
    /// entries" success line should skip it in that case.
    pub saved: bool,
}

/// Write every decoded owned-Prime-Part-component entry to
/// `owned-prime-parts.json` as an exact, [`wf_relic::owned::Source::MemScan`]-tagged
/// snapshot (ADR-0009's revision, applied to Prime Parts per issue #81): the
/// mem-scanned inventory becomes the new ground truth, and any prior entry
/// not covered by it is dropped as a confirmed zero (see
/// [`wf_relic::owned_parts::apply_exact_snapshot`]'s doc for why absence is
/// authoritative here). Running `mem-scan` (or clicking Scan Memory) at all
/// is already the map's required in-the-moment consent — no separate opt-in
/// is needed to let its findings take effect.
///
/// A raw entry [`wf_relic::owned_part_from_item_type`] can't resolve to a
/// `PrimePart` — because it isn't a Prime component at all, or `quantities`
/// doesn't recognize its prime — is skipped here, counted in the returned
/// report's `skipped` field. Unlike [`write_owned_relics`]'s `undecoded`,
/// this is *not* logged via `tracing::warn!`: a typical account's
/// `MiscItems[]` built components are mostly non-Prime gear the OCR scanner
/// never tracked either, so warning on every mem-scan would be noise, not
/// signal — the raw entries themselves are already visible via the CLI's
/// own "Owned Parts" listing (`src/main.rs`'s `print_owned_parts`).
pub fn write_owned_parts(
    state: &OwnedPartsState,
    quantities: &wf_relic::PartQuantities,
) -> PartsWriteReport {
    let mut snapshot: Vec<(wf_relic::PrimePart, u32)> = Vec::new();
    let mut skipped = 0usize;
    for p in &state.parts {
        match wf_relic::owned_part_from_item_type(&p.item_type, quantities) {
            Some(pp) => snapshot.push((pp, p.item_count)),
            None => skipped += 1,
        }
    }

    let mut owned: wf_relic::OwnedPrimeParts = wf_cache::load_blob_or_reset(wf_relic::OWNED_PRIME_PARTS_FILE);
    wf_relic::owned_parts::apply_exact_snapshot(&mut owned, &snapshot);
    let written = snapshot.len();
    let saved = match wf_cache::save_blob(wf_relic::OWNED_PRIME_PARTS_FILE, &owned) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("failed to write {}: {e}", wf_relic::OWNED_PRIME_PARTS_FILE);
            false
        }
    };
    // Written even when `snapshot` is empty (a real all-zero inventory) — see
    // the marker file's own doc for why that case still needs to record
    // "a mem-scan happened," not just "here are the parts it found."
    if saved {
        if let Err(e) =
            wf_cache::save_blob(wf_relic::owned_parts::OWNED_PARTS_MEM_SCANNED_MARKER_FILE, &true)
        {
            tracing::warn!(
                "failed to write {}: {e}",
                wf_relic::owned_parts::OWNED_PARTS_MEM_SCANNED_MARKER_FILE
            );
        }
    }

    PartsWriteReport { written, skipped, saved }
}
