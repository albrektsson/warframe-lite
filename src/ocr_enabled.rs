//! Real implementation of `wf-lite`'s OCR-dependent commands and the live
//! overlay's reward-picker auto-detection, compiled in when the `ocr` cargo
//! feature is on (see the root `Cargo.toml`'s `ocr` feature and README.md).
//! This module and [`crate`]'s `ocr_disabled.rs` (picked via `#[cfg]`/
//! `#[path]` in `main.rs`) expose the same public surface — the disabled
//! variant is a friendly "not compiled in" stand-in — so callers (dispatch
//! in `main`, `run_overlay`) never need to know which one they got.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use wf_config::Config;
use wf_data::market::MarketClient;

use crate::{load_mastery, print_reward_table, reward_rows, RelicScanStatus, RewardState, CATALOGUE_TTL};

/// Build the reward-screen candidate-centre geometry, applying any
/// `[overlay]` config overrides (see issue #6) on top of the built-in
/// calibration.
fn reward_regions(config: &Config) -> wf_relic::RewardRegions {
    let mut regions = wf_relic::RewardRegions::default_calibration();
    if let Some(pitch) = config.overlay.reward_pitch {
        regions.pitch = pitch;
    }
    if let Some(center_x) = config.overlay.reward_center_x {
        regions.center_x = center_x;
    }
    regions
}

/// Run the full reward pipeline on a saved PNG (calibration/validation):
/// OCR the candidate slots, show each slot's text, pick the real rewards, and
/// rank them. Usage: `wf-lite relic-file <path>`.
pub(crate) async fn relic_file(config: &Config) -> Result<()> {
    let path = std::env::args()
        .nth(2)
        .context("usage: relic-file <path-to-reward.png>")?;
    println!("\n== Relic file: {path} ==");
    let image = image::open(&path)
        .with_context(|| format!("opening {path}"))?
        .to_rgba8();

    let client = wf_data::http_client();
    let index =
        wf_relic::ItemIndex::load_cached(&client, CATALOGUE_TTL, &config.market_platform).await?;
    let regions = reward_regions(config);
    let ocr = wf_ocr::Ocr::new()?;

    let slots = ocr_regions(&image, &ocr, &regions);
    for (i, s) in slots.iter().enumerate() {
        let m = index.best_match(s);
        println!(
            "  slot {i}: {:<32} -> {}",
            format!("{:?}", s),
            m.map(|m| format!("{} ({:.0}%)", m.item.name, m.score * 100.0))
                .unwrap_or_else(|| "—".into())
        );
    }
    let names = wf_relic::select_rewards(&slots, &index);
    println!("  selected {} rewards: {names:?}", names.len());
    if names.len() < 2 {
        return Ok(());
    }

    let market = MarketClient::new(client.clone(), config.market_platform.clone());
    let cache = wf_relic::price_cache();
    let vaulted = crate::load_vaulted(&client, &index).await;
    let evals = wf_relic::evaluate_cached(
        &names,
        &index,
        &market,
        &cache,
        wf_relic::PriceOpts::default(),
        &vaulted,
    )
    .await;
    let mastery = load_mastery(config, &client).await;
    print_reward_table(&evals, &mastery);
    Ok(())
}

/// Normalized (mean-subtracted, unit-norm) grayscale of the bundled "unowned"
/// eye-icon template, plus its dimensions. Decoded once.
fn eye_template() -> &'static (Vec<f32>, usize, usize) {
    static T: std::sync::OnceLock<(Vec<f32>, usize, usize)> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let img = image::load_from_memory(include_bytes!("../assets/relic-unowned-eye.png"))
            .expect("bundled eye template decodes")
            .to_luma8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let vals: Vec<f32> = img.pixels().map(|p| p[0] as f32).collect();
        let mean = vals.iter().sum::<f32>() / vals.len() as f32;
        let centered: Vec<f32> = vals.iter().map(|v| v - mean).collect();
        let norm = centered.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
        (centered.iter().map(|v| v / norm).collect(), w, h)
    })
}

/// Peak brightness-invariant (normalized cross-correlation) match of the "unowned"
/// eye template anywhere in the `eye` search window. NCC is contrast-invariant (a
/// card brightens on hover, but the eye persists) and the window tolerates
/// column-pitch drift. `1.0` = perfect match.
fn eye_ncc(image: &image::RgbaImage, eye: &wf_relic::Rect) -> f32 {
    let (tmpl, tw, th) = eye_template();
    let (rw, rh) = (eye.w as usize, eye.h as usize);
    if rw < *tw || rh < *th {
        return -1.0;
    }
    // Luma of the search window (clamped to the frame).
    let mut luma = vec![0f32; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let px = image
                .get_pixel_checked(eye.x + x as u32, eye.y + y as u32)
                .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
                .unwrap_or(0.0);
            luma[y * rw + x] = px;
        }
    }
    let mut best = -1.0f32;
    for oy in 0..=(rh - th) {
        for ox in 0..=(rw - tw) {
            let mut sum = 0.0;
            for ty in 0..*th {
                for tx in 0..*tw {
                    sum += luma[(oy + ty) * rw + ox + tx];
                }
            }
            let mean = sum / (tw * th) as f32;
            let (mut dot, mut nrm) = (0.0f32, 0.0f32);
            for ty in 0..*th {
                for tx in 0..*tw {
                    let pv = luma[(oy + ty) * rw + ox + tx] - mean;
                    dot += pv * tmpl[ty * tw + tx];
                    nrm += pv * pv;
                }
            }
            let ncc = if nrm > 1e-6 { dot / nrm.sqrt() } else { 0.0 };
            best = best.max(ncc);
        }
    }
    best
}

/// Threshold on [`eye_ncc`] above which a card is treated as unowned.
const EYE_THRESHOLD: f32 = 0.5;

/// Whether the "unowned" eye icon appears in `eye` (player does not own the relic).
fn card_has_eye(image: &image::RgbaImage, eye: &wf_relic::Rect) -> bool {
    eye_ncc(image, eye) >= EYE_THRESHOLD
}

/// Upper bound on a plausible per-refinement Intact owned count. Any badge that
/// reads above this is rejected as OCR garbage rather than trusted (see
/// ADR-0005). Deliberately well above a realistic hoard of one relic so genuine
/// large counts still register; the observed failure mode was a spurious "145"
/// for a real "15". Tunable — one of the two scan-calibration knobs.
const RELIC_COUNT_CAP: u32 = 99;

