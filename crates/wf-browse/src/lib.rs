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
//! [`run`] is the entry point both the standalone `wf-browse` binary (kept
//! for dev/embedding) and the merged `wf-lite` binary call — `wf-lite`'s
//! `browse` subcommand runs this in-process rather than spawning a separate
//! browse process (see ticket #69).
//!
//! It does not initialize a `tracing` subscriber itself — the process that
//! owns `main` does that once, since a second `tracing_subscriber::fmt()
//! ...init()` call in the same process would panic.
//!
//! The relic catalogue, mastery, and Sell/Farm-tab prices are loaded once, but
//! never block the window from opening: [`run`] shows it immediately and
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
    DecodedRiven, DucatPick, EquipmentCategory, EvRefinement, FarmPick, ItemIndex, MasteryEntry,
    MasterySet, OwnedRivens, PartMarketInfo, PartQuantities, PrimePart, PrimePlan, RelicIndex,
    RelicInfo, RelicPick, RivenTypeVerdict, RivenVerdict, CATEGORY_ORDER,
};

const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const MASTERY_TTL: Duration = Duration::from_secs(24 * 3600);
/// How many price lookups run at once when pricing the Sell/Farm tabs. A
/// player can own hundreds of distinct relics, so fetching prices one at a
/// time (like the CLI's small, explicit-args case) could block for minutes
/// once the shared cache goes stale; unbounded concurrency, on the other
/// hand, would fire every lookup at warframe.market in one burst.
const PRICE_FETCH_CONCURRENCY: usize = 8;
/// How many backed-off in-batch retries [`fetch_prices`]/[`fetch_riven_verdicts`]
/// give an item that comes back with nothing at all (no stale value, failed
/// live fetch — see their docs) before leaving it unresolved for this batch.
const BATCH_FETCH_RETRIES: u32 = 2;
/// Base backoff delay between batch-fetch retry rounds, doubling per round
/// via [`wf_data::poll::backoff_interval`].
const BATCH_RETRY_BASE: Duration = Duration::from_secs(2);
/// Ceiling on the batch-fetch retry backoff — kept short since this delays
/// `load_data` completing, which the loading placeholder is already covering
/// for (see this module's docs), not a background poll.
const BATCH_RETRY_CAP: Duration = Duration::from_secs(20);
/// How long a lazily-fetched price that resolved to "no listing found" (see
/// [`LazyPrice`]) waits before [`BrowseApp::ensure_lazy_prices`] retries it.
/// Long enough that `relics_tab`'s every-frame, unvirtualized re-render (see
/// its docs) doesn't refire a fetch dozens of times a second; short enough
/// that a fetch that failed on its first attempt (ADR-0012's original bug —
/// a slug with nothing cached yet whose fetch times out gets stuck on `None`
/// forever) clears up within the same session instead of needing a relaunch.
const LAZY_PRICE_RETRY_COOLDOWN: Duration = Duration::from_secs(45);
/// Ceiling on the lazy price/riven-verdict retry backoff (issue #100) — a
/// run of failed fetches (network error, timeout, or a warframe.market 429;
/// see [`wf_relic::FetchStatus`]) doubles [`LAZY_PRICE_RETRY_COOLDOWN`] per
/// consecutive failure up to this cap, rather than retrying at the same
/// fixed 45s forever.
const LAZY_PRICE_RETRY_CAP: Duration = Duration::from_secs(600);
/// How many relic badges the Relics & Plan tab's "relics you own that can
/// still drop it" cell shows before collapsing the rest into a "+N more"
/// label (mirroring [`worst_off_part_cell`]'s "+N more" pattern). A part
/// with many owned sourcing relics would otherwise force that column — and,
/// via `egui::Grid`'s shared column widths, every row in the whole table —
/// wide enough to overflow the window horizontally. `g.relics` is already
/// sorted cheapest-first (see [`wf_relic::PrimePartGroup::relics`]), so the
/// shown relics are also the most actionable ones.
const RELIC_LIST_MAX_VISIBLE: usize = 3;
/// How often the Relics & Plan, Sell, and Farm tabs' Owned relic counts and
/// active-Fissure flag refresh while the window stays open. Only these two (a
/// local file and a lightweight world-state fetch) are cheap enough to poll;
/// mastery, the relic catalogue, and Sell/Farm-tab prices are loaded once at
/// launch and never re-fetched on this timer. Matches the overlay's default
/// `fissure_refresh_secs` (issue #100) — Fissures change on the order of
/// minutes in-game, so the old fixed 15s bought no real freshness, just
/// extra load on warframestat.us.
const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// Ceiling on the poll loop's failure backoff (see [`wf_data::poll::backoff_interval`]),
/// mirroring the overlay's `WORLDSTATE_RETRY_CAP`.
const POLL_RETRY_CAP: Duration = Duration::from_secs(600);
/// Jitter fraction applied to the poll interval (issue #100), so relaunch
/// clustering across independent installs doesn't line up into a
/// synchronized burst against warframestat.us.
const POLL_JITTER: f64 = 0.2;
/// Ceiling on the one-time random delay before `load_data`'s first network
/// call (issue #100) — see [`wf_data::poll::startup_delay`].
const STARTUP_JITTER_MAX: Duration = Duration::from_secs(3);
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
/// Shown on the Rivens tab when no Unveiled riven has been scanned yet.
/// Unlike relics/parts, rivens only ever come from a mem-scan (Scan Memory /
/// `wf-lite mem-scan`) — there is no OCR path for them (ADR-0001/ADR-0003).
const NO_OWNED_RIVENS_MSG: &str =
    "no owned riven data yet. Click Scan Memory on the Home tab (or run `wf-lite mem-scan`) \
     once you have at least one Unveiled riven.";
/// Relic tiers offered by the tier-filter checkboxes, in drop order.
const TIERS: [&str; 5] = ["Lith", "Meso", "Neo", "Axi", "Requiem"];
/// Standard Warframe mission types, offered by the fissure-filter's
/// mission-type checkboxes. The live API's `mission_type` field is
/// free-form text, so this is a curated (not authoritative) list — the
/// values here must match the API's own spelling exactly, since
/// `FissureFilter::matches` compares by plain string equality. `warframestat.us`
/// sends `"Extermination"` (confirmed against the live API, 2026-08-14), not
/// the in-game menu's "Exterminate" — using the latter here silently
/// excluded every Exterminate fissure from the panel, filter checked or not.
const MISSION_TYPES: [&str; 13] = [
    "Assassination",
    "Capture",
    "Defense",
    "Disruption",
    "Excavation",
    "Extermination",
    "Hijack",
    "Interception",
    "Mobile Defense",
    "Rescue",
    "Sabotage",
    "Spy",
    "Survival",
];
/// Below this distance from an axis's center, the drag-to-place box is
/// locked exactly centered on that axis — see [`BrowseApp::drag_to_place`].
/// Widened from the `/prototype`-validated 10px after live-testing the real
/// widget: the anchor *label* (and thus which anchor the box will commit
/// to) flips as soon as a drag crosses this radius, well before the
/// rendered box has visually moved far from center — a 10px radius made
/// that flip feel like an immediate jump to a corner anchor even though the
/// pixel position itself stayed continuous. 20px gives a comfort zone that
/// actually matches normal mouse precision.
const PLACE_LOCK_RADIUS: f32 = 20.0;
/// Beyond this distance, the box tracks the cursor exactly on that axis.
/// Between [`PLACE_LOCK_RADIUS`] and this, it eases between the two (see
/// [`magnetic_axis`]) — the *position* never jumps (settled via
/// `/prototype` on issue #85), but a live-tested build revealed a second,
/// subtler issue a plain linear blend leaves: it matches position at both
/// boundaries but not velocity, so the box visibly goes from moving to
/// completely frozen right at the lock radius — which reads as "it just
/// snapped to center" even though no pixel ever jumped. A smoothstep ease
/// fixes that by matching slope (not just position) at both ends. Widened
/// alongside `PLACE_LOCK_RADIUS` — see its docs.
const PLACE_CAPTURE_RADIUS: f32 = 90.0;
/// Size of the mock overlay-panel rectangle in [`BrowseApp::drag_to_place`].
const PLACE_BOX_SIZE: egui::Vec2 = egui::Vec2::new(110.0, 66.0);

/// Which screen edge(s) an anchor string pins to: `(left, right, top,
/// bottom)`. An axis with neither flag set centers on that axis — the only
/// state [`magnetic_axis`] can't just track the cursor for.
fn edges_for(anchor: &str) -> (bool, bool, bool, bool) {
    match anchor {
        "top-left" => (true, false, true, false),
        "top-right" => (false, true, true, false),
        "bottom-left" => (true, false, false, true),
        "bottom-right" => (false, true, false, true),
        "top" => (false, false, true, false),
        "bottom" => (false, false, false, true),
        "left" => (true, false, false, false),
        "right" => (false, true, false, false),
        _ => (false, false, false, false), // center
    }
}

/// One axis's pin state: which screen edge (if any) the drag-to-place box
/// is currently flush against on that axis.
#[derive(Clone, Copy, PartialEq)]
enum AxisPin {
    Neg, // left/top edge
    Pos, // right/bottom edge
    Centered,
}

/// Where the drag-to-place box's top-left corner sits on one axis, and
/// which state that implies, purely as a function of the raw dragged
/// corner position on that axis. A pinned edge's margin is an exact
/// inverse of the drag position, so it always renders at literally the raw
/// cursor position — nothing to correct, ever. The only state that can't
/// just track the cursor is "centered" (the real overlay's un-pinned axis
/// forces to a fixed value, independent of where it was dragged), so a
/// magnetic well around each axis's center eases between raw tracking and
/// the locked-center value. The ease matches both position *and slope* at
/// its two boundaries (a smoothstep, not a linear blend) — matching only
/// position leaves a velocity kink at the lock radius where the box goes
/// from moving to instantly frozen, which reads as a snap even though the
/// position itself never jumps.
fn magnetic_axis(raw: f32, screen_lo: f32, screen_hi: f32, box_dim: f32) -> (f32, AxisPin) {
    let raw = raw.clamp(screen_lo, screen_hi - box_dim);
    let center = screen_lo + (screen_hi - screen_lo - box_dim) / 2.0;
    let d = raw - center;
    let ad = d.abs();

    let value = if ad <= PLACE_LOCK_RADIUS {
        center
    } else if ad < PLACE_CAPTURE_RADIUS {
        let t = (ad - PLACE_LOCK_RADIUS) / (PLACE_CAPTURE_RADIUS - PLACE_LOCK_RADIUS);
        // Smoothstep, not a plain lerp: its derivative is 0 at t=0 and t=1,
        // matching the flat "locked" region's zero slope and the outer
        // "raw tracking" region's unit slope respectively — a linear `t`
        // matches position at both boundaries but not velocity, which is
        // what read as a snap (see this constant's docs).
        let eased = t * t * (3.0 - 2.0 * t);
        center + d * eased
    } else {
        raw
    };

    let pin = if ad <= PLACE_LOCK_RADIUS {
        AxisPin::Centered
    } else if d < 0.0 {
        AxisPin::Neg
    } else {
        AxisPin::Pos
    };

    (value, pin)
}

