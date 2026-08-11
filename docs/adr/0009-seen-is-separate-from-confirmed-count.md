# Owned-relic "Seen" is a separate, weaker signal from its Confirmed count

ADR-0005 trusts a relic's owned count only once two frames agree on the same
value, specifically to defeat noisy digit misreads on the `xNN` badge.
Comparing our persisted owned-relic set against the game's own "Collected
N/772" showed the gap is mostly not wrong counts — it's relics that never
clear that bar at all, even identity-wise, because a natural continuous
scroll rarely revisits the same card twice within one scan cycle (see
ADR-0008). The badge-noise problem and the relic-identity problem are
different questions with different failure modes: a relic's name+refinement,
matched at ≥80% similarity against the known catalogue
(`RelicIndex::best_match`) with no "unowned" eye icon present, is reasonably
trustworthy from a single clean read — the exact stack size is not.

We add **Seen**: a relic is marked owned from one clean identity read,
independent of its **Confirmed count**. The count itself keeps requiring two
agreeing frames as before, so ADR-0005's trust guarantee for numeric counts is
unchanged. A Seen relic with no confirmed count yet counts toward the owned
total (the figure compared against the game's "Collected N/772"), but its
exact count is reported as unknown/pending rather than guessed, until it
separately clears the count's own agreement bar.

## Consequences

- `owned-relics.json`'s schema changes from `code → refinement →
  {count, last_seen}` to `code → refinement → {seen, count: Option<{value,
  last_seen}>}`. Existing files need the same "back up and start clean"
  treatment ADR-0005 already established for incompatible formats.
- `wf-relic::owned::intact_counts` and the mastery/farm/sell planners need an
  actual number to rank by, and keep using only entries with a confirmed
  count — a Seen-but-uncounted relic doesn't feed those yet.
- The relic-scan-loop's owned-relic total (compared against the game's own
  count) now includes Seen entries, not just Confirmed-count ones.

## Scope

`wf-relic::owned` (schema, `apply_confirmed_count` and friends) and the
relic-grid scan loop in `src/main.rs` that feeds it. Does not touch
`wf-ocr`'s trust core (`Tally`, `parse_badge`), which ADR-0008 changes for
unrelated (performance) reasons.

## Revision (2026-08-11): a third source, `wf-mem`'s exact mem-scan

`wf-mem`'s mem-scan (issues [#63](https://github.com/albrektsson/warframe-lite/issues/63)/[#64](https://github.com/albrektsson/warframe-lite/issues/64)/[#66](https://github.com/albrektsson/warframe-lite/issues/66))
reads owned-relic counts directly from DE's own inventory payload — exact,
not a frame-agreement estimate. Wiring it into `owned-relics.json` (issue
[#67](https://github.com/albrektsson/warframe-lite/issues/67)) means the OCR
loop and mem-scan now both write the same file, so the two-tier Seen/
Confirmed model above needs a third dimension: **who wrote the current
count**, not just how sure the writer was.

`OwnedEntry` gains a `source: Source` field (`Ocr` | `MemScan`), stamped
whenever `count` is set. This doesn't collapse or replace the Seen/Confirmed
split — a mem-scanned count is definitionally also Confirmed (and Seen) —
it adds provenance to the Confirmed tier specifically, to answer a question
Seen/Confirmed alone can't: whether an *existing* confirmed count came from
an exact read or an OCR frame-agreement estimate.

That answer gates one thing: the OCR scan loop's agreement bar to overwrite
it. A `MemScan`-sourced count needs `RELIC_AGREEMENT_MEMSCAN_OVERRIDE`
(`src/main.rs`, 4× the normal `RELIC_AGREEMENT`) agreeing OCR frames before
it's replaced, instead of the normal two — a single lucky misread pair
shouldn't be able to clobber a value read straight from game memory. Once
OCR does clear that higher bar, the entry's source flips back to `Ocr` and
the normal bar applies from then on; this is a supersede-resistance
mechanism, not a permanent distrust of OCR for that relic.

mem-scan writes are also, deliberately, more than an additive update: a
mem-scanned inventory only ever lists relics actually owned (≥1), so a
`(code, refinement)` *absent* from a fresh scan is authoritative proof of
zero. `apply_exact_snapshot` (`wf-relic::owned`) clears every existing entry
the snapshot doesn't cover, alongside writing the ones it does — self-
correcting the one class of staleness OCR structurally can't fix itself (a
refined relic's card just disappears from the grid once fully consumed,
with no eye-icon equivalent to confirm the zero — see ADR-0010). No extra
opt-in gate: running `wf-lite mem-scan` at all is already this map's
required in-the-moment consent (see `wf-mem`'s module docs and
ADR-0001/ADR-0013), so its findings taking effect immediately doesn't need a
second confirmation.

An owned-relic entry `wf-mem` can't decode to a `(code, refinement)` (no
catalogue match — see [#66](https://github.com/albrektsson/warframe-lite/issues/66)'s
rare `(undecoded)` case) is skipped from the snapshot rather than written
raw or guessed at; `mem-scan`'s own output logs a warning when this happens
so it isn't silently lost from view.

## Revision (2026-08-11): the same `Source` model also covers owned Prime Part components

[#81](https://github.com/albrektsson/warframe-lite/issues/81) wires
`wf-mem`'s mem-scan into `owned-prime-parts.json`
(`wf-relic::owned_parts`, the built-component counterpart to
`OwnedRelics`) the same way #67 wired it into `owned-relics.json` above.
`owned_parts.rs` has no Seen/Confirmed split — its own module doc explains
why: this screen's badge is always a single-frame passive read, so there's
nothing for a Seen tier to distinguish — but the *provenance* question this
revision's `Source` answers (was the current count read exactly from game
memory, or estimated from OCR frames?) doesn't depend on a Seen tier
existing. So `PartCount` gains the identical `source: Source` field
(`crate::owned::Source`, reused verbatim rather than duplicated), OCR needs
`INVENTORY_AGREEMENT_MEMSCAN_OVERRIDE` (`src/ocr_enabled.rs`, the same 4×
multiplier as `RELIC_AGREEMENT_MEMSCAN_OVERRIDE`) to overwrite a
`MemScan`-sourced part count, and `owned_parts::apply_exact_snapshot`
mirrors `owned::apply_exact_snapshot`'s absence-is-zero authoritative reset.

A raw owned-part entry that isn't a Prime component at all (most of a
typical account's `MiscItems[]` built components are non-Prime gear) is
silently dropped, not logged — unlike an undecoded relic above, this is the
ordinary case here, not a name-index gap (see `wf-mem::write_owned_parts`'s
doc).