/// Max ink coverage for a name-crop to be treated as a text line rather than
/// relic-orb artwork. A name line is thin strokes on dark (well under this); an
/// orb or item render fills most of the crop. Lets the dense phase sampling skip
/// the many candidates that land on artwork without an OCR call.
const MAX_NAME_COVERAGE: f32 = 0.30;

/// Min horizontal ink spread (fraction of crop width) for a name crop to count
/// as a name *line* during phase alignment. A relic name spans most of the crop;
/// the `xN` count badge one row up is text too but only a narrow left strip, so
/// this stops phase selection from locking onto the badge row.
const MIN_NAME_HSPAN: f32 = 0.35;

/// How many frames must agree on a relic's count before it is believed. Two is
/// enough to defeat a lone OCR outlier while still confirming within a single
/// dwell on the card; mode-voting lets the true value overtake a wrong pair as
/// the player keeps scrolling (see ADR-0005).
const RELIC_AGREEMENT: u32 = 2;

/// The agreement bar OCR must clear to overwrite a relic count that was last
/// written by `wf-mem`'s mem-scan (ADR-0009's revision) rather than OCR
/// itself. A mem-scanned count is read directly from the game's own
/// inventory payload — exact, not a frame-agreement estimate — so casually
/// overwriting it on the strength of a lone lucky pair of misreads would be a
/// regression, not a refresh. Four times [`RELIC_AGREEMENT`]: high enough
/// that a stray OCR misread pair can't plausibly clear it, but still
/// reachable through ordinary repeated scrolling across the relic (a natural
/// scroll session revisiting a card that many times is rare but not
/// impossible — see ADR-0009's note that revisits within one continuous
/// scroll are uncommon at all). Once OCR does clear this bar, the entry's
/// source flips to `Ocr` (see [`wf_relic::apply_confirmed_count`]) and the
/// normal, lower [`RELIC_AGREEMENT`] bar applies from then on — the point is
/// resisting a single bad read, not permanently distrusting OCR for that
/// relic.
const RELIC_AGREEMENT_MEMSCAN_OVERRIDE: u32 = RELIC_AGREEMENT * 4;

/// How many frames must resolve the same `(code, refinement)` identity before
/// it's marked Seen. `RelicIndex::best_match` still occasionally ties two
/// different real codes on a single garbled read; a lone frame's identity is
/// cheaper to reach agreement on than a full [`RELIC_AGREEMENT`]-frame count
/// match, but requiring a second one guards Seen against acting on a single
/// tie-broken bad match (see ADR-0009).
const SEEN_AGREEMENT: u32 = 2;

/// What a single frame concluded about one card — the relic and Inventory/
/// Sell scanners share this exact shape, so both use [`wf_gridscan::ObsKind`]
/// directly rather than each keeping their own copy.
use wf_gridscan::ObsKind;

/// One relic card's resolved reading on a single frame.
struct RelicObservation {
    /// Relic display code, e.g. "Meso B9".
    display: String,
    refinement: wf_relic::Refinement,
    kind: ObsKind,
}

/// Preprocessing for the relic grid: 4× upscale, light-on-dark threshold.
fn grid_pre() -> wf_ocr::Preprocess {
    wf_gridscan::default_grid_preprocess()
}

/// OCR the Void Relics grid in `image` and resolve each visible card to a
/// `(relic, refinement)` observation, via [`wf_gridscan::scan_grid`] — the
/// screen-agnostic phase-anchored scan/OCR-confirm loop shared with the
/// Inventory/Sell scanner (see ADR-0006). Reads *every* card — including ones
/// flagged with the "unowned" eye icon, whose name we still need so a later
/// scan can zero the right relic — at the single best-aligned vertical phase,
/// collapsing the frame to **one** vote per `(code, refinement)` so the
/// caller's agreement gate counts *frames*, not slots.
fn scan_relic_grid(
    image: &image::RgbaImage,
    ocr: &wf_ocr::Ocr,
    regions: &wf_relic::RelicGridRegions,
    index: &wf_relic::RelicIndex,
) -> Vec<RelicObservation> {
    let resolve = |raw: &str| -> Option<(String, wf_relic::Refinement)> {
        let (base, refinement) = wf_relic::parse_refinement(raw);
        let info = index.best_match(&base)?;
        Some((info.display.clone(), refinement))
    };
    let ownership_signal = |image: &image::RgbaImage, eye: &wf_gridscan::Rect| card_has_eye(image, eye);
    let cfg = wf_gridscan::GridConfig {
        pre: grid_pre(),
        max_name_coverage: MAX_NAME_COVERAGE,
        min_name_hspan: MIN_NAME_HSPAN,
        badge_cap: RELIC_COUNT_CAP,
        resolve: &resolve,
        ownership_signal: Some(&ownership_signal),
    };
    let slots_for_phase = |w: u32, h: u32, phase: f32| -> Vec<wf_gridscan::Slot> {
        regions.slots(w, h, phase).into_iter().map(Into::into).collect()
    };

    wf_gridscan::scan_grid(image, ocr, slots_for_phase, &cfg)
        .into_iter()
        .map(|obs| RelicObservation {
            display: obs.key.0,
            refinement: obs.key.1,
            kind: obs.kind,
        })
        .collect()
}

/// Upper bound on a plausible owned-count reading on the Inventory/Sell
/// screen. Meaningfully higher than the relic grid's cap: a real capture
/// showed `✓303` on a non-Prime item in the (out-of-scope) All tab, and even
/// scoped to the Prime Parts tab a common Blueprint/Systems/Chassis part can
/// pile up well past a relic's realistic hoard (see issue #37's
/// region-calibration research, resolving #32).
const INVENTORY_COUNT_CAP: u32 = 999;

/// How many frames must agree on an Inventory/Sell part's count before it's
/// believed (see ADR-0005) — same floor the relic scanner uses. Unlike the
/// relic scanner there is no separate Seen tier here (see issue #37's
/// catalog-matching decision: this screen's badge is always a single-frame
/// passive read, so a Seen/Confirmed split has nothing to distinguish);
/// identity and count are trusted together, once frames agree.
const INVENTORY_AGREEMENT: u32 = 2;

/// The agreement bar OCR must clear to overwrite a Prime Part count that was
/// last written by `wf-mem`'s mem-scan (ADR-0009's revision, applied to
/// Prime Parts per issue #81) rather than OCR itself — same rationale and
/// same 4× multiplier as [`RELIC_AGREEMENT_MEMSCAN_OVERRIDE`], reused here
/// even though this screen has no separate Seen tier: the provenance
/// question this bar answers (was the current count read exactly from game
/// memory, or estimated from OCR frames?) doesn't depend on that.
const INVENTORY_AGREEMENT_MEMSCAN_OVERRIDE: u32 = INVENTORY_AGREEMENT * 4;

