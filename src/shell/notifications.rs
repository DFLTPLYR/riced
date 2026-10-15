//! Notification service state. Presentation belongs to the generic scene host.
use super::screens::Notification;
use iced_wayland_subscriber::OutputId;
use std::collections::VecDeque;
#[derive(Debug)]
pub(crate) struct NotificationState {
    pub queue: VecDeque<Notification>,
    pub next_id: u32,
}
impl Default for NotificationState {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            next_id: 1,
        }
    }
}
impl NotificationState {
    pub(crate) fn holds_images(&self, output: OutputId) -> bool {
        self.queue
            .iter()
            .any(|item| item.output == Some(output) && item.image.is_some())
    }
    pub(crate) fn forget_output(&mut self, output: OutputId) {
        self.queue.retain(|item| item.output != Some(output));
    }
    pub(crate) fn allocate_id(&mut self) -> u32 {
        let id = self.next_id.max(1);
        self.next_id = id.wrapping_add(1);
        id
    }
}
