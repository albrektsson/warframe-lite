//! `wf-settings` — a small graphical settings window for warframe-lite.
//!
//! Edits the same `~/.config/warframe-lite/config.toml` the overlay reads:
//! placement (anchor + margins), opacity, and whether the world-state panel
//! shows. It also helps bind the show/hide hotkey — on Wayland the click-through
//! overlay can't grab a global key itself, so toggling goes through
//! `wf-lite toggle`, which the user binds as a KDE custom shortcut; this window
//! surfaces that command and can open KDE's shortcut settings.
//!
//! Kept in a separate binary from `wf-lite` so the overlay stays a lean,
//! self-contained binary; the GUI's heavier dependencies live only here.

use std::path::PathBuf;

use eframe::egui;
use wf_config::Config;

const ANCHORS: &[&str] = &[
    "top-left",
    "top-right",
    "bottom-left",
    "bottom-right",
    "top",
    "bottom",
    "left",
    "right",
    "center",
];

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wf_settings=info".into()),
        )
        .with_target(false)
        .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("warframe-lite settings")
            .with_inner_size([420.0, 460.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "warframe-lite settings",
        options,
        Box::new(|_cc| Ok(Box::new(SettingsApp::load()))),
    )
}

struct SettingsApp {
    config_path: PathBuf,
    config: Config,
    /// Editable text buffer for the account id field.
    account_id: String,
    status: String,
}

impl SettingsApp {
    fn load() -> Self {
        let config_path = Config::default_path().unwrap_or_else(|_| PathBuf::from("config.toml"));
        let config = Config::load(&config_path).unwrap_or_default();
        let account_id = config.account_id.clone().unwrap_or_default();
        Self {
            config_path,
            config,
            account_id,
            status: String::new(),
        }
    }

    fn save(&mut self) {
        self.config.account_id = if self.account_id.trim().is_empty() {
            None
        } else {
            Some(self.account_id.trim().to_string())
        };
        match self.config.save(&self.config_path) {
            Ok(()) => {
                self.status = format!("Saved to {}", self.config_path.display());
            }
            Err(e) => self.status = format!("Save failed: {e:#}"),
        }
    }

    /// Run `wf-lite detect-account` (locating the sibling/`PATH` binary), then
    /// reload the config so the detected id appears in the field.
    fn detect_account(&mut self) {
        let bin = wf_lite_binary();
        let out = std::process::Command::new(&bin)
            .arg("detect-account")
            .output();
        match out {
            Ok(o) if o.status.success() => {
                self.config = Config::load(&self.config_path).unwrap_or_default();
                self.account_id = self.config.account_id.clone().unwrap_or_default();
                self.status = "Detected and saved account id".to_string();
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let msg = err
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| "see terminal for details".to_string());
                self.status = format!("Detect failed: {msg}");
            }
            Err(e) => self.status = format!("Couldn't run {}: {e}", bin.display()),
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("warframe-lite settings");
            ui.add_space(8.0);

            egui::Grid::new("placement")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Anchor");
                    egui::ComboBox::from_id_salt("anchor")
                        .selected_text(&self.config.overlay.anchor)
                        .show_ui(ui, |ui| {
                            for a in ANCHORS {
                                ui.selectable_value(
                                    &mut self.config.overlay.anchor,
                                    (*a).to_string(),
                                    *a,
                                );
                            }
                        });
                    ui.end_row();

                    ui.label("Margin X");
                    ui.add(egui::DragValue::new(&mut self.config.overlay.margin_x).range(0..=2000));
                    ui.end_row();

                    ui.label("Margin Y");
                    ui.add(egui::DragValue::new(&mut self.config.overlay.margin_y).range(0..=2000));
                    ui.end_row();

                    ui.label("Opacity");
                    ui.add(egui::Slider::new(&mut self.config.overlay.opacity, 0.1..=1.0));
                    ui.end_row();

                    ui.label("World-state panel");
                    ui.checkbox(
                        &mut self.config.overlay.world_state,
                        "show (off = reward picker only)",
                    );
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.separator();
            ui.label("Mastery account id");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.account_id)
                        .hint_text("24-hex account id")
                        .desired_width(240.0),
                );
                if ui.button("Detect from log").clicked() {
                    self.detect_account();
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.label("Show/hide hotkey");
            ui.label(
                egui::RichText::new(
                    "Wayland can't let the overlay grab a global key. Bind this command \
                     as a KDE custom shortcut:",
                )
                .weak(),
            );
            ui.horizontal(|ui| {
                ui.code("wf-lite toggle");
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text("wf-lite toggle".to_string());
                    self.status = "Copied command".to_string();
                }
                if ui.button("Open KDE shortcuts").clicked() {
                    open_kde_shortcuts(&mut self.status);
                }
            });

            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.save();
                }
                ui.label(egui::RichText::new(&self.status).weak());
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Restart `wf-lite overlay` to apply placement changes.")
                    .small()
                    .weak(),
            );
        });
    }
}

/// Locate the `wf-lite` binary: prefer a sibling of this executable, else rely on
/// `PATH`.
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

/// Best-effort: open KDE's global-shortcuts settings module.
fn open_kde_shortcuts(status: &mut String) {
    for (cmd, args) in [
        ("systemsettings", &["kcm_keys"][..]),
        ("kcmshell6", &["kcm_keys"][..]),
        ("kcmshell5", &["kcm_keys"][..]),
    ] {
        if std::process::Command::new(cmd).args(args).spawn().is_ok() {
            *status = "Opened KDE shortcut settings".to_string();
            return;
        }
    }
    *status = "Couldn't open KDE settings — add a custom shortcut manually".to_string();
}
