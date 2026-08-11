//! `wf-tray` standalone binary — thin entry point over the [`wf_tray`] library.
//!
//! Kept for standalone dev/embedding builds (`cargo run -p wf-tray`); the
//! distributed binary is `wf-lite`, which links this crate as a library and
//! runs [`wf_tray::run`] in-process for its `tray` subcommand and its
//! default (no-subcommand) invocation instead of spawning this binary (see
//! #69). See [`wf_tray::self_binary`]'s docs (crate-private, but summarized
//! in `lib.rs`'s module docs) for the one behavioral difference standalone
//! use has: it can't re-exec itself with a subcommand, so overlay/settings/
//! browse/detect-account launches from this binary's tray menu won't work
//! standalone — use `wf-lite tray` (or a bare `wf-lite`) for full behavior.

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wf_tray=info".into()),
        )
        .with_target(false)
        .init();

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(wf_tray::run())
}
