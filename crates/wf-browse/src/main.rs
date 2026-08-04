//! `wf-browse` — a small graphical browser for warframe-lite: which primes
//! you've mastered, which Owned relics can still get you there, which are
//! worth selling instead of cracking, and which owned relic's crack has the
//! best odds of a valuable already-mastered part to sell.
//!
//! Read-only for the *values* the OCR scan of the in-game Void Relics screen
//! produces (via `wf-lite overlay`) — the only source of Owned relic counts
//! (see ADR-0001, ADR-0003). The Owned tab's clear/reset actions are a narrow
//! exception (delete-only, user-initiated, see ADR-0010), for entries the
//! scanner itself can never observe as depleted.
//!
//! Kept in a separate binary from `wf-lite`/`wf-settings` so each companion
//! stays purpose-built and lean (see ADR-0002).
//!
//! The Relics & Plan, Sell, and Farm tabs' Owned relic counts and
//! active-Fissure flag are live: a background task ([`poll`]) re-reads
//! `owned-relics.json` and re-fetches world state every [`POLL_INTERVAL`]
//! while the window is open, so they catch up with a scan happening in
//! another window without a restart. Mastery, the relic catalogue, and
//! Sell/Farm-tab prices are loaded once at launch and never touched by that
//! timer.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use futures::stream::{self, StreamExt};
use wf_config::Config;
use wf_relic::{
    FarmPick, ItemIndex, MasteryEntry, MasterySet, PartQuantities, PrimePlan, RelicIndex, RelicPick,
};

const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const MASTERY_TTL: Duration = Duration::from_secs(24 * 3600);
/// How many price lookups run at once when pricing the Sell/Farm tabs. A
/// player can own hundreds of distinct relics, so fetching prices one at a
/// time (like the CLI's small, explicit-args case) could block for minutes
/// once the shared cache goes stale; unbounded concurrency, on the other
/// hand, would fire every lookup at warframe.market in one burst.
const PRICE_FETCH_CONCURRENCY: usize = 8;
/// How often the Relics & Plan, Sell, and Farm tabs' Owned relic counts and
/// active-Fissure flag refresh while the window stays open. Only these two (a
/// local file and a lightweight world-state fetch) are cheap enough to poll;
/// mastery, the relic catalogue, and Sell/Farm-tab prices are loaded once at
/// launch and never re-fetched on this timer.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Shown on the Relics & Plan, Sell, and Farm tabs when no relic scan has
/// happened yet.
const NO_OWNED_DATA_MSG: &str = "no owned-relic data yet. Run `wf-lite overlay` (or the tray) and \
     open the in-game Void Relics screen once — it scans automatically as you scroll.";
/// Relic tiers offered by the tier-filter checkboxes, in drop order.
const TIERS: [&str; 5] = ["Lith", "Meso", "Neo", "Axi", "Requiem"];

/// Accent color for selection/hover/active widget state — a muted teal,
/// distinct from both egui's default blue and Warframe's own UI palette.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(64, 176, 174);
/// Mastered-status color.
const MASTERED_COLOR: egui::Color32 = egui::Color32::from_rgb(120, 200, 140);
/// Unmastered-status color: muted, not alarming — there's nothing wrong with
/// an unmastered prime, it's just the thing being tracked.
const UNMASTERED_COLOR: egui::Color32 = egui::Color32::from_gray(150);
/// Warframe's own reward-rarity colors, reused so the Farm tab reads the same
/// way the in-game reward screen does.
const RARITY_COMMON_COLOR: egui::Color32 = egui::Color32::from_gray(210);
const RARITY_UNCOMMON_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 165, 230);
const RARITY_RARE_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 190, 60);

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
    let LoadedData { index, mastery, quantities, owned, active_tiers, prices } =
        rt.block_on(load_data(&config));

    let mastery_rows = wf_relic::mastery_browser(&index, &mastery);
    let live = Arc::new(Mutex::new(Live::compute(
        owned.as_ref(),
        &index,
        &mastery,
        &quantities,
        &prices,
        active_tiers,
    )));

    rt.spawn(poll(live.clone(), index, mastery, quantities, prices, config.platform));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("warframe-lite browse")
            .with_inner_size([700.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "warframe-lite browse",
        options,
        Box::new(move |cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(BrowseApp::new(mastery_rows, live)))
        }),
    )
}

