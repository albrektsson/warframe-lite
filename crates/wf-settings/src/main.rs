//! `wf-settings` standalone binary — thin entry point over the [`wf_settings`]
//! library.
//!
//! Kept for standalone dev/embedding builds (`cargo run -p wf-settings`); the
//! distributed binary is `wf-lite`, which links this crate as a library and
//! runs [`wf_settings::run`] in-process for its `settings` subcommand instead
//! of spawning this binary (see #69).

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wf_settings=info".into()),
        )
        .with_target(false)
        .init();

    wf_settings::run()
}
