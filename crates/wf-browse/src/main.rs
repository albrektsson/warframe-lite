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
//!
//! The Relics & Plan and Sell tabs' Owned relic counts and active-Fissure flag
//! are live: a background task ([`poll`]) re-reads `owned-relics.json` and
//! re-fetches world state every [`POLL_INTERVAL`] while the window is open, so
//! they catch up with a scan happening in another window without a restart.
//! Mastery, the relic catalogue, and Sell-tab prices are loaded once at launch
//! and never touched by that timer.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use futures::stream::{self, StreamExt};
use wf_config::Config;
use wf_relic::{MasteryEntry, MasterySet, PrimePlan, RelicIndex, RelicPick};

const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const MASTERY_TTL: Duration = Duration::from_secs(24 * 3600);
/// How many relic price lookups run at once when pricing the Sell tab. A
/// player can own hundreds of distinct relics, so fetching prices one at a
/// time (like the CLI's small, explicit-args case) could block for minutes
/// once the shared cache goes stale; unbounded concurrency, on the other
/// hand, would fire every lookup at warframe.market in one burst.
const PRICE_FETCH_CONCURRENCY: usize = 8;
/// How often the Relics & Plan tab's Owned relic counts and active-Fissure
/// flag refresh while the window stays open. Only these two (a local file and
/// a lightweight world-state fetch) are cheap enough to poll; mastery, the
/// relic catalogue, and Sell-tab prices are loaded once at launch and never
/// re-fetched on this timer.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Shown on the Relics & Plan and Sell tabs when no relic scan has happened yet.
const NO_OWNED_DATA_MSG: &str = "no owned-relic data yet. Run `wf-lite overlay` (or the tray) and \
     open the in-game Void Relics screen once — it scans automatically as you scroll.";

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

    // `rt` is never dropped before `run_native` returns, so the poller spawned
    // on it below keeps running for as long as the window stays open.
    let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
    let LoadedData { index, mastery, owned, active_tiers, sell_prices } = rt.block_on(load_data(&config));

    let mastery_rows = wf_relic::mastery_browser(&index, &mastery);
    let live = Arc::new(Mutex::new(Live::compute(owned.as_ref(), &index, &mastery, &sell_prices, active_tiers)));

    rt.spawn(poll(live.clone(), index, mastery, sell_prices, config.platform));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("warframe-lite browse")
            .with_inner_size([620.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "warframe-lite browse",
        options,
        Box::new(move |_cc| Ok(Box::new(BrowseApp::new(mastery_rows, live)))),
    )
}

/// The Relics & Plan / Sell tabs' data — refreshed periodically by [`poll`]
/// while the window is open, independent of the launch-time [`LoadedData`].
struct Live {
    /// `None` when no relics have been scanned yet.
    plans: Option<Vec<PrimePlan>>,
    /// `None` when no relics have been scanned yet.
    sell_picks: Option<Vec<RelicPick>>,
    owned_age: Option<Duration>,
    active_tiers: HashSet<String>,
}

/// Lock `m`, recovering the guard even if a previous holder panicked while
/// holding it (rather than poisoning every future frame's render) — `Live`'s
/// own derivation is pure and shouldn't panic, but a stale-but-working UI
/// beats a permanent crash-loop if it ever does.
fn lock_live(m: &Mutex<Live>) -> std::sync::MutexGuard<'_, Live> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Live {
    /// Derive the live view from a fresh Owned relic read + active-Fissure
    /// set, against the launch-time relic catalogue/mastery/Sell prices
    /// (which never change after launch).
    fn compute(
        owned: Option<&wf_cache::Stamped<HashMap<String, u32>>>,
        index: &RelicIndex,
        mastery: &MasterySet,
        sell_prices: &HashMap<String, Option<u32>>,
        active_tiers: HashSet<String>,
    ) -> Self {
        // mastery_plan/sell_picks already rank their output.
        let plans = owned.map(|o| wf_relic::mastery_plan(&o.value, index, mastery));
        let sell_picks = owned.map(|o| wf_relic::sell_picks(&o.value, sell_prices, index, mastery));
        let owned_age = owned.map(|o| o.age());
        Self { plans, sell_picks, owned_age, active_tiers }
    }
}

