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
//! 3-reward screen).

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
    pub fn candidate_slots(&self, width: u32, height: u32) -> Vec<Rect> {
        let sx = width as f32 / self.ref_width as f32;
        let sy = height as f32 / self.ref_height as f32;
        let cx = self.center_x as f32 * sx;
        let half = self.pitch as f32 * sx / 2.0;
        let w = (self.name_w as f32 * sx).round() as u32;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
