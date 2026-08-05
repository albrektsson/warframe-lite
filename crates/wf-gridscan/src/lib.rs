//! Generic grid-scan/OCR-confirm loop shared by every "scroll a card grid,
//! OCR its cards, trust a value once frames agree" screen scanner (the Void
//! Relics screen today; the Inventory/Sell screen's Prime Parts tab next).
//!
//! A scrolling card grid has no fixed row offset that lines up with the real
//! cards on every frame — the list scrolls continuously, so the true rows sit
//! at an arbitrary sub-pitch offset per frame (see ADR-0006). This crate
//! factors out the *screen-agnostic* half of a scan cycle: find this frame's
//! best-aligned vertical phase, OCR each visible card's name + count badge at
//! that phase, and collapse the frame's per-slot reads to one observation per
//! resolved identity. What differs between call sites — crop geometry, how a
//! raw OCR'd name resolves to an identity, whether an ownership-signal icon
//! (like the Void Relics screen's "unowned" eye) exists at all — is supplied
//! per call through [`GridConfig`], not baked in here. `wf-ocr` stays scoped
//! to OCR primitives; this crate is where the grid/slot/frame concepts live.

use std::collections::HashMap;
use std::hash::Hash;

use image::RgbaImage;

/// An ownership-signal detector: does this slot's search window show a
/// positive "unowned" icon (e.g. the Void Relics screen's eye)?
type OwnershipSignal<'a> = dyn Fn(&RgbaImage, &Rect) -> bool + Sync + 'a;

/// A rectangle in screen/window pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// One card's crop rectangles at a given scan phase.
#[derive(Debug, Clone, Copy)]
pub struct Slot {
    /// The item name text.
    pub name: Rect,
    /// The owned-count badge (e.g. relic's `xNN`, Inventory's `✓N`).
    pub badge: Rect,
    /// Search window for an optional ownership-signal icon (present for the
    /// Void Relics screen's "unowned" eye; `None` for screens with no such
    /// signal, like the Inventory/Sell screen, which never lists a 0-owned
    /// card).
    pub ownership: Option<Rect>,
}

/// What one frame concluded about one card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsKind {
    /// The badge read as this value (a genuinely blank badge means 1 owned —
    /// both known screens' convention for "no badge shown").
    Count(u32),
    /// The card resolved to an identity but its badge was present-yet-
    /// unreadable, so this frame casts no count vote rather than guessing.
    Abstain,
    /// The ownership-signal icon matched — positive proof of zero owned.
    Unowned,
}

/// One card's resolved reading, collapsed across a single frame's slots.
#[derive(Debug, Clone)]
pub struct Observation<T> {
    pub key: T,
    pub kind: ObsKind,
}

/// Per-call-site configuration: geometry and identity resolution differ by
/// screen; the scan/dedupe/agreement mechanics below don't.
pub struct GridConfig<'a, T> {
    pub pre: wf_ocr::Preprocess,
    /// Ink-coverage band for a name crop to count as real text — screens
    /// phase-align on this before paying for an OCR call.
    pub max_name_coverage: f32,
    /// Minimum horizontal ink spread (fraction of crop width) for a name crop
    /// to count as a name *line* rather than a narrower label (e.g. a count
    /// badge) during phase alignment.
    pub min_name_hspan: f32,
    /// Plausibility cap for a parsed badge value (see [`wf_ocr::parse_badge`]).
    pub badge_cap: u32,
    /// Resolve a card's raw OCR'd (block-mode, newline-joined) name text to
    /// this screen's identity type. Returns `None` for anything that doesn't
    /// match the catalogue, so it casts no vote rather than guessing.
    pub resolve: &'a (dyn Fn(&str) -> Option<T> + Sync),
    /// Detect a positive "unowned" signal for a slot's ownership window (the
    /// Void Relics screen's eye icon). `None` when the screen has no such
    /// signal at all.
    pub ownership_signal: Option<&'a OwnershipSignal<'a>>,
}

