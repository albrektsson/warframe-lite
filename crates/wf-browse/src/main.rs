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
//! The relic catalogue, mastery, and Sell/Farm-tab prices are loaded once, but
//! never block the window from opening: [`main`] shows it immediately and
//! runs [`load_data`] on a background task, so every tab that depends on it
//! (Mastery, Relics & Plan, Buy or Farm, Sell, Farm) renders a "Loading…"
//! placeholder until it lands.
//!
//! The Relics & Plan, Sell, and Farm tabs' Owned relic counts and
//! active-Fissure flag stay live after that: the same background task
//! ([`poll`]) re-reads `owned-relics.json` and re-fetches world state every
//! [`POLL_INTERVAL`] while the window is open, so they catch up with a scan
//! happening in another window without a restart.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use futures::stream::{self, StreamExt};
use wf_config::Config;
use wf_relic::{
    DucatPick, EquipmentCategory, EvRefinement, FarmPick, ItemIndex, MasteryEntry, MasterySet,
    PartMarketInfo, PartQuantities, PrimePart, PrimePlan, RelicIndex, RelicInfo, RelicPick,
    CATEGORY_ORDER,
};

const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const MASTERY_TTL: Duration = Duration::from_secs(24 * 3600);
/// How many price lookups run at once when pricing the Sell/Farm tabs. A
/// player can own hundreds of distinct relics, so fetching prices one at a
/// time (like the CLI's small, explicit-args case) could block for minutes
/// once the shared cache goes stale; unbounded concurrency, on the other
/// hand, would fire every lookup at warframe.market in one burst.
const PRICE_FETCH_CONCURRENCY: usize = 8;
/// How long a lazily-fetched price that resolved to "no listing found" (see
/// [`LazyPrice`]) waits before [`BrowseApp::ensure_lazy_prices`] retries it.
/// Long enough that `relics_tab`'s every-frame, unvirtualized re-render (see
/// its docs) doesn't refire a fetch dozens of times a second; short enough
/// that a fetch that failed on its first attempt (ADR-0012's original bug —
/// a slug with nothing cached yet whose fetch times out gets stuck on `None`
/// forever) clears up within the same session instead of needing a relaunch.
const LAZY_PRICE_RETRY_COOLDOWN: Duration = Duration::from_secs(45);
/// How often the Relics & Plan, Sell, and Farm tabs' Owned relic counts and
/// active-Fissure flag refresh while the window stays open. Only these two (a
/// local file and a lightweight world-state fetch) are cheap enough to poll;
/// mastery, the relic catalogue, and Sell/Farm-tab prices are loaded once at
/// launch and never re-fetched on this timer.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Repaint cadence while [`BrowseApp::loaded`] is still `None`, so the
/// "Loading…" placeholder resolves promptly instead of waiting out a full
/// [`POLL_INTERVAL`] for the next scheduled repaint.
const LOADING_REPAINT: Duration = Duration::from_millis(200);
/// Shown on the Relics & Plan, Sell, and Farm tabs when no relic scan has
/// happened yet.
const NO_OWNED_DATA_MSG: &str = "no owned-relic data yet. Run `wf-lite overlay` (or the tray) and \
     open the in-game Void Relics screen once — it scans automatically as you scroll.";
/// Shown on the Ducats tab when no Prime Part has been scanned yet.
const NO_OWNED_PARTS_MSG: &str = "no owned Prime Part data yet. Run `wf-lite overlay` (or the tray) \
     and open the in-game Inventory/Sell screen once — it scans automatically as you scroll.";
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

    // `rt` is never dropped before `run_native` returns, so the background
    // loader/poller spawned on it below — and any on-demand price fetch the
    // Relics EV tab spawns via `rt_handle` — keep running for as long as the
    // window stays open.
    let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
    let rt_handle = rt.handle().clone();
    let loaded: Arc<Mutex<Option<Loaded>>> = Arc::new(Mutex::new(None));
    // Hand-curated, so loaded once synchronously here (a local disk read, no
    // network) rather than through the background `load_data`/`poll` path —
    // this window is the only writer, so there's nothing else to catch up
    // with (see ADR-0004).
    let wishlist = wf_cache::load_blob::<wf_relic::Wishlist>(wf_relic::WISHLIST_FILE)
        .map(|s| s.value)
        .unwrap_or_default();
    // For the Relics EV tab's on-demand, per-relic price fetch — a separate
    // client/platform from `load_data`'s own (which stays scoped to its
    // background task) rather than threading one shared instance across both.
    let client = wf_data::http_client();
    let market_platform = config.market_platform.clone();
    // The Relics & Plan tab's lazy, auto-retrying relic-level sell prices and
    // Set prices (see ADR-0012) — empty until a tab that needs them
    // (Relics & Plan/Sell for relic prices, Relics & Plan/Buy or Farm for Set
    // prices) is first viewed. Shared between `BrowseApp` (which triggers and
    // reads fetches from the UI thread) and `load_and_poll`/`poll` (which
    // fold a snapshot into `Live::compute` every tick).
    let relic_prices: LazyPriceMap = Arc::new(Mutex::new(HashMap::new()));
    let set_prices: LazyPriceMap = Arc::new(Mutex::new(HashMap::new()));
    // `load_data` (catalogue/mastery/quantity fetches, plus a price lookup per
    // owned relic) runs in the background rather than blocking the window
    // from opening at all — see the module docs and `BrowseApp`'s "Loading…"
    // placeholder.
    rt.spawn(load_and_poll(loaded.clone(), config, relic_prices.clone(), set_prices.clone()));

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
            Ok(Box::new(BrowseApp::new(
                loaded,
                wishlist,
                rt_handle,
                client,
                market_platform,
                relic_prices,
                set_prices,
            )))
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

/// Insert or remove a wishlist `key` (see [`wf_relic::wishlist::key`]) from
/// `set` per `wishlisted` — the pure mutation [`BrowseApp::set_wishlisted`]'s
/// checkbox handler applies before persisting.
fn toggle_membership(set: &mut HashSet<String>, key: &str, wishlisted: bool) {
    if wishlisted {
        set.insert(key.to_string());
    } else {
        set.remove(key);
    }
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

/// A small filled circle in a drop's rarity color — drawn rather than a
/// bundled bitmap, since neither WFCD dataset this app already draws from
/// (`warframe-drop-data`, `warframe-items`) ships a rarity icon asset to
/// bundle; this reproduces the same color the in-game reward screen uses.
fn rarity_pip(ui: &mut egui::Ui, rarity: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(rect.center(), 4.0, rarity_color(rarity));
    }
}

/// Launch-time-resolved market prices for the Sell tab (relic slug → plat),
/// the Farm tab (mastered reward name → plat), each unmastered built Prime's
/// Set (built prime name → plat), and the Ducats tab (owned Prime Part's
/// [`wf_relic::reward_label`] → plat). Bundled because all four are fetched
/// once in [`load_data`] and travel together everywhere after —
/// [`LoadedData`], [`Live::compute`], and [`poll`] all need them at once.
struct Prices {
    sell: HashMap<String, Option<u32>>,
    farm: HashMap<String, Option<u32>>,
    set: HashMap<String, Option<u32>>,
    ducats: HashMap<String, Option<u32>>,
}

/// The Relics EV tab's lazy, on-expand pricing state for one relic — `None`
/// (absent from the map) means never triggered; `Loading` means a fetch is in
/// flight; `Ready` carries each reward's resolved plat price (reward item
/// name → `Option<u32>`, `None` = checked, no market listing).
#[derive(Clone)]
enum RelicPriceState {
    Loading,
    Ready(HashMap<String, Option<u32>>),
}

/// One item's lazily-fetched, auto-retrying market price — the Relics & Plan
/// tab's relic-level sell prices and Set prices both use this shape (see
/// ADR-0012). Unlike [`RelicPriceState`] (fetched once, on row expand, never
/// retried), `Ready`'s `plat: None` case records when it resolved so
/// [`BrowseApp::ensure_lazy_prices`] knows when [`LAZY_PRICE_RETRY_COOLDOWN`]
/// has elapsed and it's time to try again — for as long as a tab that needs
/// it stays open.
#[derive(Clone, Copy)]
enum LazyPrice {
    Loading,
    Ready { plat: Option<u32>, resolved_at: Instant },
}

/// Shared between the UI thread — which triggers fetches from
/// `relics_tab`/`sell_tab`/`buy_or_farm_tab` and reads them back every frame
/// — and the background `load_and_poll`/`poll` task, which folds a snapshot
/// (see [`snapshot_prices`]) into each [`Live::compute`] tick. Two instances
/// exist: one for relic-level sell prices (keyed by market slug, shared by
/// the Relics & Plan and Sell tabs), one for Set prices (keyed by built-prime
/// name, shared by the Relics & Plan and Buy or Farm tabs) — both tabs in
/// each pair read the same underlying map, since whichever loads first
/// legitimately triggers the fetch the other needs too.
type LazyPriceMap = Arc<Mutex<HashMap<String, LazyPrice>>>;

