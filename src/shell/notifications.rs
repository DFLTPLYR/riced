//! Notification service state. Presentation belongs to the generic scene host.
use super::screens::Notification;
use iced_wayland_subscriber::OutputId;
use std::collections::VecDeque;
#[derive(Debug)]
pub(crate) struct NotificationState {
    pub queue: VecDeque<Notification>,
    next_generation: u64,
}
impl Default for NotificationState {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            next_generation: 1,
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
        for item in &mut self.queue {
            if item.output == Some(output) {
                item.popup_visible = false;
                item.output = None;
            }
        }
    }
    pub(crate) fn has_deadlines(&self) -> bool {
        self.queue
            .iter()
            .any(|item| item.popup_visible && !item.closed && item.timeout.is_some())
    }
    pub(crate) fn allocate_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        generation
    }
    pub(crate) fn allocate_id(&mut self) -> u32 {
        crate::notify::allocate_id()
    }
}
