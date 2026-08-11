# Plan: `warframe-lite` — a standalone, Linux-native light variant of AlecaFrame

> **Scope update (2026-08, [ADR-0007](adr/0007-live-world-state-out-of-scope.md)):**
> The project is now scoped to **relics, equipment, and mastery** and the data
> they touch (relic reward picking, owned-relic scanning, the mastery/fissure
> plan, market prices, drop tables, mastery). General live **world state** — the
> Void Trader and the Cetus/Vallis/Cambion cycles — is **out of scope** and being
> removed. Live **Fissures** are the one `warframestat.us` feed kept, as a relic
> feature (the mastery plan flags which relic tiers are runnable now). Text below
> describing "timers & world state" as a co-equal must-have reflects the original
> plan and is superseded by this note where the two disagree.

## Context

AlecaFrame is the most capable Warframe companion app, but it is built as an
**Overwolf** app. Overwolf has no official Linux support, so running AlecaFrame
on Linux requires a fragile stack (umu/Proton + Overwolf injected into the same
Wine prefix, `ptrace_scope=0`, OOPO flags, manifest patches) that breaks on
window moves, renders the in-game overlay black under DXVK, and can be remotely
disabled by Overwolf. The goal is a **standalone tool that runs natively on
Linux** and covers the most-used features, without Overwolf.

Research established that AlecaFrame's data splits into two tiers:

| Tier | Data | How AlecaFrame gets it | Linux-native feasibility |
|------|------|------------------------|--------------------------|
| **Easy** | World state (fissures, Baro, sortie, nightwave, cycles) | `warframestat.us` public API | Trivial (HTTP) |
| **Easy** | Static data: item list, relic drop tables, ducat values, mastery | WFCD data packages | Trivial (bundled data) |
| **Easy** | Market prices / order book / order management | `warframe.market` API v2 | Easy (HTTP + token) |
| **Medium** | Real-time gameplay events (mission start/end, host migration, deaths) | `EE.log` | Easy (tail-parse) |
| **Medium** | **Relic reward choices** (the 4 options on screen) | **NOT in EE.log** → screen OCR | Medium (screenshot + Tesseract; proven by WFInfo-linux) |
| **Hard** | Full inventory: foundry, mastery, owned items, riven details, credits/plat, stats history | **Overwolf native plugin reads `Warframe.x64.exe` process memory** (`ReadProcessMemory`) | Hard (memory RE, fragile, breaks each patch) |

**Feasibility verdict:** A light variant covering the **relic reward picker +
timers/world state** (the chosen must-haves) is **clearly feasible and low-risk**
on Linux — WFInfo-linux already proves the OCR path works. The **full-inventory
tier is feasible but high-effort and high-maintenance**; it is scoped as an
optional later phase, not part of the core "light" deliverable.

## Decisions (from user)

- **Language:** Rust
- **Form factor:** In-game overlay
- **Must-have features:** Relic reward picker (OCR); Timers & world state
  _(superseded — see the scope-update note above: general world state is now out
  of scope; only live Fissures remain, as a relic feature)_
- **Inventory tier:** Pursue via **memory reading** (later phase; user preferred this over credential-login API)
- **Environment:** KDE Plasma (Wayland); Warframe launched via **Steam (Proton)**

## Key environment facts that shape the design

- Under **Steam + Proton on KDE Wayland, Warframe renders as an Xwayland (X11)
  window.** This is a gift: we can **capture its pixels via X11 (`XShm`/`XGetImage`)
  — fast and silent, with no `xdg-desktop-portal` permission prompt** (the thing
  that makes repeated Wayland screenshots painful).
- **KWin implements `wlr-layer-shell`**, so a native Wayland **layer-shell surface
  on the overlay layer** gives a true always-on-top overlay above the game
  (run Warframe borderless/windowed-fullscreen for reliable compositing).
- **Recommended split:** capture in **X11/Xwayland**, draw the overlay in
  **native Wayland layer-shell**. (Fallback: do both in Xwayland X11 — simplest,
  matches the WFInfo/AlecaFrame model — if layer-shell over the game misbehaves.)
