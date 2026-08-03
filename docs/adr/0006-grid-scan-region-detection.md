# Void Relics grid: per-frame phase anchoring, not a fixed row offset

The Void Relics grid scanner locates each card's name and count badge from a
measured card geometry (column pitch, row pitch, badge offset) plus a **vertical
phase chosen per frame**: the list scrolls continuously, so before OCR we score
a handful of candidate row offsets by cheap ink analysis — with no OCR — and keep
the one whose name band best lands on real text, then OCR only that single phase.

## Context

The original calibration used one fixed row offset (`name_cy0`) and, to cope
with scroll, an option to OCR several interleaved phases per frame. Measured
against real captures both failed: the fixed offset landed the name crop *on the
orb* (0 cards resolved), and OCR-ing 7 phases to cover the scroll cost minutes
per frame. The name band and the `xN` count badge are ~180px apart and both are
"text", so a naive coverage score also locks onto the badge row instead of the
name row.

## What we do

- **Measured, mostly-fixed geometry.** Card pitch and the name/badge offsets are
  measured from reference 3440×1440 captures. Refined relics wrap their
  `[Radiant]`/`[Exceptional]`/… suffix onto a second line, so the name crop is
  tall enough for two lines and is OCR'd as a block — the suffix is the only
  thing distinguishing a refined card from its Intact twin.
- **Phase alignment by ink, then one OCR pass.** `best_grid_phase` scores ~12
  candidate offsets by how many name crops look like a *name line* — text-band
  ink coverage **and** horizontal spread across the crop. The spread test is what
  separates a wide name from the narrow `xN` badge above it. Scoring uses pixel
  counting only (no upscaling, no Tesseract); OCR then runs on just the winning
  phase, restoring roughly single-pass cost.
- **Cheap text-vs-artwork gate.** Even at the aligned phase, columns that land on
  orb artwork or empty cells are skipped by the same coverage gate before paying
  for an OCR call.

## Consequences

- Reliable on the reference 3440×1440 resolution (0 → ~all visible cards, with
  correct counts and refinements); the cross-frame vote (ADR-0005) absorbs the
  residual per-frame OCR noise.
- **Non-native resolutions are not yet solved.** A windowed 2301×906 capture
  showed the card pitch is ~fixed in pixels (not scaled to resolution) while the
  grid *origin* shifts — so the current resolution-scaling of the geometry is
  wrong off-native. Deriving the true scaling law needs more labelled captures at
  known resolutions (2560×1440, 1920×1080); until then, off-native captures may
  align poorly.
- A single scan is OCR-bound (~dozens of Tesseract subprocess spawns). Acceptable
  because the loop re-scans while the player browses and the vote accumulates,
  but linking libtesseract or lowering the upscale is the obvious later win.

## Scope

The phase-anchoring idea and the ink-analysis gates (`looks_like_text`,
`looks_like_name_line`, `is_blank`) live in `wf-ocr` so the forthcoming
equipment scan reuses them; only the card geometry and the eye-icon ownership
signal are relic-specific.
