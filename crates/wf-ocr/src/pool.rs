//! A small blocking pool of independent [`LepTess`] engine instances.
//!
//! Tesseract's `TessBaseAPI` is not safe for concurrent recognition calls on a
//! single shared instance (tesseract-ocr/tesseract#4281), but `scan_relic_grid`
//! relies on true parallel recognition across dozens of cards per scan cycle
//! (see ADR-0008). A bounded pool of independent engines preserves that
//! parallelism (each engine only ever serves one caller at a time) without
//! serialising every call behind one mutex, and without the unbounded
//! thread/engine count an unpooled "one instance per call" scheme would create.

use std::sync::{Condvar, Mutex};

use anyhow::{Context, Result};
use leptess::LepTess;

pub(crate) struct Pool {
    idle: Mutex<Vec<LepTess>>,
    cvar: Condvar,
    // Zero only for the test stub (`Pool::empty`) — `acquire` panics on it.
    capacity: usize,
}

impl Pool {
    /// Build a pool of `capacity` independent engines, all for `lang`. Fails
    /// on the first engine that can't initialise (missing tessdata, bad
    /// language) — the same "OCR unavailable" failure mode `Ocr::new()`
    /// callers already expect.
    pub(crate) fn new(capacity: usize, lang: &str) -> Result<Self> {
        let mut idle = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            let mut engine = LepTess::new(None, lang)
                .with_context(|| format!("initialising libtesseract engine (lang={lang})"))?;
            let _ = engine.set_variable(leptess::Variable::DebugFile, "/dev/null");
            idle.push(engine);
        }
        Ok(Self { idle: Mutex::new(idle), cvar: Condvar::new(), capacity })
    }

    /// An engine-less stub for unit tests that must never actually touch
    /// libtesseract (so they don't depend on tessdata being installed in the
    /// test environment). See [`Pool::acquire`]. Also built under the
    /// `test-util` feature so other crates' tests (e.g. `wf-gridscan`'s) can
    /// get a stub `Ocr` via [`crate::Ocr::empty_for_test`] without linking
    /// libtesseract either.
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn empty() -> Self {
        Self { idle: Mutex::new(Vec::new()), cvar: Condvar::new(), capacity: 0 }
    }

    /// Block until an engine is free, then hand out exclusive access to it.
    /// The engine returns to the pool when the guard drops.
    pub(crate) fn acquire(&self) -> PooledEngine<'_> {
        let mut idle = self.idle.lock().unwrap();
        loop {
            if let Some(engine) = idle.pop() {
                return PooledEngine { pool: self, engine: Some(engine) };
            }
            assert!(
                self.capacity > 0,
                "wf-ocr: acquired from an empty (test-stub) pool — recognize() should have \
                 short-circuited on the blank-crop check before ever reaching the pool"
            );
            idle = self.cvar.wait(idle).unwrap();
        }
    }

    fn release(&self, engine: LepTess) {
        self.idle.lock().unwrap().push(engine);
        self.cvar.notify_one();
    }
}

/// An engine on loan from a [`Pool`]; returns it on drop.
pub(crate) struct PooledEngine<'a> {
    pool: &'a Pool,
    engine: Option<LepTess>,
}

impl std::ops::Deref for PooledEngine<'_> {
    type Target = LepTess;
    fn deref(&self) -> &LepTess {
        self.engine.as_ref().expect("engine present until drop")
    }
}

impl std::ops::DerefMut for PooledEngine<'_> {
    fn deref_mut(&mut self) -> &mut LepTess {
        self.engine.as_mut().expect("engine present until drop")
    }
}

impl Drop for PooledEngine<'_> {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            self.pool.release(engine);
        }
    }
}