/// One Inventory/Sell card's resolved reading on a single frame.
struct InventoryObservation {
    part: wf_relic::PrimePart,
    kind: ObsKind,
}

/// OCR the Inventory/Sell screen's Prime Parts grid in `image` and resolve
/// each visible card to a [`wf_relic::PrimePart`] observation, via
/// [`wf_gridscan::scan_grid`] — the same screen-agnostic phase-anchored
/// scan/OCR-confirm loop [`scan_relic_grid`] uses. No ownership-signal icon:
/// this screen never lists a 0-owned card (see issue #37's
/// region-calibration research, resolving #32), so `ObsKind::Unowned` is
/// structurally possible but never produced here.
fn scan_inventory_grid(
    image: &image::RgbaImage,
    ocr: &wf_ocr::Ocr,
    regions: &wf_relic::InventoryGridRegions,
    quantities: &wf_relic::PartQuantities,
) -> Vec<InventoryObservation> {
    let resolve = |raw: &str| wf_relic::inventory_prime_part(raw, quantities);
    let cfg = wf_gridscan::GridConfig {
        pre: grid_pre(),
        max_name_coverage: MAX_NAME_COVERAGE,
        min_name_hspan: MIN_NAME_HSPAN,
        badge_cap: INVENTORY_COUNT_CAP,
        resolve: &resolve,
        ownership_signal: None,
    };
    let slots_for_phase = |w: u32, h: u32, phase: f32| -> Vec<wf_gridscan::Slot> {
        regions.slots(w, h, phase).into_iter().map(Into::into).collect()
    };

    wf_gridscan::scan_grid(image, ocr, slots_for_phase, &cfg)
        .into_iter()
        .map(|obs| InventoryObservation { part: obs.key, kind: obs.kind })
        .collect()
}

/// Calibration: OCR the Inventory/Sell Prime Parts grid from a PNG and print
/// what resolved. `wf-lite inventory-grid-file <path-to-inventory.png>` — use
/// this against a real capture to correct [`wf_relic::InventoryGridRegions`]'s
/// currently-estimated absolute-position constants (see its doc comment).
pub(crate) async fn inventory_grid_file() -> Result<()> {
    let path = std::env::args()
        .nth(2)
        .context("usage: inventory-grid-file <path-to-inventory.png>")?;
    println!("\n== Inventory grid file: {path} ==");
    let image = image::open(&path).with_context(|| format!("opening {path}"))?.to_rgba8();
    let client = wf_data::http_client();
    let quantities = wf_relic::PartQuantities::load_cached(&client, CATALOGUE_TTL).await?;
    let ocr = wf_ocr::Ocr::new()?;
    let regions = wf_relic::InventoryGridRegions::default_calibration();

    let found = scan_inventory_grid(&image, &ocr, &regions, &quantities);
    println!("  resolved {} card observations:", found.len());
    for o in &found {
        let what = match o.kind {
            ObsKind::Count(n) => format!("x{n}"),
            ObsKind::Abstain => "badge unreadable (abstain)".to_string(),
            ObsKind::Unowned => "UNOWNED (unexpected on this screen)".to_string(),
        };
        println!("    {} {} {what}", o.part.prime, o.part.part);
    }
    Ok(())
}

/// Calibration: OCR the Void Relics grid from a PNG and print what resolved.
/// `wf-lite relic-grid-file <path-to-relics.png>`.
pub(crate) async fn relic_grid_file() -> Result<()> {
    let path = std::env::args()
        .nth(2)
        .context("usage: relic-grid-file <path-to-relics.png>")?;
    println!("\n== Relic grid file: {path} ==");
    let image = image::open(&path)
        .with_context(|| format!("opening {path}"))?
        .to_rgba8();
    let client = wf_data::http_client();
    let index = wf_relic::RelicIndex::load_cached(&client, CATALOGUE_TTL).await?;
    let ocr = wf_ocr::Ocr::new()?;
    let regions = wf_relic::RelicGridRegions::default_calibration();

    // Per-slot debug (eye NCC + OCR) to calibrate ownership + regions, at the
    // same best-aligned phase the live scanner picks.
    let pre = grid_pre();
    let resolve = |raw: &str| -> Option<(String, wf_relic::Refinement)> {
        let (base, refinement) = wf_relic::parse_refinement(raw);
        let info = index.best_match(&base)?;
        Some((info.display.clone(), refinement))
    };
    let ownership_signal = |image: &image::RgbaImage, eye: &wf_gridscan::Rect| card_has_eye(image, eye);
    let cfg = wf_gridscan::GridConfig {
        pre,
        max_name_coverage: MAX_NAME_COVERAGE,
        min_name_hspan: MIN_NAME_HSPAN,
        badge_cap: RELIC_COUNT_CAP,
        resolve: &resolve,
        ownership_signal: Some(&ownership_signal),
    };
    let slots_for_phase = |w: u32, h: u32, phase: f32| -> Vec<wf_gridscan::Slot> {
        regions.slots(w, h, phase).into_iter().map(Into::into).collect()
    };
    let phase = wf_gridscan::best_phase(&image, slots_for_phase, &cfg);
    for (i, slot) in regions.slots(image.width(), image.height(), phase).iter().enumerate() {
        let ncc = eye_ncc(&image, &slot.eye);
        let raw = ocr
            .recognize(
                &image::imageops::crop_imm(&image, slot.name.x, slot.name.y, slot.name.w, slot.name.h)
                    .to_image(),
                pre,
                wf_ocr::PageMode::Block,
            )
            .unwrap_or_default()
            .replace('\n', " ");
        let (base, refinement) = wf_relic::parse_refinement(&raw);
        let matched = index.best_match(&base).map(|r| r.display.as_str());
        let badge = wf_gridscan::read_badge(&image, &slot.count, &ocr, pre, RELIC_COUNT_CAP);
        println!(
            "  slot {i:2}: eye={ncc:+.2} {} ocr={raw:?} -> {} [{refinement:?}] {}",
            if ncc >= EYE_THRESHOLD { "UNOWNED" } else { "owned  " },
            matched.unwrap_or("—"),
            match badge {
                ObsKind::Count(n) => format!("x{n}"),
                ObsKind::Abstain => "badge?".to_string(),
                ObsKind::Unowned => "-".to_string(),
            }
        );
    }

    let found = scan_relic_grid(&image, &ocr, &regions, &index);
    println!("  resolved {} card observations:", found.len());
    for o in &found {
        let what = match o.kind {
            ObsKind::Count(n) => format!("x{n}"),
            ObsKind::Abstain => "badge unreadable (abstain)".to_string(),
            ObsKind::Unowned => "UNOWNED (eye)".to_string(),
        };
        println!("    {:<16} [{:?}] {what}", o.display, o.refinement);
    }
    Ok(())
}

