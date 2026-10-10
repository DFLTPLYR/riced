//! Independent ownership for persistence and theme-regeneration jobs.
#[derive(Debug, Default)]
pub(crate) struct SaveState {
    pub dirty: bool,
    pub sequence: u64,
    pub last_error: Option<String>,
}

impl SaveState {
    pub(crate) fn stage(&mut self) -> u64 {
        self.dirty = true;
        self.sequence += 1;
        self.sequence
    }
    pub(crate) fn invalidate(&mut self) {
        self.dirty = false;
        self.sequence += 1;
    }
    pub(crate) fn should_flush(&self, sequence: u64) -> bool {
        self.dirty && self.sequence == sequence
    }
}

#[derive(Debug, Default)]
pub(crate) struct ThemeJobState {
    pub dirty: bool,
    pub running: bool,
    pub sequence: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coalesced_save_ignores_superseded_timers_and_reload_invalidates_pending_work() {
        let mut saves = SaveState::default();
        let first = saves.stage();
        let second = saves.stage();
        assert!(!saves.should_flush(first));
        assert!(saves.should_flush(second));
        saves.invalidate();
        assert!(!saves.should_flush(second));
    }
}
