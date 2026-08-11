# warframe-lite

A standalone, **Linux-native** companion for [Warframe](https://www.warframe.com/) —
a light alternative to [AlecaFrame](https://alecaframe.com/) that runs **without
Overwolf**. Built for KDE Plasma (Wayland) with the game under Steam Proton, in Rust.

It shows a click-through overlay on top of the game with:

- **Live Void Fissures** — sorted normal → Steel Path → Storm, with ETAs that
  tick down each second. (General world state — the Void Trader and open-world
  cycles — is out of scope; see `docs/adr/0007-live-world-state-out-of-scope.md`.)
- **An automatic relic reward picker** — when a Void Fissure reward screen
  appears, the overlay swaps to the 2–4 rewards ranked by live warframe.market
  plat price, with the best pick highlighted and a mastery emblem in front of
  primes you have already **mastered**. No keypress needed.
- **An owned-relic mastery guide** — open the in-game **Void Relics** screen and
  the overlay automatically scans your relics as you scroll, then shows which of
  the ones you own can still drop a prime you **haven't mastered**, ranked by
  relic price. Also `wf-lite relics <codes…>` from the CLI.
- **A mastery planner** — `wf-lite mastery-plan` flips the view around: for every
  prime you haven't mastered, which of your owned relics (and how many) can
  still drop it, so you know which fissure tier is worth farming next. The
  scanned owned-relic set is saved to disk, so this works any time — no need to
  have the Relics screen open.

It only *observes* the game — no Overwolf, no memory reading, no account
credentials.

## Install & run

warframe-lite is a single self-contained binary (`wf-lite`) — no runtime other
than glibc and a `dlopen`'d `libwayland-client.so.0` (present on every Wayland
desktop). It reaches every subsystem (tray, overlay, browse window, mem-scan)
by re-execing itself, not by spawning separately-installed sibling binaries —
one file to build and install. The relic OCR features (the automatic
reward picker, the owned-relic scanner, and the `ocr`/`ocr-file`/`relic-file`/
`relic-scan` commands) are an **opt-in build-time feature** — see
[docs/ocr.md](docs/ocr.md) for building with it and its runtime library
requirements; a binary built without it runs fine, those commands just print
a message pointing at a rebuild.

**1. Build it.** No prebuilt releases are published pre-1.0 — clone and build
from source (see [Build](#build)), then put the binary on your `PATH`:

```
git clone https://github.com/albrektsson/warframe-lite.git
cd warframe-lite
cargo build --release
install -Dm755 target/release/wf-lite ~/.local/bin/wf-lite
```

**2. Run it — bare `wf-lite` is the easy way.** Launch it (from a menu shortcut
or terminal) with no arguments. It sits in the KDE system tray, waits for
Warframe to start, and **auto-starts the overlay when the game window
appears** (and stops it when the game closes). The tray menu shows/hides the
overlay, opens the browse window, detects your account id, runs a memory
scan, and quits. Install the desktop shortcut so it appears in your launcher:

```
install -Dm644 packaging/warframe-lite.desktop ~/.local/share/applications/warframe-lite.desktop
install -Dm644 packaging/warframe-lite.svg ~/.local/share/icons/hicolor/scalable/apps/warframe-lite.svg
```

Prefer no tray? Start the overlay directly instead — either in a terminal
(`wf-lite overlay`) or from Warframe's **Steam launch options**:

```
wf-lite overlay & %command%
```

`wf-lite overlay` polls up to 30s for the game window, then anchors the panel to
its top-right corner (correct in fullscreen *and* borderless-windowed).

**3. (Optional) Enable mastery badges.** Detect your account id from the game log
(scraped and verified against the public profile, so it can't pick a squadmate):

```
wf-lite detect-account
```

The id only appears in the log after some activity (a relic crack in a squad, a
Duviri race); if detection can't find it, set it manually — find it at
`warframe.com/api/user-data`:

```
wf-lite set-account <id>
```

## Commands

Every subcommand below runs inside this same `wf-lite` binary and process
(or a self-re-exec of it, for the overlay/mem-scan's crash isolation) —
there's nothing separate to install or keep alongside it:

```
wf-lite                    # (no command) show this list of commands
wf-lite status             # live Void Fissures
wf-lite <market_slug>      # price an item, e.g. `wf-lite mirage_prime_set`
wf-lite relics <codes…>    # owned-relic guide: unmastered rewards + prices
wf-lite mastery-plan       # unmastered primes + which owned relics drop them
wf-lite tray               # tray companion: waits for the game, runs the overlay
wf-lite overlay            # show the live overlay (live fissures + relic picker)
wf-lite browse             # open the mastery/relic browser (Home/Mastery/Relics/Sell/Settings)
wf-lite settings           # alias for `browse` — Settings is a tab there, not its own window
wf-lite toggle             # show/hide a running overlay (also: show / hide)
wf-lite copy               # copy the current best-pick reward (name + plat) to the clipboard
wf-lite capture [out.png]  # capture the Warframe window to a PNG
wf-lite relic [names…]     # evaluate reward names → matched item + plat
wf-lite relic-scan         # (feature-gated: ocr) capture the reward screen, OCR the names, rank them
wf-lite detect-account     # auto-detect your account id from EE.log (verified)
wf-lite set-account <id>   # save your account id for mastery lookup
wf-lite mastery [id]       # report your mastered-item count
wf-lite logstats           # parse whole EE.log history, report coverage/events
wf-lite logwatch           # follow EE.log live, print recognized events
wf-lite mem-scan           # read live Foundry/relic/equipment state — see docs/mem-scan.md
```

### OCR / relic-grid scanning (opt-in feature)

The relic OCR features — `ocr`, `ocr-file`, `relic-file`, `relic-scan`,
`relic-grid-file`, `inventory-grid-file`, and the live overlay's automatic
reward-picker and owned-relic/Prime-Part scanners — need the `ocr` cargo
feature at build time. Without it, those commands print a short message
pointing at a rebuild instead of failing to compile or erroring confusingly,
and the overlay still runs normally otherwise. See
[docs/ocr.md](docs/ocr.md) for the build command, required packages, and
runtime library table.

### mem-scan

`wf-lite mem-scan` reads Foundry/relic/equipment state straight out of the
running game's own memory (read-only, ADR-0001) — also reachable from
`wf-lite browse`'s Home tab and `wf-tray`'s right-click menu. It's part of
`wf-lite`'s default build; running it (from any of the three entry points)
is itself the required consent, and it needs a one-time `CAP_SYS_PTRACE`
grant. See [docs/mem-scan.md](docs/mem-scan.md) for the full permission
setup and consent-model detail.

## Configuration

Config lives at `~/.config/warframe-lite/config.toml` (created on demand); the
`EE.log` path is auto-detected from the Steam Proton prefix but can be overridden
there. Network results (item catalogue, prices, mastered set) are cached under
`~/.cache/warframe-lite/`.

### Overlay placement

Warframe uses every screen corner for HUD and menu elements, so the overlay's
position and visibility are configurable under `[overlay]`:

```toml
[overlay]
anchor = "top-right"   # top-left | top-right | bottom-left | bottom-right
                       # | top | bottom | left | right | center
margin_x = 24          # horizontal inset from the anchored edge(s), px
margin_y = 24          # vertical inset
fissures = true        # false = reward-only: invisible until a relic reward screen
opacity = 1.0          # 1.0 = as-drawn, lower = more transparent (e.g. 0.7)
```

**Hide it on a hotkey.** The overlay is click-through and can't grab a global
key itself, so bind a **KDE custom shortcut** (System Settings → Shortcuts →
Add Custom → Command) to `wf-lite toggle` (or `wf-lite hide` / `wf-lite show`).
The running overlay listens on a control socket and shows/hides instantly.

**Copy a reward to the clipboard.** Bind another KDE custom shortcut to
`wf-lite copy` to copy the currently-displayed best-pick reward's name and
plat price (e.g. `Mirage Prime Systems 45p`) to the clipboard, ready to paste
into Warframe's trade chat. Needs `wl-clipboard` ≥ 2.3.0 for
`ext-data-control-v1` support on KWin ≥ 6.5 — older packaged versions (e.g.
Fedora/Debian/Ubuntu's stock 2.2.1) may hit `wl-copy`'s own documented
popup-surface hang fallback instead of copying instantly. Override the binary
with `WF_WL_COPY` if yours is named or pathed differently.

### Settings tab

`wf-lite settings` (an alias for `wf-lite browse`) opens the browse window on
its **Settings** tab, to edit placement, opacity, and the fissure-panel
toggle, detect your account id, and help bind the KDE hotkey — all writing
the same `config.toml`. Restart `wf-lite overlay` to apply placement changes.

## Build

```
cargo build --release
cargo test
```

Only `wf-lite` needs installing — this also happens to build a few other
crates' own standalone `[[bin]]` targets (`wf-tray`, `wf-browse`,
`wf-settings`) into `target/release/`, but those are dev/embedding-only, not
part of the distributed product; see [docs/development.md](docs/development.md).
The plain build above needs none of `tesseract-devel`/`leptonica-devel`/
`clang` — those are only needed for the OCR feature, see
[docs/ocr.md](docs/ocr.md).

## License

MIT — see [`LICENSE`](LICENSE).

The bundled *Warframe* UI assets — the mastery laurel
(`crates/wf-overlay/assets/mastered.png`, shown on mastered rewards) and the
"unowned" eye icon (`assets/relic-unowned-eye.png`, used only to detect which
relics you don't own) — remain the property of **Digital Extremes Ltd.** They are
included solely to identify game state in this fan companion; *Warframe* is a
trademark of Digital Extremes. This project is unofficial and not affiliated with
or endorsed by Digital Extremes.

## Design & roadmap

See [`docs/PLAN.md`](docs/PLAN.md) for the feasibility analysis, architecture, and
implementation status; [`CONTEXT.md`](CONTEXT.md) for the project vision and
domain vocabulary; and [`AGENT.md`](AGENT.md) for contributing conventions.