/// Combine independent x/y pin states into one of the 9 anchor strings
/// `wf_overlay::layer::Placement::parse` understands.
fn anchor_for(x_pin: AxisPin, y_pin: AxisPin) -> &'static str {
    use AxisPin::*;
    match (x_pin, y_pin) {
        (Neg, Neg) => "top-left",
        (Pos, Neg) => "top-right",
        (Neg, Pos) => "bottom-left",
        (Pos, Pos) => "bottom-right",
        (Centered, Neg) => "top",
        (Centered, Pos) => "bottom",
        (Neg, Centered) => "left",
        (Pos, Centered) => "right",
        (Centered, Centered) => "center",
    }
}

/// Half the room left over once the mock box is subtracted from the mock
/// screen on each axis — the mock-space distance at which a pinned edge's
/// position coincides with being centered. Stored margins are *fractions* of
/// this (see [`wf_config::OverlayConfig::margin_x`]), not raw pixels: a drag
/// that's N% of the way from flush-against-an-edge to centered in the mock
/// stores exactly N%, and `wf_overlay::layer::edge_margins` re-derives the
/// same N% against whatever real screen the overlay actually lands on — no
/// cross-monitor scale factor needed, and thus nothing that can be thrown
/// off by the settings window sitting on a different monitor than the game
/// (see issue where a drag near the mock's edge landed the real overlay
/// near center on a much wider monitor).
fn mock_max_margin(mock_screen: egui::Vec2, mock_box: egui::Vec2) -> egui::Vec2 {
    (mock_screen - mock_box) / 2.0
}

/// Top-left corner of a `size`-sized box anchored+margined on `screen`,
/// matching `wf_overlay::layer`'s own anchor semantics.
fn place_box(screen: egui::Rect, size: egui::Vec2, anchor: &str, margin_x: f32, margin_y: f32) -> egui::Pos2 {
    let (left, right, top, bottom) = edges_for(anchor);
    let x = if left {
        screen.left() + margin_x
    } else if right {
        screen.right() - margin_x - size.x
    } else {
        screen.left() + (screen.width() - size.x) / 2.0
    };
    let y = if top {
        screen.top() + margin_y
    } else if bottom {
        screen.bottom() - margin_y - size.y
    } else {
        screen.top() + (screen.height() - size.y) / 2.0
    };
    egui::pos2(x, y)
}

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

/// The warframe-lite mark, same bundled PNG as `wf-tray`'s tray icon, decoded
/// into egui's plain-RGBA `IconData` (unlike ksni's tray pixmap, no ARGB
/// byte-order swap needed).
fn app_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon.png");
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let img = img.to_rgba8();
            let (width, height) = (img.width(), img.height());
            egui::IconData { rgba: img.into_vec(), width, height }
        }
        Err(e) => {
            tracing::error!("failed to decode bundled window icon: {e}");
            egui::IconData::default()
        }
    }
}

/// Open the browse window and run its event loop until closed.
pub fn run() -> eframe::Result<()> {
    let config_path = Config::default_path().unwrap_or_else(|_| PathBuf::from("config.toml"));
    let config = Config::load(&config_path).unwrap_or_default();

    // Reuse the caller's tokio runtime when there already is one — the
    // shipped `wf-lite browse`/`settings` path: `main`'s `#[tokio::main]`
    // runtime is already driving this call via `block_on` (see
    // `run_browse`'s docs in `src/main.rs`) — rather than nesting a second
    // owned `Runtime` inside it. A nested `Runtime` dropped while still
    // inside an outer runtime's async context panics ("Cannot drop a
    // runtime in a context where blocking is not allowed") the instant this
    // function returns and a local `Runtime` would go out of scope — which
    // used to happen on every window close. The standalone `wf-browse`
    // binary (a plain, non-async `fn main`, kept for `cargo run -p
    // wf-browse`) has no outer runtime to borrow, so it falls back to
    // owning one here. `_owned_rt` is never dropped before `run_native`
    // returns, so the background loader/poller spawned below — and any
    // on-demand price fetch the Relics EV tab spawns via `rt_handle` — keep
    // running for as long as the window stays open; its later drop is safe
    // in that fallback case since that path is never itself inside an
    // async context.
    let _owned_rt;
    let rt_handle = match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            _owned_rt = None;
            handle
        }
        Err(_) => {
            let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
            let handle = rt.handle().clone();
            _owned_rt = Some(rt);
            handle
        }
    };
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
    //
    // The Settings section (#72, folded into Home at #77) needs its own long-lived, mutable `Config`
    // to edit and save, same as the standalone `wf-settings` crate does —
    // cloned here *before* `config` is moved into `load_and_poll`, so a
    // change made on that tab doesn't retroactively affect the background
    // loader/poller's own copy (that copy only drives fissure-refresh
    // polling, so this doesn't affect the settings tab's own anchor/margin/
    // opacity/fissures fields live-applying to a running overlay — see
    // `settings_tab`'s docs).
    let settings_config = config.clone();
    rt_handle.spawn(load_and_poll(loaded.clone(), config, relic_prices.clone(), set_prices.clone()));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("warframe-lite browse")
            // Resizable, wider default than the old fixed 700x700 (#77) —
            // the grouped two-tier nav (see `Group`) and denser tabs like
            // Mastery/Relics & Plan need more room to breathe.
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([760.0, 560.0])
            .with_icon(app_icon())
            // Matches `packaging/warframe-lite.desktop`'s filename, which is
            // how the Wayland compositor's taskbar looks up an app icon for
            // xdg-shell clients — `with_icon`'s pixel buffer alone only
            // covers X11 (`_NET_WM_ICON`); without a matching app_id a
            // KDE/Wayland taskbar falls back to some unrelated default (seen
            // showing the system default browser's icon instead).
            .with_app_id("warframe-lite"),
        ..Default::default()
    };
    eframe::run_native(
        "warframe-lite browse",
        options,
        Box::new(move |cc| {
            apply_theme(&cc.egui_ctx, settings_config.ui.font_scale);
            Ok(Box::new(BrowseApp::new(
                loaded,
                wishlist,
                rt_handle,
                client,
                market_platform,
                relic_prices,
                set_prices,
                settings_config,
                config_path,
            )))
        }),
    )
}

/// Apply a clean, dark, teal-accented theme, then `font_scale` on top of it.
/// Scoped to `wf-browse` only — the default egui look is serviceable but
/// visually flat, so this tightens spacing, rounds panels/widgets, and gives
/// interactive/selected elements a consistent accent color instead of egui's
/// default blue. Keeps egui's default fonts (see [`apply_font_scale`] for
/// why only their size, not the family, is user-configurable).
fn apply_theme(ctx: &egui::Context, font_scale: f32) {
    // Always dark, regardless of the system preference — this is a deliberate
    // app-wide look, not a system-theme follow.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, style_theme);
    apply_font_scale(ctx, font_scale);
}

/// Scale every egui text style's font size by `font_scale` off of egui's own
/// defaults (`1.0` = unscaled) — called once at launch and again live on
/// every `Self::home_overview_tab` UI-text-size slider frame, so dragging it
/// previews immediately instead of waiting for a restart. Only size is
/// user-configurable, not family: egui only bundles one proportional face
/// (Ubuntu-Light) and one monospace face (Hack) itself, and offering a real
/// typeface picker would mean sourcing, licensing, and bundling additional
/// font files as new repo assets — out of scope for now.
fn apply_font_scale(ctx: &egui::Context, font_scale: f32) {
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = egui::style::default_text_styles()
            .into_iter()
            .map(|(text_style, mut font_id)| {
                font_id.size *= font_scale;
                (text_style, font_id)
            })
            .collect();
    });
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

    // egui's default scroll style floats the scrollbar over the content
    // (zero allocated width), so a scrolled tab's rightmost text sits right
    // under the bar and gets visually clipped by it. Solid reserves real
    // space for the bar instead, at the cost of a few px of content width.
    style.spacing.scroll = egui::style::ScrollStyle::solid();
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

/// Mission-type checkboxes for the fissure filter, wrapped across lines since
/// [`MISSION_TYPES`] is too long for one row at this panel's width.
fn mission_type_filter_ui(ui: &mut egui::Ui, selected: &mut HashSet<String>) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Mission:");
        for mission_type in MISSION_TYPES {
            let mut checked = selected.contains(mission_type);
            if ui.checkbox(&mut checked, mission_type).changed() {
                if checked {
                    selected.insert(mission_type.to_string());
                } else {
                    selected.remove(mission_type);
                }
            }
        }
    });
}

