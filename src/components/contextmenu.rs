use crate::composables::menu::{Menu, menu};

/// Right-click context menu overlay (the "Add Top" menu).
/// `width` comes from `[context_menu]`; position is pre-clamped local coords.
pub fn contextmenu<'a>(width: f32, x: f32, y: f32) -> Menu<'a> {
    menu().padding(4.0).width(width).position(x, y)
}
