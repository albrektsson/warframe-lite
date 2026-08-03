//! OCR via the Tesseract CLI.
//!
//! We shell out to the `tesseract` binary rather than linking `libtesseract`,
//! so the build stays pure-cargo and works regardless of where Tesseract comes
//! from (Homebrew, distrobox, system) — only a reachable `tesseract` binary is
//! required. OCR runs at most once per reward screen, so process-spawn overhead
//! is irrelevant.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use image::{GrayImage, RgbaImage};

pub mod count;
pub use count::{parse_badge, Tally};

/// Tesseract page-segmentation mode for a recognition call.
#[derive(Debug, Clone, Copy)]
pub enum PageMode {
    /// One uniform block of text (`--psm 6`).
    Block,
    /// A single text line (`--psm 7`) — good for one reward name.
    Line,
    /// A single word (`--psm 8`).
    Word,
}

impl PageMode {
    fn psm(self) -> &'static str {
        match self {
            PageMode::Block => "6",
            PageMode::Line => "7",
            PageMode::Word => "8",
        }
    }
}

/// Preprocessing options tuned for Warframe's light-on-dark UI text.
#[derive(Debug, Clone, Copy)]
pub struct Preprocess {
    /// Integer upscale factor before thresholding (Tesseract likes ~30px caps).
    pub scale: u32,
    /// Luminance threshold (0–255) for binarisation.
    pub threshold: u8,
    /// True when the source text is lighter than its background (Warframe).
    pub light_text: bool,
}

impl Default for Preprocess {
    fn default() -> Self {
        Self {
            scale: 3,
            threshold: 140,
            light_text: true,
        }
    }
}

/// A configured OCR engine (a located `tesseract` binary + language).
pub struct Ocr {
    bin: String,
    lang: String,
}