/// Capture the live reward screen, OCR the candidate slots, pick the real
/// rewards (handles 2–4 centred cards + wrapped names), and rank by plat/ducats.
pub(crate) async fn relic_scan(config: &Config) -> Result<()> {
    println!("\n== Relic scan ==");
    let cap = wf_capture::capture_warframe(None)?;
    println!("  captured {}x{}", cap.image.width(), cap.image.height());

    let client = wf_data::http_client();
    let index =
        wf_relic::ItemIndex::load_cached(&client, CATALOGUE_TTL, &config.market_platform).await?;
    let regions = reward_regions(config);
    let ocr = wf_ocr::Ocr::new()?;

    let slots = ocr_regions(&cap.image, &ocr, &regions);
    let names = wf_relic::select_rewards(&slots, &index);
    if names.len() < 2 {
        println!(
            "  only {} reward name(s) resolved — likely not on the Void Fissure reward screen",
            names.len()
        );
        return Ok(());
    }

    let market = MarketClient::new(client.clone(), config.market_platform.clone());
    let cache = wf_relic::price_cache();
    let vaulted = crate::load_vaulted(&client, &index).await;
    let evals = wf_relic::evaluate_cached(
        &names,
        &index,
        &market,
        &cache,
        wf_relic::PriceOpts::default(),
        &vaulted,
    )
    .await;
    let mastery = load_mastery(config, &client).await;
    print_reward_table(&evals, &mastery);
    Ok(())
}

/// OCR a region of a saved PNG using the real `wf-ocr` pipeline — used to
/// calibrate reward-name crop coordinates and preprocessing against a captured
/// reward screen. Threshold/scale tunable via `WF_OCR_THRESHOLD` / `WF_OCR_SCALE`.
/// Usage: `wf-lite ocr-file <path> <x> <y> <w> <h>`; saves `<path>.pre.png`.
pub(crate) fn ocr_file() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(2).collect();
    anyhow::ensure!(a.len() == 5, "usage: ocr-file <path> <x> <y> <w> <h>");
    let (path, x, y, w, h): (String, u32, u32, u32, u32) = (
        a[0].clone(),
        a[1].parse().context("x")?,
        a[2].parse().context("y")?,
        a[3].parse().context("w")?,
        a[4].parse().context("h")?,
    );

    let threshold: u8 = std::env::var("WF_OCR_THRESHOLD").ok().and_then(|v| v.parse().ok()).unwrap_or(140);
    let scale: u32 = std::env::var("WF_OCR_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let pre = wf_ocr::Preprocess { scale, threshold, light_text: true };

    let img = image::open(&path).with_context(|| format!("opening {path}"))?.to_rgba8();
    let crop = image::imageops::crop_imm(&img, x, y, w, h).to_image();
    wf_ocr::preprocess(&crop, pre)
        .save(format!("{path}.pre.png"))
        .ok();

    let ocr = wf_ocr::Ocr::new()?;
    let text = ocr.recognize(&crop, pre, wf_ocr::PageMode::Line)?;
    println!("region ({x},{y},{w},{h}) thr={threshold} scale={scale}: '{text}'");
    Ok(())
}

/// Capture the Warframe window (or a sub-region `x y w h`) and OCR it — an
/// end-to-end test of the capture → preprocess → tesseract pipeline.
pub(crate) fn ocr_test() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(2).collect();
    let region = if a.len() == 4 {
        Some(wf_capture::Region {
            x: a[0].parse().context("x")?,
            y: a[1].parse().context("y")?,
            width: a[2].parse().context("w")?,
            height: a[3].parse().context("h")?,
        })
    } else {
        None
    };

    println!("\n== OCR test ==");
    let cap = wf_capture::capture_warframe(region)?;
    println!("  captured {}x{}", cap.image.width(), cap.image.height());

    let ocr = wf_ocr::Ocr::new()?;
    let text = ocr.recognize(&cap.image, wf_ocr::Preprocess::default(), wf_ocr::PageMode::Block)?;
    println!("  --- recognized text ---\n{}\n  --- end ---", text);
    Ok(())
}

/// Deadline the owned-relic scan should keep running until, shared between
/// `relic_watch_loop` (which extends it from `EE.log`) and `relic_scan_loop`
/// (which reads it every iteration to decide whether to scan or idle).
type RelicDeadline = Arc<Mutex<Option<std::time::Instant>>>;
/// Deadline the owned-Prime-Part scan should keep running until — the
/// Inventory/Sell screen's counterpart to [`RelicDeadline`], extended by
/// whatever detects the Inventory/Sell screen opening and read every
/// iteration by [`inventory_scan_loop`].
type InventoryDeadline = Arc<Mutex<Option<std::time::Instant>>>;

/// Shared handles the renderer loop needs to background-warm the price cache
/// when a relic tier the player owns becomes crackable (see
/// [`prewarm_new_tiers`]) — the four pieces of the "Relic auto-detection"
/// block's state that pricing actually needs, `None` (via the caller's
/// `Option<PrewarmCtx>`) when that block couldn't load the relic catalogue.
#[derive(Clone)]
pub(crate) struct PrewarmCtx {
    market: MarketClient,
    cache: Arc<wf_relic::PriceCache>,
    relic_index: Arc<wf_relic::RelicIndex>,
    item_index: Arc<wf_relic::ItemIndex>,
}