/// Whether a `LazyPriceMap` entry (`None` if the key is absent) should have a
/// fetch (re)triggered right now — the decision [`BrowseApp::ensure_lazy_prices`]
/// applies per key: fetch when never attempted, or when the last attempt
/// resolved to "no listing" and [`LAZY_PRICE_RETRY_COOLDOWN`] has elapsed
/// since; skip while a fetch is already in flight, or a resolved price is
/// known, or the cooldown hasn't elapsed yet.
fn needs_fetch(current: Option<&LazyPrice>, now: Instant) -> bool {
    match current {
        None => true,
        Some(LazyPrice::Ready { plat: None, resolved_at }) => {
            now.duration_since(*resolved_at) >= LAZY_PRICE_RETRY_COOLDOWN
        }
        _ => false,
    }
}

/// Collapse a [`LazyPriceMap`] into the plain `key → resolved plat` map the
/// pure `wf_relic` planning functions (`mastery_plan`, `sell_picks`,
/// `buy_or_farm_plan`) already expect — a `Loading` entry is treated the same
/// as "not yet fetched" (absent), matching how every other price map in this
/// app represents "unresolved." Used to fold each `poll` tick's map state
/// into the pure planning layer; UI rendering reads the map directly instead
/// (see [`lazy_price_str`]) so a just-landed price shows immediately rather
/// than waiting for the next tick's snapshot.
fn snapshot_prices(map: &LazyPriceMap) -> HashMap<String, Option<u32>> {
    map.lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter_map(|(k, v)| match v {
            LazyPrice::Ready { plat, .. } => Some((k.clone(), *plat)),
            LazyPrice::Loading => None,
        })
        .collect()
}

/// The Relics EV tab's per-row read-only lookups, bundled (mirroring
/// [`wf_relic::RelicContext`]) so [`BrowseApp::relic_ev_row`] doesn't carry
/// them as separate parameters.
#[derive(Clone, Copy)]
struct RelicEvContext<'a> {
    item_index: &'a Arc<ItemIndex>,
    quantities: &'a PartQuantities,
    owned_parts: &'a wf_relic::OwnedPrimeParts,
}

/// The Relics & Plan tab's per-frame snapshot from [`Loaded`], bundled
/// (mirroring [`RelicEvContext`]) so its `loaded_or_placeholder` call doesn't
/// grow into an unreadable tuple now that this tab's lazy pricing (see
/// ADR-0012) pulls in more fields than the original
/// `plans`/`owned_age_range`/`ages`/`active_tiers`.
struct RelicsTabData {
    plans: Option<Vec<PrimePlan>>,
    owned_age_range: Option<(Duration, Duration)>,
    ages: HashMap<String, Duration>,
    active_tiers: HashSet<String>,
    priceable_relic_slugs: Vec<String>,
    unmastered_primes: Vec<String>,
    index: Arc<RelicIndex>,
    item_index: Arc<ItemIndex>,
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
    /// The Buy-or-Farm tab's full-BOM view — always populated (driven by
    /// `PartQuantities`/mastery, not owned-relic evidence), unlike the other
    /// three fields above.
    bom_plans: Vec<wf_relic::BomPlan>,
    /// Freshest and stalest Intact scan ages `(newest, oldest)`, for the summary
    /// line; `None` when nothing has been scanned.
    owned_age_range: Option<(Duration, Duration)>,
    /// Per-relic-code Intact scan age, for the per-relic freshness markers.
    ages: HashMap<String, Duration>,
    active_tiers: HashSet<String>,
    /// The scanned owned-Prime-Part set, for the Mastery tab's part-level
    /// owned/need cell — refreshed on the same poll cadence as every other
    /// scan-derived field here, unlike the launch-time-only [`Loaded::quantities`].
    owned_parts: wf_relic::OwnedPrimeParts,
    /// The Ducats tab's ducat-efficiency ranking of every owned Prime Part —
    /// recomputed each poll tick against the same launch-time `Prices::ducats`
    /// (a newly-scanned part's ducat value shows immediately since
    /// `part_market` is catalogue-wide, but its plat price waits for the next
    /// launch, same as every other owned-driven price in this app).
    ducat_picks: Vec<wf_relic::DucatPick>,
    /// Market slugs of every relic with Confirmed *or* Seen evidence (see
    /// [`wf_relic::owned_evidence`]) — the lazy relic-price fetch's target
    /// set for the Relics & Plan and Sell tabs (see ADR-0012). Unlike
    /// `sell_picks`'/`mastery_plan`'s own filtering, this is not
    /// Confirmed-only: a Seen-only relic is displayed in the Relics & Plan
    /// tab (see [`wf_relic::RelicEvidence::SeenOnly`] at its render site) and
    /// so needs a price fetched for it too, even though it never earns a
    /// sellable count from `owned_counts()`.
    priceable_relic_slugs: Vec<String>,
    /// Every unmastered Prime — the lazy Set-price fetch's target set for the
    /// Relics & Plan and Buy or Farm tabs (see ADR-0012). Ownership-
    /// independent, like [`wf_relic::buy_or_farm_plan`]'s own use of the same
    /// set: it's driven by `PartQuantities`/mastery, not owned-relic
    /// evidence.
    unmastered_primes: Vec<String>,
}

/// Launch-time [`load_data`] plus the first [`Live::compute`] derived from
/// it, gated behind `None` until the background load ([`load_and_poll`])
/// finishes — every tab that needs either renders a "Loading…" placeholder
/// until then instead of `main` blocking the window from opening on it.
struct Loaded {
    mastery_rows: Vec<MasteryEntry>,
    live: Live,
    /// Shared with the background poller ([`poll`]) rather than cloned — build
    /// quantities never change after launch, and the Mastery tab's wishlist
    /// checkboxes need every part label per prime ([`PartQuantities::parts_for`]).
    quantities: Arc<PartQuantities>,
    /// Vaulted status + ducat value per Prime Part, for the Mastery tab's
    /// tree — resolved once at launch (see [`wf_relic::part_market_info`]),
    /// shared with the poller for the same reason as `quantities`.
    part_market: Arc<HashMap<PrimePart, PartMarketInfo>>,
    /// The whole relic catalogue, for the Relics EV tab's era → code tree
    /// (unlike every other tab, which only ever shows *owned* relics). Shared
    /// with the poller, which also holds it for `Live::compute`.
    index: Arc<RelicIndex>,
    /// The item catalogue, for the Relics EV tab's on-demand ducat-value and
    /// market-slug lookups (see `BrowseApp::spawn_relic_price_fetch`).
    item_index: Arc<ItemIndex>,
}

/// Lock `m`, recovering the guard even if a previous holder panicked while
/// holding it (rather than poisoning every future frame's render) — neither
/// `Live`'s derivation nor the initial load should panic, but a
/// stale-but-working UI beats a permanent crash-loop if either ever does.
fn lock_loaded(m: &Mutex<Option<Loaded>>) -> std::sync::MutexGuard<'_, Option<Loaded>> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The catalogue-side inputs [`Live::compute`] derives every tab from that
/// stay fixed after launch (see [`Loaded`]'s docs) — bundled so `compute`
/// takes one reference instead of growing an argument per catalogue lookup
/// (clippy's `too_many_arguments`).
#[derive(Clone, Copy)]
struct StaticData<'a> {
    index: &'a RelicIndex,
    mastery: &'a MasterySet,
    quantities: &'a PartQuantities,
    part_market: &'a HashMap<PrimePart, PartMarketInfo>,
}