impl Ocr {
    /// Locate a usable `tesseract` binary (env `WF_TESSERACT`, then `PATH`, then
    /// common install locations) and default to English.
    pub fn new() -> Result<Self> {
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(env) = std::env::var("WF_TESSERACT") {
            candidates.push(env);
        }
        candidates.push("tesseract".to_string());
        candidates.push("/home/linuxbrew/.linuxbrew/bin/tesseract".to_string());
        candidates.push("/usr/bin/tesseract".to_string());

        for bin in candidates {
            if Command::new(&bin)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                tracing::info!("using tesseract: {bin}");
                return Ok(Self {
                    bin,
                    lang: "eng".to_string(),
                });
            }
        }
        anyhow::bail!(
            "no working `tesseract` found (set WF_TESSERACT or install via `brew install tesseract`)"
        )
    }

    /// Override the binary path and language.
    pub fn with_bin(bin: impl Into<String>, lang: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            lang: lang.into(),
        }
    }

    /// Recognise text in `img`, applying `pre` preprocessing and `mode` layout.
    ///
    /// Skips the tesseract call entirely (returning an empty string) when the
    /// preprocessed crop has essentially no text pixels: it's a wasted
    /// subprocess spawn, and some tesseract builds are known to crash with
    /// SIGFPE in `--psm 7` (single-line) row-cleanup on a near-blank image —
    /// this is a real, reproducible crash (`Textord::CleanupSingleRowResult`)
    /// hit while scanning the Void Relics grid, where most candidate crops on
    /// any given frame are legitimately blank (empty grid cells, or slots that
    /// land on artwork rather than text while the list scrolls).
    pub fn recognize(&self, img: &RgbaImage, pre: Preprocess, mode: PageMode) -> Result<String> {
        let processed = preprocess(img, pre);
        if text_fraction(&processed) < MIN_TEXT_FRACTION {
            return Ok(String::new());
        }
        let path = temp_png_path();
        processed
            .save(&path)
            .with_context(|| format!("writing preprocessed image {}", path.display()))?;

        let result = self.run_tesseract(&path, mode);
        let _ = std::fs::remove_file(&path); // best-effort cleanup
        result
    }

    fn run_tesseract(&self, image_path: &std::path::Path, mode: PageMode) -> Result<String> {
        let output = Command::new(&self.bin)
            .arg(image_path)
            .arg("stdout")
            .args(["-l", &self.lang])
            .args(["--psm", mode.psm()])
            .output()
            .with_context(|| format!("running {}", self.bin))?;

        if !output.status.success() {
            anyhow::bail!(
                "tesseract failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

/// Below this fraction of "text" (dark, post-binarisation) pixels, a crop is
/// treated as blank and never handed to tesseract. Calibrated well below any
/// real (even short, e.g. a two-letter relic code) text crop's coverage, so
/// only genuinely empty/background crops are skipped.
const MIN_TEXT_FRACTION: f32 = 0.001;

/// Whether `img` has essentially no text after `pre` preprocessing — i.e. the
/// crop is blank background, not recognisable glyphs. The relic scanner uses
/// this to tell a *missing* count badge (which, on the Void Relics screen, means
/// the player owns exactly one copy) apart from a badge that's present but
/// unreadable (which should cast no count vote — see ADR-0005).
pub fn is_blank(img: &RgbaImage, pre: Preprocess) -> bool {
    text_fraction(&preprocess(img, pre)) < MIN_TEXT_FRACTION
}

/// Fraction of `img` classified as ink (text-coloured) after `pre`
/// preprocessing, in `0.0..=1.0`. A line of text covers only a small fraction;
/// a solid graphic (a relic orb, item artwork) covers a large one. Grid
/// scanners use this to skip candidate crops that landed on artwork rather than
/// a name line before paying for an OCR call — see [`looks_like_text`].
pub fn text_coverage(img: &RgbaImage, pre: Preprocess) -> f32 {
    text_fraction(&preprocess(img, pre))
}

/// Whether a preprocessed crop's ink coverage falls in the band typical of a
/// **line of text** — above the blank floor, below the density of solid
/// artwork. Lets a scanner reject both empty gaps and relic-orb/artwork crops
/// cheaply, so dense candidate sampling doesn't spend an OCR call on every one.
pub fn looks_like_text(img: &RgbaImage, pre: Preprocess, max_coverage: f32) -> bool {
    let c = text_coverage(img, pre);
    c >= MIN_TEXT_FRACTION && c <= max_coverage
}

/// Whether a crop looks like a **full line of text** — text-band coverage *and*
/// ink spread across a good fraction of its width. The extra width test tells a
/// wide name line apart from a short label that is also text-like but occupies
/// only a narrow strip (a relic card's `xN` count badge), so grid phase-
/// alignment locks onto the name row rather than the badge row above it. One
/// preprocess pass computes both signals.
pub fn looks_like_name_line(
    img: &RgbaImage,
    pre: Preprocess,
    max_coverage: f32,
    min_hspan: f32,
) -> bool {
    let g = preprocess(img, pre);
    let (w, h) = (g.width(), g.height());
    if w == 0 || h == 0 {
        return false;
    }
    let mut ink = 0u64;
    let mut cols_with_ink = 0u32;
    for x in 0..w {
        let mut col_has_ink = false;
        for y in 0..h {
            if g.get_pixel(x, y).0[0] == 0 {
                ink += 1;
                col_has_ink = true;
            }
        }
        if col_has_ink {
            cols_with_ink += 1;
        }
    }
    let coverage = ink as f32 / (w as u64 * h as u64) as f32;
    let hspan = cols_with_ink as f32 / w as f32;
    coverage >= MIN_TEXT_FRACTION && coverage <= max_coverage && hspan >= min_hspan
}

/// Fraction of pixels classified as "text" (dark) in a preprocessed image.
fn text_fraction(img: &GrayImage) -> f32 {
    let total = (img.width() as u64 * img.height() as u64).max(1);
    let dark = img.pixels().filter(|p| p.0[0] == 0).count() as u64;
    dark as f32 / total as f32
}

/// Binarise `img` for OCR: upscale, convert to luminance, and threshold so the
/// text ends up dark on a light background (what Tesseract expects).
pub fn preprocess(img: &RgbaImage, pre: Preprocess) -> GrayImage {
    let scale = pre.scale.max(1);
    let scaled = if scale > 1 {
        image::imageops::resize(
            img,
            img.width() * scale,
            img.height() * scale,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img.clone()
    };

    let mut out = GrayImage::new(scaled.width(), scaled.height());
    for (x, y, px) in scaled.enumerate_pixels() {
        let [r, g, b, _] = px.0;
        // Rec. 601 luma.
        let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as u8;
        let is_text = if pre.light_text {
            luma > pre.threshold
        } else {
            luma < pre.threshold
        };
        out.put_pixel(x, y, image::Luma([if is_text { 0 } else { 255 }]));
    }
    out
}

fn temp_png_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("wf-ocr-{}-{}.png", std::process::id(), nanos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_binarises_light_text() {
        // 2x2 image: two bright pixels (text) and two dark (background).
        let mut img = RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([240, 240, 240, 255])); // bright → text
        img.put_pixel(1, 0, image::Rgba([10, 10, 10, 255])); // dark → bg
        let g = preprocess(
            &img,
            Preprocess {
                scale: 1,
                threshold: 140,
                light_text: true,
            },
        );
        assert_eq!(g.get_pixel(0, 0).0[0], 0); // text → black
        assert_eq!(g.get_pixel(1, 0).0[0], 255); // bg → white
    }

    #[test]
    fn psm_mapping() {
        assert_eq!(PageMode::Line.psm(), "7");
        assert_eq!(PageMode::Block.psm(), "6");
    }

    #[test]
    fn name_line_needs_width_not_just_ink() {
        let pre = Preprocess { scale: 1, threshold: 140, light_text: true };
        // A wide line: bright (text) pixels spread across most columns.
        let mut wide = RgbaImage::from_pixel(40, 8, image::Rgba([10, 10, 10, 255]));
        for x in 2..38 {
            wide.put_pixel(x, 4, image::Rgba([240, 240, 240, 255]));
        }
        assert!(looks_like_name_line(&wide, pre, 0.30, 0.35));

        // A narrow blob (like an "xN" badge) has ink but only in a few columns —
        // rejected, so phase alignment won't lock onto the badge row.
        let mut narrow = RgbaImage::from_pixel(40, 8, image::Rgba([10, 10, 10, 255]));
        for x in 0..5 {
            for y in 0..8 {
                narrow.put_pixel(x, y, image::Rgba([240, 240, 240, 255]));
            }
        }
        assert!(!looks_like_name_line(&narrow, pre, 0.30, 0.35));

        // A solid bright block (a relic orb) is too dense — rejected by coverage.
        let dense = RgbaImage::from_pixel(40, 8, image::Rgba([240, 240, 240, 255]));
        assert!(!looks_like_name_line(&dense, pre, 0.30, 0.35));
    }

    #[test]
    fn text_fraction_distinguishes_blank_from_text() {
        let blank = GrayImage::from_pixel(20, 20, image::Luma([255]));
        assert!(text_fraction(&blank) < MIN_TEXT_FRACTION);

        let mut some_text = GrayImage::from_pixel(20, 20, image::Luma([255]));
        for x in 5..15 {
            some_text.put_pixel(x, 10, image::Luma([0]));
        }
        assert!(text_fraction(&some_text) >= MIN_TEXT_FRACTION);
    }

    #[test]
    fn recognize_skips_tesseract_on_blank_crop() {
        // A blank crop must resolve to an empty string without ever invoking a
        // `tesseract` binary — use a path that doesn't exist, so any attempt to
        // actually run it would fail loudly rather than silently succeed.
        let ocr = Ocr::with_bin("/nonexistent/tesseract", "eng");
        let blank = RgbaImage::from_pixel(20, 20, image::Rgba([10, 10, 10, 255]));
        let result = ocr.recognize(&blank, Preprocess::default(), PageMode::Line);
        assert_eq!(result.unwrap(), "");
    }
}