/// Bring up relic auto-detection (reward-screen watch + owned-relic/
/// Prime-Part grid scans) if OCR, an EE.log, and the item catalogue are all
/// available — the OCR-feature counterpart of `run_overlay`'s "Relic
/// auto-detection" block, extracted here so the `ocr`-feature-off build
/// (`ocr_disabled.rs`'s stub of the same name) can stand in for it wholesale.
/// Returns the shared pricing context the renderer loop uses for
/// fissure-start price pre-warming, or `None` if any prerequisite is missing
/// (auto-detect stays off; the rest of the overlay runs normally).
pub(crate) async fn start_relic_watch(
    config: &Config,
    client: &reqwest::Client,
    reward: RewardState,
    relic_scan_status: RelicScanStatus,
) -> Option<PrewarmCtx> {
    match (wf_ocr::Ocr::new(), config.resolve_ee_log()) {
        (Ok(ocr), Ok(ee_log)) => match wf_relic::ItemIndex::load_cached(
            client,
            CATALOGUE_TTL,
            &config.market_platform,
        )
        .await
        {
            Ok(index) => {
                let index = Arc::new(index);
                let cache = Arc::new(wf_relic::price_cache());
                let mastery = Arc::new(load_mastery(config, client).await);
                // Relic drop tables for the owned-relic guide (best-effort).
                let relic_index = wf_relic::RelicIndex::load_cached(client, CATALOGUE_TTL)
                    .await
                    .ok()
                    .map(Arc::new);
                // Reward-name → vaulted-status lookup (best-effort, empty if the
                // relic drop tables above didn't load) — computed once here since
                // it's a pure join over already-cached catalogues, not something
                // to redo on every reward-screen detection.
                let vaulted: Arc<HashMap<String, bool>> = Arc::new(
                    relic_index
                        .as_ref()
                        .map(|ri| wf_relic::vaulted_rewards(ri, &index))
                        .unwrap_or_default(),
                );
                // Prime Part build quantities, doubling as the Inventory/Sell
                // scanner's catalog-matching authority (see
                // `wf_relic::inventory_prime_part`) — best-effort, same as
                // the relic catalogue above.
                let part_quantities = wf_relic::PartQuantities::load_cached(client, CATALOGUE_TTL)
                    .await
                    .ok()
                    .map(Arc::new);
                println!(
                    "  relic auto-detect: ON ({} items; {} relics; {} cached prices; {} mastered; watching {})",
                    index.len(),
                    relic_index.as_ref().map_or(0, |r| r.len()),
                    cache.len(),
                    mastery.len(),
                    ee_log.display()
                );
                let market = MarketClient::new(client.clone(), config.market_platform.clone());
                let ocr = Arc::new(ocr);

                // Background price pre-warm (see `prewarm_new_tiers` and
                // `run_overlay`'s renderer loop): only possible once the relic
                // drop tables have loaded, since it needs a relic's reward pool.
                let prewarm_ctx = relic_index.clone().map(|relic_index| PrewarmCtx {
                    market: market.clone(),
                    cache: cache.clone(),
                    relic_index,
                    item_index: index.clone(),
                });

                // The owned-relic scan runs in its own tightly-looped task (see
                // relic_scan_loop) so it isn't throttled by relic_watch_loop's
                // POLL_INTERVAL or by how long a scan itself takes — the Relics
                // list scrolls continuously, so sampling as fast as possible is
                // what actually catches it, not waiting out a fixed interval.
                let relic_deadline: Option<RelicDeadline> = if let Some(ridx) = relic_index.clone() {
                    let deadline: RelicDeadline = Arc::new(Mutex::new(None));
                    tokio::spawn(relic_scan_loop(
                        deadline.clone(),
                        ocr.clone(),
                        ridx,
                        wf_relic::RelicGridRegions::default_calibration(),
                        relic_scan_status.clone(),
                    ));
                    Some(deadline)
                } else {
                    None
                };

                // The owned-Prime-Part scan mirrors the owned-relic scan
                // above: its own tightly-looped task, armed by an EE.log
                // event via `relic_watch_loop`, independent of both the
                // relic scan and reward-screen detection.
                let inventory_deadline: Option<InventoryDeadline> =
                    if let Some(quantities) = part_quantities.clone() {
                        let deadline: InventoryDeadline = Arc::new(Mutex::new(None));
                        tokio::spawn(inventory_scan_loop(
                            deadline.clone(),
                            ocr.clone(),
                            quantities,
                            wf_relic::InventoryGridRegions::default_calibration(),
                        ));
                        Some(deadline)
                    } else {
                        None
                    };

                let reward_regions = reward_regions(config);
                tokio::spawn(async move {
                    if let Err(e) = relic_watch_loop(
                        ee_log,
                        ocr,
                        index,
                        market,
                        cache,
                        mastery,
                        vaulted,
                        reward,
                        reward_regions,
                        relic_deadline,
                        inventory_deadline,
                    )
                    .await
                    {
                        tracing::error!("relic watcher stopped: {e:#}");
                    }
                });

                prewarm_ctx
            }
            Err(e) => {
                println!("  relic auto-detect: OFF (item catalogue load failed: {e:#})");
                None
            }
        },
        (Err(e), _) => {
            println!("  relic auto-detect: OFF ({e})");
            None
        }
        (_, Err(e)) => {
            println!("  relic auto-detect: OFF (no EE.log: {e:#})");
            None
        }
    }
}

/// Load the current owned-relic set from disk and warm the price cache for
/// every tradable reward of an owned relic whose tier is in `tiers` (see
/// [`wf_relic::active_tier_reward_names`]) — the actual pre-warm work behind
/// the renderer loop's newly-active-tier dispatch. Spawned as its own
/// detached task each time new tiers appear, so a slow warframe.market
/// response never delays the next rendered frame.
/// [`wf_relic::PriceOpts::fetch_timeout`] used for pre-warming, well past
/// [`wf_relic::PriceOpts::default`]'s 2.5s — that default is tuned for the
/// ~15s reward-screen selection window, but a pre-warm runs minutes ahead of
/// it with no such deadline, so a slow warframe.market response should get a
/// real chance to land here instead of timing out and leaving the item to
/// warm cold later anyway.
const PREWARM_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

pub(crate) async fn prewarm_new_tiers(ctx: PrewarmCtx, tiers: std::collections::HashSet<String>) {
    let owned = wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE)
        .map(|s| s.value)
        .unwrap_or_default();
    let evidence = wf_relic::owned_evidence(&owned);
    let names = wf_relic::active_tier_reward_names(&evidence, &ctx.relic_index, &tiers);
    if names.is_empty() {
        return;
    }
    tracing::info!(
        "fissure pre-warm: tier(s) {} active — warming {} reward price(s)",
        tiers.iter().cloned().collect::<Vec<_>>().join(", "),
        names.len()
    );
    let opts = wf_relic::PriceOpts { fetch_timeout: PREWARM_FETCH_TIMEOUT, ..wf_relic::PriceOpts::default() };
    wf_relic::prewarm_reward_prices(&names, &ctx.item_index, &ctx.market, &ctx.cache, opts).await;
}