/// Apply a clean, dark, teal-accented theme. Scoped to `wf-browse` only — the
/// default egui look is serviceable but visually flat, so this tightens
/// spacing, rounds panels/widgets, and gives interactive/selected elements a
/// consistent accent color instead of egui's default blue. Keeps egui's
/// default fonts.
fn apply_theme(ctx: &egui::Context) {
    // Always dark, regardless of the system preference — this is a deliberate
    // app-wide look, not a system-theme follow.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, style_theme);
}

fn style_theme(style: &mut egui::Style) {
    let visuals = &mut style.visuals;
    *visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(24, 26, 28);
    visuals.window_fill = egui::Color32::from_rgb(24, 26, 28);
    visuals.extreme_bg_color = egui::Color32::from_rgb(18, 19, 21);
    visuals.faint_bg_color = egui::Color32::from_rgb(31, 33, 36);

    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke.color = egui::Color32::from_rgb(10, 12, 12);
    visuals.hyperlink_color = ACCENT;
    visuals.widgets.hovered.bg_stroke.color = ACCENT;
    visuals.widgets.hovered.fg_stroke.color = ACCENT;
    visuals.widgets.active.bg_stroke.color = ACCENT;
    visuals.widgets.active.fg_stroke.color = ACCENT;
    visuals.widgets.open.bg_stroke.color = ACCENT;

    let widget_rounding = egui::CornerRadius::same(6);
    visuals.widgets.noninteractive.corner_radius = widget_rounding;
    visuals.widgets.inactive.corner_radius = widget_rounding;
    visuals.widgets.hovered.corner_radius = widget_rounding;
    visuals.widgets.active.corner_radius = widget_rounding;
    visuals.widgets.open.corner_radius = widget_rounding;
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.menu_corner_radius = egui::CornerRadius::same(6);

    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
}

/// The era prefix of a relic display label matches one of these checkboxes.
fn tier_filter_ui(ui: &mut egui::Ui, selected: &mut HashSet<String>) {
    ui.horizontal(|ui| {
        ui.label("Tier:");
        for tier in TIERS {
            let mut checked = selected.contains(tier);
            if ui.checkbox(&mut checked, tier).changed() {
                if checked {
                    selected.insert(tier.to_string());
                } else {
                    selected.remove(tier);
                }
            }
        }
    });
}

/// An empty filter set means "no filter" — every tier passes.
fn tier_matches(selected: &HashSet<String>, tier: &str) -> bool {
    selected.is_empty() || selected.contains(tier)
}

/// Ordinal for sorting by rarity (Rare highest), matching Warframe's own
/// Common < Uncommon < Rare ordering.
fn rarity_rank(rarity: &str) -> u8 {
    match rarity.to_ascii_lowercase().as_str() {
        "rare" => 2,
        "uncommon" => 1,
        _ => 0,
    }
}

/// Warframe's own in-game reward-rarity color for a drop-table rarity string.
fn rarity_color(rarity: &str) -> egui::Color32 {
    match rarity.to_ascii_lowercase().as_str() {
        "rare" => RARITY_RARE_COLOR,
        "uncommon" => RARITY_UNCOMMON_COLOR,
        _ => RARITY_COMMON_COLOR,
    }
}

/// Launch-time-resolved market prices for the Sell tab (relic slug → plat)
/// and the Farm tab (mastered reward name → plat). Bundled because both are
/// fetched once in [`load_data`] and travel together everywhere after —
/// [`LoadedData`], [`Live::compute`], and [`poll`] all need both at once.
struct Prices {
    sell: HashMap<String, Option<u32>>,
    farm: HashMap<String, Option<u32>>,
}

