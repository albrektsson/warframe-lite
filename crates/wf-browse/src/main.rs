//! `wf-browse` — a small graphical browser for warframe-lite: which primes
//! you've mastered, which Owned relics can still get you there, and which are
//! worth selling instead of cracking.
//!
//! Strictly read-only: this never writes to `owned-relics.json`. The OCR scan
//! of the in-game Void Relics screen (via `wf-lite overlay`) is the only
//! source of Owned relic counts (see ADR-0001, ADR-0003).
//!
//! Kept in a separate binary from `wf-lite`/`wf-settings` so each companion
//! stays purpose-built and lean (see ADR-0002).

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use wf_config::Config;
use wf_relic::{MasteryEntry, MasterySet, RelicIndex};

const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const MASTERY_TTL: Duration = Duration::from_secs(24 * 3600);

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wf_browse=info".into()),
        )
        .with_target(false)
        .init();

    let config_path = Config::default_path().unwrap_or_else(|_| PathBuf::from("config.toml"));
    let config = Config::load(&config_path).unwrap_or_default();

    let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
    let (index, mastery) = rt.block_on(load_data(&config));
    let mastery_rows = wf_relic::mastery_browser(&index, &mastery);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("warframe-lite browse")
            .with_inner_size([560.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "warframe-lite browse",
        options,
        Box::new(move |_cc| Ok(Box::new(BrowseApp::new(mastery_rows)))),
    )
}

/// Load the relic catalogue + the player's mastered set, falling back to an
/// empty catalogue / empty mastery set on failure rather than blocking the GUI
/// from opening at all.
async fn load_data(config: &Config) -> (RelicIndex, MasterySet) {
    let client = wf_data::http_client();
    let index = RelicIndex::load_cached(&client, CATALOGUE_TTL).await.unwrap_or_else(|e| {
        tracing::warn!("relic catalogue load failed: {e:#}");
        RelicIndex::new(Vec::new())
    });
    let mastery = match &config.account_id {
        Some(id) => wf_relic::mastery::load_cached(&client, id, MASTERY_TTL).await,
        None => MasterySet::default(),
    };
    (index, mastery)
}

/// The browser's tabs. Only `Mastery` exists so far — `Relics` and `Sell`
/// land in later tickets (#13, #14).
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Mastery,
}

struct BrowseApp {
    tab: Tab,
    mastery_rows: Vec<MasteryEntry>,
    /// Editable text buffer for the Mastery tab's search box.
    filter: String,
}

impl BrowseApp {
    fn new(mastery_rows: Vec<MasteryEntry>) -> Self {
        Self { tab: Tab::Mastery, mastery_rows, filter: String::new() }
    }

    fn mastery_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.filter);
        });
        ui.add_space(6.0);

        let filter = self.filter.to_ascii_lowercase();
        let rows: Vec<&MasteryEntry> = self
            .mastery_rows
            .iter()
            .filter(|e| filter.is_empty() || e.prime.to_ascii_lowercase().contains(&filter))
            .collect();

        let mastered = rows.iter().filter(|e| e.mastered).count();
        ui.label(format!("{mastered} / {} mastered", rows.len()));
        ui.add_space(4.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("mastery_grid").num_columns(2).striped(true).show(ui, |ui| {
                for entry in &rows {
                    ui.label(&entry.prime);
                    ui.label(if entry.mastered { "✓ mastered" } else { "— unmastered" });
                    ui.end_row();
                }
            });
        });
    }
}

impl eframe::App for BrowseApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Mastery, "Mastery");
            });
            ui.separator();

            match self.tab {
                Tab::Mastery => self.mastery_tab(ui),
            }
        });
    }
}