impl Live {
    /// Derive the live view from a fresh Owned relic read + active-Fissure
    /// set, against the launch-time relic catalogue/mastery/prices (which
    /// never change after launch).
    fn compute(
        owned: Option<&wf_cache::Stamped<wf_relic::OwnedRelics>>,
        owned_parts: &wf_relic::OwnedPrimeParts,
        static_data: &StaticData,
        prices: &Prices,
        active_tiers: HashSet<String>,
    ) -> Self {
        let StaticData { index, mastery, quantities, part_market } = *static_data;
        // sell_picks/farm_picks only rank relics with a confirmed count;
        // mastery_plan additionally surfaces seen-but-unconfirmed relics via
        // the richer evidence map (see wf_relic::owned_evidence).
        let counts = owned.map(|o| wf_relic::owned_counts(&o.value));
        let evidence = owned.map(|o| wf_relic::owned_evidence(&o.value));
        let ctx = wf_relic::RelicContext { index, mastery, quantities, owned_parts };
        // mastery_plan/sell_picks/farm_picks already rank their output.
        let plans = evidence
            .as_ref()
            .map(|e| wf_relic::mastery_plan(e, &prices.sell, &prices.set, &ctx));
        let sell_picks = counts.as_ref().map(|c| wf_relic::sell_picks(c, &prices.sell, &ctx));
        let farm_picks = counts.as_ref().map(|c| wf_relic::farm_picks(c, &prices.farm, index, mastery));
        let bom_plans = wf_relic::buy_or_farm_plan(
            &prices.sell,
            &prices.set,
            index,
            mastery,
            quantities,
            owned_parts,
        );
        let owned_age_range = owned.and_then(|o| wf_relic::intact_age_range(&o.value));
        let ages = owned.map(|o| wf_relic::intact_ages(&o.value)).unwrap_or_default();
        let ducat_picks =
            wf_relic::ducat_picks(owned_parts, part_market, &prices.ducats, quantities, mastery);
        let priceable_relic_slugs: Vec<String> = evidence
            .as_ref()
            .map(|e| index.all().iter().filter(|r| e.contains_key(&r.display)).map(|r| r.slug()).collect())
            .unwrap_or_default();
        let unmastered_primes = wf_relic::unmastered_primes(quantities, mastery);
        Self {
            plans,
            sell_picks,
            farm_picks,
            bom_plans,
            owned_age_range,
            ages,
            active_tiers,
            owned_parts: owned_parts.clone(),
            ducat_picks,
            priceable_relic_slugs,
            unmastered_primes,
        }
    }
}

/// Run [`load_data`] in the background and populate `loaded` the moment it's
/// ready — unblocking the Mastery/Relics & Plan/Buy or Farm/Sell/Farm tabs'
/// "Loading…" placeholder — then hand off into [`poll`]'s ongoing refresh
/// loop for as long as the window stays open.
async fn load_and_poll(
    loaded: Arc<Mutex<Option<Loaded>>>,
    config: Config,
    relic_prices: LazyPriceMap,
    set_prices: LazyPriceMap,
) {
    let LoadedData {
        index,
        mastery,
        quantities,
        owned,
        owned_parts,
        active_tiers,
        mut prices,
        part_market,
        item_index,
    } = load_data(&config).await;
    let quantities = Arc::new(quantities);
    let part_market = Arc::new(part_market);
    let index = Arc::new(index);
    let item_index = Arc::new(item_index);
    let mastery_rows = wf_relic::mastery_browser(&index, &mastery);
    let static_data =
        StaticData { index: &index, mastery: &mastery, quantities: &quantities, part_market: &part_market };
    // Empty at launch (see `load_data`'s docs) — populated once the Relics &
    // Plan/Sell/Buy or Farm tabs' lazy fetch (see ADR-0012) has landed
    // anything, which `poll`'s own snapshot below picks up on its next tick.
    prices.sell = snapshot_prices(&relic_prices);
    prices.set = snapshot_prices(&set_prices);
    let live = Live::compute(owned.as_ref(), &owned_parts, &static_data, &prices, active_tiers);
    *lock_loaded(&loaded) = Some(Loaded {
        mastery_rows,
        live,
        quantities: quantities.clone(),
        part_market: part_market.clone(),
        index: index.clone(),
        item_index: item_index.clone(),
    });

    poll(PollArgs {
        loaded,
        index,
        mastery,
        quantities,
        part_market,
        prices,
        relic_prices,
        set_prices,
        platform: config.platform,
    })
    .await;
}

/// [`poll`]'s inputs, bundled to keep the function under clippy's
/// `too_many_arguments` threshold — each field is exactly one of
/// [`load_and_poll`]'s own locals, handed off unchanged for the rest of the
/// window's lifetime (only `prices.sell`/`.set` and `loaded` itself actually
/// change after that, via the lazy price maps and each poll tick's write).
struct PollArgs {
    loaded: Arc<Mutex<Option<Loaded>>>,
    index: Arc<RelicIndex>,
    mastery: MasterySet,
    quantities: Arc<PartQuantities>,
    part_market: Arc<HashMap<PrimePart, PartMarketInfo>>,
    prices: Prices,
    relic_prices: LazyPriceMap,
    set_prices: LazyPriceMap,
    platform: String,
}

/// Re-read `owned-relics.json` and re-fetch world state every [`POLL_INTERVAL`],
/// refreshing `loaded`'s `live` field — the only two things cheap/fast-changing
/// enough to poll, plus a fresh snapshot of `relic_prices`/`set_prices` (see
/// ADR-0012) so a lazy fetch that lands between ticks shows up without
/// waiting on user interaction. The relic catalogue, mastery, and Farm/Ducats-
/// tab prices stay exactly as loaded at launch; only re-running the app
/// refreshes those.
async fn poll(args: PollArgs) {
    let PollArgs { loaded, index, mastery, quantities, part_market, mut prices, relic_prices, set_prices, platform } =
        args;
    let client = wf_data::http_client();
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let owned = wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE);
        let owned_parts =
            wf_cache::load_blob::<wf_relic::OwnedPrimeParts>(wf_relic::OWNED_PRIME_PARTS_FILE)
                .map(|s| s.value)
                .unwrap_or_default();
        let active_tiers = wf_data::worldstate::fetch(&client, &platform)
            .await
            .map(|ws| ws.active_fissure_tiers())
            .unwrap_or_default();
        let static_data = StaticData {
            index: &index,
            mastery: &mastery,
            quantities: &quantities,
            part_market: &part_market,
        };
        prices.sell = snapshot_prices(&relic_prices);
        prices.set = snapshot_prices(&set_prices);
        let fresh = Live::compute(owned.as_ref(), &owned_parts, &static_data, &prices, active_tiers);
        if let Some(l) = lock_loaded(&loaded).as_mut() {
            l.live = fresh;
        }
    }
}

/// Data loaded once at launch: the relic catalogue, the player's mastered set,
/// Prime Part build quantities, their scanned Owned relic counts (if any),
/// their scanned owned-Prime-Part counts (if any), which relic tiers
/// currently have an active Fissure, and — for owned relics only — each
/// already-mastered reward's resolved part price. Relic-level sell prices and
/// Set prices are *not* included (`prices.sell`/`.set` start empty); those
/// are fetched lazily per [`LazyPriceMap`] instead (see ADR-0012).
struct LoadedData {
    index: RelicIndex,
    mastery: MasterySet,
    quantities: PartQuantities,
    owned: Option<wf_cache::Stamped<wf_relic::OwnedRelics>>,
    owned_parts: wf_relic::OwnedPrimeParts,
    active_tiers: HashSet<String>,
    prices: Prices,
    /// Vaulted status + ducat value per Prime Part, resolved once against the
    /// item catalogue for the Mastery tab's tree — see
    /// [`wf_relic::part_market_info`].
    part_market: HashMap<PrimePart, PartMarketInfo>,
    /// The item catalogue, kept (rather than dropped after `load_data`'s own
    /// pricing lookups) for the Relics EV tab's on-demand ducat/market-slug
    /// lookups.
    item_index: ItemIndex,
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
/// relic counts, active Fissure tiers, and (once, up front) mastered-reward
/// part prices for every owned relic (Farm tab) and every owned Prime Part
/// (Ducats tab) — falling back to empty values on failure rather than
/// blocking the GUI from opening at all. Relic-level sell prices and Set
/// prices are fetched lazily instead (see ADR-0012), so `prices.sell`/`.set`
/// on the returned [`LoadedData`] always start empty.
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
    let owned_parts =
        wf_cache::load_blob::<wf_relic::OwnedPrimeParts>(wf_relic::OWNED_PRIME_PARTS_FILE)
            .map(|s| s.value)
            .unwrap_or_default();
    let active_tiers = wf_data::worldstate::fetch(&client, &config.platform)
        .await
        .map(|ws| ws.active_fissure_tiers())
        .unwrap_or_default();

    let market = wf_data::market::MarketClient::new(client.clone(), config.market_platform.clone());
    let cache = wf_relic::price_cache();
    let item_index = ItemIndex::load_cached(&client, CATALOGUE_TTL).await.unwrap_or_else(|e| {
        tracing::warn!("item catalogue load failed: {e:#}");
        ItemIndex::new(Vec::new())
    });

    // Relic-level sell prices (Relics & Plan/Sell tabs) and Set prices
    // (Relics & Plan/Buy or Farm tabs) are no longer fetched here: both are
    // now fetched lazily, on first view of a tab that needs them, with
    // automatic per-item retry (see ADR-0012 and
    // `BrowseApp::ensure_lazy_prices`) rather than once, eagerly, with no
    // retry if a lookup times out.
    let mut farm_prices = HashMap::new();
    if let Some(owned) = &owned {
        // Prices are only needed for relics with a confirmed count (what the
        // Farm tab ranks); seen-only copies don't drive it.
        let counts = wf_relic::owned_counts(&owned.value);
        let reward_names = wf_relic::farm_reward_names(&counts, &index, &mastery);
        let resolved: Vec<(String, String)> = reward_names
            .into_iter()
            .filter_map(|name| item_index.best_match(&name).map(|m| (name, m.item.slug.clone())))
            .collect();
        farm_prices = fetch_prices(resolved, &cache, &market, |(name, slug)| (name, slug)).await;
    }

