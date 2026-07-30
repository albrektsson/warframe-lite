# warframe-lite

A standalone, **Linux-native** light variant of [AlecaFrame](https://alecaframe.com/) —
a Warframe companion that runs without Overwolf. Built for KDE Plasma (Wayland) +
Steam Proton, in Rust.

See [`docs/PLAN.md`](docs/PLAN.md) for the full feasibility analysis and roadmap.

## Status

**Phase 0 — data plumbing: done.** Config load, Steam-Proton `EE.log`
auto-detection, live world state (fissures / Void Trader / cycles), and
warframe.market pricing all verified live.

**Phase 1 — timers/world-state overlay: done.** A pure-Rust `wlr-layer-shell`
overlay (top layer, click-through, anchored top-right) shows live fissures
(sorted normal → Steel Path → Storm), Baro, and Cetus/Vallis/Cambion cycles,
re-rendering every second so ETAs tick. Verified on-screen on KDE Plasma (KWin).

**Phase 2 — relic reward picker: done.** OCR (`wf-ocr`, tesseract CLI) + picker
brain (`wf-relic`: fuzzy match to the 3.8k-item warframe.market catalogue +
plat/ducat ranking). Handles the **variable, centred reward layout** (2–4 cards
depending on how many squadmates cracked) by scanning a superset of candidate
slot centres and keeping whichever OCR to items, and reads **two-line wrapped
names**. Validated on real 3440×1440 screens — a 4-reward screen and a 3-reward
screen with a wrapped name (*Voruna Prime Systems Blueprint*) both fully resolve.

**Mastery tracking (`wf-relic::mastery`): done.** Using DE's **public** profile
API (`getProfileViewingData.php`, no auth), warframe-lite marks which rewards you
have already **mastered**. It reads `LoadOutInventory.XPInfo` and treats an item
as mastered once its lifetime affinity passes the rank-30 cap (450k weapons /
900k frames — verified against a real profile), maps each reward *part* to its
built prime, and shows a green **MR** badge (and dims the name) on mastered
items. Set your account id once (`wf-lite set-account <id>` — find it at
`warframe.com/api/user-data`); the mastered set is cached for a day. The ducat
column was dropped (it's clearly shown in-game during selection).

**Caching (`wf-cache`): done.** The item catalogue is cached to
`~/.cache/warframe-lite/items.json` (7-day TTL, stale-served offline) — warm
startup is ~19× faster (0.58s → 0.03s). Prices are cached per item with a
freshness TTL; during a relic scan all four are fetched **concurrently**, fresh
cache is used instantly, and **if warframe.market is slow/unreachable the last
known price is served immediately** — so the panel is ready inside the
few-second selection window.

**Phase 3 — automatic detection + overlay integration: done.** `wf-lite overlay`
now shows world state normally and **automatically swaps to the ranked reward
result when the fissure reward screen is on screen** — no keypress needed.

Because the reward screen is a mid-mission, ~15-second thing the player controls
(it can be brought up via Tab/progress) and Warframe flushes its log in bursts,
we don't treat the log as a stopwatch. Instead, a relic **crack**
(`DVRCAftermath`) or a **reward-screen** line (`ProjectionRewardChoice: Got
rewards`) opens a ~150s **polling window**, during which the screen is OCR-scanned
every 2s. The OCR guard (≥2 of 4 names resolve) is what confirms the screen is up,
so it's caught whenever it appears (auto, Tab-shown, or flush-delayed). The result
shows for ~20s, de-bounced so one screen isn't re-shown. **Confirmed working in a
live fissure** (the ranked panel rendered with the correct best pick). The overlay
is placed on the **monitor Warframe is on** (matched by the game window's centre
against each output's logical geometry).

**Two hardest Linux unknowns de-risked (verified against the running game):**

- **EE.log parsing** (`wf-log`): line parser hits **98.1%** coverage on real
  logs (remainder are multi-line continuations); rotation/append-aware tailer.
  Confirmed that Warframe **buffers log output and flushes in bursts**, so relic
  detection must offer a **manual hotkey trigger**, not rely on log timing.
- **X11 capture** (`wf-capture`): pure-Rust `x11rb` locates the Warframe
  Xwayland window and reads a full 3440×1440 frame via `GetImage` — real,
  legible content, **no black-frame/DXVK issue**, no portal prompt.

```
wf-lite                    # world-state + EE.log detection + price lookup
wf-lite <market_slug>      # price summary, e.g. `wf-lite mirage_prime_set`
wf-lite logstats           # parse whole EE.log history, report coverage/events
wf-lite logwatch           # follow EE.log live, print recognized events
wf-lite capture [out.png]  # capture the Warframe window to a PNG
wf-lite overlay-png [p]    # render the world-state panel to a PNG (offscreen)
wf-lite overlay            # show the live wlr-layer-shell overlay
wf-lite ocr [x y w h]      # OCR the Warframe window (or a region) — pipeline test
wf-lite relic [names…]     # evaluate reward names → matched item, plat, ducats
wf-lite relic-scan         # capture the reward screen, OCR 4 names, rank them
wf-lite set-account <id>   # save your account id for mastery lookup
wf-lite mastery [id]       # report your mastered-item count
```

> OCR shells out to the `tesseract` CLI (no linking). On Bazzite it was installed
> with `brew install tesseract`; any reachable `tesseract` works (override with
> the `WF_TESSERACT` env var).

> Overlay uses `smithay-client-toolkit` with **default features off + only
> `calloop`**, deliberately avoiding the `xkbcommon` system dev dependency. The
> only runtime system lib is `libwayland-client` (present on any Wayland desktop).

Config lives at `~/.config/warframe-lite/config.toml` (created on demand); the
`EE.log` path is auto-detected but can be overridden there.

## Roadmap

- **Phase 0** — data plumbing ✅
- **Phase 1** — timers/world-state overlay ✅
- **Phase 2** — relic reward picker ✅ (calibrated for 3440×1440)
- **Phase 3** — auto-detect at fissure crack + overlay integration ✅
- **Caching** — item-catalogue + stale-serving price cache ✅
- **Later polish** — config-overridable reward regions, per-output placement,
  optional global hotkey, background price pre-warm at fissure start
- **Phase 4** *(optional)* — inventory via `process_vm_readv` memory reading

## Architecture

Cargo workspace:

- `crates/wf-config` — TOML config + Steam-Proton `EE.log` auto-detection
- `crates/wf-data` — world-state (warframestat.us) and market (warframe.market **v2**) clients
- `crates/wf-log` — EE.log line parser + rotation-aware tailer + event classifier
- `crates/wf-capture` — pure-Rust X11 capture of the Warframe Xwayland window
- `crates/wf-overlay` — dependency-light canvas/renderer + `wlr-layer-shell` display
- `crates/wf-ocr` — Tesseract-CLI OCR with Warframe-tuned preprocessing
- `crates/wf-relic` — item catalogue index, fuzzy OCR-name matching, plat/ducat ranking
- `crates/wf-cache` — disk-backed caches (`~/.cache/warframe-lite/`)
- `src/main.rs` — `wf-lite` binary (subcommands above)

### External API notes

- **warframe.market:** use the **v2** API (`/v2/orders/item/{slug}`). The legacy
  v1 endpoint returns **403**.
- **warframestat.us:** fissure/cycle objects carry only an `expiry` timestamp
  (no pre-formatted `eta`/`timeLeft`); remaining time is computed locally.

## Build

```
cargo build
cargo test
```