/// Preprocessing for a card grid: 4× upscale, light-on-dark threshold —
/// tuned once against the Void Relics screen and reused as-is for every
/// other grid screen calibrated against the same in-game UI text style.
pub fn default_grid_preprocess() -> wf_ocr::Preprocess {
    wf_ocr::Preprocess { scale: 4, threshold: 140, light_text: true }
}

/// Crop a slot rectangle out of a frame.
pub fn crop_rect(image: &RgbaImage, r: &Rect) -> RgbaImage {
    image::imageops::crop_imm(image, r.x, r.y, r.w, r.h).to_image()
}

/// How many phases of a row pitch to sample when picking the best-aligned
/// vertical offset (see [`best_phase`]).
const PHASE_SAMPLES: u32 = 12;

/// The vertical phase (fraction of a row pitch, in `0.0..1.0`) whose name
/// band best lands on real text this frame. The grid scrolls continuously, so
/// the true rows sit at an arbitrary sub-pitch offset; this scores a handful
/// of phases by how many name crops have text-like ink coverage — cheaply,
/// with **no** OCR and no upscale — and keeps the best. This is what makes
/// one aligned OCR pass enough per frame, instead of OCR-ing every phase.
pub fn best_phase<T>(
    image: &RgbaImage,
    slots_for_phase: impl Fn(u32, u32, f32) -> Vec<Slot>,
    cfg: &GridConfig<'_, T>,
) -> f32 {
    let coarse = wf_ocr::Preprocess { scale: 1, threshold: 140, light_text: true };
    (0..PHASE_SAMPLES)
        .map(|p| {
            let phase = p as f32 / PHASE_SAMPLES as f32;
            let score = slots_for_phase(image.width(), image.height(), phase)
                .iter()
                .filter(|s| {
                    // A name line spreads across the crop; a badge is also
                    // text but narrow, so require horizontal spread too.
                    wf_ocr::looks_like_name_line(
                        &crop_rect(image, &s.name),
                        coarse,
                        cfg.max_name_coverage,
                        cfg.min_name_hspan,
                    )
                })
                .count();
            (p, score)
        })
        .max_by_key(|&(_, score)| score)
        .map(|(p, _)| p as f32 / PHASE_SAMPLES as f32)
        .unwrap_or(0.0)
}