/// The Relics & Plan / Sell / Farm tabs' data — refreshed periodically by
/// [`poll`] while the window is open, independent of the launch-time
/// [`LoadedData`].
struct Live {
    /// `None` when no relics have been scanned yet.
    plans: Option<Vec<PrimePlan>>,
    /// `None` when no relics have been scanned yet.
    sell_picks: Option<Vec<RelicPick>>,
    /// `None` when no relics have been scanned yet.
    farm_picks: Option<Vec<FarmPick>>,
    /// Freshest and stalest Intact scan ages `(newest, oldest)`, for the summary
    /// line; `None` when nothing has been scanned.
    owned_age_range: Option<(Duration, Duration)>,
    /// Per-relic-code Intact scan age, for the per-relic freshness markers.
    ages: HashMap<String, Duration>,
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
    /// set, against the launch-time relic catalogue/mastery/prices (which
    /// never change after launch).
    fn compute(
        owned: Option<&wf_cache::Stamped<wf_relic::OwnedRelics>>,
        index: &RelicIndex,
        mastery: &MasterySet,
        quantities: &PartQuantities,
        prices: &Prices,
        active_tiers: HashSet<String>,
    ) -> Self {
        // The planners consume the Intact-only projection (relic drop tables are
        // Intact-only); refined copies are tracked but excluded here.
        let intact = owned.map(|o| wf_relic::intact_counts(&o.value));
        // mastery_plan/sell_picks/farm_picks already rank their output.
        let plans = intact.as_ref().map(|c| wf_relic::mastery_plan(c, index, mastery, quantities));
        let sell_picks = intact.as_ref().map(|c| wf_relic::sell_picks(c, &prices.sell, index, mastery));
        let farm_picks = intact.as_ref().map(|c| wf_relic::farm_picks(c, &prices.farm, index, mastery));
        let owned_age_range = owned.and_then(|o| wf_relic::intact_age_range(&o.value));
        let ages = owned.map(|o| wf_relic::intact_ages(&o.value)).unwrap_or_default();
        Self { plans, sell_picks, farm_picks, owned_age_range, ages, active_tiers }
    }
}

/// Re-read `owned-relics.json` and re-fetch world state every [`POLL_INTERVAL`],
/// refreshing `live` — the only two things cheap/fast-changing enough to poll.
/// The relic catalogue, mastery, and Sell/Farm-tab prices stay exactly as
/// loaded at launch; only re-running the app refreshes those.
async fn poll(
    live: Arc<Mutex<Live>>,
    index: RelicIndex,
    mastery: MasterySet,
    quantities: PartQuantities,
    prices: Prices,
    platform: String,
) {
    let client = wf_data::http_client();
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let owned = wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE);
        let active_tiers = wf_data::worldstate::fetch(&client, &platform)
            .await
            .map(|ws| ws.active_fissure_tiers())
            .unwrap_or_default();
        let fresh = Live::compute(owned.as_ref(), &index, &mastery, &quantities, &prices, active_tiers);
        *lock_live(&live) = fresh;
    }
}

/// Data loaded once at launch: the relic catalogue, the player's mastered set,
/// Prime Part build quantities, their scanned Owned relic counts (if any),
/// which relic tiers currently have an active Fissure, and — for owned relics
/// only — each relic's resolved sell price and each already-mastered reward's
/// resolved part price.
struct LoadedData {
    index: RelicIndex,
    mastery: MasterySet,
    quantities: PartQuantities,
    owned: Option<wf_cache::Stamped<wf_relic::OwnedRelics>>,
    active_tiers: HashSet<String>,
    prices: Prices,
}

/// Look up a bounded-concurrency batch of market prices, keyed by whatever
/// `key_and_slug` maps each input to. Shared by the Sell tab's relic prices
/// and the Farm tab's reward-part prices — both fetch dozens to hundreds of
/// entries against the same cache and market client.
async fn fetch_prices<T>(
    inputs: Vec<T>,
    cache: &wf_relic::PriceCache,
    market: &wf_data::market::MarketClient,
    key_and_slug: impl Fn(T) -> (String, String),
) -> HashMap<String, Option<u32>>
where
    T: Send,
{
    stream::iter(inputs)
        .map(|input| {
            let (key, slug) = key_and_slug(input);
            async move {
                let plat =
                    wf_relic::cached_plat(cache, market, &slug, wf_relic::PriceOpts::default()).await;
                (key, plat)
            }
        })
        .buffer_unordered(PRICE_FETCH_CONCURRENCY)
        .collect()
        .await
}

