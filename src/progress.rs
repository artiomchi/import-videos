//! Progress reporting shared across scan and transfer phases (design
//! D6, add-scan-progress D2): a thin wrapper around an
//! `Option<ProgressBar>` so every call site can tick unconditionally —
//! `hidden()` (piped stdout, `--json`, or tests) makes every method a
//! no-op, with nothing constructed or drawn.

use indicatif::{ProgressBar, ProgressStyle};

pub struct Progress {
    bar: Option<ProgressBar>,
}

impl Progress {
    /// `enabled` should be `stdout is a TTY && !--json` — decided once
    /// by the CLI layer, never re-checked here (design D6). Byte-
    /// oriented template, for transfer progress.
    pub fn new(enabled: bool) -> Self {
        Self::with_template(
            enabled,
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}",
        )
    }

    /// Same enable/hidden/TTY-gating as `new`, but with a count-
    /// oriented template — for chapter-level scan progress
    /// (add-scan-progress design D2).
    pub fn counted(enabled: bool) -> Self {
        Self::with_template(
            enabled,
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
        )
    }

    fn with_template(enabled: bool, template: &str) -> Self {
        if !enabled {
            return Progress { bar: None };
        }
        let bar = ProgressBar::new(0);
        bar.set_style(
            ProgressStyle::with_template(template)
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("#>-"),
        );
        Progress { bar: Some(bar) }
    }

    pub fn hidden() -> Self {
        Progress { bar: None }
    }

    pub(crate) fn set_length(&self, len: u64) {
        if let Some(bar) = &self.bar {
            bar.set_length(len);
        }
    }

    pub(crate) fn set_message(&self, msg: String) {
        if let Some(bar) = &self.bar {
            bar.set_message(msg);
        }
    }

    pub(crate) fn inc(&self, delta: u64) {
        if let Some(bar) = &self.bar {
            bar.inc(delta);
        }
    }

    pub(crate) fn finish(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
    }

    #[cfg(test)]
    pub(crate) fn position(&self) -> u64 {
        self.bar.as_ref().map(|b| b.position()).unwrap_or(0)
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
        let progress = Progress::new(true);
        progress.set_length(100);
        progress.inc(50);
        assert_eq!(progress.position(), 50);
        progress.finish();
    }

    #[test]
    fn counted_hidden_never_constructs_a_bar() {
        // add-scan-progress task 5.2: mirrors the byte-oriented hidden
        // test above for the count-oriented constructor.
        let progress = Progress::counted(false);
        progress.set_length(10);
        progress.set_message("chapter".to_string());
        progress.inc(3);
        progress.finish();
        assert_eq!(progress.position(), 0);
    }

    #[test]
    fn counted_visible_progress_bar_can_be_constructed_and_used() {
        let progress = Progress::counted(true);
        progress.set_length(10);
        progress.inc(4);
        assert_eq!(progress.position(), 4);
        progress.finish();
    }
}
