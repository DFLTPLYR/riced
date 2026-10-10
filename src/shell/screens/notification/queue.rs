//! Queue/animation transitions take no window, service, or shell borrows.
use super::{Notification, default_tree, enqueue, enter_from, output_keys, visible_order};
use crate::{
    config::NotificationConfig,
    shell::notifications::NotificationState,
    ui::{anim::AnimRuntime, listview::Axis},
};
use iced_wayland_subscriber::OutputId;
use std::time::{Duration, Instant};

pub(super) struct QueueContext<'a> {
    pub state: &'a mut NotificationState,
    pub motion: &'a mut AnimRuntime,
    pub config: &'a NotificationConfig,
    pub duration: Duration,
}

pub(super) struct Arrival {
    pub notification: Notification,
    output: OutputId,
    previous_keys: Vec<(OutputId, u32)>,
}

impl QueueContext<'_> {
    pub fn arrive(&mut self, mut notification: Notification, output: OutputId) -> Arrival {
        notification.output = Some(output);
        if notification.id == 0 {
            notification.id = self.state.allocate_id();
        }
        let id = notification.id;
        let before = output_keys(&self.state.queue, output);
        enqueue(&mut self.state.queue, notification);
        let stored = self
            .state
            .queue
            .iter()
            .find(|notification| notification.id == id)
            .expect("enqueued notification")
            .clone();
        Arrival {
            notification: stored,
            output,
            previous_keys: before,
        }
    }

    /// Render/load may install author transitions between enqueue and animate.
    pub fn animate_arrival(&mut self, arrival: &Arrival) {
        self.state.list.set_enter_from_x(enter_from(self.config).x);
        let after = output_keys(&self.state.queue, arrival.output);
        self.state.list.update(
            self.motion,
            self.duration,
            Axis::Vertical,
            &arrival.previous_keys,
            &after,
            &[],
        );
    }

    pub fn retire(&mut self, id: u32) -> bool {
        let order = visible_order(&self.state.queue);
        let Some(position) = order
            .iter()
            .position(|(notification, _)| *notification == id)
        else {
            self.state.trees.remove(&id);
            return false;
        };
        let output = order[position].1;
        let before = output_keys(&self.state.queue, output);
        let output_position = before
            .iter()
            .position(|(_, notification)| *notification == id)
            .unwrap_or(position);
        let cached = self.state.trees.remove(&id);
        let fallback = self
            .state
            .queue
            .iter()
            .find(|notification| notification.id == id);
        let node = cached.unwrap_or_else(|| default_tree(fallback));
        if let Some(image) = fallback.and_then(|notification| notification.image.clone()) {
            self.state.exit_images.insert(id, image);
        }
        self.state
            .queue
            .retain(|notification| notification.id != id);
        let after = output_keys(&self.state.queue, output);
        self.state.list.set_enter_from_x(enter_from(self.config).x);
        self.state.list.update(
            self.motion,
            self.duration,
            Axis::Vertical,
            &before,
            &after,
            &[(output_position, (output, id), node)],
        );
        true
    }

    pub fn action_known(&self, id: u32, key: &str) -> bool {
        self.state
            .queue
            .iter()
            .find(|notification| notification.id == id)
            .is_some_and(|notification| {
                notification.actions.iter().any(|(action, _)| action == key)
            })
    }

    pub fn expired(&self, now: Instant) -> Vec<u32> {
        self.state
            .queue
            .iter()
            .filter(|notification| notification.expired(now))
            .map(|notification| notification.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_arrival_uses_transitions_installed_after_enqueue_and_retirement_is_idempotent() {
        let config = NotificationConfig::default();
        let mut state = NotificationState::default();
        let mut motion = AnimRuntime::default();
        let mut context = QueueContext {
            state: &mut state,
            motion: &mut motion,
            config: &config,
            duration: Duration::from_secs(1),
        };
        let arrival = context.arrive(
            Notification::internal(&config, "first", "body".into(), 1),
            OutputId(1),
        );
        let mut enter = context.state.list.enter_spec();
        enter.from.x = 500.0;
        context.state.list.set_transitions(
            enter,
            context.state.list.exit_spec(),
            context.state.list.displaced_spec(),
            true,
        );
        context.animate_arrival(&arrival);
        let key = (OutputId(1), arrival.notification.id);
        assert_eq!(context.state.list.motion_of(context.motion, &key).x, 500.0);
        assert!(context.retire(arrival.notification.id));
        assert_eq!(context.state.list.ghosts_for(|_| true).len(), 1);
        assert!(!context.retire(arrival.notification.id));
        assert_eq!(context.state.list.ghosts_for(|_| true).len(), 1);
    }
}
