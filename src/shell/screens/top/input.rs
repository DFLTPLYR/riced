//! Bar input identity, independent of shell dispatch.
use crate::shell::input::InputState;
use iced::{mouse::Button, window};
use std::time::{Duration, Instant};

pub(super) const HOLD_THRESHOLD: Duration = Duration::from_millis(500);

pub(super) struct InputContext<'a> {
    pub windows: &'a crate::shell::windows::WindowState,
    pub input: &'a mut InputState,
}

impl InputContext<'_> {
    pub fn widget_press(&mut self, bar: window::Id, slot: usize, placement: String, now: Instant) {
        if matches!(
            self.windows.ids.get(&bar),
            Some(crate::shell::windows::PlotInfo::Top(_))
        ) {
            self.input.presses.insert(bar, (slot, Some(placement), now));
        }
    }
}

pub(super) fn release_matches_press(
    target: Option<(usize, Option<String>, Instant)>,
    slot: usize,
    widget: Option<&str>,
    button: Button,
    now: Instant,
) -> bool {
    button == Button::Left
        && matches!(target, Some((pressed_slot, ref pressed_widget, start)) if pressed_slot == slot && pressed_widget.as_deref() == widget && now.duration_since(start) < HOLD_THRESHOLD)
}
