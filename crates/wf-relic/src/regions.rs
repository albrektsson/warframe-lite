//! Screen regions of the reward names on the Void Fissure reward screen.
//!
//! The reward cards are **centred on the screen centre** with a fixed pitch, and
//! the number of cards varies (2–4, depending on how many squadmates cracked a
//! relic). Rather than assume four fixed slots, we scan a superset of candidate
//! centres spaced at half the pitch — the union of the 2/3/4-card layouts — and
//! keep whichever ones actually OCR to an item (see `select_rewards`). Long names
//! wrap to two lines, so each crop is tall enough for two lines and is OCR'd as a
//! block.
//!
//! Calibrated against captured 3440×1440 reward screens (both a 4-reward and a
//! 3-reward screen), and verified against a captured 2560×1440 4-reward screen
//! (see AGENT.md's "Environment specifics" for the recalibration process). The
//! panel scales with the capture's **height**, not its width, and stays
//! centred on the capture's actual screen centre rather than a width-scaled
//! position.

/// A rectangle in screen/window pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Centred-layout parameters at a reference resolution.
#[derive(Debug, Clone)]
pub struct RewardRegions {
    pub ref_width: u32,
    pub ref_height: u32,
    /// Horizontal screen centre (cards are centred here).
    pub center_x: u32,
    /// Distance between adjacent card centres.
    pub pitch: u32,
    /// Top of the name crop.
    pub name_y: u32,
    /// Height of the name crop (tall enough for a two-line name).
    pub name_h: u32,
    /// Width of the name crop.
    pub name_w: u32,
}

impl RewardRegions {
    /// Default calibration, measured on 3440×1440 reward screens.
    pub fn default_calibration() -> Self {
        Self {
            ref_width: 3440,
            ref_height: 1440,
            center_x: 1708,
            pitch: 316,
            name_y: 555,
            name_h: 88,
            name_w: 330,
        }
    }

    /// Candidate name rectangles for an actual `width`×`height` capture.
    ///
    /// Seven centres spaced at `pitch/2` around the screen centre — the union of
    /// the 2-, 3- and 4-card layouts. Even-indexed centres are the 4-card slots;
    /// odd-indexed centres are the 3-card slots.
    ///
    /// All magnitudes (pitch, name size) scale with `height` — the panel keeps
    /// its on-screen size relative to vertical resolution regardless of aspect
    /// ratio, and stays centred on the capture's actual horizontal centre
    /// (offset by the reference calibration's own centre bias, scaled the same
    /// way) rather than a width-scaled `center_x`.
    pub fn candidate_slots(&self, width: u32, height: u32) -> Vec<Rect> {
        let sy = height as f32 / self.ref_height as f32;
        let center_bias = self.ref_width as f32 / 2.0 - self.center_x as f32;
        let cx = width as f32 / 2.0 - center_bias * sy;
        let half = self.pitch as f32 * sy / 2.0;
        let w = (self.name_w as f32 * sy).round() as u32;
        let h = (self.name_h as f32 * sy).round() as u32;
        let y = (self.name_y as f32 * sy).round() as u32;

        (0..7)
            .map(|i| {
                let center = cx + (i as f32 - 3.0) * half;
                let x = (center - w as f32 / 2.0).max(0.0).round() as u32;
                Rect { x, y, w, h }
            })
            .collect()
    }
}

/// The name + count-badge rectangles for one relic card in the Void Relics grid.
#[derive(Debug, Clone, Copy)]
pub struct RelicSlot {
    /// The relic name text, e.g. "Neo T2 Relic".
    pub name: Rect,
    /// The owned-count badge, e.g. "x62".
    pub count: Rect,
    /// Search window for the "unowned" eye icon (present only on relics the
    /// player does *not* own). A match here means the card should be skipped.
    pub eye: Rect,
}