/// Load the relic catalogue, the player's mastered set, their scanned Owned
/// relic counts, active Fissure tiers, and (once, up front) both relic sell
/// prices and mastered-reward part prices for every owned relic — falling
/// back to empty values on failure rather than blocking the GUI from opening
/// at all.
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
    let quantities = PartQuantities::load_cached(&client, CATALOGUE_TTL).await.unwrap_or_else(|e| {
        tracing::warn!("part quantity load failed: {e:#}");
        PartQuantities::empty()
    });
    let owned = wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE);
    let active_tiers = wf_data::worldstate::fetch(&client, &config.platform)
        .await
        .map(|ws| ws.active_fissure_tiers())
        .unwrap_or_default();

    let mut sell_prices = HashMap::new();
    let mut farm_prices = HashMap::new();
    if let Some(owned) = &owned {
        // Prices are only needed for relics with an Intact count (what the tabs
        // rank); refined-only copies don't drive the guide.
        let intact = wf_relic::intact_counts(&owned.value);
        let market = wf_data::market::MarketClient::new(client.clone(), config.market_platform.clone());
        let cache = wf_relic::price_cache();

        let relic_slugs: Vec<String> = index
            .all()
            .iter()
            .filter(|relic| intact.get(&relic.display).copied().unwrap_or(0) > 0)
            .map(|relic| relic.slug())
            .collect();
        sell_prices = fetch_prices(relic_slugs, &cache, &market, |slug| (slug.clone(), slug)).await;

        let item_index = ItemIndex::load_cached(&client, CATALOGUE_TTL).await.unwrap_or_else(|e| {
            tracing::warn!("item catalogue load failed: {e:#}");
            ItemIndex::new(Vec::new())
        });
        let reward_names = wf_relic::farm_reward_names(&intact, &index, &mastery);
        let resolved: Vec<(String, String)> = reward_names
            .into_iter()
            .filter_map(|name| item_index.best_match(&name).map(|m| (name, m.item.slug.clone())))
            .collect();
        farm_prices = fetch_prices(resolved, &cache, &market, |(name, slug)| (name, slug)).await;

        cache.save();
    }

    LoadedData {
        index,
        mastery,
        quantities,
        owned,
        active_tiers,
        prices: Prices { sell: sell_prices, farm: farm_prices },
    }
}

/// The browser's tabs: Mastery, Relics & Plan, Sell, Farm, and Owned.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Mastery,
    Relics,
    Sell,
    Farm,
    Owned,
}

#[derive(Clone, Copy, PartialEq)]
enum MasteryFilter {
    All,
    MasteredOnly,
    UnmasteredOnly,
}

#[derive(Clone, Copy, PartialEq)]
enum MasterySort {
    Alphabetical,
    UnmasteredFirst,
}

#[derive(Clone, Copy, PartialEq)]
enum RelicsSort {
    MostOwned,
    ActionableNow,
    Alphabetical,
}

#[derive(Clone, Copy, PartialEq)]
enum SellFilter {
    All,
    PureSell,
    StillHasValue,
}

#[derive(Clone, Copy, PartialEq)]
enum SellSort {
    Price,
    Owned,
    Unmastered,
    Alphabetical,
}

#[derive(Clone, Copy, PartialEq)]
enum FarmSort {
    Price,
    Owned,
    Alphabetical,
    Rarity,
}

struct BrowseApp {
    tab: Tab,
    mastery_rows: Vec<MasteryEntry>,
    /// Editable text buffer for the Mastery tab's search box.
    filter: String,
    mastery_filter: MasteryFilter,
    mastery_sort: MasterySort,
    relics_sort: RelicsSort,
    sell_tier_filter: HashSet<String>,
    sell_status_filter: SellFilter,
    sell_sort: SellSort,
    farm_tier_filter: HashSet<String>,
    farm_sort: FarmSort,
    /// Editable text buffer for the Owned tab's search box.
    owned_filter: String,
    /// Must be checked before "reset all" is clickable — a guard against an
    /// accidental click on a destructive, ADR-0010 action.
    reset_confirm: bool,
    /// Refreshed by the background [`poll`] task every [`POLL_INTERVAL`].
    live: Arc<Mutex<Live>>,
}

impl BrowseApp {
    fn new(mastery_rows: Vec<MasteryEntry>, live: Arc<Mutex<Live>>) -> Self {
        Self {
            tab: Tab::Mastery,
            mastery_rows,
            filter: String::new(),
            mastery_filter: MasteryFilter::All,
            mastery_sort: MasterySort::Alphabetical,
            relics_sort: RelicsSort::MostOwned,
            sell_tier_filter: HashSet::new(),
            sell_status_filter: SellFilter::All,
            sell_sort: SellSort::Price,
            farm_tier_filter: HashSet::new(),
            farm_sort: FarmSort::Price,
            owned_filter: String::new(),
            reset_confirm: false,
            live,
        }
    }

