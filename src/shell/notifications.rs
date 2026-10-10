//! Notification queue, renderer, and per-output presentation state.
use super::screens::Notification;
use crate::ui::{listview::ListView, node::WidgetNode};
use iced::widget::image::Handle;
use iced_wayland_subscriber::OutputId;
use std::collections::{HashMap, VecDeque};

#[derive(Debug)]
pub(crate) struct NotificationState {
    pub queue: VecDeque<Notification>,
    pub sizes: HashMap<OutputId, u32>,
    pub scroll: HashMap<OutputId, f32>,
    pub exit_images: HashMap<u32, Handle>,
    pub next_id: u32,
    pub renderer: crate::lua::notifications::NotificationRenderer,
    pub trees: HashMap<u32, WidgetNode>,
    pub list: ListView<(OutputId, u32), WidgetNode>,
    /// Last input-region rects pushed to the compositor. Fresh native
    /// surfaces (NewShell) reset this so the mask is always reinstalled.
    pub masks: HashMap<OutputId, Vec<(i32, i32, i32, i32)>>,
}

impl Default for NotificationState {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            sizes: HashMap::new(),
            scroll: HashMap::new(),
            exit_images: HashMap::new(),
            next_id: 1,
            renderer: Default::default(),
            trees: HashMap::new(),
            list: ListView::new(super::screens::notification::CARD_PITCH),
            masks: HashMap::new(),
        }
    }
}

impl NotificationState {
    pub(crate) fn forget_output(
        &mut self,
        output: OutputId,
        motion: &mut crate::ui::anim::AnimRuntime,
    ) {
        let removed: std::collections::HashSet<_> = self
            .queue
            .iter()
            .filter(|notification| notification.output == Some(output))
            .map(|notification| notification.id)
            .chain(
                self.list
                    .ghosts_for(|(owner, _)| *owner == output)
                    .iter()
                    .map(|ghost| ghost.key.1),
            )
            .collect();
        self.sizes.remove(&output);
        self.scroll.remove(&output);
        self.masks.remove(&output);
        self.queue
            .retain(|notification| notification.output != Some(output));
        let live: std::collections::HashSet<_> = self
            .queue
            .iter()
            .map(|notification| notification.id)
            .collect();
        self.trees.retain(|id, _| live.contains(id));
        self.exit_images.retain(|id, _| !removed.contains(id));
        self.list.clear_scope(motion, |(owner, _)| *owner == output);
    }

    pub(crate) fn allocate_id(&mut self) -> u32 {
        let id = self.next_id.max(1);
        self.next_id = id.wrapping_add(1);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{anim::AnimRuntime, listview::Axis};
    use std::time::Duration;

    #[test]
    fn removing_output_releases_exit_images_and_ghost_motion_but_preserves_other_queue() {
        let output = OutputId(1);
        let other = OutputId(2);
        let mut state = NotificationState::default();
        let mut motion = AnimRuntime::default();
        let key = (output, 1);
        state.list.update(
            &mut motion,
            Duration::from_secs(1),
            Axis::Vertical,
            &[],
            &[key],
            &[],
        );
        state.list.update(
            &mut motion,
            Duration::from_secs(1),
            Axis::Vertical,
            &[key],
            &[],
            &[(0, key, WidgetNode::Spinner)],
        );
        assert_eq!(state.list.ghosts_for(|_| true).len(), 1);
        state
            .exit_images
            .insert(1, Handle::from_rgba(1, 1, vec![255; 4]));
        let mut retained = Notification::internal(
            &crate::config::NotificationConfig::default(),
            "other",
            "body".into(),
            1,
        );
        retained.id = 2;
        retained.output = Some(other);
        state.queue.push_back(retained);
        state.trees.insert(2, WidgetNode::Spinner);
        state.scroll.insert(output, 30.0);
        state.scroll.insert(other, 40.0);
        state.forget_output(output, &mut motion);
        assert!(state.exit_images.is_empty());
        assert!(state.list.ghosts_for(|_| true).is_empty());
        assert_eq!(motion.motion_count(), 0);
        assert_eq!(state.queue.len(), 1);
        assert!(state.trees.contains_key(&2));
        assert!(!state.scroll.contains_key(&output));
        assert_eq!(state.scroll[&other], 40.0);
        state.forget_output(output, &mut motion);
        assert_eq!(state.queue.len(), 1);
    }
}
