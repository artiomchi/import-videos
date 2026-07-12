//! Progress reporting shared across scan and transfer phases (design
//! D6, add-scan-progress D2, improve-console-output D1/D8): a thin
//! wrapper around an `Option<ProgressBar>` so every call site can tick
//! unconditionally — `hidden()` (piped stdout, `--json`, or tests)
//! makes every method a no-op, with nothing constructed or drawn.
//!
//! Visible bars register with a process-global `MultiProgress`
//! (`REGISTRY`) so `suspend` — used by `cli::init_tracing`'s log
//! writer — can clear whichever bar is active, print a diagnostic
//! line, and redraw, without either side needing to know about the
//! other. The registry is confined to this module: nothing outside it
//! knows a `MultiProgress` exists.

use std::sync::OnceLock;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

static REGISTRY: OnceLock<MultiProgress> = OnceLock::new();

/// Runs `f` with any currently-registered bar suspended (cleared for
/// the duration, redrawn after), so a diagnostic line printed inside
/// `f` doesn't garble it (design D8). A plain call-through when no bar
/// has ever been registered — most runs never touch a `MultiProgress`
/// at all.
pub(crate) fn suspend<R>(f: impl FnOnce() -> R) -> R {
    match REGISTRY.get() {
        Some(multi) => multi.suspend(f),
        None => f(),
    }
}

pub struct Progress {
    bar: Option<ProgressBar>,
    /// Every message ever passed to `set_message`, in order — a test
    /// seam only (task 4.2: proving the copying → verifying sequence
    /// end to end through `execute`, not just the bar's current
    /// state, which only ever remembers the latest one).
    #[cfg(test)]
    history: std::cell::RefCell<Vec<String>>,
}

impl Progress {
    /// `enabled` should be `stdout is a TTY && !--json` — decided once
    /// by the CLI layer, never re-checked here (design D6). Byte-
    /// oriented template, for transfer progress. `label` names the
    /// running operation (e.g. `"Importing"`) and is set once as the
    /// bar's prefix (design D1) — call sites only ever update the
    /// per-item message after construction.
    pub fn new(enabled: bool, label: &str) -> Self {
        Self::with_template(
            enabled,
            label,
            "{prefix:.bold} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}",
        )
    }

    /// Same enable/hidden/TTY-gating and prefix behavior as `new`, but
    /// with a count-oriented template — for chapter/event-level scan
    /// progress (add-scan-progress design D2).
    pub fn counted(enabled: bool, label: &str) -> Self {
        Self::with_template(
            enabled,
            label,
            "{prefix:.bold} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
        )
    }

    fn with_template(enabled: bool, label: &str, template: &str) -> Self {
        if !enabled {
            return Self::wrap(None);
        }
        // Add to the `MultiProgress` *before* configuring: a standalone
        // `ProgressBar` draws to stderr on its first state change (e.g.
        // `set_prefix`), so styling it before `add` paints a stray,
        // unmanaged copy at the current cursor — orphaned (frozen at
        // `0/0`, since `set_length` hasn't run yet) once `MultiProgress`
        // takes over and redraws in its own coordinate space. Adding
        // first means every draw is managed from the start.
        let bar = REGISTRY
            .get_or_init(MultiProgress::new)
            .add(ProgressBar::new(0));
        bar.set_style(
            ProgressStyle::with_template(template)
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("#>-"),
        );
        bar.set_prefix(label.to_string());
        // Redraw on a background cadence so the spinner and elapsed
        // timer keep animating even across long stretches with no
        // `inc` — notably the clash-detection and read-back hashes,
        // which read whole files without ticking any bytes. Without
        // this the bar looks hung during verification.
        bar.enable_steady_tick(std::time::Duration::from_millis(100));
        Self::wrap(Some(bar))
    }

    fn wrap(bar: Option<ProgressBar>) -> Self {
        Progress {
            bar,
            #[cfg(test)]
            history: std::cell::RefCell::new(Vec::new()),
        }
    }

    pub fn hidden() -> Self {
        Self::wrap(None)
    }

    pub(crate) fn set_length(&self, len: u64) {
        if let Some(bar) = &self.bar {
            bar.set_length(len);
        }
    }

    pub(crate) fn set_message(&self, msg: String) {
        #[cfg(test)]
        self.history.borrow_mut().push(msg.clone());
        if let Some(bar) = &self.bar {
            bar.set_message(msg);
        }
    }

    pub(crate) fn inc(&self, delta: u64) {
        if let Some(bar) = &self.bar {
            bar.inc(delta);
        }
    }

    /// Finishes and clears the bar, then deregisters it — after this,
    /// `suspend` no longer touches it (design D2: "deregister on
    /// `finish()`").
    pub(crate) fn finish(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
            if let Some(multi) = REGISTRY.get() {
                multi.remove(bar);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn position(&self) -> u64 {
        self.bar.as_ref().map(|b| b.position()).unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn message_history(&self) -> Vec<String> {
        self.history.borrow().clone()
    }

    #[cfg(test)]
    pub(crate) fn prefix(&self) -> Option<String> {
        self.bar.as_ref().map(|b| b.prefix().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_progress_never_constructs_a_bar() {
        // Task 5.4 (add-core-cli): piped/JSON output must carry no
        // progress or terminal-control bytes. `Progress::hidden()`
        // never allocates an indicatif `ProgressBar`, so every method
        // is an inert no-op by construction.
        let progress = Progress::hidden();
        progress.set_length(100);
        progress.set_message("test".to_string());
        progress.inc(50);
        progress.finish();
        assert_eq!(progress.position(), 0);
    }

    #[test]
    fn visible_progress_bar_can_be_constructed_and_used() {
        let progress = Progress::new(true, "Importing");
        progress.set_length(100);
        progress.inc(50);
        assert_eq!(progress.position(), 50);
        progress.finish();
    }

    #[test]
    fn counted_hidden_never_constructs_a_bar() {
        // add-scan-progress task 5.2: mirrors the byte-oriented hidden
        // test above for the count-oriented constructor.
        let progress = Progress::counted(false, "Scanning");
        progress.set_length(10);
        progress.set_message("chapter".to_string());
        progress.inc(3);
        progress.finish();
        assert_eq!(progress.position(), 0);
    }

    #[test]
    fn counted_visible_progress_bar_can_be_constructed_and_used() {
        let progress = Progress::counted(true, "Scanning");
        progress.set_length(10);
        progress.inc(4);
        assert_eq!(progress.position(), 4);
        progress.finish();
    }

    // --- prefix / registry (improve-console-output design D1, D2) ---

    #[test]
    fn visible_bars_carry_their_operation_label_as_prefix() {
        let scanning = Progress::counted(true, "Scanning");
        assert_eq!(scanning.prefix().as_deref(), Some("Scanning"));
        scanning.finish();

        let importing = Progress::new(true, "Importing");
        assert_eq!(importing.prefix().as_deref(), Some("Importing"));
        importing.finish();
    }

    #[test]
    fn hidden_bars_have_no_prefix_and_touch_no_registry() {
        assert_eq!(Progress::hidden().prefix(), None);
        assert_eq!(Progress::counted(false, "Scanning").prefix(), None);
        // `suspend` must still work (as a plain call-through) even when
        // no visible bar has ever registered.
        assert_eq!(suspend(|| 42), 42);
    }
}
