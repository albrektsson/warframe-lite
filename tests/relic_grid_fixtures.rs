//! Regression test for the Void Relics grid scanner against real captured
//! frames (`tests/fixtures/ocr/`). This guards the calibration: a mis-set region
//! silently reads *zero* cards (the bug these fixtures were added to catch), so
//! the test asserts a healthy fraction of known relics still resolve.
//!
//! It drives the real `wf-lite relic-grid-file` command rather than reaching
//! into the binary's internals, and **self-skips** when the environment can't
//! run OCR — no `tesseract` on PATH, or no cached relic catalogue (a fresh CI
//! box has neither), so the suite stays green there. Run it locally with a
//! populated `~/.cache/warframe-lite/` and Tesseract installed.

use std::path::PathBuf;
use std::process::Command;

/// Relics the scanner should resolve from the reference frame. Deliberately a
/// subset of the visible cards, and the assertion only requires a majority: a
/// single frame reads one aligned scroll phase and OCR is fuzzy, so this guards
/// the calibration against the "zero cards" regression without being flaky about
/// exactly which cards a given run happens to catch.
const EXPECTED_NATIVE: &[&str] =
    &["Meso P13", "Meso V12", "Meso V15", "Neo D5", "Axi C6", "Axi G5", "Axi L6", "Axi N10"];

fn tesseract_available() -> bool {
    let bin = std::env::var("WF_TESSERACT").unwrap_or_else(|_| "tesseract".to_string());
    Command::new(bin).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// The scanner reads the relic catalogue from the disk cache; without it the
/// command would need the network, which a CI box doesn't have.
fn catalogue_cached() -> bool {
    wf_cache::cache_dir().map(|d| d.join("relics.json").exists()).unwrap_or(false)
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr").join(name)
}

#[test]
fn native_resolution_grid_resolves_most_known_relics() {
    if !tesseract_available() || !catalogue_cached() {
        eprintln!("skipping: tesseract or cached catalogue unavailable");
        return;
    }
    let path = fixture("Screenshot_20260731_114749.png");
    let out = Command::new(env!("CARGO_BIN_EXE_wf-lite"))
        .args(["relic-grid-file", path.to_str().unwrap()])
        .output()
        .expect("run wf-lite relic-grid-file");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let found = EXPECTED_NATIVE.iter().filter(|r| stdout.contains(**r)).count();
    assert!(
        found >= 6,
        "grid scanner resolved only {found}/{} expected relics — calibration regressed.\n{stdout}",
        EXPECTED_NATIVE.len()
    );
}
