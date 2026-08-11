# Building the individual crates standalone

`wf-lite` is the one binary this project distributes — a bare invocation
launches the tray, which auto-starts the overlay when the game appears (see
the wayfinder map [#68](https://github.com/albrektsson/warframe-lite/issues/68)).
It gets there by re-execing itself per subsystem (`overlay`, `browse`, …)
rather than spawning separately-installed sibling binaries.

Several subsystem crates also keep their own `[[bin]]` target, built
alongside `wf-lite` by a plain `cargo build --release` at the workspace
root. These are **not** part of the distributed product — they exist purely
for development or embedding:

- **`wf-tray`** (`cargo run -p wf-tray`) — the tray UI on its own, for
  iterating on tray behavior without the rest of the app.
- **`wf-browse`** (`cargo run -p wf-browse`) — the Mastery/Relics/Sell/
  Settings/Home window on its own.
- **`wf-settings`** (`cargo run -p wf-settings`, `cargo test -p
  wf-settings`) — the original standalone settings window. Its UI was
  folded into `wf-browse`'s tab bar as a "Settings" tab
  ([#72](https://github.com/albrektsson/warframe-lite/issues/72)), and
  nothing in `wf-lite`'s own command dispatch calls it anymore — it's kept
  purely as a standalone, independently buildable/testable crate.

None of these need to be installed or kept next to `wf-lite` for normal use;
`wf-lite tray`/`wf-lite browse` run the equivalent code in-process.

For the workspace's crate breadth, implementation conventions, and
contributing guidelines generally, see [`AGENT.md`](../AGENT.md).
