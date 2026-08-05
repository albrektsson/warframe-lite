//! The deadline/idle/cadence skeleton every scan loop runs on top of — armed
//! by an external EE.log watcher, watched here for as long as the process
//! lives. `relic_scan_loop` and `inventory_scan_loop` in `wf-lite` were
//! structurally identical copies of this bookkeeping with only the capture,
//! per-frame scan, and confirmed-observation handling swapped; this factors
//! that shell out so a screen only supplies [`ScanLoopBody`].

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The scan window a [`run_scan_loop`] watches: `Some(t)` while a scan is
/// live (armed by an external EE.log event, extended by scan activity),
/// `None` once it lapses or before the first event ever arrives.
pub type ScanDeadline = Arc<Mutex<Option<Instant>>>;

/// Timing knobs for [`run_scan_loop`]. Identical across every screen so far,
/// but broken out (not hardcoded in the loop) so a future screen can tune its
/// own cadence without touching the shared skeleton.
#[derive(Debug, Clone, Copy)]
pub struct ScanCadence {
    /// End a scan once no new observation has landed for this long (the
    /// player stopped scrolling or left the screen).
    pub idle: Duration,
    /// Between-capture gap while actively scanning.
    pub cooldown: Duration,
    /// How often to check for a new deadline while idle.
    pub idle_poll: Duration,
}

impl Default for ScanCadence {
    /// The cadence every current screen scanner uses. A future screen with
    /// different timing needs can override individual fields via struct
    /// update syntax rather than repeating these three literals.
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(12),
            cooldown: Duration::from_millis(100),
            idle_poll: Duration::from_millis(400),
        }
    }
}

/// One screen's scan-loop behavior. The deadline/idle/cadence bookkeeping in
/// [`run_scan_loop`] is the same for every screen; what happens at each state
/// transition and on each active tick is not.
pub trait ScanLoopBody {
    /// The scan window just opened (idle -> active): reset per-session
    /// bookkeeping and announce the scan starting.
    fn activate(&mut self);

    /// The scan window just closed (active -> idle). Default no-op — only a
    /// screen with its own progress indicator needs this.
    fn deactivate(&mut self) {}

    /// Capture and scan exactly one frame, folding any newly-confirmed
    /// observations into the persisted owned state (including its own
    /// save-to-disk and logging). Returns whether anything changed, so the
    /// loop can reset its idle timer.
    fn tick(&mut self) -> impl std::future::Future<Output = bool> + Send;
}

/// The shared half of every scan loop's tally-agreement application: `tally`
/// votes a key's value across frames (see [`crate::scan_grid`]'s callers), and
/// once `agreement` frames agree, `apply` is called with the confirmed value
/// — but only the first time this session a given key confirms *this*
/// value, via `session_applied` (a stable count then just refreshes nothing
/// until it actually changes). Returns whether `apply` was called, so the
/// caller can fold it into its own `changed` tracking.
pub fn confirm_once<K: Eq + Hash + Clone>(
    tally: &wf_ocr::Tally<K>,
    session_applied: &mut HashMap<K, u32>,
    key: &K,
    agreement: u32,
    apply: impl FnOnce(u32),
) -> bool {
    let Some(confirmed) = tally.confirmed(key, agreement) else {
        return false;
    };
    if session_applied.get(key) == Some(&confirmed) {
        return false;
    }
    session_applied.insert(key.clone(), confirmed);
    apply(confirmed);
    true
}

/// Run a screen's scan loop for as long as the process lives: watch
/// `deadline` for an open scan window, and while one is open, call
/// `body.tick()` every `cadence.cooldown`; idle, poll `deadline` every
/// `cadence.idle_poll` instead.
pub async fn run_scan_loop<B: ScanLoopBody>(deadline: ScanDeadline, cadence: ScanCadence, mut body: B) {
    let mut was_active = false;
    // Bogus initial value (not a real deadline any watcher could ever set) so
    // the very first observed deadline always registers as a change below.
    let mut last_seen_deadline: Option<Instant> = Some(Instant::now() - cadence.idle * 2);
    let mut last_new = Instant::now() - cadence.idle * 2;

    loop {
        let current = *deadline.lock().unwrap();
        // The external watcher just (re)armed the window from a fresh event —
        // treat that as activity so the idle check below doesn't see a
        // `last_new` that's stale from long before this screen was ever
        // opened (this loop runs for the whole process lifetime).
        if current != last_seen_deadline {
            last_seen_deadline = current;
            if current.is_some() {
                last_new = Instant::now();
            }
        }
        let active = current.is_some_and(|t| Instant::now() < t) && last_new.elapsed() <= cadence.idle;

        if !active {
            if was_active {
                body.deactivate();
            }
            was_active = false;
            tokio::time::sleep(cadence.idle_poll).await;
            continue;
        }
        if !was_active {
            was_active = true;
            body.activate();
        }

        if body.tick().await {
            last_new = Instant::now();
        }
        tokio::time::sleep(cadence.cooldown).await;
    }
}