/// How long a relic-inventory-open event keeps [`relic_scan_loop`] scanning
/// (shared: `relic_watch_loop` extends the deadline, `relic_scan_loop` reads it).
const RELIC_SCAN_WINDOW: std::time::Duration = std::time::Duration::from_secs(180);
/// [`RELIC_SCAN_WINDOW`]'s counterpart for [`inventory_scan_loop`], armed by
/// `Event::InventorySellOpen`.
const INVENTORY_SCAN_WINDOW: std::time::Duration = std::time::Duration::from_secs(180);

/// Watch the EE.log for a relic **crack** / reward-screen line, which opens a
/// poll window scanned until the 4-choice screen resolves (publishing the
/// ranked reward to `reward`). A **Relics inventory** open
/// (`RelicInventoryOpen`) instead extends `relic_deadline`, and an
/// **Inventory/Sell** open (`InventorySellOpen`) extends `inventory_deadline`
/// the same way — the actual owned-relic/owned-Prime-Part scanning happens in
/// their own separate, tightly-looped tasks ([`relic_scan_loop`],
/// [`inventory_scan_loop`]), so a slow or expensive scan never throttles
/// reward-screen detection (and vice versa: reward-screen debounce logic
/// never throttles either scan).
#[allow(clippy::too_many_arguments)]
async fn relic_watch_loop(
    ee_log: std::path::PathBuf,
    ocr: Arc<wf_ocr::Ocr>,
    index: Arc<wf_relic::ItemIndex>,
    market: MarketClient,
    cache: Arc<wf_relic::PriceCache>,
    mastery: Arc<wf_relic::MasterySet>,
    vaulted: Arc<HashMap<String, bool>>,
    reward: RewardState,
    regions: wf_relic::RewardRegions,
    relic_deadline: Option<RelicDeadline>,
    inventory_deadline: Option<InventoryDeadline>,
) -> Result<()> {
    use std::time::{Duration, Instant};
    use wf_log::Event;

    // After a crack (or a reward-screen line), poll the screen for this long.
    // Covers the ~1min crack→screen gap, log-flush lag, and the player bringing
    // the screen up manually (Tab / progress).
    const POLL_WINDOW: Duration = Duration::from_secs(150);
    // Also bounds how quickly a RelicInventoryOpen line arms relic_scan_loop's
    // deadline — kept short so opening the Relics screen registers almost
    // immediately. Cheap now that a reward-screen scan attempt (when active) is
    // a single in-process OCR pass rather than a CLI subprocess (ADR-0008).
    const POLL_INTERVAL: Duration = Duration::from_millis(500);
    // Don't re-show the same 15s screen repeatedly.
    const SHOW_DEBOUNCE: Duration = Duration::from_secs(20);

    let mut tailer = wf_log::LogTailer::from_end(&ee_log);
    let mut scan_until: Option<Instant> = None;
    let mut window_open = false; // whether we've logged the current window
    let mut last_shown = Instant::now() - SHOW_DEBOUNCE;

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        // Fold new log lines into the polling-window state.
        for line in tailer.poll().unwrap_or_default() {
            match wf_log::event_from_line(&line) {
                Some(Event::RelicCrack) | Some(Event::RelicRewardScreen) => {
                    scan_until = Some(Instant::now() + POLL_WINDOW);
                    if !window_open {
                        window_open = true;
                        tracing::info!("relic activity — watching for the reward screen");
                    }
                }
                Some(Event::RelicInventoryOpen) => {
                    if let Some(d) = &relic_deadline {
                        *d.lock().unwrap() = Some(Instant::now() + RELIC_SCAN_WINDOW);
                    }
                }
                Some(Event::InventorySellOpen) => {
                    if let Some(d) = &inventory_deadline {
                        *d.lock().unwrap() = Some(Instant::now() + INVENTORY_SCAN_WINDOW);
                    }
                }
                _ => {}
            }
        }

        // --- Reward screen scan -------------------------------------------
        let active = scan_until.is_some_and(|t| Instant::now() < t);
        if !active {
            window_open = false;
            continue;
        }
        if last_shown.elapsed() < SHOW_DEBOUNCE {
            continue;
        }

        // Scan the screen. If the reward screen is up, ≥2 slots resolve.
        let (ocr2, regions2) = (ocr.clone(), regions.clone());
        let slots = match tokio::task::spawn_blocking(move || capture_and_ocr(&ocr2, &regions2)).await
        {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                tracing::warn!("reward capture failed: {e:#}");
                continue;
            }
            Err(e) => {
                tracing::warn!("scan task join error: {e}");
                continue;
            }
        };
        let mut names = wf_relic::select_rewards(&slots, &index);
        if names.len() < 2 {
            continue; // screen not up yet (or already gone) — keep polling
        }

        // The reward cards fan in with a brief entrance animation, so this first
        // hit can catch the screen mid-render with a card's text not yet legible
        // (see the "only detected 3 of 4 parts" report). Resample a few times
        // over the settle window and keep whichever pass resolved the most
        // rewards, rather than locking in an incomplete first read.
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let (ocr2, regions2) = (ocr.clone(), regions.clone());
            let Ok(Ok(slots)) =
                tokio::task::spawn_blocking(move || capture_and_ocr(&ocr2, &regions2)).await
            else {
                continue;
            };
            let found = wf_relic::select_rewards(&slots, &index);
            if found.len() > names.len() {
                names = found;
            }
        }

        let evals = wf_relic::evaluate_cached(
            &names,
            &index,
            &market,
            &cache,
            wf_relic::PriceOpts::default(),
            &vaulted,
        )
        .await;
        // Re-read fresh each time: the Inventory/Sell scanner (a separate
        // task) can update this file independently of this loop's cadence.
        let owned_parts =
            wf_cache::load_blob::<wf_relic::OwnedPrimeParts>(wf_relic::OWNED_PRIME_PARTS_FILE)
                .map(|s| s.value)
                .unwrap_or_default();
        // Also re-read fresh: `wf-browse`'s Mastery tab can mark/unmark a
        // wishlist entry directly (ADR-0004) while this loop keeps running.
        let wishlist = wf_cache::load_blob::<wf_relic::Wishlist>(wf_relic::WISHLIST_FILE)
            .map(|s| s.value)
            .unwrap_or_default();
        let rows = reward_rows(&evals, &mastery, &owned_parts, &wishlist);
        if let Some(best) = wf_relic::best_pick(&evals, &mastery) {
            tracing::info!("reward screen captured — best pick = {}", rows[best].name);
        }
        *reward.lock().unwrap() = Some((Instant::now(), rows));
        last_shown = Instant::now();
    }
}

