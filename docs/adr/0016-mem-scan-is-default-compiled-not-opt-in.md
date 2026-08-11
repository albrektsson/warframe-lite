# mem-scan is default-compiled; build-time default is not the consent gate

Map [#55](https://github.com/albrektsson/warframe-lite/issues/55) (charting
Notes, plus comments in the root and `wf-lite` `Cargo.toml`) decided to keep
`wf-mem` out of the workspace's `default-members` and `mem-scan` out of
`wf-lite`'s default feature set: a bare `cargo build`/`cargo build --release`
built every other binary but never linked the memory-reading code, requiring
`--features mem-scan` explicitly. That decision is reversed here, per map
[#68](https://github.com/albrektsson/warframe-lite/issues/68)'s Q3 and
[issue #70](https://github.com/albrektsson/warframe-lite/issues/70): `wf-mem`
is now a default workspace member, and `mem-scan` is now part of `wf-lite`'s
`default` feature set. A bare `cargo build`, `cargo build --release`, and
official release binaries all include the `mem-scan` subcommand without any
extra flag.

This is recorded as its own ADR, narrowing map #55's decision rather than
editing it in place, following the convention ADR-0013 set relative to
ADR-0001.

## Why this doesn't touch ADR-0001 or ADR-0013

ADR-0001's observe-only rule and ADR-0013's token-relay reasoning are about
*what* `mem-scan` is allowed to do at runtime (read-only, a relayed session
nonce is not a held credential) — neither says anything about whether the
code compiles in by default. ADR-0013 separately establishes that invoking
`wf-lite mem-scan` (or, later, a GUI "Scan Memory" button) is itself the
required in-the-moment consent, with no additional interactive prompt. That
consent gate is unchanged: compiling the subcommand into the binary is not
running it, and running it is still the only thing that triggers a memory
read or an `inventory.php` call. Nothing about this ticket adds a prompt,
removes the ability to say no, or makes `mem-scan` run unattended.

The other half of map #55's decision — that `sudo setcap
cap_sys_ptrace=+ep` stays a manual, un-packaged step rather than being
wired into `packaging/warframe-lite.spec` or auto-granted by the app — is
*not* reversed by this ADR. That's the actual privileged-capability grant;
this ADR is purely about which crates a plain `cargo build` links.

## Why reverse it

`wf-mem` (`crates/wf-mem`) carries no extra system build dependencies
(pure Rust: `anyhow`, `tracing`, `reqwest`, `serde`, `serde_json`, `time`),
so making it default-compiled adds no new `BuildRequires`/packaging burden
(unlike, say, `wf-ocr`'s native `tesseract`/`leptonica` linkage). With
`wf-mem`'s feature set now covering Foundry, Rivens, equipment, mastery
cross-reference, and owned-relic decoding (map #55, closed), the map #68
destination treats `mem-scan` as a first-class default feature of the
single distributed binary rather than a niche opt-in — matching how every
other `wf-lite` subcommand ships by default. The `mem-scan` cargo feature
flag itself is kept (not deleted) specifically so
`--no-default-features`/`--no-default-features --features mem-scan` still
lets tests and CI exercise both configurations, per the regression-testing
value #58 and #67 already relied on.
