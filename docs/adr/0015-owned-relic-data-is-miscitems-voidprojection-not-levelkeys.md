# Owned-relic data is `MiscItems[]`/`VoidProjection`, not `LevelKeys[]`

[ADR-0014](0014-phase-4-inventory-scope-relics-equipment-mastery-only.md)
named `LevelKeys[]` as "a candidate future authoritative replacement for the
OCR-based Owned relic scan" — an inference from third-party consumers' source
(`docs/research/mobile-inventory-api-coverage.md`), not a real payload. Live
verification ([issue #60](https://github.com/albrektsson/warframe-lite/issues/60))
found `LevelKeys[]` holds only legacy mission Keys (Dojo Key,
boss-assassination keys, Syndicate Cache Hunt, Grendel Key C) — no Void
Relics at all, on a real account.

A follow-up feasibility check ([issue #63](https://github.com/albrektsson/warframe-lite/issues/63))
found the real field: owned relics are `MiscItems[]` entries whose `ItemType`
matches DE's internal `VoidProjection` naming
(`/Lotus/Types/Game/Projections/T{1-5}VoidProjection{RewardPoolName}{Letter}{Refinement}`
— DE calls a relic a "VoidProjection", not a "Relic" or "LevelKey"), each
carrying a real `ItemCount`. `T1`–`T5` are Lith/Meso/Neo/Axi/Requiem and the
trailing `Bronze`/`Silver`/`Gold`/`Platinum` suffix maps exactly to
Intact/Exceptional/Flawless/Radiant — confirmed against WFCD `warframe-items`'
`Relics.json` directly ([issue #66](https://github.com/albrektsson/warframe-lite/issues/66)),
not inferred from consumer code this time.

This is the field [issue #67](https://github.com/albrektsson/warframe-lite/issues/67)
built on (see [ADR-0009](0009-seen-is-separate-from-confirmed-count.md)'s
revision) to replace the OCR-based owned-relic scan's ground truth with exact
mem-scan reads — the replacement ADR-0014 only ever called a "candidate", now
actually implemented.

## Scope

Documentation only — corrects ADR-0014's field-name claim in the historical
record. No code changes; `crates/wf-mem/src/relics.rs`, `crates/wf-relic/src/relic_names.rs`,
and `crates/wf-mem/src/level_keys.rs` (the raw, still-accurate `LevelKeys[]`
exposure, kept for what it actually holds) are unaffected.
