//! A tiny straight-alpha RGBA canvas with just enough drawing for the overlay
//! panel: filled (optionally rounded) rectangles, anti-aliased text via fontdue,
//! and the bundled mastery-icon blit.

use std::sync::OnceLock;

use fontdue::Font;
use image::RgbaImage;

/// The Warframe `<MASTERED>` laurel-wreath icon (a game UI asset), bundled so the
/// reward panel can mark mastered rewards. Decoded once on first use.
fn mastery_icon() -> Option<&'static RgbaImage> {
    static ICON: OnceLock<Option<RgbaImage>> = OnceLock::new();
    ICON.get_or_init(|| {
        let bytes = include_bytes!("../assets/mastered.png");
        match image::load_from_memory(bytes) {
            Ok(img) => Some(img.to_rgba8()),
            Err(e) => {
                tracing::error!("failed to decode bundled mastery icon: {e}");
                None
            }
        }
    })
    .as_ref()
}

/// RGBA colour with straight (non-premultiplied) alpha, 0–255 per channel.
#[derive(Debug, Clone, Copy)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color(r, g, b, 255)
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color(r, g, b, a)
    }
}

/// A top-left-origin RGBA8 (straight alpha) pixel buffer.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, RGBA.
    pub buf: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            buf: vec![0; (width * height * 4) as usize],
        }
    }

    /// Source-over blend one pixel using straight alpha.
    #[inline]
    fn blend(&mut self, x: i32, y: i32, c: Color, coverage: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let sa = (c.3 as f32 / 255.0) * coverage.clamp(0.0, 1.0);
        if sa <= 0.0 {
            return;
        }
        let idx = ((y as u32 * self.width + x as u32) * 4) as usize;
        let (dr, dg, db, da) = (
            self.buf[idx] as f32 / 255.0,
            self.buf[idx + 1] as f32 / 255.0,
            self.buf[idx + 2] as f32 / 255.0,
            self.buf[idx + 3] as f32 / 255.0,
        );
        let (sr, sg, sb) = (c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0);
        let out_a = sa + da * (1.0 - sa);
        let mix = |s: f32, d: f32| {
            if out_a <= 0.0 {
                0.0
            } else {
                (s * sa + d * da * (1.0 - sa)) / out_a
            }
        };
        self.buf[idx] = (mix(sr, dr) * 255.0).round() as u8;
        self.buf[idx + 1] = (mix(sg, dg) * 255.0).round() as u8;
        self.buf[idx + 2] = (mix(sb, db) * 255.0).round() as u8;
        self.buf[idx + 3] = (out_a * 255.0).round() as u8;
    }

    /// Fill a rectangle with optional corner radius (in pixels).
    pub fn fill_round_rect(&mut self, x: i32, y: i32, w: u32, h: u32, radius: u32, c: Color) {
        let r = radius as f32;
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                // Anti-alias the rounded corners by distance from the corner arc centre.
                let coverage = corner_coverage(col as f32, row as f32, w as f32, h as f32, r);
                if coverage > 0.0 {
                    self.blend(x + col, y + row, c, coverage);
                }
            }
        }
    }

    /// Draw the mastery emblem — Warframe's `<MASTERED>` **laurel wreath** icon,
    /// bundled as a PNG — inside the `w`×`h` box at top-left `(x, y)`, tinted to
    /// colour `c`. The source is a white shape on a transparent background, so each
    /// source pixel's luminance × alpha becomes the paint coverage. Used to flag
    /// rewards whose prime the player has already mastered.
    pub fn draw_mastery_mark(&mut self, x: i32, y: i32, w: u32, h: u32, c: Color) {
        let Some(icon) = mastery_icon() else {
            return; // icon failed to decode; skip the emblem rather than crash
        };
        // Area-filter down to the target size for a clean small emblem.
        let scaled = image::imageops::resize(icon, w, h, image::imageops::FilterType::Triangle);
        for (px, py, pixel) in scaled.enumerate_pixels() {
            let [r, g, b, a] = pixel.0;
            let luma = (r as f32 + g as f32 + b as f32) / (3.0 * 255.0);
            let coverage = luma * (a as f32 / 255.0);
            if coverage > 0.0 {
                self.blend(x + px as i32, y + py as i32, c, coverage);
            }
        }
    }

    /// Draw a run of text with its baseline at `baseline_y`, returning the pen x
    /// position after the last glyph (so callers can chain coloured segments).
    pub fn draw_text(
        &mut self,
        font: &Font,
        text: &str,
        x: f32,
        baseline_y: f32,
        px: f32,
        c: Color,
    ) -> f32 {
        let mut pen_x = x;
        for ch in text.chars() {
            let (m, bitmap) = font.rasterize(ch, px);
            let gx = pen_x + m.xmin as f32;
            let gy = baseline_y - m.ymin as f32 - m.height as f32;
            for row in 0..m.height {
                for col in 0..m.width {
                    let cov = bitmap[row * m.width + col] as f32 / 255.0;
                    if cov > 0.0 {
                        self.blend(gx as i32 + col as i32, gy as i32 + row as i32, c, cov);
                    }
                }
            }
            pen_x += m.advance_width;
        }
        pen_x
    }

    /// Measure the pixel width a text run would advance to.
    pub fn text_width(font: &Font, text: &str, px: f32) -> f32 {
        text.chars()
            .map(|ch| font.metrics(ch, px).advance_width)
            .sum()
    }

    /// Return a new `width`×`height` transparent canvas with `self` copied to
    /// the top-left. Used to place variable-height panels onto a fixed-size
    /// overlay surface (the remainder stays transparent / click-through).
    pub fn embed(&self, width: u32, height: u32) -> Canvas {
        let mut out = Canvas::new(width, height);
        let copy_w = self.width.min(width);
        let copy_h = self.height.min(height);
        for y in 0..copy_h {
            let src = ((y * self.width) * 4) as usize;
            let dst = ((y * width) * 4) as usize;
            let n = (copy_w * 4) as usize;
            out.buf[dst..dst + n].copy_from_slice(&self.buf[src..src + n]);
        }
        out
    }

    /// Scale every pixel's alpha by `factor` (`0.0`–`1.0`), making the whole
    /// canvas more transparent so it obscures less of what's behind it. A factor
    /// of `1.0` (or more) is a no-op.
    pub fn scale_alpha(&mut self, factor: f32) {
        if factor >= 1.0 {
            return;
        }
        let f = factor.clamp(0.0, 1.0);
        for px in self.buf.chunks_exact_mut(4) {
            px[3] = (px[3] as f32 * f).round() as u8;
        }
    }

    /// Pack into premultiplied ARGB8888 (native-endian `0xAARRGGBB`) for wl_shm.
    pub fn to_argb_premul(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.buf.len()];
        for (i, px) in self.buf.chunks_exact(4).enumerate() {
            let a = px[3] as u32;
            let pm = |ch: u8| ((ch as u32 * a) / 255) as u8;
            let (r, g, b) = (pm(px[0]), pm(px[1]), pm(px[2]));
            let word = ((a as u8 as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_ne_bytes());
        }
        out
    }
}