    // Ducats-tab pricing: every owned Prime Part's plat price, resolved once
    // at launch like sell/farm/set pricing above. A newly-scanned part shows
    // its ducat value immediately (`part_market_info` below is catalogue-wide,
    // not owned-driven) but waits for its plat price until the next launch,
    // same as every other owned-driven price in this app.
    let owned_part_names: Vec<String> = owned_parts
        .iter()
        .flat_map(|(prime, parts)| {
            parts.keys().map(move |part| {
                wf_relic::reward_label(&PrimePart { prime: prime.clone(), part: part.clone() })
            })
        })
        .collect();
    let resolved_owned_parts: Vec<(String, String)> = owned_part_names
        .into_iter()
        .filter_map(|name| item_index.best_match(&name).map(|m| (name, m.item.slug.clone())))
        .collect();
    let ducat_prices =
        fetch_prices(resolved_owned_parts, &cache, &market, |(name, slug)| (name, slug)).await;

    cache.save();

    let part_market = wf_relic::part_market_info(&quantities, &item_index);

    LoadedData {
        index,
        mastery,
        quantities,
        owned,
        owned_parts,
        active_tiers,
        prices: Prices {
            sell: HashMap::new(),
            farm: farm_prices,
            set: HashMap::new(),
            ducats: ducat_prices,
        },
        part_market,
        item_index,
    }
}

/// The browser's tabs: Mastery, Relics & Plan, Relics EV, Buy or Farm, Sell,
/// Farm, Ducats, and Owned.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Mastery,
    Relics,
    RelicsEv,
    BuyOrFarm,
    Sell,
    Farm,
    Ducats,
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
    /// Editable text buffer for the Mastery tab's search box.
    filter: String,
    mastery_filter: MasteryFilter,
    mastery_sort: MasterySort,
    relics_sort: RelicsSort,
    /// Editable text buffer for the Relics & Plan tab's search box.
    relics_filter: String,
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
    /// Editable text buffer for the Relics EV tab's search box.
    relic_ev_filter: String,
    /// The Ducats tab's default filter: only show parts whose Built prime is
    /// already mastered (pure ducat-fodder) — toggled off to reveal every
    /// owned part.
    ducats_mastered_only: bool,
    /// `None` until the background [`load_and_poll`] task finishes; its
    /// `live` field is refreshed in place afterward by [`poll`] every
    /// [`POLL_INTERVAL`].
    loaded: Arc<Mutex<Option<Loaded>>>,
    /// The hand-curated equipment wishlist, loaded once at launch and
    /// written back to `wishlist.json` on every mark/unmark (ADR-0004). No
    /// polling: this window is the only writer.
    wishlist: wf_relic::Wishlist,
    /// Per-relic reward pricing for the Relics EV tab's lazy, on-expand fetch
    /// (see [`RelicPriceState`]) — written to by tasks spawned on
    /// [`Self::rt_handle`], read every frame to render EV/plat once ready.
    /// Unrelated to the Relics & Plan tab's own lazy pricing below despite
    /// the similar name — this one fetches a whole relic's reward map at
    /// once, keyed by relic display, and is never retried.
    relic_ev_prices: Arc<Mutex<HashMap<String, RelicPriceState>>>,
    /// The Relics & Plan tab's lazy, auto-retrying relic-level sell prices
    /// (keyed by market slug) and Set prices (keyed by built-prime name) —
    /// see [`LazyPriceMap`] and ADR-0012. Shared with `load_and_poll`/`poll`
    /// (via `main`), which fold a snapshot into every `Live::compute` tick.
    relics_plan_relic_prices: LazyPriceMap,
    relics_plan_set_prices: LazyPriceMap,
    /// Handle onto `main`'s tokio runtime, so the Relics EV tab can spawn an
    /// on-demand price fetch from inside `eframe::App::ui` (which runs
    /// synchronously on the UI thread, unlike [`load_and_poll`]/[`poll`]).
    rt_handle: tokio::runtime::Handle,
    /// Shared HTTP client for on-demand price fetches — separate from
    /// `load_data`'s own (which stays scoped to its background task).
    client: reqwest::Client,
    market_platform: String,
}

impl BrowseApp {
    fn new(
        loaded: Arc<Mutex<Option<Loaded>>>,
        wishlist: wf_relic::Wishlist,
        rt_handle: tokio::runtime::Handle,
        client: reqwest::Client,
        market_platform: String,
        relics_plan_relic_prices: LazyPriceMap,
        relics_plan_set_prices: LazyPriceMap,
    ) -> Self {
        Self {
            tab: Tab::Mastery,
            filter: String::new(),
            mastery_filter: MasteryFilter::All,
            mastery_sort: MasterySort::Alphabetical,
            relics_sort: RelicsSort::MostOwned,
            relics_filter: String::new(),
            sell_tier_filter: HashSet::new(),
            sell_status_filter: SellFilter::All,
            sell_sort: SellSort::Price,
            farm_tier_filter: HashSet::new(),
            farm_sort: FarmSort::Price,
            owned_filter: String::new(),
            reset_confirm: false,
            relic_ev_filter: String::new(),
            ducats_mastered_only: true,
            loaded,
            wishlist,
            relic_ev_prices: Arc::new(Mutex::new(HashMap::new())),
            relics_plan_relic_prices,
            relics_plan_set_prices,
            rt_handle,
            client,
            market_platform,
        }
    }

    /// Mark/unmark `key` and persist the wishlist immediately — best-effort,
    /// matching every other `wf_cache::save_blob` call site in this app.
    fn set_wishlisted(&mut self, key: &str, wishlisted: bool) {
        toggle_membership(&mut self.wishlist, key, wishlisted);
        let _ = wf_cache::save_blob(wf_relic::WISHLIST_FILE, &self.wishlist);
    }

    /// Derive `f` from the background-loaded state, or show the "Loading…"
    /// placeholder and return `None` if [`load_and_poll`] hasn't finished yet
    /// — the shared gate every tab that depends on [`Loaded`] opens with.
    fn loaded_or_placeholder<T>(&self, ui: &mut egui::Ui, f: impl FnOnce(&Loaded) -> T) -> Option<T> {
        let result = lock_loaded(&self.loaded).as_ref().map(f);
        if result.is_none() {
            ui.label("Loading…");
        }
        result
    }

    /// Fire the Relics EV tab's lazy, on-expand price fetch for `relic`:
    /// marks it `Loading` immediately (so a re-render this same frame or the
    /// next doesn't spawn a second fetch), then resolves every reward's
    /// market slug via `item_index` and prices it, same bounded-concurrency
    /// `fetch_prices` path the Sell/Farm tabs use at launch — just triggered
    /// on demand here instead. A reward with no catalogue match keeps its
    /// entry (price `None`), so [`wf_relic::expected_value`] sees every
    /// reward accounted for once the fetch completes.
    fn spawn_relic_price_fetch(&self, relic: RelicInfo, item_index: Arc<ItemIndex>) {
        let relic_prices = self.relic_ev_prices.clone();
        relic_prices
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(relic.display.clone(), RelicPriceState::Loading);

        let client = self.client.clone();
        let market_platform = self.market_platform.clone();
        self.rt_handle.spawn(async move {
            let market = wf_data::market::MarketClient::new(client, market_platform);
            let cache = wf_relic::price_cache();
            let resolved: Vec<(String, String)> = relic
                .rewards
                .iter()
                .filter_map(|r| {
                    item_index.best_match(&r.item_name).map(|m| (r.item_name.clone(), m.item.slug.clone()))
                })
                .collect();
            let mut prices = fetch_prices(resolved, &cache, &market, |(name, slug)| (name, slug)).await;
            for r in &relic.rewards {
                prices.entry(r.item_name.clone()).or_insert(None);
            }
            cache.save();
            relic_prices
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(relic.display, RelicPriceState::Ready(prices));
        });
    }