    fn mastery_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.filter);
        });
        ui.horizontal(|ui| {
            ui.label("Show:");
            ui.selectable_value(&mut self.mastery_filter, MasteryFilter::All, "All");
            ui.selectable_value(&mut self.mastery_filter, MasteryFilter::MasteredOnly, "Mastered");
            ui.selectable_value(&mut self.mastery_filter, MasteryFilter::UnmasteredOnly, "Unmastered");
            ui.add_space(12.0);
            ui.label("Sort:");
            ui.selectable_value(&mut self.mastery_sort, MasterySort::Alphabetical, "Alphabetical");
            ui.selectable_value(&mut self.mastery_sort, MasterySort::UnmasteredFirst, "Unmastered first");
        });
        ui.add_space(6.0);

        let filter = self.filter.to_ascii_lowercase();
        let mut rows: Vec<&MasteryEntry> = self
            .mastery_rows
            .iter()
            .filter(|e| filter.is_empty() || e.prime.to_ascii_lowercase().contains(&filter))
            .filter(|e| match self.mastery_filter {
                MasteryFilter::All => true,
                MasteryFilter::MasteredOnly => e.mastered,
                MasteryFilter::UnmasteredOnly => !e.mastered,
            })
            .collect();
        if self.mastery_sort == MasterySort::UnmasteredFirst {
            rows.sort_by_key(|e| e.mastered);
        } // Alphabetical: mastery_rows is already alphabetical.

        let mastered = rows.iter().filter(|e| e.mastered).count();
        ui.label(format!("{mastered} / {} mastered", rows.len()));
        ui.add_space(4.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("mastery_grid").num_columns(2).striped(true).show(ui, |ui| {
                ui.strong("prime");
                ui.strong("status");
                ui.end_row();

                for entry in &rows {
                    ui.label(&entry.prime);
                    let (text, color) = if entry.mastered {
                        ("✓ mastered", MASTERED_COLOR)
                    } else {
                        ("— unmastered", UNMASTERED_COLOR)
                    };
                    ui.colored_label(color, text);
                    ui.end_row();
                }
            });
        });
    }

    fn relics_tab(&mut self, ui: &mut egui::Ui) {
        // Clone the pieces this frame needs and drop the lock immediately,
        // rather than holding it across the whole render below — the only
        // other lock-holder is the background poller's brief write.
        let (plans, owned_age_range, ages, active_tiers) = {
            let live = lock_live(&self.live);
            (live.plans.clone(), live.owned_age_range, live.ages.clone(), live.active_tiers.clone())
        };
        let Some(mut plans) = plans else {
            ui.label(NO_OWNED_DATA_MSG);
            return;
        };

        ui.horizontal(|ui| {
            ui.label("Sort:");
            ui.selectable_value(&mut self.relics_sort, RelicsSort::MostOwned, "Most owned");
            ui.selectable_value(&mut self.relics_sort, RelicsSort::ActionableNow, "Actionable now");
            ui.selectable_value(&mut self.relics_sort, RelicsSort::Alphabetical, "Alphabetical");
        });
        ui.add_space(4.0);

        owned_age_label(ui, owned_age_range);

        if plans.is_empty() {
            ui.label("no unmastered primes found among your scanned relics");
            return;
        }

        match self.relics_sort {
            RelicsSort::MostOwned => {} // mastery_plan already sorts this way.
            RelicsSort::ActionableNow => plans.sort_by(|a, b| {
                plan_is_live(b, &active_tiers)
                    .cmp(&plan_is_live(a, &active_tiers))
                    .then(b.total_owned.cmp(&a.total_owned))
                    .then_with(|| a.prime.cmp(&b.prime))
            }),
            RelicsSort::Alphabetical => plans.sort_by(|a, b| a.prime.cmp(&b.prime)),
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            for p in &plans {
                ui.horizontal(|ui| {
                    ui.strong(&p.prime);
                    ui.weak(format!("owned {}", p.total_owned));
                });
                egui::Grid::new(format!("relics_plan_grid_{}", p.prime))
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("part");
                        ui.strong("need");
                        ui.strong("relics you own that can still drop it");
                        ui.end_row();

                        for g in &p.parts {
                            let need = g.build_quantity.map(|q| format!("x{q}")).unwrap_or_default();
                            let breakdown = g
                                .relics
                                .iter()
                                .map(|r| {
                                    let is_live =
                                        active_tiers.contains(wf_relic::tier_of(&r.relic_display));
                                    let flag = if is_live { "*" } else { "" };
                                    let stale = stale_marker(&ages, &r.relic_display);
                                    format!("{}{flag} x{}{stale}", r.relic_display, r.owned_count)
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            ui.label(&g.part.part);
                            ui.label(need);
                            ui.label(breakdown);
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
            }
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "* = a fissure of that relic's tier is active right now   ⚠ = count not \
                 re-confirmed recently, may be stale",
            )
            .small()
            .weak(),
        );
    }

    fn sell_tab(&mut self, ui: &mut egui::Ui) {
        let (picks, owned_age_range, ages) = {
            let live = lock_live(&self.live);
            (live.sell_picks.clone(), live.owned_age_range, live.ages.clone())
        };
        let Some(mut picks) = picks else {
            ui.label(NO_OWNED_DATA_MSG);
            return;
        };

        tier_filter_ui(ui, &mut self.sell_tier_filter);
        ui.horizontal(|ui| {
            ui.label("Show:");
            ui.selectable_value(&mut self.sell_status_filter, SellFilter::All, "All");
            ui.selectable_value(&mut self.sell_status_filter, SellFilter::PureSell, "Pure sell");
            ui.selectable_value(&mut self.sell_status_filter, SellFilter::StillHasValue, "Still has value");
            ui.add_space(12.0);
            ui.label("Sort:");
            ui.selectable_value(&mut self.sell_sort, SellSort::Price, "Price");
            ui.selectable_value(&mut self.sell_sort, SellSort::Owned, "Owned");
            ui.selectable_value(&mut self.sell_sort, SellSort::Unmastered, "Unmastered");
            ui.selectable_value(&mut self.sell_sort, SellSort::Alphabetical, "Alphabetical");
        });
        ui.add_space(4.0);

        owned_age_label(ui, owned_age_range);

        if picks.is_empty() {
            ui.label("no owned relics found");
            return;
        }

        picks.retain(|p| tier_matches(&self.sell_tier_filter, wf_relic::tier_of(&p.display)));
        picks.retain(|p| match self.sell_status_filter {
            SellFilter::All => true,
            SellFilter::PureSell => p.unmastered.is_empty(),
            SellFilter::StillHasValue => !p.unmastered.is_empty(),
        });

        if show_if_filtered_empty(ui, &picks) {
            return;
        }

        match self.sell_sort {
            SellSort::Price => {} // sell_picks already sorts this way.
            SellSort::Owned => picks.sort_by_key(|p| std::cmp::Reverse(p.count)),
            SellSort::Unmastered => picks.sort_by_key(|p| std::cmp::Reverse(p.unmastered.len())),
            SellSort::Alphabetical => picks.sort_by(|a, b| a.display.cmp(&b.display)),
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("sell_grid").num_columns(5).striped(true).show(ui, |ui| {
                ui.strong("relic");
                ui.strong("owned");
                ui.strong("plat");
                ui.strong("unmastered");
                ui.strong("scanned");
                ui.end_row();

                for p in &picks {
                    let plat = p.plat.map(|v| format!("{v}p")).unwrap_or_else(|| "—".into());
                    ui.label(&p.display);
                    ui.label(p.count.to_string());
                    ui.label(plat);
                    ui.label(p.unmastered.len().to_string());
                    ui.label(age_cell(&ages, &p.display));
                    ui.end_row();
                }
            });
        });
    }

    fn farm_tab(&mut self, ui: &mut egui::Ui) {
        let (picks, owned_age_range, ages) = {
            let live = lock_live(&self.live);
            (live.farm_picks.clone(), live.owned_age_range, live.ages.clone())
        };
        let Some(mut picks) = picks else {
            ui.label(NO_OWNED_DATA_MSG);
            return;
        };

        tier_filter_ui(ui, &mut self.farm_tier_filter);
        ui.horizontal(|ui| {
            ui.label("Sort:");
            ui.selectable_value(&mut self.farm_sort, FarmSort::Price, "Price");
            ui.selectable_value(&mut self.farm_sort, FarmSort::Owned, "Owned");
            ui.selectable_value(&mut self.farm_sort, FarmSort::Alphabetical, "Alphabetical");
            ui.selectable_value(&mut self.farm_sort, FarmSort::Rarity, "Rarity");
        });
        ui.add_space(4.0);

        owned_age_label(ui, owned_age_range);

        if picks.is_empty() {
            ui.label("no owned relics have an already-mastered prime reward to farm");
            return;
        }

        picks.retain(|p| tier_matches(&self.farm_tier_filter, wf_relic::tier_of(&p.display)));

        if show_if_filtered_empty(ui, &picks) {
            return;
        }

        match self.farm_sort {
            FarmSort::Price => {} // farm_picks already sorts this way.
            FarmSort::Owned => picks.sort_by_key(|p| std::cmp::Reverse(p.count)),
            FarmSort::Alphabetical => picks.sort_by(|a, b| a.display.cmp(&b.display)),
            FarmSort::Rarity => picks.sort_by(|a, b| {
                rarity_rank(&b.rarity).cmp(&rarity_rank(&a.rarity)).then(b.plat.unwrap_or(0).cmp(&a.plat.unwrap_or(0)))
            }),
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("farm_grid").num_columns(6).striped(true).show(ui, |ui| {
                ui.strong("relic");
                ui.strong("owned");
                ui.strong("best mastered reward");
                ui.strong("plat");
                ui.strong("rarity");
                ui.strong("scanned");
                ui.end_row();

                for p in &picks {
                    let plat = p.plat.map(|v| format!("{v}p")).unwrap_or_else(|| "—".into());
                    ui.label(&p.display);
                    ui.label(p.count.to_string());
                    ui.label(&p.best_reward);
                    ui.label(plat);
                    ui.colored_label(rarity_color(&p.rarity), &p.rarity);
                    ui.label(age_cell(&ages, &p.display));
                    ui.end_row();
                }
            });
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "crack the relic and sell only this part — or run a 4-player radiant share on it \
                 to maximize the odds of rolling it",
            )
            .small()
            .weak(),
        );
    }

    /// Raw owned-relic inventory, across every refinement (unlike the
    /// Intact-only Relics/Sell/Farm tabs) — where a specific `(code,
    /// refinement)` entry can be cleared, or the whole set reset. See
    /// ADR-0010: a narrow, user-initiated exception to ADR-0003, needed
    /// because a depleted refined relic's card disappears from the in-game
    /// screen entirely (no eye icon), so no future scan can ever clear it.
    /// Reads `owned-relics.json` fresh every frame rather than through
    /// [`Live`] — cheap (a local file), and lets a clear/reset take effect
    /// immediately instead of waiting for [`POLL_INTERVAL`].
    fn owned_tab(&mut self, ui: &mut egui::Ui) {
        let Some(owned) = wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE)
            .map(|s| s.value)
        else {
            ui.label(NO_OWNED_DATA_MSG);
            return;
        };

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.reset_confirm, "I understand this deletes all scanned relic data");
            if ui
                .add_enabled(self.reset_confirm, egui::Button::new("Reset all owned-relic data"))
                .clicked()
            {
                reset_owned_relics();
                self.reset_confirm = false;
            }
        });
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.owned_filter);
        });
        ui.add_space(4.0);

        let filter = self.owned_filter.to_ascii_lowercase();
        let mut rows: Vec<(String, wf_relic::Refinement, wf_relic::OwnedEntry)> = owned
            .into_iter()
            .flat_map(|(code, by_ref)| by_ref.into_iter().map(move |(r, e)| (code.clone(), r, e)))
            .filter(|(code, _, _)| filter.is_empty() || code.to_ascii_lowercase().contains(&filter))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0).then(refinement_rank(a.1).cmp(&refinement_rank(b.1))));

        ui.label(format!("{} entries", rows.len()));
        ui.add_space(4.0);

        let mut to_clear: Option<(String, wf_relic::Refinement)> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("owned_grid").num_columns(5).striped(true).show(ui, |ui| {
                ui.strong("relic");
                ui.strong("refinement");
                ui.strong("status");
                ui.strong("age");
                ui.strong("");
                ui.end_row();

                for (code, refinement, entry) in &rows {
                    ui.label(code);
                    ui.label(format!("{refinement:?}"));
                    let status = match &entry.count {
                        Some(c) => format!("x{}", c.value),
                        None => "seen".to_string(),
                    };
                    ui.label(status);
                    let age = entry
                        .count
                        .as_ref()
                        .map(|c| wf_cache::format_age(c.age()))
                        .unwrap_or_else(|| "—".to_string());
                    ui.label(age);
                    if ui.button("Clear").clicked() {
                        to_clear = Some((code.clone(), *refinement));
                    }
                    ui.end_row();
                }
            });
        });

        if let Some((code, refinement)) = to_clear {
            clear_owned_entry(&code, refinement);
        }
    }
}

