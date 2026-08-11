//! `wf-browse` standalone binary — thin entry point over the [`wf_browse`]
//! library.
//!
//! Kept for standalone dev/embedding builds (`cargo run -p wf-browse`); the
//! distributed binary is `wf-lite`, which links this crate as a library and
//! runs [`wf_browse::run`] in-process for its `browse` subcommand instead of
//! spawning this binary (see #69).

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wf_browse=info".into()),
        )
        .with_target(false)
        .init();

    wf_browse::run()
}