/// The Void **Relics/Refinement** inventory grid: a regular grid of cards, each
/// with an `xNN` count badge (top-left) and the relic name below the orb.
/// Calibrated against a captured 3440×1440 Relics screen.
#[derive(Debug, Clone)]
pub struct RelicGridRegions {
    pub ref_width: u32,
    pub ref_height: u32,
    pub cols: u32,
    pub rows: u32,
    /// Centre x of the first column.
    pub col0_cx: u32,
    /// Distance between adjacent column centres.
    pub col_pitch: u32,
    /// Centre y of the first row's name text.
    pub name_cy0: u32,
    /// Distance between adjacent row name centres.
    pub row_pitch: u32,
    pub name_w: u32,
    pub name_h: u32,
    /// Count-badge centre offset from the card's (column centre, name centre).
    pub count_dx: i32,
    pub count_dy: i32,
    pub count_w: u32,
    pub count_h: u32,
    /// "Unowned" eye-icon search window centre offset from the card centre, and
    /// its size (larger than the icon, to tolerate column-pitch drift).
    pub eye_dx: i32,
    pub eye_dy: i32,
    pub eye_w: u32,
    pub eye_h: u32,
}

impl RelicGridRegions {
    /// Default calibration, measured on 3440×1440 Relics screens (8 columns).
    pub fn default_calibration() -> Self {
        Self {
            ref_width: 3440,
            ref_height: 1440,
            cols: 8,
            col0_cx: 238,
            col_pitch: 283,
            // Phase-tiling anchor: the first name band near the top of the grid.
            // With continuous scroll the real rows sit at an arbitrary sub-pitch
            // offset from here, so absolute placement matters far less than the
            // dense `row_phases` sampling that tiles a whole `row_pitch` (below).
            name_cy0: 350,
            row_pitch: 283,
            name_w: 250,
            // Tall enough for a two-line name: a refined relic wraps its
            // "[Radiant]"/"[Exceptional]"/… suffix onto a second line, and the
            // suffix is the *only* thing distinguishing it from the Intact card —
            // so the name crop must capture both lines (OCR'd as a block).
            name_h: 84,
            // Measured against the reference capture: the "xN" badge sits at the
            // card's top-left, ~79px left of and ~180px above the name centre.
            count_dx: -79,
            count_dy: -180,
            count_w: 120,
            count_h: 48,
            eye_dx: -70,
            eye_dy: -65,
            eye_w: 100,
            eye_h: 62,
            // Six rows covers the visible grid from `name_cy0` to the bottom at
            // the reference height. The Relics list scrolls *continuously*, so
            // the caller picks the single best-aligned vertical phase per frame
            // (see `scan_relic_grid`) rather than baking one fixed offset in.
            rows: 6,
        }
    }

