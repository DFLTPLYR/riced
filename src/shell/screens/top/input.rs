//! Bar input identity and slot hit-testing, independent of shell dispatch.
use super::Top;
use crate::shell::input::InputState;
use crate::shell::screens::{Background, Popup};
use iced::{Point, mouse::Button, window};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub(super) const HOLD_THRESHOLD: Duration = Duration::from_millis(500);

pub(super) fn cursor_slot(
    top: &Top,
    output: OutputId,
    outputs: &HashMap<OutputId, OutputInfo>,
    cursor: Option<Point>,
) -> Option<usize> {
    let (_, _, width, height) = Background::available_rect(output, outputs)?;
    let horizontal = top.is_horizontal();
    let (width, height) = top.local.px_size(width, height, horizontal);
    let (left, up, right, down) = if top.local.floating {
        let margins = top.local.margins;
        (
            margins.left.max(0) as f32,
            margins.top.max(0) as f32,
            margins.right.max(0) as f32,
            margins.bottom.max(0) as f32,
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    Popup::slot_at_point(
        (
            left,
            up,
            width as f32 - left - right,
            height as f32 - up - down,
        ),
        top.local.slots.clamp(1, super::TopLocal::MAX_SLOTS) as usize,
        top.local
            .slot_spacing
            .clamp(0.0, super::TopLocal::MAX_SLOT_GAP),
        horizontal,
        cursor?,
    )
}

pub(super) fn record_gap_press(
    input: &mut InputState,
    bar: window::Id,
    slot: Option<usize>,
    button: Button,
    now: Instant,
) {
    match (button, slot) {
        (Button::Left, Some(slot)) => {
            input.presses.insert(bar, (slot, None, now));
        }
        _ => {
            input.presses.remove(&bar);
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