/// Order refinements display in: Intact first, Radiant last.
fn refinement_rank(r: wf_relic::Refinement) -> u8 {
    match r {
        wf_relic::Refinement::Intact => 0,
        wf_relic::Refinement::Exceptional => 1,
        wf_relic::Refinement::Flawless => 2,
        wf_relic::Refinement::Radiant => 3,
    }
}

/// Remove one `(code, refinement)` entry from `owned-relics.json` at the
/// player's request (see ADR-0010). Best-effort: a failed load/save just
/// leaves the entry in place for another try.
fn clear_owned_entry(code: &str, refinement: wf_relic::Refinement) {
    let Some(mut owned) =
        wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE).map(|s| s.value)
    else {
        return;
    };
    wf_relic::clear_entry(&mut owned, code, refinement);
    let _ = wf_cache::save_blob(wf_relic::OWNED_RELICS_FILE, &owned);
}

/// Back up and clear all owned-relic data — the player-initiated equivalent
/// of the scanner's own "back up and start clean" treatment for an
/// incompatible file (ADR-0005), see ADR-0010. `owned-relics.json`'s absence
/// is already `wf-browse`'s existing "no data yet" state, so a bare rename is
/// enough — no need to write a fresh empty file.
fn reset_owned_relics() {
    let Ok(dir) = wf_cache::cache_dir() else {
        return;
    };
    let path = dir.join(wf_relic::OWNED_RELICS_FILE);
    if path.exists() {
        let bak = path.with_extension("json.bak");
        if std::fs::rename(&path, &bak).is_ok() {
            tracing::info!("backed up {} to {}", path.display(), bak.display());
        }
    }
}

