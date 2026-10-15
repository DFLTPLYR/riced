//! Desktop chrome state shared across output surfaces.
use super::screens::{ContextMenu, SelectionRect};
use crate::lua::composable::ComposableRuntime;
use iced::Point;
use std::{cell::RefCell, time::Instant};

#[derive(Debug, Default)]
pub(crate) struct DesktopState {
    pub overlays: super::overlay::State,
    pub selection_rect: SelectionRect,
    pub composable_runtime: RefCell<ComposableRuntime>,
    pub fade_rect: Option<SelectionRect>,
    pub fade_start: Option<Instant>,
    pub context_menu: Option<ContextMenu>,
    pub repaint_seq: u64,
    pub wallpaper: crate::wallpaper::State,
}

impl DesktopState {
    pub(crate) fn begin_selection(&mut self, point: Point) {
        if let Some(menu) = &mut self.context_menu {
            menu.open = false;
        }
        self.fade_rect = None;
        self.fade_start = None;
        self.selection_rect.selecting = true;
        self.selection_rect.start_point = Some(point);
        self.selection_rect.x = point.x;
        self.selection_rect.y = point.y;
        self.selection_rect.width = 0.0;
        self.selection_rect.height = 0.0;
    }

    pub(crate) fn end_selection(&mut self, now: Instant) {
        if self.selection_rect.selecting {
            self.fade_rect = Some(self.selection_rect.clone());
            self.fade_start = Some(now);
        }
        self.selection_rect.reset();
    }
}