/// Re-read `owned-relics.json` and re-fetch world state every [`POLL_INTERVAL`],
/// refreshing `live` — the only two things cheap/fast-changing enough to poll.
/// The relic catalogue, mastery, and Sell-tab prices stay exactly as loaded at
/// launch; only re-running the app refreshes those.
async fn poll(
    live: Arc<Mutex<Live>>,
    index: RelicIndex,
    mastery: MasterySet,
    sell_prices: HashMap<String, Option<u32>>,
    platform: String,
) {
    let client = wf_data::http_client();
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let owned = wf_cache::load_blob::<HashMap<String, u32>>(wf_relic::OWNED_RELICS_FILE);
        let active_tiers = wf_data::worldstate::fetch(&client, &platform)
            .await
            .map(|ws| ws.active_fissure_tiers())
            .unwrap_or_default();
        let fresh = Live::compute(owned.as_ref(), &index, &mastery, &sell_prices, active_tiers);
        *lock_live(&live) = fresh;
    }
}

/// Data loaded once at launch: the relic catalogue, the player's mastered set,
/// their scanned Owned relic counts (if any), which relic tiers currently have
/// an active Fissure, and — for owned relics only — each one's resolved plat
/// price (relic market slug → price, `None` where unresolved).
struct LoadedData {
    index: RelicIndex,
    mastery: MasterySet,
    owned: Option<wf_cache::Stamped<HashMap<String, u32>>>,
    active_tiers: HashSet<String>,
    sell_prices: HashMap<String, Option<u32>>,
}

/// Load the relic catalogue, the player's mastered set, their scanned Owned
/// relic counts, active Fissure tiers, and (once, up front) plat prices for
/// every owned relic — falling back to empty values on failure rather than
/// blocking the GUI from opening at all.
async fn load_data(config: &Config) -> LoadedData {
    let client = wf_data::http_client();
    let index = RelicIndex::load_cached(&client, CATALOGUE_TTL).await.unwrap_or_else(|e| {
        tracing::warn!("relic catalogue load failed: {e:#}");
        RelicIndex::new(Vec::new())
    });
    let mastery = match &config.account_id {
        Some(id) => wf_relic::mastery::load_cached(&client, id, MASTERY_TTL).await,
        None => MasterySet::default(),
    };
    let owned = wf_cache::load_blob::<HashMap<String, u32>>(wf_relic::OWNED_RELICS_FILE);
    let active_tiers = wf_data::worldstate::fetch(&client, &config.platform)
        .await
        .map(|ws| ws.active_fissure_tiers())
        .unwrap_or_default();

    let mut sell_prices = HashMap::new();
    if let Some(owned) = &owned {
        let market = wf_data::market::MarketClient::new(client, config.market_platform.clone());
        let cache = wf_relic::price_cache();
        let slugs: Vec<String> = index
            .all()
            .iter()
            .filter(|relic| owned.value.get(&relic.display).copied().unwrap_or(0) > 0)
            .map(|relic| relic.slug())
            .collect();

        let results: Vec<(String, Option<u32>)> = stream::iter(slugs)
            .map(|slug| {
                let cache = &cache;
                let market = &market;
                async move {
                    let plat = wf_relic::cached_plat(cache, market, &slug, wf_relic::PriceOpts::default())
                        .await;
                    (slug, plat)
                }
            })
            .buffer_unordered(PRICE_FETCH_CONCURRENCY)
            .collect()
            .await;
        sell_prices.extend(results);
        cache.save();
    }

    LoadedData { index, mastery, owned, active_tiers, sell_prices }
}

/// The browser's tabs: Mastery, Relics & Plan, and Sell.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Mastery,
    Relics,
    Sell,
}