- **Memory reading is actually cleaner natively than Overwolf's approach:**
  `Warframe.x64.exe` inside the Proton prefix is a normal Linux process, so a
  native Rust tool can read it with **`process_vm_readv(2)`** (same uid; needs
  `ptrace_scope ≤ 1` or `CAP_SYS_PTRACE`). The hard part is not the read — it's
  reverse-engineering the inventory structures/pointer chains, which are
  proprietary in AlecaFrame and break on every game update.

## Architecture

Single Rust workspace, a few crates:

```
warframe-lite/
├─ crates/
│  ├─ wf-data/        # static data + external APIs (worldstate, market, drop tables)
│  ├─ wf-log/         # EE.log locator + tail parser (fissure/reward-screen triggers)
│  ├─ wf-capture/     # X11/XShm capture of the Xwayland Warframe window
│  ├─ wf-ocr/         # crop reward regions, preprocess, Tesseract, name normalization
│  ├─ wf-overlay/     # Wayland wlr-layer-shell surface + rendering (wgpu/egui)
│  ├─ wf-relic/       # relic reward picker: OCR results → prices/ducats → best pick
│  └─ wf-mem/         # (Phase 4, optional) process_vm_readv inventory reader
└─ src/main.rs        # daemon: hotkeys, orchestration, config
```

