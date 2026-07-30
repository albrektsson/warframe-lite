# Plan: `warframe-lite` — a standalone, Linux-native light variant of AlecaFrame

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

## Phased implementation plan (with effort)

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