/// Kind checkboxes (Normal / Steel Path / Void Storm) for the fissure
/// filter. Stored as the no-space tokens `FissureFilter::kinds` expects
/// ("Normal"/"SteelPath"/"VoidStorm"), labeled with their display names.
fn fissure_kind_filter_ui(ui: &mut egui::Ui, selected: &mut HashSet<String>) {
    const KINDS: [(&str, &str); 3] =
        [("Normal", "Normal"), ("SteelPath", "Steel Path"), ("VoidStorm", "Void Storm")];
    ui.horizontal(|ui| {
        ui.label("Kind:");
        for (token, label) in KINDS {
            let mut checked = selected.contains(token);
            if ui.checkbox(&mut checked, label).changed() {
                if checked {
                    selected.insert(token.to_string());
                } else {
                    selected.remove(token);
                }
            }
        }
    });
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
    /// The Rivens tab's per-owned-weapon Floor/Ceiling/Verdict, keyed by
    /// `weapon_unique_name` (DE's own id — the same string
    /// [`DecodedRiven::weapon_unique_name`] carries) rather than the
    /// warframe.market slug, since that's what a [`RivenGroup`] already
    /// groups by. Resolved once in [`load_data`], same "fetched at launch,
    /// not re-fetched on poll" rule as `ducats` (a newly mem-scanned riven
    /// of an already-priced weapon shows its Verdict immediately; a riven of
    /// a brand-new weapon waits for the next launch).
    riven_verdicts: HashMap<String, RivenTypeVerdict>,
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
/// [`BrowseApp::ensure_lazy_prices`] knows when the retry cooldown has
/// elapsed and it's time to try again — for as long as a tab that needs it
/// stays open. `consecutive_failures` (issue #100) tracks a run of *failed*
/// live fetches — network error, timeout, or a warframe.market 429 — as
/// reported by [`wf_relic::FetchStatus`], distinct from a successful fetch
/// that genuinely found no listings: only the failure streak backs the
/// cooldown off past [`LAZY_PRICE_RETRY_COOLDOWN`], so a real rate limit
/// doesn't get hammered at the same fixed cadence as a plain cache miss.
#[derive(Clone, Copy)]
enum LazyPrice {
    Loading,
    Ready { plat: Option<u32>, resolved_at: Instant, consecutive_failures: u32 },
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
/// resolved to "no listing" and its cooldown has elapsed since; skip while a
/// fetch is already in flight, or a resolved price is known, or the cooldown
/// hasn't elapsed yet. The cooldown itself is [`LAZY_PRICE_RETRY_COOLDOWN`]
/// on a clean "no listing" result, backed off past that
/// (`consecutive_failures > 0`) after a run of failed fetches — see
/// [`LazyPrice`].
fn needs_fetch(current: Option<&LazyPrice>, now: Instant) -> bool {
    match current {
        None => true,
        Some(LazyPrice::Ready { plat: None, resolved_at, consecutive_failures }) => {
            let cooldown = wf_data::poll::backoff_interval(
                LAZY_PRICE_RETRY_COOLDOWN,
                *consecutive_failures,
                LAZY_PRICE_RETRY_CAP,
            );
            now.duration_since(*resolved_at) >= cooldown
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
    mem_scanned_parts: bool,
}

/// The Mastery tab's per-row read-only lookups, bundled (mirroring
/// [`RelicEvContext`]) so [`BrowseApp::mastery_prime_row`] doesn't carry them
/// as separate parameters (clippy's `too_many_arguments`).
#[derive(Clone, Copy)]
struct MasteryRowContext<'a> {
    quantities: &'a PartQuantities,
    part_market: &'a HashMap<PrimePart, PartMarketInfo>,
    owned_parts: &'a wf_relic::OwnedPrimeParts,
    mem_scanned_parts: bool,
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
    /// Whether `owned_parts` reflects at least one completed `wf-mem`
    /// mem-scan (see [`wf_relic::owned_parts::OWNED_PARTS_MEM_SCANNED_MARKER_FILE`]).
    /// Lets every owned/need cell fed by `owned_parts` show a part absent
    /// from the scan as confirmed-zero rather than unknown, since a mem-scan
    /// snapshot already treats absence that way (see
    /// [`wf_relic::owned_parts::get_or_confirmed_zero`]).
    mem_scanned_parts: bool,
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
    /// The Rivens tab's data, grouped by owned weapon and Verdict-attached —
    /// recomputed each poll tick against the same launch-time
    /// `Prices::riven_verdicts` (mirrors `ducat_picks`'s same "recompute
    /// ranking, don't refetch price" rule).
    riven_groups: Vec<RivenGroup>,
}

/// Every Unveiled riven the player owns of one weapon, plus that weapon's
/// Floor/Ceiling/Verdict (a group-level fact per CONTEXT.md's Verdict entry
/// — computed once for the group, not per copy). The Rivens tab's grouped-
/// by-type layout (`docs/specs/riven-browse-tab.md` §4) renders one of
/// these per collapsing section.
#[derive(Clone)]
struct RivenGroup {
    weapon_name: String,
    weapon_unique_name: String,
    /// `None` when the weapon has no cached Verdict yet — a weapon newly
    /// mem-scanned since launch (see `Prices::riven_verdicts`'s doc), not an
    /// "insufficient data" abstain (that's [`RivenTypeVerdict::verdict`]
    /// being [`RivenVerdict::InsufficientData`], a distinct, already-priced
    /// state).
    verdict: Option<RivenTypeVerdict>,
    rivens: Vec<DecodedRiven>,
}

/// Group `owned` by [`DecodedRiven::weapon_unique_name`], attaching each
/// group's Verdict from `verdicts` — sorted by weapon name for a stable,
/// scan-order-independent render.
fn group_owned_rivens(
    owned: &OwnedRivens,
    verdicts: &HashMap<String, RivenTypeVerdict>,
) -> Vec<RivenGroup> {
    let mut groups: Vec<RivenGroup> = Vec::new();
    for riven in owned {
        match groups.iter_mut().find(|g| g.weapon_unique_name == riven.weapon_unique_name) {
            Some(g) => g.rivens.push(riven.clone()),
            None => groups.push(RivenGroup {
                weapon_name: riven.weapon_name.clone(),
                weapon_unique_name: riven.weapon_unique_name.clone(),
                verdict: verdicts.get(&riven.weapon_unique_name).copied(),
                rivens: vec![riven.clone()],
            }),
        }
    }
    groups.sort_by(|a, b| a.weapon_name.cmp(&b.weapon_name));
    groups
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
        mem_scanned_parts: bool,
        owned_rivens: &OwnedRivens,
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
        let ctx = wf_relic::RelicContext { index, mastery, quantities, owned_parts, mem_scanned_parts };
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
            mem_scanned_parts,
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
        let riven_groups = group_owned_rivens(owned_rivens, &prices.riven_verdicts);
        Self {
            plans,
            sell_picks,
            farm_picks,
            bom_plans,
            owned_age_range,
            ages,
            active_tiers,
            owned_parts: owned_parts.clone(),
            mem_scanned_parts,
            ducat_picks,
            priceable_relic_slugs,
            unmastered_primes,
            riven_groups,
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
        mem_scanned_parts,
        owned_rivens,
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
    let live = Live::compute(
        owned.as_ref(),
        &owned_parts,
        mem_scanned_parts,
        &owned_rivens,
        &static_data,
        &prices,
        active_tiers,
    );
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
    // Consecutive world-state fetch failures — backs off the poll cadence
    // instead of hammering a struggling warframestat.us at a fixed interval
    // forever (issue #100; mirrors the overlay's own worldstate refresh loop
    // in `src/main.rs`).
    let mut consecutive_failures: u32 = 0;
    loop {
        let interval = wf_data::poll::jitter(
            wf_data::poll::backoff_interval(POLL_INTERVAL, consecutive_failures, POLL_RETRY_CAP),
            POLL_JITTER,
        );
        tokio::time::sleep(interval).await;
        let owned = wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE);
        let owned_parts =
            wf_cache::load_blob::<wf_relic::OwnedPrimeParts>(wf_relic::OWNED_PRIME_PARTS_FILE)
                .map(|s| s.value)
                .unwrap_or_default();
        let mem_scanned_parts =
            wf_cache::load_blob::<bool>(wf_relic::owned_parts::OWNED_PARTS_MEM_SCANNED_MARKER_FILE)
                .is_some();
        let owned_rivens: OwnedRivens = wf_cache::load_blob::<OwnedRivens>(wf_relic::OWNED_RIVENS_FILE)
            .map(|s| s.value)
            .unwrap_or_default();
        let active_tiers = match wf_data::worldstate::fetch(&client, &platform).await {
            Ok(ws) => {
                consecutive_failures = 0;
                ws.active_fissure_tiers()
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!(
                    "worldstate refresh failed: {e:#} — backing off to {}s",
                    wf_data::poll::backoff_interval(POLL_INTERVAL, consecutive_failures, POLL_RETRY_CAP).as_secs()
                );
                HashSet::default()
            }
        };
        let static_data = StaticData {
            index: &index,
            mastery: &mastery,
            quantities: &quantities,
            part_market: &part_market,
        };
        prices.sell = snapshot_prices(&relic_prices);
        prices.set = snapshot_prices(&set_prices);
        let fresh = Live::compute(
            owned.as_ref(),
            &owned_parts,
            mem_scanned_parts,
            &owned_rivens,
            &static_data,
            &prices,
            active_tiers,
        );
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
    mem_scanned_parts: bool,
    /// Every decoded Unveiled riven from the last mem-scan (`rivens.json`),
    /// if any — see [`wf_relic::owned_rivens`].
    owned_rivens: OwnedRivens,
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
///
/// Firing [`PRICE_FETCH_CONCURRENCY`] requests at once is exactly the kind
/// of burst likely to trip a rate limit (issue #100 — confirmed live: a
/// 429 from `api.warframe.market` blanking out prices for an entire batch).
/// An item that comes back with nothing at all — no stale cache to fall
/// back on and a failed live fetch, per [`wf_relic::FetchStatus`] — gets a
/// few backed-off retries within this same call rather than being left
/// permanently blank for the rest of the session; an item with *some*
/// value (fresh or stale) is accepted immediately.
async fn fetch_prices<T>(
    inputs: Vec<T>,
    cache: &wf_relic::PriceCache,
    market: &wf_data::market::MarketClient,
    key_and_slug: impl Fn(T) -> (String, String),
) -> HashMap<String, (Option<u32>, wf_relic::FetchStatus)>
where
    T: Send,
{
    let mut pending: Vec<(String, String)> = inputs.into_iter().map(key_and_slug).collect();
    let mut resolved: HashMap<String, (Option<u32>, wf_relic::FetchStatus)> = HashMap::new();
    let mut attempt: u32 = 0;
    loop {
        let batch = std::mem::take(&mut pending);
        let results: Vec<(String, String, Option<u32>, wf_relic::FetchStatus)> = stream::iter(batch)
            .map(|(key, slug)| async move {
                let (plat, status) =
                    wf_relic::cached_plat_status(cache, market, &slug, wf_relic::PriceOpts::default())
                        .await;
                (key, slug, plat, status)
            })
            .buffer_unordered(PRICE_FETCH_CONCURRENCY)
            .collect()
            .await;
        for (key, slug, plat, status) in results {
            if plat.is_none() && status == wf_relic::FetchStatus::Failed && attempt < BATCH_FETCH_RETRIES
            {
                pending.push((key, slug));
            } else {
                resolved.insert(key, (plat, status));
            }
        }
        if pending.is_empty() {
            break;
        }
        attempt += 1;
        tokio::time::sleep(wf_data::poll::backoff_interval(
            BATCH_RETRY_BASE,
            attempt,
            BATCH_RETRY_CAP,
        ))
        .await;
    }
    resolved
}

/// Look up a bounded-concurrency batch of riven Floor/Ceiling/Verdicts, one
/// per distinct owned weapon — mirrors [`fetch_prices`]'s exact pattern
/// (same concurrency cap, same cache-first/timeout-falls-back-to-stale
/// behavior via [`wf_relic::cached_riven_verdict_status`], same backed-off
/// in-batch retry for an item that failed with nothing to fall back on —
/// see [`fetch_prices`]'s doc), just keyed by `weapon_unique_name` instead
/// of a market item slug. A weapon absent from `slug_by_weapon` (not in
/// warframe.market's riven-weapon catalogue at all) is skipped — nothing to
/// query.
async fn fetch_riven_verdicts(
    weapon_unique_names: Vec<String>,
    slug_by_weapon: &HashMap<String, String>,
    cache: &wf_relic::RivenPriceCache,
    market: &wf_data::riven_market::RivenMarketClient,
) -> HashMap<String, RivenTypeVerdict> {
    // Resolved synchronously, before entering the stream — a slug lookup is
    // cheap and borrowing `slug_by_weapon` across the stream's own futures
    // would tie their lifetime to this call, which `tokio::spawn`'s `'static`
    // bound (see `load_and_poll`'s caller) doesn't allow.
    let mut pending: Vec<(String, String)> = weapon_unique_names
        .into_iter()
        .filter_map(|weapon_unique_name| {
            slug_by_weapon.get(&weapon_unique_name).cloned().map(|slug| (weapon_unique_name, slug))
        })
        .collect();

    let mut resolved: HashMap<String, RivenTypeVerdict> = HashMap::new();
    let mut attempt: u32 = 0;
    loop {
        let batch = std::mem::take(&mut pending);
        let results: Vec<(String, String, Option<RivenTypeVerdict>, wf_relic::FetchStatus)> =
            stream::iter(batch)
                .map(|(weapon_unique_name, slug)| async move {
                    let (verdict, status) = wf_relic::cached_riven_verdict_status(
                        cache,
                        market,
                        &slug,
                        wf_relic::PriceOpts::default(),
                    )
                    .await;
                    (weapon_unique_name, slug, verdict, status)
                })
                .buffer_unordered(PRICE_FETCH_CONCURRENCY)
                .collect()
                .await;
        for (weapon_unique_name, slug, verdict, status) in results {
            match verdict {
                Some(v) => {
                    resolved.insert(weapon_unique_name, v);
                }
                None if status == wf_relic::FetchStatus::Failed && attempt < BATCH_FETCH_RETRIES => {
                    pending.push((weapon_unique_name, slug));
                }
                None => {}
            }
        }
        if pending.is_empty() {
            break;
        }
        attempt += 1;
        tokio::time::sleep(wf_data::poll::backoff_interval(
            BATCH_RETRY_BASE,
            attempt,
            BATCH_RETRY_CAP,
        ))
        .await;
    }
    resolved
}

/// Load the relic catalogue, the player's mastered set, their scanned Owned
/// relic counts, active Fissure tiers, and (once, up front) mastered-reward
/// part prices for every owned relic (Farm tab) and every owned Prime Part
/// (Ducats tab) — falling back to empty values on failure rather than
/// blocking the GUI from opening at all. Relic-level sell prices and Set
/// prices are fetched lazily instead (see ADR-0012), so `prices.sell`/`.set`
/// on the returned [`LoadedData`] always start empty.
async fn load_data(config: &Config) -> LoadedData {
    // A small random delay before the very first network call (issue #100):
    // many independent installs launching around the same real-world moment
    // (a patch drop, a scheduled restart), or all waking up to a
    // cache-format bump that invalidates every cache at once, would
    // otherwise all fire their first request in the same instant. The
    // window already opens immediately with a "Loading…" placeholder (see
    // this module's docs), so a couple of extra seconds here is invisible.
    tokio::time::sleep(wf_data::poll::startup_delay(STARTUP_JITTER_MAX)).await;

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
    let mem_scanned_parts =
        wf_cache::load_blob::<bool>(wf_relic::owned_parts::OWNED_PARTS_MEM_SCANNED_MARKER_FILE)
            .is_some();
    let owned_rivens: OwnedRivens = wf_cache::load_blob::<OwnedRivens>(wf_relic::OWNED_RIVENS_FILE)
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
        farm_prices = fetch_prices(resolved, &cache, &market, |(name, slug)| (name, slug))
            .await
            .into_iter()
            .map(|(k, (plat, _))| (k, plat))
            .collect();
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
    let ducat_prices = fetch_prices(resolved_owned_parts, &cache, &market, |(name, slug)| (name, slug))
        .await
        .into_iter()
        .map(|(k, (plat, _))| (k, plat))
        .collect();

    cache.save();

    // Rivens-tab pricing: one Floor/Ceiling/Verdict per distinct owned
    // weapon, resolved once at launch like every other owned-driven price
    // above. The weapon catalogue itself (`/v2/riven/weapons`) is small
    // (~400 entries) and fetched fresh each launch rather than disk-cached,
    // same as `active_tiers`'s worldstate fetch above.
    let riven_weapons = wf_data::riven_market::weapon_catalogue(&client).await.unwrap_or_else(|e| {
        tracing::warn!("riven weapon catalogue load failed: {e:#}");
        Vec::new()
    });
    let riven_slug_by_weapon: HashMap<String, String> =
        riven_weapons.into_iter().map(|w| (w.game_ref, w.slug)).collect();
    let riven_market =
        wf_data::riven_market::RivenMarketClient::new(client.clone(), config.market_platform.clone());
    let riven_price_cache = wf_relic::riven_price_cache();
    let distinct_owned_weapons: Vec<String> = {
        let mut seen = HashSet::new();
        owned_rivens
            .iter()
            .filter(|r| seen.insert(r.weapon_unique_name.clone()))
            .map(|r| r.weapon_unique_name.clone())
            .collect()
    };
    let riven_verdicts = fetch_riven_verdicts(
        distinct_owned_weapons,
        &riven_slug_by_weapon,
        &riven_price_cache,
        &riven_market,
    )
    .await;
    riven_price_cache.save();

    let part_market = wf_relic::part_market_info(&quantities, &item_index);

    LoadedData {
        index,
        mastery,
        quantities,
        owned,
        owned_parts,
        mem_scanned_parts,
        owned_rivens,
        active_tiers,
        prices: Prices {
            sell: HashMap::new(),
            farm: farm_prices,
            set: HashMap::new(),
            ducats: ducat_prices,
            riven_verdicts,
        },
        part_market,
        item_index,
    }
}

/// The browser's tabs: Home, Mastery, Relics & Plan, Relics EV, Buy or Farm,
/// Sell, Farm, Ducats, and Owned. Home (not Mastery) is selected on open
/// (#72) — see `BrowseApp::new`. Settings used to be its own tab too, but
/// folded into Home (#77) — this app doesn't have enough options to earn a
/// dedicated destination. Reachable through [`Group`]'s two-tier nav rather
/// than a single flat row (#77) — see `impl eframe::App for BrowseApp`.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Home,
    Mastery,
    Relics,
    RelicsEv,
    BuyOrFarm,
    Sell,
    Farm,
    Rivens,
    Ducats,
    Owned,
}

impl Tab {
    /// Display label — distinct from the variant name so each one says what
    /// it acts on (#77): a *relic* (the thing you crack) or a *Prime Part*
    /// (what a relic drops, that builds a prime). The old flat "Market"
    /// grouping bundled Sell/Farm (relic-level) with Ducats (part-level)
    /// under one unclear label; [`Group::Relics`] now only groups
    /// relic-level tabs, and Ducats/Buy or Farm Parts stay visibly
    /// part-level.
    fn label(&self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Mastery => "Mastery",
            Tab::Relics => "Relic Plan",
            Tab::RelicsEv => "Relic Value",
            Tab::BuyOrFarm => "Buy or Farm Parts",
            Tab::Sell => "Sell Relics",
            Tab::Farm => "Farm Relics",
            Tab::Rivens => "Rivens",
            Tab::Ducats => "Ducats",
            Tab::Owned => "Owned Relics",
        }
    }
}

/// The shell's top-level nav (#77) — five groups instead of the old flat
/// 10-tab row (or the five-group "Home/Plan/Market/Owned/Settings" first
/// pass): Settings folds into Home rather than staying a destination of its
/// own, and `Relics`/`Rivens`/`Ducats` split by *what they act on* rather
/// than being lumped into one "Market" group. `Relics` is every owned-relic
/// action (inventory, sell-whole, crack-for-parts, plus the relic-catalogue
/// reference); `Rivens` is the riven-economy view (decode/price/verdict,
/// per `docs/specs/riven-browse-tab.md` §5 — a riven isn't a Prime Part, so
/// it doesn't fold into `Ducats` despite both being single-tab "is this
/// worth anything" valuation views); `Ducats` is the one Prime-Part
/// (post-crack) action; `Progress` is explicitly "parts you still need."
/// Positioned between `Relics` and `Ducats` (owned-relic economy → riven
/// economy → prime-part economy).
#[derive(Clone, Copy, PartialEq)]
enum Group {
    Home,
    Progress,
    Relics,
    Rivens,
    Ducats,
}

impl Group {
    fn label(&self) -> &'static str {
        match self {
            Group::Home => "Home",
            Group::Progress => "Progress",
            Group::Relics => "Relics",
            Group::Rivens => "Rivens",
            Group::Ducats => "Ducats",
        }
    }

    fn of(tab: Tab) -> Self {
        match tab {
            Tab::Home => Group::Home,
            Tab::Mastery | Tab::Relics | Tab::BuyOrFarm => Group::Progress,
            Tab::Owned | Tab::Sell | Tab::Farm | Tab::RelicsEv => Group::Relics,
            Tab::Rivens => Group::Rivens,
            Tab::Ducats => Group::Ducats,
        }
    }

    fn children(&self) -> &'static [Tab] {
        match self {
            Group::Progress => &[Tab::Mastery, Tab::Relics, Tab::BuyOrFarm],
            Group::Relics => &[Tab::Owned, Tab::Sell, Tab::Farm, Tab::RelicsEv],
            // Single tab, no children — same shape as `Group::Ducats`.
            Group::Home | Group::Rivens | Group::Ducats => &[],
        }
    }

    fn default_tab(&self) -> Tab {
        match self {
            Group::Home => Tab::Home,
            Group::Progress => Tab::Mastery,
            // Opens on the inventory itself, not an action on it.
            Group::Relics => Tab::Owned,
            Group::Rivens => Tab::Rivens,
            Group::Ducats => Tab::Ducats,
        }
    }
}

const GROUPS: [Group; 5] =
    [Group::Home, Group::Progress, Group::Relics, Group::Rivens, Group::Ducats];

/// The Home tab's own sub-nav: general app settings/actions, the
/// drag-to-place overlay mock, and the fissure filter. Split out so
/// Placement's mock screen (260px tall on its own) doesn't force Overview's
/// shorter content to scroll past it too, and so Placement's fields (which
/// already live-apply and save themselves on release) don't need to share a
/// page with the one field that still needs an explicit Save (the account id
/// text box). Fissures gets its own page rather than sharing Placement's
/// since the filter's three checkbox rows are a distinct, growing concern
/// (mission types alone are 13 checkboxes) with nothing placement-related
/// about it beyond both living under [`OverlayConfig`].
#[derive(Clone, Copy, PartialEq)]
enum HomeSubTab {
    Overview,
    Placement,
    Fissures,
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
    /// The Home tab's own sub-nav (see [`HomeSubTab`]).
    home_sub_tab: HomeSubTab,
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
    /// `load_data`'s own (which stays scoped to its background task). Also
    /// what the Home tab's Scan Memory action uses (#72).
    client: reqwest::Client,
    market_platform: String,
    /// The Settings section's own long-lived, mutable `Config` — cloned from
    /// the launch-time config before it was moved into `load_and_poll` (see
    /// `run`'s docs). Edited and saved here the same way the standalone
    /// `wf-settings` crate's `SettingsApp` does; a change here does not
    /// retroactively affect the background loader/poller's own copy.
    config: Config,
    config_path: PathBuf,
    /// Editable text buffer for the Settings section's account id field —
    /// doubles as the Home tab's "account set?" readout (#72), same as
    /// `SettingsApp::account_id`.
    account_id: String,
    /// The Settings section's own status line (Save/Detect/Copy feedback) —
    /// named distinctly from `scan_status` (the Home tab's Scan Memory
    /// result) so the two are never confused.
    settings_status: String,
    /// Whether a Scan Memory task is currently in flight. UI-thread-only
    /// (not shared with the spawned task): set on click, cleared once
    /// `home_tab` observes `scan_status` land a fresh result.
    scanning: bool,
    /// The Home tab's Scan Memory result (#72), written by a task spawned on
    /// `rt_handle` and read every frame `home_tab` renders. `Ok(summary
    /// text)` on success, `Err(display text)` on failure — every `wf_mem`
    /// error propagates via its own `Display` verbatim (e.g. the exact
    /// `sudo setcap cap_sys_ptrace=+ep <path>` hint), never reworded here.
    scan_status: Arc<Mutex<Option<Result<String, String>>>>,
    /// Whether the last `demo-on` we sent actually landed (see
    /// [`Self::sync_demo_mode`]) — drives the Home tab's drag-to-place
    /// preview into showing curated content on a running overlay.
    demo_active: bool,
    /// Set once we've fired an auto-launch attempt for the current "trying
    /// to reach demo mode" stretch, so a still-starting overlay's control
    /// socket not existing yet doesn't spawn a second overlay process on
    /// every retry — see [`Self::sync_demo_mode`].
    overlay_launch_tried: bool,
    /// Whether [`Self::drag_to_place`]'s mock overlay box is currently
    /// being dragged.
    dragging_placement: bool,
    /// Cursor position minus the dragged box's top-left corner, captured on
    /// drag start — see [`Self::drag_to_place`].
    drag_offset: egui::Vec2,
}

impl BrowseApp {
    #[allow(clippy::too_many_arguments)]
    fn new(
        loaded: Arc<Mutex<Option<Loaded>>>,
        wishlist: wf_relic::Wishlist,
        rt_handle: tokio::runtime::Handle,
        client: reqwest::Client,
        market_platform: String,
        relics_plan_relic_prices: LazyPriceMap,
        relics_plan_set_prices: LazyPriceMap,
        config: Config,
        config_path: PathBuf,
    ) -> Self {
        let account_id = config.account_id.clone().unwrap_or_default();
        Self {
            tab: Tab::Home,
            home_sub_tab: HomeSubTab::Overview,
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
            config,
            config_path,
            account_id,
            settings_status: String::new(),
            scanning: false,
            scan_status: Arc::new(Mutex::new(None)),
            demo_active: false,
            overlay_launch_tried: false,
            dragging_placement: false,
            drag_offset: egui::Vec2::ZERO,
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
            let mut prices: HashMap<String, Option<u32>> =
                fetch_prices(resolved, &cache, &market, |(name, slug)| (name, slug))
                    .await
                    .into_iter()
                    .map(|(k, (plat, _))| (k, plat))
                    .collect();
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
        // Consecutive-failure count each key carried into this attempt (0 if
        // never attempted, or if it last resolved successfully) — carried
        // forward so a run of failures keeps backing off across calls to
        // `ensure_lazy_prices` instead of resetting every time (issue #100).
        let mut prev_failures: HashMap<String, u32> = HashMap::new();
        {
            let mut guard = map.lock().unwrap_or_else(|p| p.into_inner());
            for (key, slug) in needed {
                if needs_fetch(guard.get(&key), now) {
                    if let Some(LazyPrice::Ready { consecutive_failures, .. }) = guard.get(&key) {
                        prev_failures.insert(key.clone(), *consecutive_failures);
                    }
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
            for (key, (plat, status)) in resolved {
                let consecutive_failures = match status {
                    wf_relic::FetchStatus::Ok => 0,
                    wf_relic::FetchStatus::Failed => {
                        prev_failures.get(&key).copied().unwrap_or(0) + 1
                    }
                };
                guard.insert(key, LazyPrice::Ready { plat, resolved_at, consecutive_failures });
            }
        });
    }

    /// The Home tab (#72): the default tab on open, replacing the old
    /// Mastery default. Its own sub-nav (see [`HomeSubTab`]) — Overview (Scan
    /// Memory, account id, hotkey help, Save), Placement (the drag-to-place
    /// overlay mock), and Fissures (the fissure-panel filter) — replaces the
    /// single scroll-everything page the Settings section folded into at
    /// #77: that page's drag-to-place mock alone is 260px tall, easily
    /// pushing Overview's shorter content past the window's min height
    /// (760x560) even though the sections have nothing to do with each
    /// other.
    fn home_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Home");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.home_sub_tab, HomeSubTab::Overview, "Overview");
            ui.selectable_value(&mut self.home_sub_tab, HomeSubTab::Placement, "Placement");
            ui.selectable_value(&mut self.home_sub_tab, HomeSubTab::Fissures, "Fissures");
        });
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(8.0);

        egui::ScrollArea::vertical().id_salt("home_scroll").show(ui, |ui| match self.home_sub_tab {
            HomeSubTab::Overview => self.home_overview_tab(ui),
            HomeSubTab::Placement => self.home_placement_tab(ui),
            HomeSubTab::Fissures => self.home_fissures_tab(ui),
        });
    }

    /// The Home tab's Overview page (see [`HomeSubTab`]): account-id status,
    /// the Scan Memory action, Mastery account id (with "Detect from log"),
    /// hotkey-bind help, UI text size, and Save.
    ///
    /// A deliberate Scan Memory click is the map's required consent (see this
    /// module's docs and CONTEXT.md's Notes) — no confirmation dialog, and
    /// deliberately no preflight "is the game running" / "do we have the
    /// capability" check before it: the real scan's own error already
    /// surfaces the right guidance reactively once it lands, exactly like the
    /// CLI (`wf-lite mem-scan`).
    ///
    /// Save here only ever needed to persist the account id text field — every
    /// other field this app can save (placement/opacity/fissures on
    /// [`Self::home_placement_tab`], UI text size below) already commits
    /// itself on change, so a bare click with nothing else pending just
    /// re-saves the same account id and re-pushes the unchanged overlay
    /// settings (harmless, see [`Self::commit_overlay_settings`]).
    fn home_overview_tab(&mut self, ui: &mut egui::Ui) {
        if self.account_id.trim().is_empty() {
            ui.label("Mastery account id: not set — set it below");
        } else {
            ui.label(format!("Mastery account id: {}", self.account_id.trim()));
        }
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(10.0);

        // A landed result clears the in-flight flag the first frame it's
        // observed; `spawn_scan` clears `scan_status` back to `None` the
        // moment it fires a new scan, so this never flashes a stale result
        // while a fresh scan is running.
        let current = self.scan_status.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if self.scanning && current.is_some() {
            self.scanning = false;
        }

        if ui.add_enabled(!self.scanning, egui::Button::new("Scan Memory")).clicked() {
            self.spawn_scan();
        }
        ui.add_space(6.0);

        let line = if self.scanning {
            "scanning…".to_string()
        } else {
            match &current {
                None => "idle — click Scan Memory to read Foundry/Rivens/owned relics from the \
                          running game"
                    .to_string(),
                Some(Ok(summary)) => format!("done: {summary}"),
                Some(Err(e)) => format!("failed: {e}"),
            }
        };
        ui.label(line);

        ui.add_space(14.0);
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
                self.settings_status = "Copied command".to_string();
            }
            if ui.button("Open KDE shortcuts").clicked() {
                open_kde_shortcuts(&mut self.settings_status);
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.label("UI text size");
        let r = ui.add(egui::Slider::new(&mut self.config.ui.font_scale, 0.8..=1.6).text("×"));
        if r.changed() {
            apply_font_scale(ui.ctx(), self.config.ui.font_scale);
        }
        if r.drag_stopped() || r.lost_focus() {
            self.save_config_only();
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                self.save_settings();
            }
            ui.label(egui::RichText::new(&self.settings_status).weak());
        });
    }

    /// The Home tab's Placement page (see [`HomeSubTab`]): the drag-to-place
    /// overlay mock, Opacity, and the Fissure panel toggle — every field here
    /// already live-applies to a running `wf-lite overlay` and saves itself on
    /// commit (drag-release/slider-release/checkbox toggle), so unlike
    /// [`Self::home_overview_tab`] there's no Save button on this page; the
    /// status line below shows the same feedback a button would.
    fn home_placement_tab(&mut self, ui: &mut egui::Ui) {
        let mut commit = false;

        ui.label("Overlay placement — drag the panel; it tracks the cursor except near the \
                   middle of each axis, where it eases into being centered.");
        ui.add_space(4.0);
        commit |= self.drag_to_place(ui);
        ui.add_space(10.0);

        egui::Grid::new("browse_settings_placement")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Opacity");
                let r = ui.add(egui::Slider::new(&mut self.config.overlay.opacity, 0.1..=1.0));
                commit |= r.drag_stopped() || r.lost_focus();
                ui.end_row();

                ui.label("Fissure panel");
                if ui
                    .checkbox(&mut self.config.overlay.fissures, "show (off = reward picker only)")
                    .changed()
                {
                    commit = true;
                }
                ui.end_row();
            });

        if commit {
            self.commit_overlay_settings();
        }

        ui.add_space(10.0);
        ui.label(egui::RichText::new(&self.settings_status).weak());
    }

    /// The Home tab's Fissures page (see [`HomeSubTab`]): which active
    /// fissures the panel shows (e.g. only Axi Capture). Same live-apply/
    /// save-on-change behavior as [`Self::home_placement_tab`] — no Save
    /// button, the status line gives the same feedback.
    fn home_fissures_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Fissure filter — leave a row blank to show every fissure.").weak());
        ui.add_space(8.0);

        let filter = &mut self.config.overlay.fissure_filter;
        let before = filter.clone();
        tier_filter_ui(ui, &mut filter.tiers);
        mission_type_filter_ui(ui, &mut filter.mission_types);
        fissure_kind_filter_ui(ui, &mut filter.kinds);

        if *filter != before {
            self.commit_overlay_settings();
        }

        ui.add_space(10.0);
        ui.label(egui::RichText::new(&self.settings_status).weak());
    }

    /// Fire the Home tab's Scan Memory action (#72) on `rt_handle` — never on
    /// the UI thread. Marks `scanning` immediately and resets `scan_status`
    /// to `None` so `home_tab` shows "scanning…" instead of a stale result
    /// from a previous run while this one is in flight.
    fn spawn_scan(&mut self) {
        self.scanning = true;
        *self.scan_status.lock().unwrap_or_else(|p| p.into_inner()) = None;

        let scan_status = self.scan_status.clone();
        let client = self.client.clone();
        self.rt_handle.spawn(async move {
            let result = run_memory_scan(&client).await;
            *scan_status.lock().unwrap_or_else(|p| p.into_inner()) = Some(result);
        });
    }

    /// The Mastery tab: a 3-level tree — category (fixed WFinfo order) →
    /// Prime → part — matching WFinfo's Equipment window. Both levels are
    /// collapsed by default ([`egui::CollapsingHeader`], no expand/collapse
    /// interaction existed anywhere in `wf-browse` before this); the
    /// Show/Sort controls apply *within* each category, not across them, so
    /// the category order itself never moves.
    fn mastery_tab(&mut self, ui: &mut egui::Ui) {
        let Some((mastery_rows, quantities, part_market, owned_parts, mem_scanned_parts)) =
            self.loaded_or_placeholder(ui, |l| {
                (
                    l.mastery_rows.clone(),
                    l.quantities.clone(),
                    l.part_market.clone(),
                    l.live.owned_parts.clone(),
                    l.live.mem_scanned_parts,
                )
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
        let row_ctx = MasteryRowContext {
            quantities: &quantities,
            part_market: &part_market,
            owned_parts: &owned_parts,
            mem_scanned_parts,
        };

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
                        self.mastery_prime_row(ui, entry, &row_ctx, force_open);
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
        ctx: &MasteryRowContext,
        force_open: Option<bool>,
    ) {
        let MasteryRowContext { quantities, part_market, owned_parts, mem_scanned_parts } = *ctx;
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
                    .num_columns(6)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("part");
                        ui.strong("owned");
                        ui.strong("need");
                        ui.strong("vaulted");
                        ui.strong("ducats");
                        ui.strong("wishlist");
                        ui.end_row();

                        for (part, quantity) in &parts {
                            let pp = PrimePart { prime: entry.prime.clone(), part: part.clone() };
                            ui.label(part);

                            let owned =
                                wf_relic::owned_parts::get_or_confirmed_zero(owned_parts, &pp, mem_scanned_parts);
                            ui.label(owned_count_cell(owned));
                            ui.label(need_count_cell(Some(*quantity)));

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
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("part");
                        ui.strong("owned");
                        ui.strong("need");
                        ui.strong("relics you own that can still drop it");
                        ui.end_row();

                        for g in &p.parts {
                            ui.label(&g.part.part);
                            ui.label(owned_count_cell(g.owned));
                            ui.label(need_count_cell(g.build_quantity));
                            ui.horizontal_wrapped(|ui| {
                                for (i, r) in g.relics.iter().take(RELIC_LIST_MAX_VISIBLE).enumerate() {
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
                                let hidden = g.relics.len().saturating_sub(RELIC_LIST_MAX_VISIBLE);
                                if hidden > 0 {
                                    ui.weak(format!("(+{hidden} more)"));
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
        let Some((index, item_index, quantities, owned_parts, mem_scanned_parts)) =
            self.loaded_or_placeholder(ui, |l| {
                (
                    l.index.clone(),
                    l.item_index.clone(),
                    l.quantities.clone(),
                    l.live.owned_parts.clone(),
                    l.live.mem_scanned_parts,
                )
            })
        else {
            return;
        };
        let ctx = RelicEvContext {
            item_index: &item_index,
            quantities: &quantities,
            owned_parts: &owned_parts,
            mem_scanned_parts,
        };

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
        let RelicEvContext { item_index, quantities, owned_parts, mem_scanned_parts } = *ctx;
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
                    .num_columns(5)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("reward");
                        ui.strong("ducats");
                        ui.strong("plat");
                        ui.strong("owned");
                        ui.strong("need");
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
                            let part_owned =
                                wf_relic::owned_parts::get_or_confirmed_zero(owned_parts, &pp, mem_scanned_parts);
                            let need = quantities.get(&pp);
                            let short = need.is_some_and(|n| part_owned.unwrap_or(0) < n);
                            let owned_cell = owned_count_cell(part_owned);
                            let need_cell = need_count_cell(need);
                            if short {
                                ui.colored_label(STALE_COLOR, owned_cell);
                                ui.colored_label(STALE_COLOR, need_cell);
                            } else {
                                ui.weak(owned_cell);
                                ui.weak(need_cell);
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
                        .num_columns(4)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("part");
                            ui.strong("owned");
                            ui.strong("need");
                            ui.strong("cheapest relic");
                            ui.end_row();

                            for g in &p.gaps {
                                let relic = g
                                    .relics
                                    .first()
                                    .map(|r| {
                                        let stale = stale_marker(&ages, &r.relic_display);
                                        format!("{} ({}){stale}", r.relic_display, plat_str(r.plat))
                                    })
                                    .unwrap_or_else(|| "—".to_string());
                                ui.label(&g.part.part);
                                ui.label(owned_count_cell(g.owned));
                                ui.label(need_count_cell(g.build_quantity));
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
            egui::Grid::new("sell_grid").num_columns(8).striped(true).show(ui, |ui| {
                ui.strong("relic");
                ui.strong("owned");
                ui.strong("plat");
                ui.strong("unmastered");
                ui.strong("worst-off part");
                ui.strong("part owned");
                ui.strong("part need");
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
                    ui.label(worst_off_part_cell(&p.parts_owned));
                    ui.label(p.parts_owned.as_ref().map(|s| owned_count_cell(s.owned)).unwrap_or_default());
                    ui.label(p.parts_owned.as_ref().map(|s| need_count_cell(s.need)).unwrap_or_default());
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
            egui::Grid::new("ducats_grid").num_columns(6).striped(true).show(ui, |ui| {
                ui.strong("part");
                ui.strong("owned");
                ui.strong("need");
                ui.strong("ducats");
                ui.strong("plat");
                ui.strong("efficiency");
                ui.end_row();

                for p in &rows {
                    ui.label(format!("{} {}", p.part.prime, p.part.part));
                    ui.label(p.owned.to_string());
                    ui.label(need_count_cell(p.build_quantity));
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

    /// The riven browse tab (`docs/specs/riven-browse-tab.md`): every
    /// Unveiled riven grouped by owned weapon, one collapsing section per
    /// weapon stating Floor/Ceiling/Verdict once (a weapon-level fact, not
    /// per-copy — see §3/§4 and CONTEXT.md's Verdict entry), with each owned
    /// copy's decoded stats nested underneath. Real production code — not
    /// the throwaway `prototype/riven-tab-layout-99` branch's mock-data
    /// variant switcher (issue #99), though the layout itself (Variant C,
    /// the winning prototype) is unchanged.
    fn rivens_tab(&mut self, ui: &mut egui::Ui) {
        let Some(groups) = self.loaded_or_placeholder(ui, |l| l.live.riven_groups.clone()) else {
            return;
        };

        if groups.is_empty() {
            ui.label(NO_OWNED_RIVENS_MSG);
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            for group in &groups {
                let header_text = match &group.verdict {
                    Some(v) => format!(
                        "{}  —  Floor {} · Ceiling {} · {}  ({} owned)",
                        group.weapon_name,
                        floor_str(v.floor),
                        ceiling_str(v.ceiling, v.ceiling_low_confidence),
                        verdict_label(v.verdict),
                        group.rivens.len()
                    ),
                    None => format!(
                        "{}  —  price not yet fetched  ({} owned)",
                        group.weapon_name,
                        group.rivens.len()
                    ),
                };
                let color = group.verdict.map(|v| verdict_color(v.verdict)).unwrap_or(UNMASTERED_COLOR);

                egui::CollapsingHeader::new(egui::RichText::new(header_text).color(color))
                    .default_open(true)
                    .id_salt(&group.weapon_unique_name)
                    .show(ui, |ui| {
                        egui::Grid::new(format!("riven_grid_{}", group.weapon_unique_name))
                            .num_columns(3)
                            .striped(true)
                            .show(ui, |ui| {
                                ui.strong("stats");
                                ui.strong("polarity");
                                ui.strong("mastery / rank / rerolls");
                                ui.end_row();
                                for r in &group.rivens {
                                    ui.label(stat_line(&r.stats));
                                    ui.label(r.polarity.as_ref().map(|p| p.display_name()).unwrap_or("—"));
                                    ui.label(format!(
                                        "{} · R{}/8 · {} rerolls",
                                        r.mastery_req.map(|v| format!("MR{v}")).unwrap_or_else(|| "—".to_string()),
                                        r.rank,
                                        r.rerolls
                                    ));
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(4.0);
            }
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

    /// Draw the mock screen + draggable overlay-panel box, updating
    /// `self.config.overlay.{anchor,margin_x,margin_y}` live every frame
    /// while dragging (local UI state only — no control-socket push here;
    /// see `commit_overlay_settings` for the on-release/commit push, matching
    /// the map's already-settled "commit, not per-frame" wire behavior).
    /// Returns whether a drag just ended, so callers can fold it into their
    /// own `commit` flag the same way the Opacity slider's `drag_stopped`
    /// already does.
    fn drag_to_place(&mut self, ui: &mut egui::Ui) -> bool {
        let mock_screen_size = egui::vec2(420.0, 260.0);
        let (screen, _) = ui.allocate_exact_size(mock_screen_size, egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(screen, 4, egui::Color32::from_gray(28));
        painter.rect_stroke(screen, 4, egui::Stroke::new(1.0, egui::Color32::from_gray(70)), egui::StrokeKind::Inside);

        // Stored margins are fractions of the max meaningful inset (see
        // `mock_max_margin`'s docs), not pixels — grow them back into the
        // mock's own pixel space to place the idle box, and shrink a
        // mock-space drag's derived pixel offset back down to a fraction
        // before storing it.
        let max = mock_max_margin(mock_screen_size, PLACE_BOX_SIZE);
        let idle_min = place_box(
            screen,
            PLACE_BOX_SIZE,
            &self.config.overlay.anchor,
            self.config.overlay.margin_x.clamp(0.0, 1.0) * max.x,
            self.config.overlay.margin_y.clamp(0.0, 1.0) * max.y,
        );
        let idle_rect = egui::Rect::from_min_size(idle_min, PLACE_BOX_SIZE);

        let rect = if let Some(pointer) = ui.ctx().pointer_latest_pos().filter(|_| self.dragging_placement) {
            let corner = pointer - self.drag_offset;
            let (rx, x_pin) = magnetic_axis(corner.x, screen.left(), screen.right(), PLACE_BOX_SIZE.x);
            let (ry, y_pin) = magnetic_axis(corner.y, screen.top(), screen.bottom(), PLACE_BOX_SIZE.y);
            let anchor = anchor_for(x_pin, y_pin);
            self.config.overlay.anchor = anchor.to_string();
            self.config.overlay.margin_x = (match x_pin {
                AxisPin::Neg => rx - screen.left(),
                AxisPin::Pos => screen.right() - (rx + PLACE_BOX_SIZE.x),
                AxisPin::Centered => 0.0,
            } / max.x)
                .clamp(0.0, 1.0);
            self.config.overlay.margin_y = (match y_pin {
                AxisPin::Neg => ry - screen.top(),
                AxisPin::Pos => screen.bottom() - (ry + PLACE_BOX_SIZE.y),
                AxisPin::Centered => 0.0,
            } / max.y)
                .clamp(0.0, 1.0);
            egui::Rect::from_min_size(egui::pos2(rx, ry), PLACE_BOX_SIZE)
        } else {
            idle_rect
        };

        let (fill, stroke) = if self.dragging_placement {
            (egui::Color32::from_rgba_unmultiplied(60, 130, 125, 190), egui::Color32::from_rgb(140, 230, 220))
        } else {
            (egui::Color32::from_rgb(45, 95, 92), egui::Color32::from_gray(200))
        };
        let painter = ui.painter();
        painter.rect_filled(rect, 6, fill);
        painter.rect_stroke(rect, 6, egui::Stroke::new(1.5, stroke), egui::StrokeKind::Outside);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Overlay", egui::FontId::proportional(12.0), egui::Color32::WHITE);

        let response = ui.interact(rect, ui.id().with("browse_settings_drag_box"), egui::Sense::click_and_drag());
        if response.drag_started() {
            self.dragging_placement = true;
            let pointer = response.interact_pointer_pos().unwrap_or(rect.center());
            self.drag_offset = pointer - rect.min;
        }
        let stopped = response.drag_stopped();
        if stopped {
            self.dragging_placement = false;
        }

        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "anchor = {}   margin_x = {:.0}%   margin_y = {:.0}%",
                self.config.overlay.anchor,
                self.config.overlay.margin_x.clamp(0.0, 1.0) * 100.0,
                self.config.overlay.margin_y.clamp(0.0, 1.0) * 100.0
            ))
            .weak()
            .monospace(),
        );

        if self.dragging_placement {
            ui.ctx().request_repaint();
        }
        stopped
    }

    /// Keep the running overlay's demo mode in sync with whether the
    /// Home / Placement sub-tab (where the drag-to-place preview lives —
    /// this app folded its old standalone Settings destination into Home,
    /// #77) is *actually* focused: that sub-tab being selected *and* the
    /// `wf-browse` window having real OS-level input focus, not just being
    /// the last-selected tab. Tab selection alone isn't enough — leaving
    /// `wf-browse` sitting in the background on Placement while playing
    /// (the common case: adjust placement, then alt-tab into the game)
    /// would otherwise never register as "left", leaving demo mode stuck
    /// on and burying the real fissure/reward panels under curated content
    /// indefinitely. Gated on Placement specifically (not all of Home) so
    /// Overview and Fissures — which don't preview the overlay — don't
    /// needlessly swap live gameplay content out for demo content. Entering
    /// shows real curated content for a true WYSIWYG preview, leaving
    /// resumes live data. Best-effort and idempotent: called every frame,
    /// it only actually sends a command when the desired state changes
    /// from what we've last confirmed. If a `demo-on` fails because no
    /// overlay is running yet, auto-launches one detached instance (once
    /// per "trying to reach demo mode" stretch, not once per failed frame)
    /// and keeps retrying — covers both "no overlay at all" and "overlay
    /// still starting, control socket not bound yet".
    fn sync_demo_mode(&mut self, ctx: &egui::Context) {
        // `None` (focus unknown on this backend) defaults to "not focused"
        // rather than "focused" — if we can't tell, err toward not
        // clobbering live gameplay with demo content over a false positive.
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        let want_active =
            self.tab == Tab::Home && self.home_sub_tab == HomeSubTab::Placement && focused;
        if want_active == self.demo_active {
            return;
        }
        if want_active {
            match wf_config::control::send_command(wf_config::control::DEMO_ON_CMD) {
                Ok(()) => {
                    self.demo_active = true;
                    self.overlay_launch_tried = false;
                }
                Err(_) if !self.overlay_launch_tried => {
                    self.overlay_launch_tried = true;
                    self.launch_overlay_detached();
                }
                Err(_) => {}
            }
        } else {
            let _ = wf_config::control::send_command(wf_config::control::DEMO_OFF_CMD);
            self.demo_active = false;
            self.overlay_launch_tried = false;
        }
    }

    /// Fire-and-forget `<self> overlay`, left running after this process
    /// exits — the same re-exec mechanism `wf-tray`'s `spawn_self`/
    /// `run_detached` use to auto-start the overlay, duplicated here since
    /// those are private to that crate and this is a two-line spawn.
    fn launch_overlay_detached(&self) {
        let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("wf-lite"));
        match std::process::Command::new(&bin).arg("overlay").spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => tracing::error!("could not launch overlay: {e}"),
        }
    }

    /// Persist the account id text buffer into `self.config`, then delegate
    /// to [`Self::commit_overlay_settings`] for the save + live-apply push —
    /// so a bare "Save" click and a placement/opacity/fissures commit both
    /// go through the same path.
    fn save_settings(&mut self) {
        self.config.account_id = if self.account_id.trim().is_empty() {
            None
        } else {
            Some(self.account_id.trim().to_string())
        };
        self.commit_overlay_settings();
    }

    /// Persist `self.config` to `self.config_path` only — no control-socket
    /// push, unlike [`Self::commit_overlay_settings`]. For fields the overlay
    /// doesn't care about (currently just UI text size): a running overlay
    /// draws its own fixed text, so there's nothing there to live-apply.
    fn save_config_only(&mut self) {
        match self.config.save(&self.config_path) {
            Ok(()) => self.settings_status = "Saved".to_string(),
            Err(e) => self.settings_status = format!("Save failed: {e:#}"),
        }
    }

    /// Persist `self.config` to `self.config_path`, then push its
    /// anchor/margin/opacity/fissures fields to a running `wf-lite overlay`
    /// over the control socket (see `wf_config::control`) — live-apply and
    /// save are the same action here (wayfinder map #82), so every commit
    /// that touches those fields does both. The push is best-effort: no
    /// overlay running just leaves a status line saying so, it doesn't fail
    /// the save.
    fn commit_overlay_settings(&mut self) {
        match self.config.save(&self.config_path) {
            Ok(()) => {
                let live = wf_config::control::LiveOverlaySettings::from(&self.config.overlay);
                let cmd = wf_config::control::format_apply_settings(&live);
                match wf_config::control::send_command(&cmd) {
                    Ok(()) => self.settings_status = "Saved & applied".to_string(),
                    Err(e) => self.settings_status = format!("Saved (not applied: {e:#})"),
                }
            }
            Err(e) => self.settings_status = format!("Save failed: {e:#}"),
        }
    }

    /// Run `<self> detect-account` (re-execing this process's own binary),
    /// then reload `self.config` so the detected id appears in the field —
    /// the same re-exec-and-reload pattern `wf-settings`'
    /// `SettingsApp::detect_account` uses.
    fn detect_account(&mut self) {
        let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("wf-lite"));
        let out = std::process::Command::new(&bin).arg("detect-account").output();
        match out {
            Ok(o) if o.status.success() => {
                self.config = Config::load(&self.config_path).unwrap_or_default();
                self.account_id = self.config.account_id.clone().unwrap_or_default();
                self.settings_status = "Detected and saved account id".to_string();
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let msg = err
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| "see terminal for details".to_string());
                self.settings_status = format!("Detect failed: {msg}");
            }
            Err(e) => self.settings_status = format!("Couldn't run {}: {e}", bin.display()),
        }
    }
}

/// Best-effort: open KDE's global-shortcuts settings module — ported from
/// `wf-settings` (#72).
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

/// The Home tab's Scan Memory action's actual work (see
/// [`BrowseApp::spawn_scan`]), factored out as a plain async fn so its
/// error path is a single `?`-chain. Runs the same pipeline
/// `wf-lite mem-scan` does — `wf_mem::scan_and_fetch` →
/// `wf_mem::parse_rivens`/`parse_owned_relics`/`parse_owned_parts` →
/// `wf_relic::RivenCatalogue::load_cached`/`wf_relic::RelicNameIndex::load_cached`/`wf_relic::PartQuantities::load_cached` →
/// `wf_mem::write_owned_rivens`/`write_owned_relics`/`write_owned_parts` (the shared
/// decode+snapshot+apply+save logic, #72/#81) — refreshing
/// `rivens.json`/`owned-relics.json`/`owned-prime-parts.json` so the Rivens/
/// Relics & Plan/Sell/Farm/Mastery tabs' existing [`POLL_INTERVAL`] refresh
/// picks them up without a restart.
///
/// Every `wf_mem`/`wf_relic` error propagates via its own `Display`
/// verbatim (`{e:#}`, matching this crate's and `wf-lite`'s existing
/// convention for inline error text) — never reworded — so a missing
/// `cap_sys_ptrace` grant surfaces the exact same `sudo setcap
/// cap_sys_ptrace=+ep <path>` guidance the CLI shows. A failed *rivens* or
/// *parts* write specifically doesn't fail the whole scan (unlike relics') —
/// each builds its own status line instead of erroring, so a `rivens.json`
/// or `owned-prime-parts.json` write hiccup doesn't discard an otherwise-
/// successful relic write.
async fn run_memory_scan(client: &reqwest::Client) -> Result<String, String> {
    let raw = wf_mem::scan_and_fetch(client).await.map_err(|e| format!("{e:#}"))?;

    let rivens = wf_mem::parse_rivens(&raw).map_err(|e| format!("{e:#}"))?;
    let riven_catalogue = wf_relic::RivenCatalogue::load_cached(client, CATALOGUE_TTL)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("riven catalogue load failed ({e:#}); owned rivens shown undecoded");
            wf_relic::RivenCatalogue::empty()
        });
    let rivens_report = wf_mem::write_owned_rivens(&rivens, &riven_catalogue);
    let rivens_line = if rivens_report.saved {
        format!(
            "wrote {} riven entries to {} ({} unrecognized weapon, undecoded)",
            rivens_report.written,
            wf_relic::OWNED_RIVENS_FILE,
            rivens_report.undecoded
        )
    } else {
        format!("failed to write {}", wf_relic::OWNED_RIVENS_FILE)
    };

    let relics = wf_mem::parse_owned_relics(&raw).map_err(|e| format!("{e:#}"))?;
    let relic_names = wf_relic::RelicNameIndex::load_cached(client, CATALOGUE_TTL)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("relic name index load failed ({e:#}); owned relics shown undecoded");
            wf_relic::RelicNameIndex::empty()
        });
    let relics_report = wf_mem::write_owned_relics(&relics, &relic_names);
    if !relics_report.saved {
        return Err(format!("scanned successfully but failed to write {}", wf_relic::OWNED_RELICS_FILE));
    }

    let parts = wf_mem::parse_owned_parts(&raw).map_err(|e| format!("{e:#}"))?;
    let quantities = wf_relic::PartQuantities::load_cached(client, CATALOGUE_TTL)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("part quantities load failed ({e:#}); owned parts shown undecoded");
            wf_relic::PartQuantities::empty()
        });
    let parts_report = wf_mem::write_owned_parts(&parts, &quantities);
    let parts_line = if parts_report.saved {
        format!(
            "; wrote {} part entries to {} ({} non-Prime/unrecognized, skipped)",
            parts_report.written,
            wf_relic::OWNED_PRIME_PARTS_FILE,
            parts_report.skipped
        )
    } else {
        format!("; failed to write {}", wf_relic::OWNED_PRIME_PARTS_FILE)
    };

    Ok(format!(
        "{rivens_line}; wrote {} relic entries to {} ({} undecoded, skipped){parts_line}",
        relics_report.written,
        wf_relic::OWNED_RELICS_FILE,
        relics_report.undecoded
    ))
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

/// Rivens tab: Floor price cell — see CONTEXT.md's Floor price entry. `None`
/// only when there are zero price-bearing listings (see
/// [`wf_relic::RivenTypeVerdict::floor`]'s doc); the low-listing-count
/// abstain case is communicated by the Verdict label sitting right next to
/// this, not by a separate caveat on the number itself (spec §3.3).
fn floor_str(floor: Option<u32>) -> String {
    floor.map(|p| format!("{p}p")).unwrap_or_else(|| "—".to_string())
}

/// Rivens tab: Ceiling price cell — see CONTEXT.md's Ceiling price entry.
/// Unlike Floor, a thin sample is flagged inline (`low_confidence`) rather
/// than just relying on the Verdict label, since Ceiling is informational
/// upside rather than the number the Verdict is derived from (spec §3.3).
fn ceiling_str(ceiling: Option<u32>, low_confidence: bool) -> String {
    match ceiling {
        None => "—".to_string(),
        Some(p) if low_confidence => format!("{p}p (low confidence)"),
        Some(p) => format!("{p}p"),
    }
}

fn verdict_label(v: RivenVerdict) -> &'static str {
    match v {
        RivenVerdict::LikelyKeep => "likely keep",
        RivenVerdict::LikelyDissolve => "likely dissolve/transmute",
        RivenVerdict::InsufficientData => "insufficient data",
    }
}

/// Mirrors the `/prototype`-validated palette from issue #99's winning
/// variant: [`MASTERED_COLOR`] for a real "keep" recommendation,
/// [`UNMASTERED_COLOR`] (neutral gray) for an abstain, and a dedicated muted
/// red for "likely dissolve/transmute" — distinct from both, since it's an
/// actionable negative signal, not a neutral unknown.
fn verdict_color(v: RivenVerdict) -> egui::Color32 {
    match v {
        RivenVerdict::LikelyKeep => MASTERED_COLOR,
        RivenVerdict::LikelyDissolve => egui::Color32::from_rgb(210, 100, 90),
        RivenVerdict::InsufficientData => UNMASTERED_COLOR,
    }
}

/// One riven's decoded stats as a single comma-joined display line, e.g.
/// `"+45.7% Crit Chance, -12.3% Recoil"`.
fn stat_line(stats: &[wf_relic::DecodedStat]) -> String {
    if stats.is_empty() {
        return "—".to_string();
    }
    stats.iter().map(stat_display).collect::<Vec<_>>().join(", ")
}

fn stat_display(s: &wf_relic::DecodedStat) -> String {
    let label = stat_tag_label(&s.tag);
    if s.is_multiplier {
        format!("{:.2}x {label}", s.value)
    } else if s.is_non_percentage {
        format!("{:+.1} {label}", s.value)
    } else if s.value >= 0.0 {
        format!("+{:.1}% {label}", s.value)
    } else {
        format!("{:.1}% {label}", s.value)
    }
}

/// A riven stat tag (e.g. `"WeaponCritChanceMod"`) to a readable label
/// (`"Crit Chance"`) — strips the common `Weapon`/`WeaponMelee` prefix and
/// `Mod` suffix, then camelCase-splits what's left. Not locTag-driven (see
/// `docs/specs/riven-browse-tab.md` §1's implementation-step note): a small,
/// local, good-enough label rather than pulling `Mods.json`'s `locTag`
/// templates and stripping their DE color-tag markup.
fn stat_tag_label(tag: &str) -> String {
    let stripped =
        tag.strip_prefix("WeaponMelee").or_else(|| tag.strip_prefix("Weapon")).unwrap_or(tag);
    let stripped = stripped.strip_suffix("Mod").unwrap_or(stripped);
    let spaced = camel_case_split(stripped);
    if spaced.trim().is_empty() {
        tag.to_string()
    } else {
        spaced
    }
}

/// Insert a space before each uppercase letter that follows a lowercase
/// letter or digit, e.g. `"CritChance"` -> `"Crit Chance"`. Mirrors
/// `wf-lite`'s `readable_item_name`/`wf_relic::mastery`'s own
/// camelCase-splitting convention, duplicated here for the same reason
/// those two duplicate each other — no shared crate this app's binary,
/// `wf-relic`, and `wf-browse` could all depend on for one tiny helper.
fn camel_case_split(leaf: &str) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in leaf.chars() {
        if c.is_uppercase() && prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit()) {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
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

/// How many of a Prime Part the player owns, for its own grid column. Never
/// renders a bare `0` for an unscanned part — an absent entry means "never
/// scanned" (owned-part counts currently come only from the opt-in OCR
/// scanner, off by default — see issue #78), not "confirmed zero owned"
/// (ADR-0011's never-guess-an-unknown-quantity precedent). Spelling that out
/// as `"not scanned"` rather than a bare `—` is deliberate: a lone dash next
/// to real numbers reads too easily as zero.
fn owned_count_cell(owned: Option<u32>) -> String {
    owned.map(|o| o.to_string()).unwrap_or_else(|| "not scanned".to_string())
}

/// How many of a Prime Part a full build needs, for its own grid column. A
/// plain number — no `x` prefix, since the column header already says
/// "need". `None` means the build-quantity catalogue has no entry for this
/// part (rare; see ADR-0011), not that zero are needed.
fn need_count_cell(need: Option<u32>) -> String {
    need.map(|q| q.to_string()).unwrap_or_else(|| "—".to_string())
}

/// The Sell tab's `part` column: the relic's worst-off unmastered Prime Part
/// (see [`wf_relic::PrimePartGroup`]'s sibling, `wf_relic::RelicPick::parts_owned`),
/// e.g. `"Receiver"` or `"Receiver (+2 more)"` when other unmastered parts
/// remain besides the one shown. Empty for a relic with no unmastered
/// rewards at all. Owned/need counts for this part render in their own
/// columns via [`owned_count_cell`]/[`need_count_cell`].
fn worst_off_part_cell(summary: &Option<wf_relic::PartsOwnedSummary>) -> String {
    let Some(s) = summary else { return String::new() };
    let more = if s.more > 0 { format!(" (+{} more)", s.more) } else { String::new() };
    format!("{}{more}", s.part.part)
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

        self.sync_demo_mode(ui.ctx());

        egui::CentralPanel::default().show(ui, |ui| {
            let current_group = Group::of(self.tab);

            // Primary row: larger, so it reads as the main nav. No
            // `.strong()` here — `apply_theme` points strong text at the
            // same ACCENT color `selectable_label` fills the selected tab
            // with, so a selected "strong" label would render as invisible
            // teal-on-teal text (hit this exact bug while prototyping).
            ui.horizontal(|ui| {
                for g in GROUPS {
                    let text = egui::RichText::new(g.label()).size(16.0);
                    if ui.selectable_label(current_group == g, text).clicked() {
                        self.tab = g.default_tab();
                    }
                    ui.add_space(6.0);
                }
            });

            // Sub-tab row: smaller, nested in a darker band, only shown for
            // a group that actually has children — so the two nav tiers are
            // told apart at a glance instead of reading as one flat row of
            // same-weight buttons (#77).
            if !current_group.children().is_empty() {
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for &child in current_group.children() {
                                let text = egui::RichText::new(child.label()).size(12.5);
                                if ui.selectable_label(self.tab == child, text).clicked() {
                                    self.tab = child;
                                }
                            }
                        });
                    });
            }
            ui.separator();

            match self.tab {
                Tab::Home => self.home_tab(ui),
                Tab::Mastery => self.mastery_tab(ui),
                Tab::Relics => self.relics_tab(ui),
                Tab::RelicsEv => self.relics_ev_tab(ui),
                Tab::BuyOrFarm => self.buy_or_farm_tab(ui),
                Tab::Sell => self.sell_tab(ui),
                Tab::Farm => self.farm_tab(ui),
                Tab::Rivens => self.rivens_tab(ui),
                Tab::Ducats => self.ducats_tab(ui),
                Tab::Owned => self.owned_tab(ui),
            }
        });
    }

    /// `sync_demo_mode` only turns demo mode back off from inside `ui`
    /// (called every frame while the window is open) when focus/tab state
    /// changes — closing the window stops those frames from ever running
    /// again, so without this, a demo-on left active right before close
    /// (the common case: the Home tab is still focused when you click the
    /// close button) would strand the overlay showing curated content
    /// forever, with nothing left running that could ever send `demo-off`.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.demo_active {
            let _ = wf_config::control::send_command(wf_config::control::DEMO_OFF_CMD);
        }
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
            Config::default(),
            PathBuf::from("config.toml"),
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
        let ready = LazyPrice::Ready { plat: Some(42), resolved_at, consecutive_failures: 0 };
        assert!(!needs_fetch(Some(&ready), Instant::now()));
    }

    #[test]
    fn needs_fetch_waits_out_the_cooldown_after_a_no_listing_result() {
        let resolved_at = Instant::now();
        let ready = LazyPrice::Ready { plat: None, resolved_at, consecutive_failures: 0 };
        assert!(!needs_fetch(Some(&ready), resolved_at + LAZY_PRICE_RETRY_COOLDOWN / 2));
    }

    #[test]
    fn needs_fetch_retries_a_no_listing_result_once_the_cooldown_elapses() {
        let resolved_at = Instant::now();
        let ready = LazyPrice::Ready { plat: None, resolved_at, consecutive_failures: 0 };
        assert!(needs_fetch(Some(&ready), resolved_at + LAZY_PRICE_RETRY_COOLDOWN));
    }

    #[test]
    fn needs_fetch_doubles_the_cooldown_after_one_failure() {
        // consecutive_failures: 1 already doubles the cooldown to 90s
        // (backoff_interval doubles per failure starting at the first one).
        let resolved_at = Instant::now();
        let ready = LazyPrice::Ready { plat: None, resolved_at, consecutive_failures: 1 };
        assert!(!needs_fetch(Some(&ready), resolved_at + LAZY_PRICE_RETRY_COOLDOWN));
        assert!(needs_fetch(Some(&ready), resolved_at + LAZY_PRICE_RETRY_COOLDOWN * 2));
    }

    #[test]
    fn needs_fetch_backs_off_further_on_a_longer_failure_streak() {
        // Issue #100: a sustained run of failures (e.g. a 429 rate limit)
        // must not be retried at the same fixed 45s cadence forever.
        let resolved_at = Instant::now();
        let ready = LazyPrice::Ready { plat: None, resolved_at, consecutive_failures: 3 };
        // 45s * 2^3 = 360s — still within cooldown just past the old flat 45s.
        assert!(!needs_fetch(Some(&ready), resolved_at + LAZY_PRICE_RETRY_COOLDOWN * 2));
        assert!(needs_fetch(Some(&ready), resolved_at + Duration::from_secs(360)));
    }

    #[test]
    fn needs_fetch_backoff_is_capped() {
        let resolved_at = Instant::now();
        let ready = LazyPrice::Ready { plat: None, resolved_at, consecutive_failures: 30 };
        assert!(!needs_fetch(Some(&ready), resolved_at + LAZY_PRICE_RETRY_CAP - Duration::from_secs(1)));
        assert!(needs_fetch(Some(&ready), resolved_at + LAZY_PRICE_RETRY_CAP));
    }

    #[test]
    fn snapshot_prices_treats_loading_the_same_as_absent() {
        let now = Instant::now();
        let map: LazyPriceMap = Arc::new(Mutex::new(HashMap::from([
            (
                "resolved".to_string(),
                LazyPrice::Ready { plat: Some(15), resolved_at: now, consecutive_failures: 0 },
            ),
            (
                "empty".to_string(),
                LazyPrice::Ready { plat: None, resolved_at: now, consecutive_failures: 0 },
            ),
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
            (
                "no_listing".to_string(),
                LazyPrice::Ready { plat: None, resolved_at: now, consecutive_failures: 0 },
            ),
            (
                "priced".to_string(),
                LazyPrice::Ready { plat: Some(7), resolved_at: now, consecutive_failures: 0 },
            ),
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
            LazyPrice::Ready { plat: Some(30), resolved_at: Instant::now(), consecutive_failures: 0 },
        )])));
        assert_eq!(lazy_price_str(&map, "axi_h3_relic"), "30p");
    }
}
