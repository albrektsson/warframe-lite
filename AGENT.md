# AGENT.md — warframe-lite: Project Vision

## What this is

warframe-lite is a **Linux-native, Overwolf-free** companion app for Warframe,
targeting KDE Plasma (Wayland) with the game running under Steam Proton. It is a
lightweight alternative to [AlecaFrame](https://alecaframe.com/) — which is an
Overwolf app and painful to run on Linux — covering the most-used features as a
small, robust, native binary.

The guiding constraint is **native and low-risk**: everything is built on data
sources that work cleanly on Linux without Overwolf, without reading game
process memory, and without handling account credentials. Where AlecaFrame reads
the game's memory for full inventory, warframe-lite deliberately stays on the
public/observable side of that line.

## The core experience

- A **world-state overlay** — a `wlr-layer-shell` panel, click-through and
  always-on-top, anchored to a corner of the game's monitor — shows live Void
  Fissures (sorted normal → Steel Path → Storm), the Void Trader, and the
  Cetus/Vallis/Cambion cycles, with ETAs that tick down each second.
- An **automatic relic reward picker** — the flagship. When a Void Fissure
  reward screen appears, the overlay automatically swaps to a ranked panel of the
  2–4 rewards, showing each one's live warframe.market price, the **best plat
  pick** highlighted, and a **mastery badge** on rewards whose built prime the
  player has already mastered. No keypress required.
- Everything is driven by observing the game, never by injecting into or
  automating it.

## Architecture (crate breadth)

A single Cargo workspace of focused crates:

- **wf-config** — TOML config + Steam-Proton `EE.log` auto-detection.
- **wf-data** — external read-only APIs: world state (`warframestat.us`), market
  prices + item catalogue (`warframe.market` v2).
- **wf-log** — `EE.log` line parser, rotation-aware tailer, and event classifier
  (relic crack / reward-screen markers).
- **wf-capture** — pure-Rust X11 capture (`x11rb`) of the Warframe Xwayland
  window, plus its geometry (for monitor placement).
- **wf-ocr** — OCR via the Tesseract CLI with Warframe-tuned preprocessing.
- **wf-relic** — fuzzy matching of OCR'd reward names to the catalogue, plat
  ranking, the centred variable-count slot layout, and mastery via DE's public
  profile API.
- **wf-cache** — disk-backed caches (item catalogue + stale-serving prices).
- **wf-overlay** — a dependency-light canvas/renderer and the `wlr-layer-shell`
  display.
- **warframe-lite** (root bin `wf-lite`) — orchestration + subcommands.

## How it works, conceptually

- Warframe's data splits into an **easy tier** and a **hard tier**. The easy tier
  — world state, market prices, static drop data, and real-time events from
  `EE.log` — is fully obtainable on Linux over HTTP and file reads. The hard tier
  — full current inventory (what you own right now) — is only available by
  reading game memory, which AlecaFrame does via Overwolf. warframe-lite covers
  the easy tier and deliberately leaves the memory tier alone.
- Under Steam + Proton on KDE Wayland, Warframe renders as an **Xwayland (X11)
  window**, so its pixels can be read with plain X11 `GetImage` — fast, silent,
  no portal prompt, and with none of the black-frame/DXVK problems Overwolf's
  in-game overlay hits.
- The reward screen is a mid-mission, ~15-second, player-controlled thing (it can
  be brought up via Tab), and Warframe flushes its log in bursts, so detection
  does **not** treat the log as a stopwatch. A relic crack or reward-screen log
  line opens a polling window; during it the screen is OCR-scanned every couple of
  seconds, and the OCR itself (≥2 of the candidate slots resolving to items) is
  what confirms the screen is up — so it is caught whenever it appears.
- Reward cards are **centred on the screen centre with a fixed pitch**, and there
  are 2–4 of them. Rather than assume fixed slots, a superset of candidate centres
  (spaced at half-pitch) is scanned and the ones that resolve to catalogue items
  are kept. Long names wrap to two lines, so each slot is OCR'd as a two-line
  block.
- **Mastery** comes from DE's *public* profile API (`getProfileViewingData`, no
  auth). An item's `XPInfo` lifetime affinity crossing its rank-30 cap means it is
  mastered (permanent, never resets on Forma); each reward *part* is mapped to the
  built prime it belongs to before the lookup.
- Network results are **cached to disk**. The item catalogue refreshes at most
  weekly; prices carry a freshness TTL and, critically, serve the last known value
  instantly if warframe.market is slow — so the reward panel is ready inside the
  short selection window.

## What "done" looks like

A player, on KDE Wayland with Warframe under Proton, runs `wf-lite overlay` and:

1. Sees the world-state panel on the game's monitor, updating live.
2. Plays a Void Fissure. When the reward screen appears, the overlay
   automatically shows the 2–4 rewards ranked by plat, the best pick highlighted,
   and a mastery badge on the ones they have already mastered.
3. Picks the reward they want — the valuable one to sell, or the one they still
   need for mastery — and the overlay reverts to world state.

No Overwolf, no memory reading, no credentials — just an observing native binary.

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

- Calibrated for **3440×1440**; reward-slot coordinates scale proportionally but
  other resolutions may need re-tuning (`wf-lite relic-file <png>` and
  `wf-lite ocr-file` help recalibrate against a captured reward screen).
- OCR shells out to `tesseract` (installed via Homebrew on Bazzite); the binary is
  overridable with the `WF_TESSERACT` env var.
- Mastery needs the account id set once (`wf-lite set-account <id>`, from
  `warframe.com/api/user-data`).