/// Scan the owned-relic grid on its own tight cadence, independent of
/// `relic_watch_loop`'s fixed `POLL_INTERVAL`.
///
/// The Relics list scrolls **continuously** (not snapped to row boundaries), so
/// any single frame's fixed grid positions only line up with the real card
/// text some of the time (see [`wf_relic::RelicGridRegions::row_phases`] for the
/// per-frame half of the fix). The other half is here: sampling as fast as the
/// hardware allows — rather than waiting out a fixed poll interval on top of
/// however long a scan itself takes — is what actually catches more of the
/// list quickly, since each additional sample is another chance to catch cards
/// at a favorable scroll offset.
/// [`wf_gridscan::ScanLoopBody`] for the Void Relics screen: on top of the
/// shared deadline/idle/cadence skeleton, tracks per-session tally/identity
/// bookkeeping and the overlay's scan-progress indicator (see ADR-0009's
/// Seen tier for why relic tracking needs `identity_reads`/`session_seen_count`
/// where the simpler Inventory/Sell scanner does not).
struct RelicScanBody {
    ocr: Arc<wf_ocr::Ocr>,
    relic_index: Arc<wf_relic::RelicIndex>,
    relic_regions: wf_relic::RelicGridRegions,
    scan_status: RelicScanStatus,
    // `relic_owned` is the cumulative, disk-persisted owned set, keyed per
    // (code, refinement) (see ADR-0005) — it survives restarts and
    // Relics-screen visits, so the mastery planner (`wf-lite mastery-plan`)
    // has data even outside a live scan. Within the *current* continuous scan
    // (cleared each fresh open), `tally` votes across frames and only
    // confirms a count once enough frames agree, and `session_applied`
    // records which confirmed value we've already written this session so a
    // stable count refreshes its last-seen stamp once, not on every frame.
    relic_owned: wf_relic::OwnedRelics,
    tally: wf_ocr::Tally<(String, wf_relic::Refinement)>,
    session_applied: HashMap<(String, wf_relic::Refinement), u32>,
    identity_reads: HashMap<(String, wf_relic::Refinement), u32>,
    session_seen_count: usize,
}

impl wf_gridscan::ScanLoopBody for RelicScanBody {
    fn activate(&mut self) {
        self.tally = wf_ocr::Tally::new();
        self.session_applied.clear();
        self.identity_reads.clear();
        self.session_seen_count = 0;
        // Show scanning has started before the first capture even completes,
        // rather than nothing until a relic clears its trust bar.
        *self.scan_status.lock().unwrap() = Some(wf_overlay::ScanProgress::default());
        tracing::info!(
            "relics screen opened — scanning as you scroll ({} relics known from before)",
            self.relic_owned.len()
        );
    }

    fn deactivate(&mut self) {
        *self.scan_status.lock().unwrap() = None;
    }

    async fn tick(&mut self) -> bool {
        let (ocr2, regions2, ridx2) =
            (self.ocr.clone(), self.relic_regions.clone(), self.relic_index.clone());
        let scanned = tokio::task::spawn_blocking(move || {
            let t0 = std::time::Instant::now();
            let cap = wf_capture::capture_warframe(None)?;
            let captured = t0.elapsed();
            let found = scan_relic_grid(&cap.image, &ocr2, &regions2, &ridx2);
            tracing::debug!(
                "relic scan cycle: capture {captured:?}, scan {:?}",
                t0.elapsed() - captured
            );
            Ok::<_, anyhow::Error>(found)
        })
        .await;
        let Ok(Ok(found)) = scanned else {
            return false;
        };

        let mut changed = false;
        for obs in found {
            let key = (obs.display.clone(), obs.refinement);
            // Any cleanly-resolved identity marks Seen, independent of count (ADR-0009).
            match obs.kind {
                ObsKind::Count(n) => {
                    if mark_seen_if_agreed(&mut self.identity_reads, &mut self.relic_owned, &key) {
                        changed = true;
                        self.session_seen_count += 1;
                    }
                    self.tally.record(key.clone(), n);
                }
                ObsKind::Unowned => self.tally.record(key.clone(), 0), // 0 = confirmed unowned
                ObsKind::Abstain => {
                    if mark_seen_if_agreed(&mut self.identity_reads, &mut self.relic_owned, &key) {
                        changed = true;
                        self.session_seen_count += 1;
                    }
                    continue;
                }
            }
            // A count last written by mem-scan needs a much higher agreement
            // bar to overwrite (ADR-0009's revision) — see
            // RELIC_AGREEMENT_MEMSCAN_OVERRIDE.
            let required_agreement =
                if wf_relic::count_source(&self.relic_owned, &key.0, key.1) == Some(wf_relic::Source::MemScan) {
                    RELIC_AGREEMENT_MEMSCAN_OVERRIDE
                } else {
                    RELIC_AGREEMENT
                };
            let relic_owned = &mut self.relic_owned;
            if wf_gridscan::confirm_once(&self.tally, &mut self.session_applied, &key, required_agreement, |confirmed| {
                wf_relic::apply_confirmed_count(relic_owned, &key.0, key.1, confirmed, wf_relic::Source::Ocr);
            }) {
                changed = true;
            }
        }
        if changed {
            *self.scan_status.lock().unwrap() = Some(wf_overlay::ScanProgress {
                seen: self.session_seen_count,
                confirmed: self.session_applied.len(),
            });
            let _ = wf_cache::save_blob(wf_relic::OWNED_RELICS_FILE, &self.relic_owned);
            tracing::info!("relic scan: {} relics owned", self.relic_owned.len());
        }
        changed
    }
}

async fn relic_scan_loop(
    relic_deadline: RelicDeadline,
    ocr: Arc<wf_ocr::Ocr>,
    relic_index: Arc<wf_relic::RelicIndex>,
    relic_regions: wf_relic::RelicGridRegions,
    scan_status: RelicScanStatus,
) {
    // Absent, or a legacy/foreign format we can no longer trust — back up any
    // existing file and start clean (ADR-0005).
    let relic_owned: wf_relic::OwnedRelics = wf_cache::load_blob_or_reset(wf_relic::OWNED_RELICS_FILE);

    let body = RelicScanBody {
        ocr,
        relic_index,
        relic_regions,
        scan_status,
        relic_owned,
        tally: wf_ocr::Tally::new(),
        session_applied: HashMap::new(),
        identity_reads: HashMap::new(),
        session_seen_count: 0,
    };
    wf_gridscan::run_scan_loop(relic_deadline, wf_gridscan::ScanCadence::default(), body).await;
}