/// OCR every visible card in `image` at its best-aligned phase (see
/// [`best_phase`]) and collapse the frame to one observation per resolved
/// identity (see [`dedupe_frame`]). Cards are OCR'd concurrently — each slot
/// shells out to tesseract, so dozens of calls run in parallel across cores
/// instead of serially.
pub fn scan_grid<T: Clone + Eq + Hash + Send>(
    image: &RgbaImage,
    ocr: &wf_ocr::Ocr,
    slots_for_phase: impl Fn(u32, u32, f32) -> Vec<Slot>,
    cfg: &GridConfig<'_, T>,
) -> Vec<Observation<T>> {
    let phase = best_phase(image, &slots_for_phase, cfg);
    let slots = slots_for_phase(image.width(), image.height(), phase);

    let resolved: Vec<Option<(T, ObsKind)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = slots
            .iter()
            .map(|slot| {
                scope.spawn(|| {
                    let name_crop = crop_rect(image, &slot.name);
                    // Even at the aligned phase, some columns land on artwork
                    // or empty cells; skip those before paying for OCR.
                    if !wf_ocr::looks_like_text(&name_crop, cfg.pre, cfg.max_name_coverage) {
                        return None;
                    }
                    // Read as a block: some names wrap onto two lines.
                    let raw = ocr
                        .recognize(&name_crop, cfg.pre, wf_ocr::PageMode::Block)
                        .unwrap_or_default()
                        .replace('\n', " ");
                    let key = (cfg.resolve)(&raw)?;
                    let kind = read_slot_badge(image, slot, ocr, cfg);
                    Some((key, kind))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    dedupe_frame(resolved.into_iter().flatten())
}

/// Resolve one already-identified slot's ownership state: an ownership
/// signal match (the Void Relics screen's "unowned" eye) wins outright and
/// skips the badge read entirely; everything else — including screens with
/// no `ownership_signal` at all, like the Inventory/Sell screen — falls
/// through to reading the count badge.
fn read_slot_badge<T>(
    image: &RgbaImage,
    slot: &Slot,
    ocr: &wf_ocr::Ocr,
    cfg: &GridConfig<'_, T>,
) -> ObsKind {
    match (cfg.ownership_signal, slot.ownership) {
        (Some(signal), Some(window)) if signal(image, &window) => ObsKind::Unowned,
        _ => read_badge(image, &slot.badge, ocr, cfg.pre, cfg.badge_cap),
    }
}

/// Read one card's count badge: a blank crop means the player owns exactly
/// one (both known screens' convention for "no badge shown"); otherwise a
/// strict parse, abstaining on anything unreadable rather than guessing (see
/// [`wf_ocr::parse_badge`]).
pub fn read_badge(
    image: &RgbaImage,
    badge: &Rect,
    ocr: &wf_ocr::Ocr,
    pre: wf_ocr::Preprocess,
    cap: u32,
) -> ObsKind {
    let crop = crop_rect(image, badge);
    if wf_ocr::is_blank(&crop, pre) {
        return ObsKind::Count(1);
    }
    let text = ocr.recognize(&crop, pre, wf_ocr::PageMode::Line).unwrap_or_default();
    match wf_ocr::parse_badge(&text, cap) {
        Some(n) => ObsKind::Count(n),
        None => ObsKind::Abstain,
    }
}

/// Collapse a frame's per-slot reads to one [`Observation`] per identity.
/// Owned reads win over an ownership-signal flag for the same identity
/// (never zero on an ambiguous frame); an owned count is trusted only if
/// every slot that read one agrees, otherwise the frame abstains for that
/// identity.
pub fn dedupe_frame<T: Eq + Hash + Clone>(
    reads: impl Iterator<Item = (T, ObsKind)>,
) -> Vec<Observation<T>> {
    let mut groups: HashMap<T, Vec<ObsKind>> = HashMap::new();
    for (key, kind) in reads {
        groups.entry(key).or_default().push(kind);
    }
    groups
        .into_iter()
        .map(|(key, kinds)| {
            let counts: Vec<u32> = kinds
                .iter()
                .filter_map(|k| if let ObsKind::Count(n) = k { Some(*n) } else { None })
                .collect();
            let owned_reads =
                counts.len() + kinds.iter().filter(|k| matches!(k, ObsKind::Abstain)).count();
            let has_unowned = kinds.iter().any(|k| matches!(k, ObsKind::Unowned));
            let kind = if owned_reads == 0 && has_unowned {
                ObsKind::Unowned
            } else if let Some(&first) = counts.first() {
                // Trust the count only if every slot that read one agrees.
                if counts.iter().all(|&c| c == first) {
                    ObsKind::Count(first)
                } else {
                    ObsKind::Abstain
                }
            } else {
                ObsKind::Abstain // resolved card, but no slot produced a usable count
            };
            Observation { key, kind }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255]))
    }

    /// Paint a horizontal "text line" (bright pixels spread across most
    /// columns of one row) into `img` at row `y`, inside `x0..x0+w`.
    fn paint_text_line(img: &mut RgbaImage, x0: u32, y: u32, w: u32) {
        for x in x0..x0 + w {
            if x < img.width() && y < img.height() {
                img.put_pixel(x, y, image::Rgba([240, 240, 240, 255]));
            }
        }
    }

    #[test]
    fn best_phase_picks_the_aligned_offset() {
        // A single row of "cards" repeating every 100px; the real text sits
        // 37px down from phase 0 — best_phase must land near 0.37.
        let mut img = solid(400, 400, [10, 10, 10]);
        let true_offset = 37u32;
        for col in 0..4 {
            paint_text_line(&mut img, col * 100 + 5, true_offset, 60);
        }

        let cfg: GridConfig<'_, ()> = GridConfig {
            pre: default_grid_preprocess(),
            max_name_coverage: 0.30,
            min_name_hspan: 0.35,
            badge_cap: 99,
            resolve: &|_| None,
            ownership_signal: None,
        };
        let slots_for_phase = |_w: u32, _h: u32, phase: f32| -> Vec<Slot> {
            let y = (100.0 * phase).round() as u32;
            (0..4)
                .map(|col| Slot {
                    name: Rect { x: col * 100, y: y.saturating_sub(4), w: 70, h: 9 },
                    badge: Rect { x: col * 100, y: 0, w: 1, h: 1 },
                    ownership: None,
                })
                .collect()
        };

        let phase = best_phase(&img, slots_for_phase, &cfg);
        let picked_y = (100.0 * phase).round() as i32;
        assert!(
            (picked_y - true_offset as i32).abs() <= 9,
            "picked phase landed at y={picked_y}, expected near {true_offset}"
        );
    }

    #[test]
    fn read_badge_blank_crop_means_one() {
        let ocr = wf_ocr::Ocr::empty_for_test();
        let img = solid(40, 20, [10, 10, 10]);
        let kind = read_badge(
            &img,
            &Rect { x: 0, y: 0, w: 40, h: 20 },
            &ocr,
            default_grid_preprocess(),
            99,
        );
        assert_eq!(kind, ObsKind::Count(1));
    }

    /// Testing Decisions (issue #37): badge-parsing/blank-means-1 behavior is
    /// correct both for a Relics-style config with an eye-icon ownership
    /// signal and for an Inventory-style config with none — exercised here
    /// via `read_slot_badge` directly, so no real (non-blank) crop ever
    /// reaches the OCR pool.
    #[test]
    fn read_slot_badge_blank_means_one_with_no_ownership_signal() {
        // Inventory-style config: the screen has no ownership-signal icon at
        // all, and this slot has no ownership window either.
        let img = solid(40, 20, [10, 10, 10]);
        let slot = Slot {
            name: Rect { x: 0, y: 0, w: 1, h: 1 },
            badge: Rect { x: 0, y: 0, w: 40, h: 20 },
            ownership: None,
        };
        let ocr = wf_ocr::Ocr::empty_for_test();
        let cfg: GridConfig<'_, ()> = GridConfig {
            pre: default_grid_preprocess(),
            max_name_coverage: 0.30,
            min_name_hspan: 0.35,
            badge_cap: 99,
            resolve: &|_| None,
            ownership_signal: None,
        };
        assert_eq!(read_slot_badge(&img, &slot, &ocr, &cfg), ObsKind::Count(1));
    }

    #[test]
    fn read_slot_badge_eye_signal_wins_over_the_badge_read() {
        // Relics-style config: the eye icon matches, so the (blank, would
        // otherwise read as 1) badge is never even consulted.
        let img = solid(40, 20, [10, 10, 10]);
        let slot = Slot {
            name: Rect { x: 0, y: 0, w: 1, h: 1 },
            badge: Rect { x: 0, y: 0, w: 40, h: 20 },
            ownership: Some(Rect { x: 0, y: 0, w: 10, h: 10 }),
        };
        let ocr = wf_ocr::Ocr::empty_for_test();
        let always_eye = |_: &RgbaImage, _: &Rect| true;
        let cfg: GridConfig<'_, ()> = GridConfig {
            pre: default_grid_preprocess(),
            max_name_coverage: 0.30,
            min_name_hspan: 0.35,
            badge_cap: 99,
            resolve: &|_| None,
            ownership_signal: Some(&always_eye),
        };
        assert_eq!(read_slot_badge(&img, &slot, &ocr, &cfg), ObsKind::Unowned);
    }

    #[test]
    fn read_slot_badge_falls_through_to_the_badge_when_the_eye_signal_misses() {
        let img = solid(40, 20, [10, 10, 10]);
        let slot = Slot {
            name: Rect { x: 0, y: 0, w: 1, h: 1 },
            badge: Rect { x: 0, y: 0, w: 40, h: 20 },
            ownership: Some(Rect { x: 0, y: 0, w: 10, h: 10 }),
        };
        let ocr = wf_ocr::Ocr::empty_for_test();
        let never_eye = |_: &RgbaImage, _: &Rect| false;
        let cfg: GridConfig<'_, ()> = GridConfig {
            pre: default_grid_preprocess(),
            max_name_coverage: 0.30,
            min_name_hspan: 0.35,
            badge_cap: 99,
            resolve: &|_| None,
            ownership_signal: Some(&never_eye),
        };
        // Eye signal missed → falls through to the (blank → 1) badge read.
        assert_eq!(read_slot_badge(&img, &slot, &ocr, &cfg), ObsKind::Count(1));
    }

    #[test]
    fn dedupe_frame_trusts_agreeing_counts_and_abstains_on_disagreement() {
        let reads = vec![
            ("A".to_string(), ObsKind::Count(5)),
            ("A".to_string(), ObsKind::Count(5)),
            ("B".to_string(), ObsKind::Count(3)),
            ("B".to_string(), ObsKind::Count(9)), // disagrees with the first
        ];
        let out = dedupe_frame(reads.into_iter());
        let a = out.iter().find(|o| o.key == "A").unwrap();
        assert_eq!(a.kind, ObsKind::Count(5));
        let b = out.iter().find(|o| o.key == "B").unwrap();
        assert_eq!(b.kind, ObsKind::Abstain);
    }

    #[test]
    fn dedupe_frame_owned_read_wins_over_unowned_signal() {
        let reads = vec![
            ("A".to_string(), ObsKind::Unowned),
            ("A".to_string(), ObsKind::Count(2)),
        ];
        let out = dedupe_frame(reads.into_iter());
        assert_eq!(out[0].kind, ObsKind::Count(2));
    }

    #[test]
    fn dedupe_frame_pure_unowned_signal_reports_unowned() {
        let reads = vec![("A".to_string(), ObsKind::Unowned)];
        let out = dedupe_frame(reads.into_iter());
        assert_eq!(out[0].kind, ObsKind::Unowned);
    }

    #[test]
    fn scan_grid_skips_blank_slots_before_ever_reaching_ocr() {
        // A fully blank frame: every candidate name crop fails
        // `looks_like_text`, so scan_grid must never call `resolve` or reach
        // the OCR pool (a stub pool would panic on a real recognize() call —
        // its absence here is the proof this path was never taken).
        let img = solid(300, 300, [10, 10, 10]);
        let panics_if_called = |_: &str| -> Option<String> {
            panic!("resolve must not be called for a slot that never looked like text")
        };
        let cfg = GridConfig {
            pre: default_grid_preprocess(),
            max_name_coverage: 0.30,
            min_name_hspan: 0.35,
            badge_cap: 99,
            resolve: &panics_if_called,
            ownership_signal: None,
        };
        let slots_for_phase = |_w: u32, _h: u32, _phase: f32| -> Vec<Slot> {
            vec![Slot {
                name: Rect { x: 0, y: 45, w: 120, h: 10 },
                badge: Rect { x: 200, y: 200, w: 20, h: 10 },
                ownership: None,
            }]
        };
        let ocr = wf_ocr::Ocr::empty_for_test();
        let out = scan_grid(&img, &ocr, slots_for_phase, &cfg);
        assert!(out.is_empty());
    }
}