    /// Ensure every `(key, slug)` pair in `needed` has a price fetch in
    /// flight, freshly resolved, or correctly waiting out
    /// [`LAZY_PRICE_RETRY_COOLDOWN`] in `map` — the Relics & Plan/Sell/Buy or
    /// Farm tabs call this every frame they render (see ADR-0012), so a price
    /// still missing after its first attempt is retried automatically for as
    /// long as one of those tabs stays open. Cheap to call unconditionally:
    /// each key is marked `Loading` synchronously, before its fetch is
    /// spawned, so a key already in flight (or not yet due for retry) is
    /// skipped without touching the network — the guard against `relics_tab`
    /// refiring a fetch every frame despite having no expand/collapse to gate
    /// on, unlike the Relics EV tab's [`Self::spawn_relic_price_fetch`].
    fn ensure_lazy_prices(&self, needed: Vec<(String, String)>, map: &LazyPriceMap) {
        let now = Instant::now();
        let mut to_fetch: Vec<(String, String)> = Vec::new();
        {
            let mut guard = map.lock().unwrap_or_else(|p| p.into_inner());
            for (key, slug) in needed {
                if needs_fetch(guard.get(&key), now) {
                    guard.insert(key.clone(), LazyPrice::Loading);
                    to_fetch.push((key, slug));
                }
            }
        }
        if to_fetch.is_empty() {
            return;
        }

        let map = map.clone();
        let client = self.client.clone();
        let market_platform = self.market_platform.clone();
        self.rt_handle.spawn(async move {
            let market = wf_data::market::MarketClient::new(client, market_platform);
            let cache = wf_relic::price_cache();
            let resolved = fetch_prices(to_fetch, &cache, &market, |(key, slug)| (key, slug)).await;
            cache.save();
            let resolved_at = Instant::now();
            let mut guard = map.lock().unwrap_or_else(|p| p.into_inner());
            for (key, plat) in resolved {
                guard.insert(key, LazyPrice::Ready { plat, resolved_at });
            }
        });
    }

    /// The Mastery tab: a 3-level tree — category (fixed WFinfo order) →
    /// Prime → part — matching WFinfo's Equipment window. Both levels are
    /// collapsed by default ([`egui::CollapsingHeader`], no expand/collapse
    /// interaction existed anywhere in `wf-browse` before this); the
    /// Show/Sort controls apply *within* each category, not across them, so
    /// the category order itself never moves.
    fn mastery_tab(&mut self, ui: &mut egui::Ui) {
        let Some((mastery_rows, quantities, part_market, owned_parts)) =
            self.loaded_or_placeholder(ui, |l| {
                (l.mastery_rows.clone(), l.quantities.clone(), l.part_market.clone(), l.live.owned_parts.clone())
            })
        else {
            return;
        };

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

        // Search reaches part names too (not just Prime names) — a match
        // anywhere in a Prime's part list keeps that Prime (and its parent
        // category) in the tree, auto-expanded below so the match is never
        // hidden inside a collapsed branch.
        let filter = self.filter.to_ascii_lowercase();
        let matches = |e: &MasteryEntry| -> bool {
            filter.is_empty()
                || e.prime.to_ascii_lowercase().contains(&filter)
                || quantities
                    .parts_for(&e.prime)
                    .iter()
                    .any(|(part, _)| part.to_ascii_lowercase().contains(&filter))
        };

        let rows: Vec<&MasteryEntry> = mastery_rows
            .iter()
            .filter(|e| matches(e))
            .filter(|e| match self.mastery_filter {
                MasteryFilter::All => true,
                MasteryFilter::MasteredOnly => e.mastered,
                MasteryFilter::UnmasteredOnly => !e.mastered,
            })
            .collect();

        let mastered = rows.iter().filter(|e| e.mastered).count();
        ui.label(format!("{mastered} / {} mastered", rows.len()));
        ui.add_space(4.0);

        let mut by_category: HashMap<EquipmentCategory, Vec<&MasteryEntry>> = HashMap::new();
        for entry in rows {
            by_category.entry(quantities.category_for(&entry.prime)).or_default().push(entry);
        }
        // Non-matching branches never render at all, same as the flat list's
        // filtering did before — an empty category (every Prime filtered out)
        // is simply skipped rather than shown collapsed-and-empty.
        let force_open = (!filter.is_empty()).then_some(true);

        egui::ScrollArea::vertical().show(ui, |ui| {
            for category in CATEGORY_ORDER {
                let Some(mut entries) = by_category.remove(&category) else { continue };
                if self.mastery_sort == MasterySort::UnmasteredFirst {
                    entries.sort_by_key(|e| e.mastered);
                } // Alphabetical: mastery_rows is already alphabetical.

                let category_mastered = entries.iter().filter(|e| e.mastered).count();
                egui::CollapsingHeader::new(format!(
                    "{}  ({category_mastered}/{})",
                    category.label(),
                    entries.len()
                ))
                .id_salt(("mastery_category", category.label()))
                .default_open(false)
                .open(force_open)
                .show(ui, |ui| {
                    for entry in entries {
                        self.mastery_prime_row(ui, entry, &quantities, &part_market, &owned_parts, force_open);
                    }
                });
            }
        });
    }

    /// One Prime's row in the Mastery tab's tree: a collapsed-by-default
    /// header carrying today's mastered/unmastered dim-or-checkmark
    /// treatment, expanding to a part-level table with the owned/need cell
    /// (reused from the Relics & Plan tab), a vaulted badge and ducat value
    /// (both already fetched via the item catalogue, never shown in
    /// `wf-browse` before this), and the existing per-part wishlist checkbox.
    fn mastery_prime_row(
        &mut self,
        ui: &mut egui::Ui,
        entry: &MasteryEntry,
        quantities: &PartQuantities,
        part_market: &HashMap<PrimePart, PartMarketInfo>,
        owned_parts: &wf_relic::OwnedPrimeParts,
        force_open: Option<bool>,
    ) {
        let (status_text, status_color) = if entry.mastered {
            ("✓ mastered", MASTERED_COLOR)
        } else {
            ("— unmastered", UNMASTERED_COLOR)
        };
        let header = egui::RichText::new(format!("{}  {status_text}", entry.prime)).color(status_color);

        let mut parts = quantities.parts_for(&entry.prime);
        parts.sort();

        egui::CollapsingHeader::new(header)
            .id_salt(("mastery_prime", &entry.prime))
            .default_open(false)
            .open(force_open)
            .show(ui, |ui| {
                egui::Grid::new(format!("mastery_parts_grid_{}", entry.prime))
                    .num_columns(5)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("part");
                        ui.strong("owned / need");
                        ui.strong("vaulted");
                        ui.strong("ducats");
                        ui.strong("wishlist");
                        ui.end_row();

                        for (part, quantity) in &parts {
                            let pp = PrimePart { prime: entry.prime.clone(), part: part.clone() };
                            ui.label(part);

                            let owned = wf_relic::owned_parts::get(owned_parts, &pp);
                            ui.label(owned_need_cell(owned, Some(*quantity)));

                            let info = part_market.get(&pp);
                            if info.is_some_and(|i| i.vaulted) {
                                ui.colored_label(STALE_COLOR, "vaulted");
                            } else {
                                ui.label("");
                            }
                            ui.label(
                                info.and_then(|i| i.ducats)
                                    .map(|d| format!("{d}d"))
                                    .unwrap_or_else(|| "—".to_string()),
                            );

                            let key = wf_relic::wishlist::key(&pp);
                            let mut checked = self.wishlist.contains(&key);
                            if ui.checkbox(&mut checked, "").changed() {
                                self.set_wishlisted(&key, checked);
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    fn relics_tab(&mut self, ui: &mut egui::Ui) {
        // Clone the pieces this frame needs and drop the lock immediately,
        // rather than holding it across the whole render below — the only
        // other lock-holder is the background poller's brief write.
        let Some(RelicsTabData {
            plans,
            owned_age_range,
            ages,
            active_tiers,
            priceable_relic_slugs,
            unmastered_primes,
            index,
            item_index,
        }) = self.loaded_or_placeholder(ui, |l| RelicsTabData {
            plans: l.live.plans.clone(),
            owned_age_range: l.live.owned_age_range,
            ages: l.live.ages.clone(),
            active_tiers: l.live.active_tiers.clone(),
            priceable_relic_slugs: l.live.priceable_relic_slugs.clone(),
            unmastered_primes: l.live.unmastered_primes.clone(),
            index: l.index.clone(),
            item_index: l.item_index.clone(),
        })
        else {
            return;
        };
        // Fire (or retry) this tab's lazy Set/relic price fetches every frame
        // it renders (see ADR-0012) — cheap: `ensure_lazy_prices` only spawns
        // a fetch for a key that's not already `Loading` or still on cooldown.
        self.ensure_lazy_prices(
            priceable_relic_slugs.iter().map(|s| (s.clone(), s.clone())).collect(),
            &self.relics_plan_relic_prices,
        );
        self.ensure_lazy_prices(
            set_price_targets(&unmastered_primes, &item_index),
            &self.relics_plan_set_prices,
        );
        let Some(mut plans) = plans else {
            ui.label(NO_OWNED_DATA_MSG);
            return;
        };

        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.relics_filter);
        });
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

        let filter = self.relics_filter.to_ascii_lowercase();
        plans.retain(|p| {
            filter.is_empty()
                || p.prime.to_ascii_lowercase().contains(&filter)
                || p.parts.iter().any(|g| {
                    g.part.part.to_ascii_lowercase().contains(&filter)
                        || g.relics.iter().any(|r| r.relic_display.to_ascii_lowercase().contains(&filter))
                })
        });
        if show_if_filtered_empty(ui, &plans) {
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
                    ui.weak(format!(
                        "Set: {}",
                        lazy_price_str(&self.relics_plan_set_prices, &p.prime)
                    ));
                });
                egui::Grid::new(format!("relics_plan_grid_{}", p.prime))
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("part");
                        ui.strong("owned / need");
                        ui.strong("relics you own that can still drop it");
                        ui.end_row();

                        for g in &p.parts {
                            let need = owned_need_cell(g.owned, g.build_quantity);
                            ui.label(&g.part.part);
                            ui.label(need);
                            ui.horizontal_wrapped(|ui| {
                                for (i, r) in g.relics.iter().enumerate() {
                                    if i > 0 {
                                        ui.label(",");
                                    }
                                    rarity_pip(ui, &r.rarity);
                                    let is_live =
                                        active_tiers.contains(wf_relic::tier_of(&r.relic_display));
                                    let flag = if is_live { "*" } else { "" };
                                    let stale = stale_marker(&ages, &r.relic_display);
                                    let price = relic_slug(&index, &r.relic_display)
                                        .map(|slug| {
                                            format!(
                                                " ({})",
                                                lazy_price_str(&self.relics_plan_relic_prices, &slug)
                                            )
                                        })
                                        .unwrap_or_default();
                                    let qty = match r.evidence {
                                        wf_relic::RelicEvidence::Confirmed(n) => format!("x{n}"),
                                        wf_relic::RelicEvidence::SeenOnly => "seen".to_string(),
                                    };
                                    ui.label(format!("{}{flag} {qty}{price}{stale}", r.relic_display));
                                }
                            });
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

    /// The Relics EV tab: a 2-level tree — era (fixed order) → relic code,
    /// both collapsed by default — over the *whole* relic catalogue (unlike
    /// Relics & Plan, which only shows owned relics). Pricing is lazy: no
    /// fetch happens until a relic code's row is actually expanded, at which
    /// point [`Self::relic_ev_row`] fires [`Self::spawn_relic_price_fetch`]
    /// for just that relic's rewards.
    fn relics_ev_tab(&mut self, ui: &mut egui::Ui) {
        let Some((index, item_index, quantities, owned_parts)) = self.loaded_or_placeholder(ui, |l| {
            (l.index.clone(), l.item_index.clone(), l.quantities.clone(), l.live.owned_parts.clone())
        }) else {
            return;
        };
        let ctx = RelicEvContext { item_index: &item_index, quantities: &quantities, owned_parts: &owned_parts };

        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.relic_ev_filter);
        });
        ui.add_space(6.0);

        // Read fresh each frame, like the Owned tab — cheap (a local file)
        // and this tab has no poll-driven `Live` counterpart of its own.
        let owned_counts = wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE)
            .map(|s| wf_relic::owned_counts(&s.value))
            .unwrap_or_default();