/// Bump `key`'s identity-read counter and mark Seen once it reaches
/// [`SEEN_AGREEMENT`]. Returns whether this call actually marked Seen (a new
/// change worth persisting).
fn mark_seen_if_agreed(
    identity_reads: &mut HashMap<(String, wf_relic::Refinement), u32>,
    relic_owned: &mut wf_relic::OwnedRelics,
    key: &(String, wf_relic::Refinement),
) -> bool {
    let reads = identity_reads.entry(key.clone()).or_insert(0);
    *reads += 1;
    *reads >= SEEN_AGREEMENT && wf_relic::mark_seen(relic_owned, &key.0, key.1)
}

/// [`wf_gridscan::ScanLoopBody`] for the Inventory/Sell screen's Prime Parts
/// grid — simpler than [`RelicScanBody`] in one respect: no Seen tier (see
/// issue #37's catalog-matching decision — this screen's badge is always a
/// single-frame passive read, so a Seen/Confirmed split has nothing to
/// distinguish), so a card only ever needs [`INVENTORY_AGREEMENT`] agreeing
/// frames before its count is applied.
struct InventoryScanBody {
    ocr: Arc<wf_ocr::Ocr>,
    quantities: Arc<wf_relic::PartQuantities>,
    inventory_regions: wf_relic::InventoryGridRegions,
    owned: wf_relic::OwnedPrimeParts,
    tally: wf_ocr::Tally<wf_relic::PrimePart>,
    session_applied: HashMap<wf_relic::PrimePart, u32>,
}

impl wf_gridscan::ScanLoopBody for InventoryScanBody {
    fn activate(&mut self) {
        self.tally = wf_ocr::Tally::new();
        self.session_applied.clear();
        tracing::info!(
            "inventory/sell screen opened — scanning as you scroll ({} parts known from before)",
            self.owned.values().map(|m| m.len()).sum::<usize>()
        );
    }

    async fn tick(&mut self) -> bool {
        let (ocr2, regions2, quantities2) =
            (self.ocr.clone(), self.inventory_regions.clone(), self.quantities.clone());
        let scanned = tokio::task::spawn_blocking(move || {
            let t0 = std::time::Instant::now();
            let cap = wf_capture::capture_warframe(None)?;
            let captured = t0.elapsed();
            let found = scan_inventory_grid(&cap.image, &ocr2, &regions2, &quantities2);
            tracing::debug!(
                "inventory scan cycle: capture {captured:?}, scan {:?}",
                t0.elapsed() - captured
            );
            Ok::<_, anyhow::Error>(found)
        })
        .await;
        let Ok(Ok(found)) = scanned else {
            return false;
        };

        let mut changed = false;
        for obs in found {
            let ObsKind::Count(n) = obs.kind else {
                continue; // abstain — no vote (Unowned never occurs on this screen)
            };
            self.tally.record(obs.part.clone(), n);
            // A count last written by mem-scan needs a much higher agreement
            // bar to overwrite (ADR-0009's revision) — see
            // INVENTORY_AGREEMENT_MEMSCAN_OVERRIDE.
            let required_agreement =
                if wf_relic::owned_parts::source(&self.owned, &obs.part) == Some(wf_relic::Source::MemScan) {
                    INVENTORY_AGREEMENT_MEMSCAN_OVERRIDE
                } else {
                    INVENTORY_AGREEMENT
                };
            let owned = &mut self.owned;
            if wf_gridscan::confirm_once(
                &self.tally,
                &mut self.session_applied,
                &obs.part,
                required_agreement,
                |confirmed| wf_relic::owned_parts::apply_count(owned, &obs.part, confirmed, wf_relic::Source::Ocr),
            ) {
                changed = true;
            }
        }
        if changed {
            let _ = wf_cache::save_blob(wf_relic::OWNED_PRIME_PARTS_FILE, &self.owned);
            tracing::info!(
                "inventory scan: {} parts owned",
                self.owned.values().map(|m| m.len()).sum::<usize>()
            );
        }
        changed
    }
}

/// Scan the Inventory/Sell screen's Prime Parts grid on its own tight
/// cadence, mirroring [`relic_scan_loop`] — same continuous-scroll,
/// phase-anchored sampling rationale (see [`wf_relic::InventoryGridRegions`]),
/// same disk-persisted cumulative owned set (see [`InventoryScanBody`]).
async fn inventory_scan_loop(
    inventory_deadline: InventoryDeadline,
    ocr: Arc<wf_ocr::Ocr>,
    quantities: Arc<wf_relic::PartQuantities>,
    inventory_regions: wf_relic::InventoryGridRegions,
) {
    // Absent, or a legacy/foreign format we can no longer trust — back up any
    // existing file and start clean (ADR-0005).
    let owned: wf_relic::OwnedPrimeParts = wf_cache::load_blob_or_reset(wf_relic::OWNED_PRIME_PARTS_FILE);

    let body = InventoryScanBody {
        ocr,
        quantities,
        inventory_regions,
        owned,
        tally: wf_ocr::Tally::new(),
        session_applied: HashMap::new(),
    };
    wf_gridscan::run_scan_loop(inventory_deadline, wf_gridscan::ScanCadence::default(), body).await;
}

/// OCR each candidate reward-name slot of an already-captured frame. Slots are
/// read as two-line blocks (long reward names wrap). Returns one string per
/// candidate slot; callers use [`wf_relic::select_rewards`] to keep the real
/// rewards.
fn ocr_regions(
    image: &image::RgbaImage,
    ocr: &wf_ocr::Ocr,
    regions: &wf_relic::RewardRegions,
) -> Vec<String> {
    regions
        .candidate_slots(image.width(), image.height())
        .iter()
        .map(|r| {
            let crop = image::imageops::crop_imm(image, r.x, r.y, r.w, r.h).to_image();
            ocr.recognize(&crop, wf_ocr::Preprocess::default(), wf_ocr::PageMode::Block)
                .unwrap_or_default()
                .replace('\n', " ")
        })
        .collect()
}

/// Blocking: capture the Warframe window and OCR all candidate reward slots.
fn capture_and_ocr(ocr: &wf_ocr::Ocr, regions: &wf_relic::RewardRegions) -> Result<Vec<String>> {
    let cap = wf_capture::capture_warframe(None)?;
    Ok(ocr_regions(&cap.image, ocr, regions))
}