    /// Name + count + eye rectangles for every visible card at one vertical
    /// **phase**, scaled to an actual `width`×`height` capture. `phase` in
    /// `0.0..1.0` shifts every row down by that fraction of a `row_pitch`: the
    /// Relics list scrolls continuously, so the real rows sit at an arbitrary
    /// sub-pitch offset the caller finds per frame (see `scan_relic_grid`, which
    /// scores a handful of phases by ink coverage and keeps the best-aligned
    /// one). Rectangles that fall outside the frame are clamped; slots that land
    /// on empty space or artwork are dropped by the caller.
    pub fn slots(&self, width: u32, height: u32, phase: f32) -> Vec<RelicSlot> {
        let sx = width as f32 / self.ref_width as f32;
        let sy = height as f32 / self.ref_height as f32;
        let rect_centered = |cx: f32, cy: f32, w: f32, h: f32| Rect {
            x: (cx - w / 2.0).max(0.0).round() as u32,
            y: (cy - h / 2.0).max(0.0).round() as u32,
            w: w.round() as u32,
            h: h.round() as u32,
        };
        let phase_off = self.row_pitch as f32 * phase;
        let mut out = Vec::with_capacity((self.cols * self.rows) as usize);
        for row in 0..self.rows {
            for col in 0..self.cols {
                let cx = (self.col0_cx + col * self.col_pitch) as f32 * sx;
                let ncy = (self.name_cy0 as f32 + phase_off + (row * self.row_pitch) as f32) * sy;
                let name = rect_centered(cx, ncy, self.name_w as f32 * sx, self.name_h as f32 * sy);
                let count = rect_centered(
                    cx + self.count_dx as f32 * sx,
                    ncy + self.count_dy as f32 * sy,
                    self.count_w as f32 * sx,
                    self.count_h as f32 * sy,
                );
                let eye = rect_centered(
                    cx + self.eye_dx as f32 * sx,
                    ncy + self.eye_dy as f32 * sy,
                    self.eye_w as f32 * sx,
                    self.eye_h as f32 * sy,
                );
                out.push(RelicSlot { name, count, eye });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relic_grid_slots_cover_the_grid() {
        let g = RelicGridRegions::default_calibration();
        let slots = g.slots(3440, 1440, 0.0);
        assert_eq!(slots.len(), (g.cols * g.rows) as usize);
        // At phase 0 the first card's name is centred on (col0_cx, name_cy0).
        let n0 = slots[0].name;
        assert_eq!(n0.x + n0.w / 2, g.col0_cx);
        assert_eq!(n0.y + n0.h / 2, g.name_cy0);
        // Count badge sits up-and-left of the name.
        let c0 = slots[0].count;
        assert!((c0.x + c0.w / 2) < g.col0_cx && (c0.y + c0.h / 2) < g.name_cy0);
        // Second column is one pitch right.
        assert_eq!(slots[1].name.x + slots[1].name.w / 2, 238 + 283);
    }

    #[test]
    fn a_vertical_phase_shifts_rows_down_by_a_pitch_fraction() {
        let g = RelicGridRegions::default_calibration();
        let base = g.slots(3440, 1440, 0.0);
        let shifted = g.slots(3440, 1440, 1.0 / 3.0);
        // Phase 1/3 pushes row 0's name a third of a row_pitch below phase 0's —
        // an intermediate scroll offset phase 0 alone wouldn't catch.
        let expected = g.row_pitch / 3;
        assert!(
            (shifted[0].name.y as i32 - base[0].name.y as i32 - expected as i32).abs() <= 1,
            "shifted y={}, base y={}, expected offset={expected}",
            shifted[0].name.y,
            base[0].name.y
        );
        // Columns are unaffected by the vertical phase.
        assert_eq!(shifted[0].name.x, base[0].name.x);
    }

    #[test]
    fn produces_seven_centered_slots() {
        let r = RewardRegions::default_calibration();
        let slots = r.candidate_slots(3440, 1440);
        assert_eq!(slots.len(), 7);
        // Middle slot (index 3) is centred on the screen centre.
        let mid = &slots[3];
        assert_eq!(mid.x + mid.w / 2, 1708);
        // Even slots are the 4-card centres (~1234 and ~2182 at the ends).
        assert!((slots[0].x as i32 + slots[0].w as i32 / 2 - 1234).abs() <= 1);
        assert!((slots[6].x as i32 + slots[6].w as i32 / 2 - 2182).abs() <= 1);
    }

    #[test]
    fn scales_with_resolution() {
        let r = RewardRegions::default_calibration();
        let slots = r.candidate_slots(1720, 720); // half
        assert_eq!(slots[3].x + slots[3].w / 2, 854); // 1708/2
    }

    #[test]
    fn scales_with_height_not_width_on_a_narrower_aspect_ratio() {
        // Pinned against a real captured 2560x1440 4-reward screen (AGENT.md).
        let r = RewardRegions::default_calibration();
        let slots = r.candidate_slots(2560, 1440);
        let mid = &slots[3];
        assert_eq!(mid.x + mid.w / 2, 1268); // 2560/2 - (3440/2 - 1708)
        assert_eq!(slots[0].x + slots[0].w / 2, 794);
        assert_eq!(slots[6].x + slots[6].w / 2, 1742);
        assert_eq!(mid.w, 330); // unscaled: same height as the reference
        assert_eq!(mid.h, 88);
    }
}