        // Search reaches reward names too, not just the relic code.
        let filter = self.relic_ev_filter.to_ascii_lowercase();
        let matches = |r: &RelicInfo| -> bool {
            filter.is_empty()
                || r.display.to_ascii_lowercase().contains(&filter)
                || r.rewards.iter().any(|rw| rw.item_name.to_ascii_lowercase().contains(&filter))
        };
        let mut by_tier: HashMap<&str, Vec<&RelicInfo>> = HashMap::new();
        for relic in index.all().iter().filter(|r| matches(r)) {
            by_tier.entry(relic.tier.as_str()).or_default().push(relic);
        }
        let force_open = (!filter.is_empty()).then_some(true);

        egui::ScrollArea::vertical().show(ui, |ui| {
            for tier in TIERS {
                let Some(mut relics) = by_tier.remove(tier) else { continue };
                relics.sort_by(|a, b| a.code.cmp(&b.code));
                egui::CollapsingHeader::new(format!("{tier}  ({})", relics.len()))
                    .id_salt(("relic_ev_tier", tier))
                    .default_open(false)
                    .open(force_open)
                    .show(ui, |ui| {
                        for relic in relics {
                            let owned = owned_counts.get(&relic.display).copied().unwrap_or(0);
                            self.relic_ev_row(ui, relic, &ctx, owned, force_open);
                        }
                    });
            }
        });
    }

    /// One relic's row in the Relics EV tab's tree: a collapsed-by-default
    /// header carrying the owned-count badge (if any) and, once priced, the
    /// Intact/Radiant EV; expanding it fires the lazy price fetch (once —
    /// guarded by `pricing.is_none()`, since a spawned fetch immediately
    /// marks itself `Loading`) and shows a per-reward table meanwhile.
    fn relic_ev_row(
        &self,
        ui: &mut egui::Ui,
        relic: &RelicInfo,
        ctx: &RelicEvContext<'_>,
        owned: u32,
        force_open: Option<bool>,
    ) {
        let RelicEvContext { item_index, quantities, owned_parts } = *ctx;
        let pricing =
            self.relic_ev_prices.lock().unwrap_or_else(|p| p.into_inner()).get(&relic.display).cloned();

        let mut header = relic.display.clone();
        if owned > 0 {
            header.push_str(&format!("  (owned {owned})"));
        }
        match &pricing {
            Some(RelicPriceState::Ready(prices)) => {
                let intact = wf_relic::expected_value(&relic.rewards, prices, EvRefinement::Intact);
                let radiant = wf_relic::expected_value(&relic.rewards, prices, EvRefinement::Radiant);
                if let (Some(intact), Some(radiant)) = (intact, radiant) {
                    header.push_str(&format!(
                        "  INT: {intact:.0}p  RAD: {radiant:.0}p (+{:.0})",
                        radiant - intact
                    ));
                }
            }
            Some(RelicPriceState::Loading) => header.push_str("  pricing…"),
            None => {}
        }

        let response = egui::CollapsingHeader::new(header)
            .id_salt(("relic_ev_code", &relic.display))
            .default_open(false)
            .open(force_open)
            .show(ui, |ui| {
                egui::Grid::new(format!("relic_ev_grid_{}", relic.display))
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("reward");
                        ui.strong("ducats");
                        ui.strong("plat");
                        ui.strong("owned / need");
                        ui.end_row();

                        for reward in &relic.rewards {
                            ui.horizontal(|ui| {
                                rarity_pip(ui, &reward.rarity);
                                ui.label(&reward.item_name);
                            });

                            let ducats = item_index
                                .best_match(&reward.item_name)
                                .and_then(|m| m.item.ducats)
                                .map(|d| format!("{d}d"))
                                .unwrap_or_else(|| "—".to_string());
                            ui.label(ducats);

                            let plat = match &pricing {
                                Some(RelicPriceState::Ready(prices)) => {
                                    prices.get(&reward.item_name).copied().flatten()
                                }
                                _ => None,
                            };
                            ui.label(plat_str(plat));

                            // Highlight a reward whose specific Prime Part
                            // still falls short of its build quantity — the
                            // same owned/need vocabulary the Equipment tree
                            // and Relics & Plan tab already speak.
                            let pp = wf_relic::mastery::prime_part(&reward.item_name);
                            let part_owned = wf_relic::owned_parts::get(owned_parts, &pp);
                            let need = quantities.get(&pp);
                            let cell = owned_need_cell(part_owned, need);
                            if need.is_some_and(|n| part_owned.unwrap_or(0) < n) {
                                ui.colored_label(STALE_COLOR, cell);
                            } else {
                                ui.weak(cell);
                            }
                            ui.end_row();
                        }
                    });
            });

        if response.body_returned.is_some() && pricing.is_none() {
            self.spawn_relic_price_fetch(relic.clone(), item_index.clone());
        }
    }

    /// The full-BOM "Buy or Farm" tab: every part of every unmastered Prime,
    /// split into what's still missing (with its cheapest sourcing relic and
    /// price) and what's already covered, plus each Prime's Set price and
    /// total gap-fill cost — so buying the Set can be weighed against farming
    /// the missing pieces.
    fn buy_or_farm_tab(&mut self, ui: &mut egui::Ui) {
        let Some((plans, ages, unmastered_primes, item_index)) = self.loaded_or_placeholder(ui, |l| {
            (l.live.bom_plans.clone(), l.live.ages.clone(), l.live.unmastered_primes.clone(), l.item_index.clone())
        }) else {
            return;
        };
        // Reads the same Set-price `LazyPriceMap` the Relics & Plan tab does
        // (see ADR-0012) — whichever of the two the player opens first
        // legitimately triggers the fetch the other one needs too.
        self.ensure_lazy_prices(
            set_price_targets(&unmastered_primes, &item_index),
            &self.relics_plan_set_prices,
        );

        if plans.is_empty() {
            ui.label("no unmastered primes found");
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            for p in &plans {
                let total = p.covered + p.gaps.len();
                ui.horizontal(|ui| {
                    ui.strong(&p.prime);
                    ui.weak(format!("{}/{total} parts covered", p.covered));
                    ui.weak(format!(
                        "Set: {}",
                        lazy_price_str(&self.relics_plan_set_prices, &p.prime)
                    ));
                    ui.weak(format!("cost to fill: {}", plat_str(p.cost_to_fill)));
                });
                if p.gaps.is_empty() {
                    ui.weak("all parts covered");
                } else {
                    egui::Grid::new(format!("buy_or_farm_grid_{}", p.prime))
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("part");
                            ui.strong("owned / need");
                            ui.strong("cheapest relic");
                            ui.end_row();

                            for g in &p.gaps {
                                let need = owned_need_cell(g.owned, g.build_quantity);
                                let relic = g
                                    .relics
                                    .first()
                                    .map(|r| {
                                        let stale = stale_marker(&ages, &r.relic_display);
                                        format!("{} ({}){stale}", r.relic_display, plat_str(r.plat))
                                    })
                                    .unwrap_or_else(|| "—".to_string());
                                ui.label(&g.part.part);
                                ui.label(need);
                                ui.label(relic);
                                ui.end_row();
                            }
                        });
                }
                ui.add_space(6.0);
            }
        });
    }

    fn sell_tab(&mut self, ui: &mut egui::Ui) {
        let Some((picks, owned_age_range, ages, priceable_relic_slugs, index)) =
            self.loaded_or_placeholder(ui, |l| {
                (
                    l.live.sell_picks.clone(),
                    l.live.owned_age_range,
                    l.live.ages.clone(),
                    l.live.priceable_relic_slugs.clone(),
                    l.index.clone(),
                )
            })
        else {
            return;
        };
        // Reads the same relic-level `LazyPriceMap` the Relics & Plan tab
        // does (see ADR-0012) — whichever of the two the player opens first
        // legitimately triggers the fetch the other one needs too.
        self.ensure_lazy_prices(
            priceable_relic_slugs.iter().map(|s| (s.clone(), s.clone())).collect(),
            &self.relics_plan_relic_prices,
        );
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
            egui::Grid::new("sell_grid").num_columns(6).striped(true).show(ui, |ui| {
                ui.strong("relic");
                ui.strong("owned");
                ui.strong("plat");
                ui.strong("unmastered");
                ui.strong("parts owned");
                ui.strong("scanned");
                ui.end_row();

                for p in &picks {
                    let plat = relic_slug(&index, &p.display)
                        .map(|slug| lazy_price_str(&self.relics_plan_relic_prices, &slug))
                        .unwrap_or_else(|| plat_str(p.plat));
                    ui.label(&p.display);
                    ui.label(p.count.to_string());
                    ui.label(plat);
                    ui.label(p.unmastered.len().to_string());
                    ui.label(parts_owned_cell(&p.parts_owned));
                    ui.label(age_cell(&ages, &p.display));
                    ui.end_row();
                }
            });
        });
    }

    fn farm_tab(&mut self, ui: &mut egui::Ui) {
        let Some((picks, owned_age_range, ages)) = self.loaded_or_placeholder(ui, |l| {
            (l.live.farm_picks.clone(), l.live.owned_age_range, l.live.ages.clone())
        }) else {
            return;
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
                    let plat = plat_str(p.plat);
                    ui.label(&p.display);
                    ui.label(p.count.to_string());
                    ui.label(&p.best_reward);
                    ui.label(plat);
                    ui.horizontal(|ui| {
                        rarity_pip(ui, &p.rarity);
                        ui.colored_label(rarity_color(&p.rarity), &p.rarity);
                    });
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

    /// The Ducats tab: every owned Prime Part ranked by Ducat efficiency
    /// (ducats ÷ plat, descending — see CONTEXT.md) — warframe.market's own
    /// "which parts are worth trading in for ducats over listing" question,
    /// applied to what's actually in inventory. Defaults to mastered-primes-
    /// only (pure ducat-fodder, nothing lost by dumping them); toggling it
    /// off reveals every owned part, including ones a build still needs.
    fn ducats_tab(&mut self, ui: &mut egui::Ui) {
        let Some(picks) = self.loaded_or_placeholder(ui, |l| l.live.ducat_picks.clone()) else {
            return;
        };

        if picks.is_empty() {
            ui.label(NO_OWNED_PARTS_MSG);
            return;
        }

        ui.checkbox(&mut self.ducats_mastered_only, "Mastered primes only");
        ui.add_space(4.0);

        let rows: Vec<&DucatPick> =
            picks.iter().filter(|p| !self.ducats_mastered_only || p.mastered).collect();

        if rows.is_empty() {
            ui.label("no owned parts match the current filter");
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("ducats_grid").num_columns(5).striped(true).show(ui, |ui| {
                ui.strong("part");
                ui.strong("owned / need");
                ui.strong("ducats");
                ui.strong("plat");
                ui.strong("efficiency");
                ui.end_row();

                for p in &rows {
                    ui.label(format!("{} {}", p.part.prime, p.part.part));
                    ui.label(owned_need_cell(Some(p.owned), p.build_quantity));
                    ui.label(p.ducats.map(|d| format!("{d}d")).unwrap_or_else(|| "—".to_string()));
                    ui.label(plat_str(p.plat));
                    ui.label(
                        p.efficiency.map(|e| format!("{e:.2}")).unwrap_or_else(|| "—".to_string()),
                    );
                    ui.end_row();
                }
            });
        });
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

/// Render a resolved plat price, or `"—"` when unresolved — the shared
/// convention across the Farm tab's mastered-reward prices, which stay
/// eagerly fetched at launch and never need a distinct "still loading" state.
fn plat_str(v: Option<u32>) -> String {
    v.map(|v| format!("{v}p")).unwrap_or_else(|| "—".into())
}

/// Render a lazily-fetched, auto-retrying price cell (see [`LazyPrice`]):
/// the resolved plat price once known, `"…"` while a fetch is in flight,
/// hasn't started yet, or is waiting out its retry cooldown, or an explicit
/// `"no listing"` once a fetch has genuinely resolved to no price — distinct
/// states instead of blank space that reads as broken (see ADR-0012). Used by
/// the Relics & Plan, Sell, and Buy or Farm tabs wherever `plat_str` used to
/// render a relic-slug or built-prime-name price.
///
/// Reads `map` directly rather than trusting the plan/pick's own baked-in
/// price (`PrimePlan::set_plat`, `RelicPick::plat`, …): those are only as
/// fresh as the last [`Live::compute`] tick, which can lag up to
/// [`POLL_INTERVAL`] behind a fetch [`BrowseApp::ensure_lazy_prices`] just
/// landed — reading `map` live means a just-resolved price (or "no listing")
/// shows the frame it lands, not up to 15s later.
fn lazy_price_str(map: &LazyPriceMap, key: &str) -> String {
    match map.lock().unwrap_or_else(|p| p.into_inner()).get(key) {
        Some(LazyPrice::Ready { plat: Some(p), .. }) => format!("{p}p"),
        Some(LazyPrice::Ready { plat: None, .. }) => "no listing".to_string(),
        _ => "…".to_string(),
    }
}

/// Every unmastered Prime's Set-item slug — the [`BrowseApp::ensure_lazy_prices`]
/// input for the lazy Set-price fetch, shared by the Relics & Plan and Buy or
/// Farm tabs since both read the same [`LazyPriceMap`] (see ADR-0012). Mirrors
/// `load_data`'s original eager `resolved_sets` computation exactly.
fn set_price_targets(unmastered_primes: &[String], item_index: &ItemIndex) -> Vec<(String, String)> {
    unmastered_primes
        .iter()
        .filter_map(|prime| {
            item_index.best_match(&format!("{prime} Set")).map(|m| (prime.clone(), m.item.slug.clone()))
        })
        .collect()
}

/// A relic's market slug by its exact display label (e.g. `"Axi H3"`) — the
/// key the Relics & Plan/Sell tabs' relic-level [`LazyPriceMap`] uses.
/// Exact-match only, unlike [`RelicIndex::best_match`]'s fuzzy OCR lookup:
/// `display` here always came from a [`RelicInfo`] in `index` in the first
/// place (see `wf_relic::relics::relic_sourced_parts`), so an exact match is
/// enough.
fn relic_slug(index: &RelicIndex, display: &str) -> Option<String> {
    index.all().iter().find(|r| r.display == display).map(|r| r.slug())
}

/// The Relics & Plan tab's combined `owned / need` cell, e.g. `"— / x1"`
/// (never scanned) or `"1 / x1"` (confirmed). Never renders `0` for an
/// unscanned part — unknown stays `—` (see ADR-0011's precedent, applied to
/// Prime Part owned counts).
fn owned_need_cell(owned: Option<u32>, need: Option<u32>) -> String {
    let owned = owned.map(|o| o.to_string()).unwrap_or_else(|| "—".to_string());
    let need = need.map(|q| format!("x{q}")).unwrap_or_else(|| "—".to_string());
    format!("{owned} / {need}")
}

/// The Sell tab's `parts owned` column: the relic's worst-off unmastered
/// Prime Part (see [`wf_relic::PrimePartGroup`]'s sibling,
/// `wf_relic::RelicPick::parts_owned`) as `"<part> <owned>/<need> +N more"`,
/// mirroring the overlay's existing "top reward, +N" truncation pattern.
/// Empty for a relic with no unmastered rewards at all.
fn parts_owned_cell(summary: &Option<wf_relic::PartsOwnedSummary>) -> String {
    let Some(s) = summary else { return String::new() };
    let owned = s.owned.map(|o| o.to_string()).unwrap_or_else(|| "—".to_string());
    let need = s.need.map(|q| format!("x{q}")).unwrap_or_else(|| "—".to_string());
    let more = if s.more > 0 { format!(" +{} more", s.more) } else { String::new() };
    format!("{} {owned}/{need}{more}", s.part.part)
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
        // updates without needing a mouse move to trigger a redraw. While the
        // initial background load hasn't landed yet, repaint much faster so
        // the "Loading…" placeholders resolve promptly instead of waiting out
        // a full POLL_INTERVAL.
        let still_loading = lock_loaded(&self.loaded).is_none();
        ui.ctx().request_repaint_after(if still_loading { LOADING_REPAINT } else { POLL_INTERVAL });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Mastery, "Mastery");
                ui.selectable_value(&mut self.tab, Tab::Relics, "Relics & Plan");
                ui.selectable_value(&mut self.tab, Tab::RelicsEv, "Relics EV");
                ui.selectable_value(&mut self.tab, Tab::BuyOrFarm, "Buy or Farm");
                ui.selectable_value(&mut self.tab, Tab::Sell, "Sell");
                ui.selectable_value(&mut self.tab, Tab::Farm, "Farm");
                ui.selectable_value(&mut self.tab, Tab::Ducats, "Ducats");
                ui.selectable_value(&mut self.tab, Tab::Owned, "Owned");
            });
            ui.separator();

            match self.tab {
                Tab::Mastery => self.mastery_tab(ui),
                Tab::Relics => self.relics_tab(ui),
                Tab::RelicsEv => self.relics_ev_tab(ui),
                Tab::BuyOrFarm => self.buy_or_farm_tab(ui),
                Tab::Sell => self.sell_tab(ui),
                Tab::Farm => self.farm_tab(ui),
                Tab::Ducats => self.ducats_tab(ui),
                Tab::Owned => self.owned_tab(ui),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_membership_marks_and_unmarks() {
        let mut set: HashSet<String> = HashSet::new();
        toggle_membership(&mut set, "Ember Prime Systems", true);
        assert!(set.contains("Ember Prime Systems"));
        toggle_membership(&mut set, "Ember Prime Systems", false);
        assert!(!set.contains("Ember Prime Systems"));
        // Unmarking something never marked is a no-op, not an error.
        toggle_membership(&mut set, "Volt Prime Chassis", false);
        assert!(set.is_empty());
    }

    #[test]
    fn set_wishlisted_round_trips_through_wf_cache_disk() {
        // Point XDG_CACHE_HOME at a throwaway temp dir for this test only, so
        // `wf_cache::cache_dir()` never touches the real
        // `~/.cache/warframe-lite/wishlist.json` a running `wf-browse` (or
        // this very test suite, run again later) might depend on.
        let dir = std::env::temp_dir().join(format!("wf-browse-wishlist-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CACHE_HOME", &dir);

        // A throwaway runtime just to obtain a `Handle` — this test never
        // actually spawns anything onto it.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut app = BrowseApp::new(
            Arc::new(Mutex::new(None)),
            wf_relic::Wishlist::new(),
            rt.handle().clone(),
            wf_data::http_client(),
            "pc".to_string(),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
        );
        app.set_wishlisted("Ember Prime Systems", true);
        let on_disk = wf_cache::load_blob::<wf_relic::Wishlist>(wf_relic::WISHLIST_FILE)
            .expect("set_wishlisted should have written wishlist.json")
            .value;
        assert!(on_disk.contains("Ember Prime Systems"));

        app.set_wishlisted("Ember Prime Systems", false);
        let on_disk = wf_cache::load_blob::<wf_relic::Wishlist>(wf_relic::WISHLIST_FILE)
            .expect("wishlist.json should still be present after unmarking")
            .value;
        assert!(!on_disk.contains("Ember Prime Systems"));

        std::env::remove_var("XDG_CACHE_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_fetch_when_never_attempted() {
        assert!(needs_fetch(None, Instant::now()));
    }

    #[test]
    fn needs_fetch_is_false_while_a_fetch_is_already_in_flight() {
        assert!(!needs_fetch(Some(&LazyPrice::Loading), Instant::now()));
    }

    #[test]
    fn needs_fetch_is_false_once_a_price_is_resolved_even_if_stale() {
        let resolved_at = Instant::now() - LAZY_PRICE_RETRY_COOLDOWN * 10;
        let ready = LazyPrice::Ready { plat: Some(42), resolved_at };
        assert!(!needs_fetch(Some(&ready), Instant::now()));
    }

    #[test]
    fn needs_fetch_waits_out_the_cooldown_after_a_no_listing_result() {
        let resolved_at = Instant::now();
        let ready = LazyPrice::Ready { plat: None, resolved_at };
        assert!(!needs_fetch(Some(&ready), resolved_at + LAZY_PRICE_RETRY_COOLDOWN / 2));
    }

    #[test]
    fn needs_fetch_retries_a_no_listing_result_once_the_cooldown_elapses() {
        let resolved_at = Instant::now();
        let ready = LazyPrice::Ready { plat: None, resolved_at };
        assert!(needs_fetch(Some(&ready), resolved_at + LAZY_PRICE_RETRY_COOLDOWN));
    }

    #[test]
    fn snapshot_prices_treats_loading_the_same_as_absent() {
        let now = Instant::now();
        let map: LazyPriceMap = Arc::new(Mutex::new(HashMap::from([
            ("resolved".to_string(), LazyPrice::Ready { plat: Some(15), resolved_at: now }),
            ("empty".to_string(), LazyPrice::Ready { plat: None, resolved_at: now }),
            ("loading".to_string(), LazyPrice::Loading),
        ])));
        let snapshot = snapshot_prices(&map);
        assert_eq!(snapshot.get("resolved"), Some(&Some(15)));
        assert_eq!(snapshot.get("empty"), Some(&None));
        assert_eq!(snapshot.get("loading"), None);
    }

    #[test]
    fn lazy_price_str_distinguishes_loading_from_no_listing_from_priced() {
        let now = Instant::now();
        let map: LazyPriceMap = Arc::new(Mutex::new(HashMap::from([
            ("no_listing".to_string(), LazyPrice::Ready { plat: None, resolved_at: now }),
            ("priced".to_string(), LazyPrice::Ready { plat: Some(7), resolved_at: now }),
            ("loading".to_string(), LazyPrice::Loading),
        ])));
        assert_eq!(lazy_price_str(&map, "priced"), "7p");
        // No entry yet reads as still-loading, not broken.
        assert_eq!(lazy_price_str(&map, "never_seen"), "…");
        assert_eq!(lazy_price_str(&map, "loading"), "…");
        // A completed fetch that genuinely found no price is explicit, not blank.
        assert_eq!(lazy_price_str(&map, "no_listing"), "no listing");
    }

    #[test]
    fn lazy_price_str_shows_a_freshly_landed_price_immediately() {
        // A fetch that lands between `poll` ticks writes straight into the
        // map; `lazy_price_str` must reflect that the same frame rather than
        // waiting up to `POLL_INTERVAL` for `Live::compute`'s next snapshot
        // to bake it into a plan/pick's own `plat`/`set_plat` field — that
        // staleness previously showed a real price as "no listing" for as
        // long as 15s (see ADR-0012 review).
        let map: LazyPriceMap = Arc::new(Mutex::new(HashMap::from([(
            "axi_h3_relic".to_string(),
            LazyPrice::Ready { plat: Some(30), resolved_at: Instant::now() },
        )])));
        assert_eq!(lazy_price_str(&map, "axi_h3_relic"), "30p");
    }
}
