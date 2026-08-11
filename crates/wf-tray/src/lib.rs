//! `wf-tray` — the system-tray subsystem of warframe-lite.
//!
//! Sits in the KDE tray (a DBus StatusNotifierItem via [`ksni`], pure-Rust so
//! no libdbus dependency) and waits for Warframe to start. When the game
//! window appears it auto-starts the overlay (`<this binary> overlay`); when
//! the game closes it stops it again. The tray menu also shows/hides the
//! overlay, opens the settings window, opens the mastery/relic browser,
//! detects the account id, and quits — a single control point for the app's
//! modes.
//!
//! [`run`] is the entry point both the standalone `wf-tray` binary (kept for
//! dev/embedding) and the merged `wf-lite` binary call — `wf-lite`'s bare
//! invocation and its `tray` subcommand both run this in-process, on the
//! same tokio runtime, rather than spawning a separate tray process (see
//! ticket #69). It supervises the overlay (and the settings/browse windows
//! opened from its menu) as separate *child processes* rather than embedding
//! them, so a GUI crash can't take the tray or the overlay down with it, and
//! the overlay can be started/stopped independently as the game
//! appears/disappears.
//!
//! It does not initialize a `tracing` subscriber itself — the process that
//! owns `main` (the standalone `wf-tray` binary, or `wf-lite`) does that
//! once, since a second `tracing_subscriber::fmt()...init()` call in the
//! same process would panic.

use std::path::PathBuf;
use std::process::Child;
use std::sync::OnceLock;
use std::time::Duration;

use ksni::TrayMethods;

const POLL: Duration = Duration::from_secs(2);

/// The warframe-lite mark (a hexagon enclosing an "M"), bundled as a PNG so the
/// tray icon lives inside the binary — no dependency on an installed icon theme,
/// which is why a bare `icon_name` showed nothing. Decoded once into ksni's
/// ARGB32 pixmap on first use.
fn app_icon() -> Vec<ksni::Icon> {
    static ICON: OnceLock<Option<ksni::Icon>> = OnceLock::new();
    ICON.get_or_init(|| {
        let bytes = include_bytes!("../assets/icon.png");
        match image::load_from_memory(bytes) {
            Ok(img) => {
                let img = img.to_rgba8();
                let (width, height) = (img.width() as i32, img.height() as i32);
                let mut data = img.into_vec();
                // ksni wants ARGB32 in network byte order; `image` gives RGBA.
                for px in data.chunks_exact_mut(4) {
                    px.rotate_right(1);
                }
                Some(ksni::Icon { width, height, data })
            }
            Err(e) => {
                tracing::error!("failed to decode bundled tray icon: {e}");
                None
            }
        }
    })
    .clone()
    .into_iter()
    .collect()
}

/// Register the tray icon and poll for the game window, driving auto
/// start/stop of the overlay through the tray state. Runs until the tray is
/// shut down (e.g. the DBus host disappears) or the process exits via the
/// menu's Quit item.
pub async fn run() -> anyhow::Result<()> {
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
    /// The supervised `overlay` child, if running.
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
        match spawn_self(&["overlay"]) {
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
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        app_icon()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "warframe-lite".into(),
            description: self.status_text(),
            icon_name: String::new(),
            icon_pixmap: app_icon(),
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

/// The binary to re-exec for a subcommand (`overlay`, `settings`, `browse`,
/// `detect-account`, `show`/`hide`): this process's own executable.
///
/// Before the single-binary merge (#69) this looked for a sibling `wf-lite`
/// next to `wf-tray`, falling back to `PATH` — necessary because the tray
/// and the overlay/GUI subcommands lived in separate installed binaries. Now
/// that `wf-tray`'s tray logic is reachable *from* `wf-lite` itself (the
/// merged binary's `tray` subcommand, and its default bare invocation, both
/// call [`run`] in-process), the binary that understands `overlay` /
/// `settings` / `browse` / `detect-account` / `show` / `hide` as
/// subcommands is simply whichever executable is currently running — hence
/// `current_exe()` rather than a name lookup. This is a self-re-exec, not a
/// sibling-binary spawn.
///
/// Falls back to bare `wf-lite` (`PATH`) if `current_exe()` fails, matching
/// the old behavior's last resort.
///
/// Note: the standalone `wf-tray` binary (kept for dev/embedding, see the
/// crate docs) doesn't itself parse subcommands, so this only fully works
/// when `run` is running inside `wf-lite` — the merged, distributed binary.
/// Standalone `wf-tray` is for tray UI development, not full end-to-end
/// supervision; use `wf-lite tray` (or a bare `wf-lite`) for that.
fn self_binary() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("wf-lite"))
}

/// Spawn `<self> <args>` and return the child (kept for the supervised overlay).
fn spawn_self(args: &[&str]) -> std::io::Result<Child> {
    std::process::Command::new(self_binary()).args(args).spawn()
}

/// Fire-and-forget a short `<self> <args>` command, reaping it on a detached
/// thread so it doesn't become a zombie.
fn run_detached(args: &[&str]) {
    match spawn_self(args) {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::error!("could not run {args:?}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #69: the overlay supervisor (and the Settings…/Browse…/Detect account
    /// id menu items) must re-exec *this running process's own binary*, not
    /// look up a same-named sibling binary next to it or on `PATH` — that
    /// sibling-lookup was the pre-merge behavior when `wf-tray` and
    /// `wf-lite` were separate installed binaries. Asserting `self_binary()`
    /// equals `current_exe()` pins the self-re-exec behavior even though a
    /// live-game overlay spawn can't be exercised in a unit test.
    #[test]
    fn self_binary_is_current_exe_not_a_sibling_lookup() {
        assert_eq!(self_binary(), std::env::current_exe().unwrap());
    }
}
