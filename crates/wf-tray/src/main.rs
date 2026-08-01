//! `wf-tray` — a system-tray companion for warframe-lite.
//!
//! Launched from a desktop shortcut, it sits in the KDE tray (a DBus
//! StatusNotifierItem via [`ksni`], pure-Rust so no libdbus dependency) and waits
//! for Warframe to start. When the game window appears it auto-starts the overlay
//! (`wf-lite overlay`); when the game closes it stops it again. The tray menu also
//! shows/hides the overlay, opens the settings window, opens the mastery/relic
//! browser, detects the account id, and quits — a single control point for the
//! app's modes.
//!
//! It supervises `wf-lite` as a child process rather than embedding the overlay,
//! so the tray stays small and the overlay keeps its own lean binary.

use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;

use ksni::TrayMethods;

const POLL: Duration = Duration::from_secs(2);

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wf_tray=info".into()),
        )
        .with_target(false)
        .init();

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let tray = WfTray {
        auto_start: true,
        game_present: false,
        overlay: None,
        overlay_visible: true,
    };
    let handle = tray
        .spawn()
        .await
        .map_err(|e| anyhow::anyhow!("registering tray icon (is a StatusNotifier host running?): {e}"))?;
    tracing::info!("warframe-lite tray running; waiting for the game");

    // Poll for the game window and drive auto start/stop through the tray state.
    loop {
        tokio::time::sleep(POLL).await;
        let present = tokio::task::spawn_blocking(|| wf_capture::warframe_geometry().is_ok())
            .await
            .unwrap_or(false);
        if handle.update(move |t: &mut WfTray| t.on_game_state(present)).await.is_none() {
            break; // tray shut down
        }
    }
    Ok(())
}

struct WfTray {
    /// Auto-start the overlay when the game appears (and stop it when it exits).
    auto_start: bool,
    game_present: bool,
    /// The supervised `wf-lite overlay` child, if running.
    overlay: Option<Child>,
    /// Our intended overlay visibility (what the last show/hide asked for).
    overlay_visible: bool,
}

impl WfTray {
    fn running(&self) -> bool {
        self.overlay.is_some()
    }

    fn status_text(&self) -> String {
        if self.running() {
            if self.overlay_visible {
                "Overlay running".into()
            } else {
                "Overlay running (hidden)".into()
            }
        } else if self.game_present {
            "Warframe detected — overlay stopped".into()
        } else {
            "Waiting for Warframe…".into()
        }
    }

    fn start_overlay(&mut self) {
        if self.overlay.is_some() {
            return;
        }
        match spawn_wf_lite(&["overlay"]) {
            Ok(child) => {
                self.overlay = Some(child);
                self.overlay_visible = true;
                tracing::info!("overlay started");
            }
            Err(e) => tracing::error!("could not start overlay: {e}"),
        }
    }

    fn stop_overlay(&mut self) {
        if let Some(mut child) = self.overlay.take() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!("overlay stopped");
        }
    }

    /// React to the game's presence: reap a self-exited overlay, then auto start
    /// when the game is up (and stop when it's gone).
    fn on_game_state(&mut self, present: bool) {
        self.game_present = present;
        if let Some(child) = &mut self.overlay {
            if matches!(child.try_wait(), Ok(Some(_))) {
                self.overlay = None;
            }
        }
        if present {
            if self.auto_start && self.overlay.is_none() {
                self.start_overlay();
            }
        } else if self.overlay.is_some() {
            self.stop_overlay();
        }
    }
}

impl ksni::Tray for WfTray {
    fn id(&self) -> String {
        "warframe-lite".into()
    }
    fn title(&self) -> String {
        "warframe-lite".into()
    }
    fn icon_name(&self) -> String {
        "applications-games".into()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "warframe-lite".into(),
            description: self.status_text(),
            icon_name: "applications-games".into(),
            icon_pixmap: Vec::new(),
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let mut items: Vec<ksni::MenuItem<Self>> = vec![
            StandardItem {
                label: self.status_text(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
        ];

        if self.running() {
            items.push(
                CheckmarkItem {
                    label: "Overlay shown".into(),
                    checked: self.overlay_visible,
                    activate: Box::new(|this: &mut Self| {
                        this.overlay_visible = !this.overlay_visible;
                        run_detached(&[if this.overlay_visible { "show" } else { "hide" }]);
                    }),
                    ..Default::default()
                }
                .into(),
            );
            items.push(
                StandardItem {
                    label: "Stop overlay".into(),
                    icon_name: "media-playback-stop".into(),
                    activate: Box::new(|this: &mut Self| this.stop_overlay()),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            items.push(
                StandardItem {
                    label: "Start overlay".into(),
                    icon_name: "media-playback-start".into(),
                    activate: Box::new(|this: &mut Self| this.start_overlay()),
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(
            CheckmarkItem {
                label: "Auto-start with game".into(),
                checked: self.auto_start,
                activate: Box::new(|this: &mut Self| this.auto_start = !this.auto_start),
                ..Default::default()
            }
            .into(),
        );
        items.push(ksni::MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Settings…".into(),
                icon_name: "configure".into(),
                activate: Box::new(|_| run_detached(&["settings"])),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Browse…".into(),
                icon_name: "view-list-details".into(),
                activate: Box::new(|_| run_detached(&["browse"])),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Detect account id".into(),
                icon_name: "system-search".into(),
                activate: Box::new(|_| run_detached(&["detect-account"])),
                ..Default::default()
            }
            .into(),
        );
        items.push(ksni::MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    this.stop_overlay();
                    std::process::exit(0);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

/// Locate the `wf-lite` binary: prefer a sibling of this executable, else `PATH`.
fn wf_lite_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("wf-lite");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("wf-lite")
}

/// Spawn `wf-lite <args>` and return the child (kept for the supervised overlay).
fn spawn_wf_lite(args: &[&str]) -> std::io::Result<Child> {
    std::process::Command::new(wf_lite_binary()).args(args).spawn()
}

/// Fire-and-forget a short `wf-lite <args>` command, reaping it on a detached
/// thread so it doesn't become a zombie.
fn run_detached(args: &[&str]) {
    match spawn_wf_lite(args) {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::error!("could not run wf-lite {args:?}: {e}"),
    }
}
