//! Shared motion clock and widget-list lifetime ownership.
use crate::ui::{anim::AnimRuntime, listview::ListView, node::WidgetNode};
use std::{collections::HashMap, time::Instant};

#[derive(Debug, Default)]
pub(crate) struct AnimationState {
    pub motion: AnimRuntime,
    pub widgets: HashMap<String, ListView<(String, String), WidgetNode>>,
}

impl AnimationState {
    pub(crate) fn forget_window(&mut self, id: iced::window::Id) {
        let bar_prefix = format!("{id:?}/");
        let popup_prefix = format!("popup:{id:?}/");
        let scopes: Vec<_> = self
            .widgets
            .keys()
            .filter(|scope| scope.starts_with(&bar_prefix) || scope.starts_with(&popup_prefix))
            .cloned()
            .collect();
        for scope in scopes {
            if let Some(mut list) = self.widgets.remove(&scope) {
                list.clear_all(&mut self.motion);
            }
        }
    }

    pub(crate) fn reset_widgets(&mut self) {
        for list in self.widgets.values_mut() {
            list.clear_all(&mut self.motion);
        }
        self.widgets.clear();
    }

    pub(crate) fn advance(&mut self, now: Instant) {
        self.motion.tick_at(now);
        for list in self.widgets.values_mut() {
            list.sweep(&mut self.motion);
        }
    }
}