**Suggested crates (all mature):**
- Worldstate/market: WFCD [`warframe.rs`](https://github.com/WFCD/warframe.rs) (`warframe-client`), or plain `reqwest` against `api.warframestat.us` + `api.warframe.market/v2`.
- Static data: bundle WFCD [`warframe-drop-data`](https://github.com/WFCD/warframe-drop-data) (relic contents/rarities) + item→ducat table; refresh on startup.
- Log watching: `notify` (file watch) + manual line tailing.
- X11 capture: `x11rb` (XShm) — capture only the reward-name sub-regions.
- OCR: `leptess` or `rusty-tesseract` (Tesseract), same as WFInfo-linux.
- Overlay: `smithay-client-toolkit` + `wayland-protocols-wlr` (layer shell) with `wgpu`; or `gtk4` + `gtk4-layer-shell` for faster UI iteration.
- Global hotkeys on Wayland: a KWin custom shortcut (via DBus) or `evdev`/`libinput` reader; hotkey is the reliable relic-screen trigger.
- Memory (Phase 4): `nix::sys::uio::process_vm_readv` + `/proc/<pid>/maps` parsing.

## Implementation status (as of 2026-07)

**Hard rule (see [ADR-0001](adr/0001-observe-only-never-touch-game-process.md)):**
warframe-lite is strictly observe-only — it must never modify, write, or send any
data to the Warframe process (no input injection, no memory writes, no ptrace, no
IPC/network to the game). The only game-side interactions are one-directional
reads: the `EE.log` file and the window's pixels.

Phases 0–3 plus caching, mastery, overlay configuration, account auto-detection,
and the tray/settings companions are **done and verified against the running
game**. Summary of what shipped:

- **Phase 0 — data plumbing ✅.** Config load, Steam-Proton `EE.log`
  auto-detection (appid 230410 via `libraryfolders.vdf`), live world state, and
  warframe.market pricing, all verified live.
- **Phase 1 — fissure overlay ✅.** A pure-Rust `wlr-layer-shell` panel (top
  layer, click-through) shows fissures (normal → Steel Path → Storm),
  re-rendering every second so ETAs tick. _(Originally also showed Baro and the
  Cetus/Vallis/Cambion cycles; those are removed per ADR-0007 — general world
  state is out of scope.)_
- **Phase 2 — relic reward picker ✅.** OCR (`wf-ocr`, tesseract CLI) + picker
  brain (`wf-relic`: fuzzy match to the ~3.8k-item warframe.market catalogue +
  plat ranking). Handles the **variable, centred reward layout** (2–4 cards
  depending on how many squadmates cracked) by scanning a superset of candidate
  slot centres and keeping whichever OCR to items, and reads **two-line wrapped
  names**. Validated on real 3440×1440 screens (a 4-reward screen and a 3-reward
  screen with a wrapped *Voruna Prime Systems Blueprint*).
- **Mastery tracking ✅.** DE's **public** profile API
  (`getProfileViewingData.php`, no auth): reads `LoadOutInventory.XPInfo` and
  treats an item as mastered once lifetime affinity passes the rank-30 cap (450k
  weapons / 900k frames — verified against a real profile), maps each reward
  *part* to its built prime, and shows a green mastery emblem in front of the
  name (and dims it). Account id set once
  (`wf-lite set-account <id>`); mastered set cached for a day. The ducat column
  was dropped (it is clearly shown in-game during selection).
- **Caching ✅.** Item catalogue cached to `~/.cache/warframe-lite/items.json`
  (7-day TTL, stale-served offline; warm startup ~19× faster). Prices cached per
  item with a freshness TTL; during a scan all are fetched **concurrently**, and
  **if warframe.market is slow/unreachable the last known price is served
  immediately** — so the panel is ready inside the few-second selection window.
- **Phase 3 — automatic detection + overlay integration ✅.** `wf-lite overlay`
  shows world state and **automatically swaps to the ranked reward result when the
  reward screen is on-screen** — no keypress. Because the reward screen is a
  mid-mission, ~15s, player-controlled thing (Tab-showable) and Warframe flushes
  its log in bursts, detection does **not** treat the log as a stopwatch: a relic
  **crack** (`DVRCAftermath`) or **reward-screen** line
  (`ProjectionRewardChoice: Got rewards`) opens a ~150s polling window during
  which the screen is OCR-scanned every 2s, and the OCR guard (≥2 names resolve)
  confirms the screen is up. Result shows ~20s, de-bounced. Confirmed in a live
  fissure.
- **Overlay placement ✅.** Anchors to the monitor Warframe is on (game window
  centre matched against each output's logical geometry) and hugs the game window
  corner in fullscreen **and** borderless-windowed (window offset folded into the
  layer-shell margins). Polls up to 30s for the window so it can be launched
  *with* the game (`wf-lite overlay & %command%` in Steam launch options).
- **Overlay configuration / anti-overlap ✅.** Since Warframe uses every screen
  corner for HUD/menu, an `[overlay]` config controls `anchor`, `margin_x/y`,
  `opacity`, and `world_state` (`false` = reward-only: invisible until a reward
  screen). Show/hide at runtime goes through a Unix control socket (`wf-lite
  toggle|show|hide`) — because a click-through Wayland surface can't grab a global
  key, the user binds `wf-lite toggle` as a KDE custom shortcut and the compositor
  owns the hotkey.
- **Account-id auto-detection ✅.** `wf-lite detect-account` scrapes the local
  account id from `EE.log` (situational lines: Duviri races, void-projection
  relaying) and **verifies every candidate against the public profile API** — only
  an id whose `DisplayName` matches the logged-in name is saved, so a squadmate's
  id can never be picked. Verified live.
- **Companion binaries ✅.** Kept as separate crates so the overlay stays lean:
  **wf-settings** (a graphical `eframe`/`egui` settings window) and **wf-tray** (a
  pure-Rust `ksni` StatusNotifierItem tray that waits for the game, auto-starts and
  supervises the overlay, and exposes the app's modes via its menu). `wf-lite
  settings` / `wf-lite tray` launch them; a `.desktop` file makes the tray a
  launcher entry.
- **Distribution.** The overlay is a single self-contained binary (no external
  link-time deps: libwayland is dlopen'd, x11rb is pure Rust, rustls uses `ring`);
  OCR (opt-in) links `libtesseract`/`libleptonica` in-process rather than
  shelling out to a CLI (ADR-0008). musl-static is unsuitable (the overlay
  dlopens libwayland). No packaged/prebuilt releases (GitHub Releases,
  Fedora/COPR) are published pre-1.0 — build from source; packaging is
  deferred until after 1.0.

**Two hardest Linux unknowns de-risked (verified against the running game):**

- **EE.log parsing** (`wf-log`): line parser hits ~98% coverage on real logs
  (remainder are multi-line continuations); rotation/append-aware tailer.
  Confirmed Warframe **buffers log output and flushes in bursts**, which is why
  detection uses a polling window rather than log timing.
- **X11 capture** (`wf-capture`): pure-Rust `x11rb` locates the Warframe Xwayland
  window and reads a full 3440×1440 frame via `GetImage` — legible content, **no
  black-frame/DXVK issue**, no portal prompt.

- **Owned-relic mastery guide ✅.** `wf-relic::relics` (`RelicIndex` from WFCD
  `warframe-drop-data`, cached weekly) cross-references a relic's rewards with
  the mastered set to list the distinct unmastered **prime** parts it can still
  drop (Requiem Mods / Ayatan Sculptures / Riven Slivers are excluded — mastery
  never applies to them), priced via the existing relic market slugs. Ownership
  is read by OCR of the in-game **Void Relics** screen: opening it is
  auto-detected from `EE.log` (`ThemedProjectionManager.lua:
  PopulateInventoryGrid`), and the overlay scans the grid (parallelized per
  card, ~3s/frame) as the player scrolls, skipping relics marked with the
  in-game "unowned" eye icon (detected by brightness-invariant template
  matching, since hovering brightens a card regardless of ownership). The owned
  set is **persisted to disk** (`~/.cache/warframe-lite/owned-relics.json`) so it
  accumulates across sessions and survives restarts. `wf-lite relics <codes…>`
  and an overlay panel (`render_relic_panel`) show the per-relic guide.
  Scanning runs in its own task (`relic_scan_loop`), decoupled from the
  reward-screen watcher's fixed poll interval, so it re-scans back-to-back
  while the screen is open — the list scrolls continuously (not row-snapped),
  so more, faster samples catch more scroll positions than fewer, slower ones;
  a `RelicGridRegions::row_phases` knob exists for trading scan rate for
  per-frame coverage if `wf-ocr` ever gains OCR bounding boxes to make that
  cheap (right now, extra phases cost close to linearly, not for free).
- **Mastery planner ✅.** `wf-relic::mastery_plan` inverts the view: for every
  unmastered prime, which owned relics (and how many of each) can still drop it,
  ranked by total relics in hand — a fissure-farming priority list. `wf-lite
  mastery-plan` prints it, cross-referencing currently active fissures (via
  `worldstate`) to flag which relics are actionable *right now*. Works from the
  persisted owned-relic set, so it doesn't require an active scan.

**Remaining / later polish** is now tracked as GitHub issues rather than a
freeform list here:
[config-overridable reward regions](https://github.com/albrektsson/warframe-lite/issues/6),
[per-resolution calibration](https://github.com/albrektsson/warframe-lite/issues/7),
[mastery-weighted ranking](https://github.com/albrektsson/warframe-lite/issues/8),
[background price pre-warm](https://github.com/albrektsson/warframe-lite/issues/9).
**Phase 4** (inventory via memory reading) remains optional and unstarted, and
per the hard rule above, strictly a *read* (`process_vm_readv`) if ever pursued
— tracked as [issue #10](https://github.com/albrektsson/warframe-lite/issues/10).

### External API notes

- **warframe.market:** use the **v2** API (`/v2/orders/item/{slug}`). The legacy
  v1 endpoint returns **403**.
- **warframestat.us:** fissure/cycle objects carry only an `expiry` timestamp (no
  pre-formatted `eta`/`timeLeft`); remaining time is computed locally.

## Phased implementation plan (with effort)

> Historical — the plan the build followed. See **Implementation status** above
> for what actually shipped.

**Phase 0 — Scaffolding & data layer (S, ~1–2 days)**
- Cargo workspace; config (TOML) with auto-detected EE.log/prefix paths for the
  Steam Proton prefix (`~/.steam/steam/steamapps/compatdata/230410/pfx/.../Local/Warframe/EE.log`).
- `wf-data`: fetch + cache worldstate and market data; bundle drop/ducat tables.

**Phase 1 — Timers & world state overlay (S, ~2–3 days)**
- Poll `warframestat.us` (fissures, Baro, sortie, nightwave, Cetus/Vallis/Cambion cycles).
- Minimal always-on-top window first (correctness), then move to layer-shell.
- Deliverable: a live info panel over the game.

**Phase 2 — Relic reward picker (M, ~1–2 weeks; the flagship)**
- `wf-capture`: locate the Xwayland Warframe window, XShm-capture the reward strip.
- `wf-log`: detect reward-screen appearance from EE.log heuristics; **always**
  provide a **manual hotkey trigger** (WFInfo-linux does exactly this because log
  buffering makes auto-detection unreliable).
- `wf-ocr`: crop the 4 reward-name regions, threshold/upscale, Tesseract → normalize
  against the item list (fuzzy match).
- `wf-relic`: map each name → market plat (warframe.market) + ducats (static) +
  drop rarity; highlight the best pick.
- Deliverable: the AlecaFrame/WFInfo core, working natively.

**Phase 3 — Overlay polish & UX (M, ~1 week)**
- True `wlr-layer-shell` overlay (overlay layer, click-through when idle),
  positioned over the reward strip; config for regions/resolution scaling;
  KWin custom-shortcut integration; first-run setup that verifies EE.log path,
  Xwayland window detection, and Tesseract.

**Phase 4 — (Optional) Inventory via memory reading (L / open-ended, weeks–months + ongoing)**
- `wf-mem`: find `Warframe.x64.exe` pid, parse `/proc/<pid>/maps`, `process_vm_readv`.
- **Bulk of the work and risk = reverse-engineering inventory structures**
  (pattern-scan signatures for stable anchors; offsets break each patch).
- Prerequisite: document the `ptrace_scope` requirement; prefer `CAP_SYS_PTRACE`
  on the binary over globally lowering hardening.
- **Recommendation:** treat as a research spike first. Decide go/no-go after a
  proof-of-concept reads one stable value (e.g. credits). Honestly, accepting "no
  full inventory" keeps the tool robust; the login-API alternative (credential
  handling, ToS-gray) was explicitly declined, so memory reading is the only
  inventory route — and it is the maintenance-heavy part of the whole project.

## Risks & mitigations

- **Layer-shell overlay over a fullscreen Proton game** may not composite as
  desired → mitigate by running Warframe borderless-fullscreen; fallback to an
  Xwayland X11 override-redirect overlay (same display server as the game).
- **OCR accuracy** across resolutions/UI themes → make crop regions
  resolution-relative and config-tunable; fuzzy-match names to the item list.
- **EE.log auto-trigger unreliable** (log buffering) → manual hotkey is the
  primary trigger, auto-detect is best-effort.
- **Memory offsets break every game patch** → keep Phase 4 isolated; use
  signature scanning, not hardcoded offsets; gate behind a feature flag.
- **ToS:** read-only screen OCR + public APIs are the same posture DE tolerates
  (WFInfo). Memory reading is read-only/no-injection (AlecaFrame's justification),
  but is a user-accepted risk — surface it in docs.

## Verification (end-to-end)

1. **Data layer:** unit-run `wf-data` and confirm live fissures/Baro match
   `warframe.market`/`warframestat.us` and an in-game check.
2. **Capture:** with Warframe on the reward screen, dump the XShm capture to PNG
   and confirm it shows the 4 reward names (proves Xwayland capture works silently).
3. **OCR/picker:** on a real fissure reward screen, hit the hotkey and confirm the
   overlay names all 4 rewards correctly and flags the highest plat/ducat pick;
   test at your native resolution and one other.
4. **Overlay:** confirm the layer-shell panel draws above Warframe running
   borderless-fullscreen on KWin, and is click-through when idle.
5. **(Phase 4)** POC: read and print live credits/plat from process memory and
   confirm it matches the in-game value across a loading screen.

## Not in scope (vs. AlecaFrame)

Market order auto-management, foundry/mastery helper, riven tracking, stats
history, and trade auto-completion — all depend on the full-inventory (memory)
tier and/or market account integration, deferred to Phase 4+ or dropped for the
"light" build.