struct BrowseApp {
    tab: Tab,
    mastery_rows: Vec<MasteryEntry>,
    /// Editable text buffer for the Mastery tab's search box.
    filter: String,
    /// Refreshed by the background [`poll`] task every [`POLL_INTERVAL`].
    live: Arc<Mutex<Live>>,
}

impl BrowseApp {
    fn new(mastery_rows: Vec<MasteryEntry>, live: Arc<Mutex<Live>>) -> Self {
        Self { tab: Tab::Mastery, mastery_rows, filter: String::new(), live }
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

    fn relics_tab(&mut self, ui: &mut egui::Ui) {
        // Clone the pieces this frame needs and drop the lock immediately,
        // rather than holding it across the whole render below — the only
        // other lock-holder is the background poller's brief write.
        let (plans, owned_age, active_tiers) = {
            let live = lock_live(&self.live);
            (live.plans.clone(), live.owned_age, live.active_tiers.clone())
        };
        let Some(plans) = plans else {
            ui.label(NO_OWNED_DATA_MSG);
            return;
        };

        owned_age_label(ui, owned_age);

        if plans.is_empty() {
            ui.label("no unmastered primes found among your scanned relics");
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("relics_plan_grid").num_columns(3).striped(true).show(ui, |ui| {
                ui.label("unmastered prime");
                ui.label("owned");
                ui.label("relics you own that can still drop it");
                ui.end_row();

                for p in &plans {
                    let breakdown = p
                        .relics
                        .iter()
                        .map(|r| {
                            let is_live = active_tiers.contains(wf_relic::tier_of(&r.relic_display));
                            let flag = if is_live { "*" } else { "" };
                            format!("{}{flag} x{}", r.relic_display, r.owned_count)
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    ui.label(&p.prime);
                    ui.label(p.total_owned.to_string());
                    ui.label(breakdown);
                    ui.end_row();
                }
            });
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("* = a fissure of that relic's tier is active right now")
                .small()
                .weak(),
        );
    }

    fn sell_tab(&mut self, ui: &mut egui::Ui) {
        let (picks, owned_age) = {
            let live = lock_live(&self.live);
            (live.sell_picks.clone(), live.owned_age)
        };
        let Some(picks) = picks else {
            ui.label(NO_OWNED_DATA_MSG);
            return;
        };

        owned_age_label(ui, owned_age);

        if picks.is_empty() {
            ui.label("no owned relics found");
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("sell_grid").num_columns(4).striped(true).show(ui, |ui| {
                ui.label("relic");
                ui.label("owned");
                ui.label("plat");
                ui.label("unmastered");
                ui.end_row();

                for p in &picks {
                    let plat = p.plat.map(|v| format!("{v}p")).unwrap_or_else(|| "—".into());
                    ui.label(&p.display);
                    ui.label(p.count.to_string());
                    ui.label(plat);
                    ui.label(p.unmastered.len().to_string());
                    ui.end_row();
                }
            });
        });
    }
}

/// The "owned relics scanned N ago" freshness line shared by the Relics &
/// Plan and Sell tabs.
fn owned_age_label(ui: &mut egui::Ui, age: Option<Duration>) {
    if let Some(age) = age {
        ui.label(format!("owned relics scanned {}", wf_cache::format_age(age)));
        ui.add_space(6.0);
    }
}

impl eframe::App for BrowseApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Keep repainting at the poll cadence even with no user input, so the
        // Relics & Plan/Sell tabs pick up the background poller's updates
        // without needing a mouse move to trigger a redraw.
        ui.ctx().request_repaint_after(POLL_INTERVAL);

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Mastery, "Mastery");
                ui.selectable_value(&mut self.tab, Tab::Relics, "Relics & Plan");
                ui.selectable_value(&mut self.tab, Tab::Sell, "Sell");
            });
            ui.separator();

            match self.tab {
                Tab::Mastery => self.mastery_tab(ui),
                Tab::Relics => self.relics_tab(ui),
                Tab::Sell => self.sell_tab(ui),
            }
        });
    }
}