/// Whether any relic sourcing `plan` currently has an active Fissure.
fn plan_is_live(plan: &PrimePlan, active_tiers: &HashSet<String>) -> bool {
    plan.parts
        .iter()
        .flat_map(|g| &g.relics)
        .any(|r| active_tiers.contains(wf_relic::tier_of(&r.relic_display)))
}

/// The owned-relic freshness line shared by the Relics & Plan, Sell, and Farm
/// tabs. Counts are stamped per relic (see ADR-0005), so this summarises the
/// span: a single age when everything was seen together, else "newest – oldest".
fn owned_age_label(ui: &mut egui::Ui, range: Option<(Duration, Duration)>) {
    if let Some((newest, oldest)) = range {
        let text = if newest == oldest {
            format!("owned relics scanned {}", wf_cache::format_age(oldest))
        } else {
            format!(
                "owned relics scanned {} – {}",
                wf_cache::format_age(newest),
                wf_cache::format_age(oldest)
            )
        };
        ui.label(text);
        ui.add_space(6.0);
    }
}

/// Colour for stale (past [`wf_relic::STALE_AFTER`]) freshness text.
const STALE_COLOR: egui::Color32 = egui::Color32::from_rgb(0xD8, 0x8A, 0x00);

/// A per-relic "last scanned" cell: dimmed when fresh, amber when stale, "—"
/// when the relic has no Intact scan on record.
fn age_cell(ages: &HashMap<String, Duration>, display: &str) -> egui::RichText {
    match ages.get(display) {
        Some(age) => {
            let text = egui::RichText::new(wf_cache::format_age(*age)).small();
            if *age >= wf_relic::STALE_AFTER {
                text.color(STALE_COLOR)
            } else {
                text.weak()
            }
        }
        None => egui::RichText::new("—").small().weak(),
    }
}