/// Coverage (0..1) for a pixel of a rounded rectangle at local `(px, py)` within
/// a `w x h` rect with corner radius `r`. Interior pixels return 1.0; corner
/// pixels are anti-aliased by their distance to the arc.
fn corner_coverage(px: f32, py: f32, w: f32, h: f32, r: f32) -> f32 {
    if r <= 0.0 {
        return 1.0;
    }
    // Determine which corner (if any) this pixel is in.
    let cx = if px < r {
        r
    } else if px > w - r {
        w - r
    } else {
        return 1.0; // horizontal middle band → always inside
    };
    let cy = if py < r {
        r
    } else if py > h - r {
        h - r
    } else {
        return 1.0; // vertical middle band → always inside
    };
    let d = ((px + 0.5 - cx).powi(2) + (py + 0.5 - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_over_transparent_gives_source() {
        let mut c = Canvas::new(2, 2);
        c.blend(0, 0, Color::rgb(200, 100, 50), 1.0);
        assert_eq!(&c.buf[0..4], &[200, 100, 50, 255]);
    }

    #[test]
    fn out_of_bounds_is_ignored() {
        let mut c = Canvas::new(2, 2);
        c.blend(-1, 0, Color::rgb(255, 255, 255), 1.0);
        c.blend(5, 5, Color::rgb(255, 255, 255), 1.0);
        assert!(c.buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn scale_alpha_dims_opacity() {
        let mut c = Canvas::new(1, 1);
        c.buf.copy_from_slice(&[10, 20, 30, 200]);
        c.scale_alpha(0.5);
        assert_eq!(c.buf[3], 100);
        // colour channels are untouched (straight alpha).
        assert_eq!(&c.buf[0..3], &[10, 20, 30]);
    }

    #[test]
    fn mastery_mark_draws_symmetric_wreath() {
        let (w, h) = (20u32, 16u32);
        let mut c = Canvas::new(w, h);
        c.draw_mastery_mark(0, 0, w, h, Color::rgb(130, 200, 140));
        let opaque = |x: u32, y: u32| c.buf[((y * w + x) * 4 + 3) as usize] > 0;
        // The wreath draws a meaningful number of pixels…
        let total = c.buf.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(total > 60, "wreath should cover many pixels, got {total}");
        // …and it is left/right symmetric (branches mirror around the centre).
        let left = (0..h).flat_map(|y| (0..w / 2).map(move |x| (x, y))).filter(|&(x, y)| opaque(x, y)).count();
        let right = (0..h).flat_map(|y| (w / 2..w).map(move |x| (x, y))).filter(|&(x, y)| opaque(x, y)).count();
        assert!((left as i32 - right as i32).abs() <= 6, "branches should be roughly symmetric ({left} vs {right})");
    }

    #[test]
    fn scale_alpha_one_is_noop() {
        let mut c = Canvas::new(1, 1);
        c.buf.copy_from_slice(&[10, 20, 30, 200]);
        c.scale_alpha(1.0);
        assert_eq!(c.buf[3], 200);
    }

    #[test]
    fn premultiply_halves_channels_at_half_alpha() {
        let mut c = Canvas::new(1, 1);
        c.buf.copy_from_slice(&[200, 100, 40, 128]);
        let argb = c.to_argb_premul();
        let word = u32::from_ne_bytes([argb[0], argb[1], argb[2], argb[3]]);
        let a = (word >> 24) & 0xff;
        let r = (word >> 16) & 0xff;
        assert_eq!(a, 128);
        assert_eq!(r, (200 * 128) / 255);
    }
}
