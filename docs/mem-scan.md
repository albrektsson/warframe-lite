# mem-scan

`wf-lite mem-scan` reads Foundry state (in-progress and owned Prime
blueprints), owned relics, owned equipment, rivens, and level keys straight
out of the running game's own memory, then echoes the session token it finds
there once to DE's own `inventory.php` endpoint — see
[ADR-0001](adr/0001-observe-only-never-touch-game-process.md) and
[ADR-0013](adr/0013-token-relay-session-nonce-is-not-a-credential.md) for why
this is a read, not a held credential. The raw scan/session token itself is
never cached or written to disk; the owned-relic *counts* it decodes do get
written to `owned-relics.json`, same as the OCR-based relic scan already
does.

## Entry points

Three ways to trigger the same scan, all equally consenting (a deliberate
click or invocation, no separate confirmation prompt):

- `wf-lite mem-scan` on the command line.
- `wf-lite browse`'s **Home** tab has a **Scan Memory** button that runs the
  scan in-process and shows the same permission guidance on failure.
- `wf-tray`'s right-click menu has a matching **Scan Memory** item, which
  instead re-execs `wf-lite mem-scan` as a child process (the tray
  supervises every GUI/scan action as a separate process for crash
  isolation — see `wf-tray`'s own module docs) and folds the outcome into
  its tooltip and status line.

## Build-time presence vs. consent

`mem-scan` is part of `wf-lite`'s default build
([ADR-0016](adr/0016-mem-scan-is-default-compiled-not-opt-in.md)), so a bare
`cargo build --release` already includes it; the `mem-scan` cargo feature
still exists so tests/CI can build without it:

```
cargo build --release                                            # includes mem-scan (default)
cargo build --release --no-default-features --features mem-scan  # explicit, equivalent
cargo build --release --no-default-features                      # excludes the mem-scan CLI subcommand
```

Building the code in is not consent to run it — running `wf-lite mem-scan`
(or clicking Scan Memory) at all **is** the explicit in-the-moment consent
this feature assumes; it doesn't prompt separately either way.

## `CAP_SYS_PTRACE`

The scan needs Warframe already running and logged in, and the binary needs
permission to read another process's memory. This map decided against
wiring that into packaging, so it's a manual, one-time step after each
build/install:

```
sudo setcap cap_sys_ptrace=+ep /path/to/wf-lite
```

Alternatively, lower `/proc/sys/kernel/yama/ptrace_scope` to `1` or below —
`setcap` is the narrower, per-binary grant and the preferred option. Without
either, `mem-scan` fails immediately with a clear permission error naming
both fixes, rather than a bare I/O error — the GUI/tray entry points above
surface that same error text verbatim.