/// A compact stale marker (`" ⚠3d ago"`) appended to a relic token in the plan
/// breakdown; empty when the relic is fresh or unscanned.
fn stale_marker(ages: &HashMap<String, Duration>, display: &str) -> String {
    match ages.get(display) {
        Some(age) if *age >= wf_relic::STALE_AFTER => format!(" ⚠{}", wf_cache::format_age(*age)),
        _ => String::new(),
    }
}

/// Shown by the Sell/Farm tabs when the tier/status filters leave nothing to
/// render (as opposed to there being no owned relics at all). Returns whether
/// it showed the message, so the caller can `return` in one line.
fn show_if_filtered_empty<T>(ui: &mut egui::Ui, picks: &[T]) -> bool {
    let empty = picks.is_empty();
    if empty {
        ui.label("no relics match the current filter");
    }
    empty
}

impl eframe::App for BrowseApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Keep repainting at the poll cadence even with no user input, so the
        // Relics & Plan/Sell/Farm tabs pick up the background poller's
        // updates without needing a mouse move to trigger a redraw.
        ui.ctx().request_repaint_after(POLL_INTERVAL);

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Mastery, "Mastery");
                ui.selectable_value(&mut self.tab, Tab::Relics, "Relics & Plan");
                ui.selectable_value(&mut self.tab, Tab::Sell, "Sell");
                ui.selectable_value(&mut self.tab, Tab::Farm, "Farm");
                ui.selectable_value(&mut self.tab, Tab::Owned, "Owned");
            });
            ui.separator();

            match self.tab {
                Tab::Mastery => self.mastery_tab(ui),
                Tab::Relics => self.relics_tab(ui),
                Tab::Sell => self.sell_tab(ui),
                Tab::Farm => self.farm_tab(ui),
                Tab::Owned => self.owned_tab(ui),
            }
        });
    }
}
