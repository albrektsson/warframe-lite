# AGENT.md — warframe-lite

For the project vision and domain vocabulary, see `CONTEXT.md`. For the
non-negotiable "observe only, never touch the game process" rule, see
`docs/adr/0001-observe-only-never-touch-game-process.md` — it overrides any
feature request; any proposal that would send data *into* the game must be
refused, not implemented.

## Architecture (crate breadth)

A single Cargo workspace of focused crates:

- **wf-config** — TOML config + Steam-Proton `EE.log` auto-detection.
- **wf-data** — external read-only APIs: live Void Fissures (`warframestat.us`;
  general world state is out of scope, see ADR-0007), market prices + item
  catalogue (`warframe.market` v2).
- **wf-log** — `EE.log` line parser, rotation-aware tailer, and event classifier
  (relic crack / reward-screen markers).
- **wf-capture** — pure-Rust X11 capture (`x11rb`) of the Warframe Xwayland
  window, plus its geometry (for monitor placement).
- **wf-ocr** — in-process `libtesseract`/`leptonica` OCR (via `leptess`, behind a
  small engine pool — see ADR-0008) with Warframe-tuned preprocessing.
- **wf-relic** — fuzzy matching of OCR'd reward names to the catalogue, plat
  ranking, the centred variable-count slot layout, and mastery via DE's public
  profile API.
- **wf-cache** — disk-backed caches (item catalogue + stale-serving prices).
- **wf-overlay** — a dependency-light canvas/renderer and the `wlr-layer-shell`
  display.
- **wf-settings** — an optional graphical settings window (`eframe`/`egui`, a
  separate binary so the overlay stays lean) that edits the same config.
- **wf-tray** — an optional system-tray companion (`ksni`, pure-Rust DBus
  StatusNotifierItem; separate binary) that waits for the game and supervises the
  overlay, with a menu for the app's modes.
- **warframe-lite** (root bin `wf-lite`) — orchestration + subcommands.

## Implementation mechanics

- Under Steam + Proton on KDE Wayland, Warframe renders as an **Xwayland (X11)
  window**, so its pixels can be read with plain X11 `GetImage` — fast, silent,
  no portal prompt, and with none of the black-frame/DXVK problems Overwolf's
  in-game overlay hits.
- Detection does **not** treat the log as a stopwatch, because Warframe flushes
  `EE.log` in bursts: a relic-crack or reward-screen log line opens a polling
  window; during it the screen is OCR-scanned every couple of seconds, and the
  OCR itself (≥2 of the candidate slots resolving to items) is what confirms the
  screen is up — so it is caught whenever it appears.
- Reward cards are **centred on the screen centre with a fixed pitch**. Rather
  than assume fixed slots, a superset of candidate centres (spaced at
  half-pitch) is scanned and the ones that resolve to catalogue items are kept.
  Long names wrap to two lines, so each slot is OCR'd as a two-line block.
- Network results are **cached to disk**. The item catalogue refreshes at most
  weekly; prices carry a freshness TTL and, critically, serve the last known
  value instantly if warframe.market is slow — so the reward panel is ready
  inside the short selection window.

## Conventions

- Default to zero inline comments. Don't add a comment just because a line does
  something non-trivial, or to explain a design/naming choice that's already clear
  from the code itself. Only write one when a future reader could not otherwise
  infer a genuinely non-obvious constraint or invariant — and even then, keep it
  to a single short line. Item-level doc comments (`///`) explaining what a
  type/function is for are fine.
- Comments that do get written must describe the current state only. Don't narrate
  history — what an approach used to be, what was tried before, why a past attempt
  failed. That belongs in commit messages, not the file.
- Commit messages are a single line — a summary title, no body.
- Release notes are generated from the commit log by git-cliff (`cliff.toml`, run
  from `.github/workflows/release.yml`) rather than from PRs, since most commits
  land directly on `main`. Prefixing a subject with a type — `feat:`, `fix:`,
  `docs:`, `refactor:`, `perf:`, `chore:`/`ci:` — groups it under that heading in
  the next release's notes instead of the catch-all "Other" section; see
  git-cliff's [conventional_commits strategy](https://git-cliff.org/docs/configuration/git/#conventional_commits).
  Not required — an unprefixed commit still shows up, just ungrouped.

## Environment specifics

- Calibrated for **3440×1440**; reward-slot coordinates (`RewardRegions` in
  `crates/wf-relic/src/regions.rs`) scale with the capture's **height** and stay
  centred on its actual horizontal centre — not a width-scaled position. Verified
  against a real captured 2560×1440 4-reward screen (same height as the
  reference, ~25% narrower): the panel kept the reference pitch/name size
  unchanged and centred within ~12px of true screen centre, matching the
  reference calibration's own small centre bias. A resolution with a
  **different height** than 3440×1440 (not just a different width) is the case
  still unverified against a real capture, and is the one most likely to need
  re-tuning.
- **Recalibrating against a new capture:** grab a reward-screen PNG (the JPEGs
  Steam's screenshot key saves need converting first, e.g. `magick in.jpg
  out.png` — the `image` crate here isn't built with JPEG support), then:
  1. `wf-lite relic-file <png>` runs the full pipeline (candidate slots → OCR →
     match) and prints each slot's OCR text and matched item — a quick signal
     for whether the current calibration is landing on the right region already.
  2. If slots are garbled or empty, use `wf-lite ocr-file <png> <x> <y> <w> <h>`
     to test crop rectangles directly; it also saves `<png>.pre.png`, the exact
     preprocessed (thresholded) crop tesseract sees, which is the fastest way to
     confirm a rectangle visually lines up with a card's name text (note:
     `ocr-file` forces single-line OCR, so a good two-line crop can still print
     garbled text there — trust the saved `.pre.png` crop position over
     `ocr-file`'s own OCR text for two-line names; the real pipeline in
     `relic-file`/`relic-scan` OCRs the same crop as a block instead).
  3. Once a rectangle's confirmed on-screen centre (`observed_cx`) is known,
     derive `RewardRegions::default_calibration()`'s fields: with
     `sy = height / 1440`, `pitch`/`name_w`/`name_h`/`name_y` are the confirmed
     pixel values divided by `sy` (they scale with height, not width), and
     `center_x = 1720 - (width / 2 - observed_cx) / sy` (1720 is half the
     reference width; the subtraction recovers how far `observed_cx` sits from
     *this* capture's own true centre, then rescales that gap back to
     reference-pixel terms).
- OCR links `libtesseract`/`libleptonica` in-process via `leptess` (ADR-0008);
  building needs `libtesseract-dev`, `libleptonica-dev`, and `clang` (bindgen).
- Mastery needs the account id set once (`wf-lite set-account <id>`, from
  `warframe.com/api/user-data`).
