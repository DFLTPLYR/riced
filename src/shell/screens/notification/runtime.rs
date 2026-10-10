//! Notification rendering borrows only renderer/cache/list state and snapshots.
use super::{CARD_PITCH, Notification, enter_from};
use crate::{
    config::NotificationConfig,
    lua::notifications::{NotificationData, NotificationRenderer},
    services::ServiceCtx,
    ui::{listview::ListView, node::WidgetNode},
};
use iced_wayland_subscriber::OutputId;
use std::collections::HashMap;

pub(super) struct RenderContext<'a> {
    pub renderer: &'a mut NotificationRenderer,
    pub trees: &'a mut HashMap<u32, WidgetNode>,
    pub list: &'a mut ListView<(OutputId, u32), WidgetNode>,
    pub config: &'a NotificationConfig,
    pub services: ServiceCtx<'a>,
}

impl RenderContext<'_> {
    pub fn render(&mut self, notification: &Notification) {
        let mut defaults: ListView<(OutputId, u32), WidgetNode> = ListView::new(CARD_PITCH);
        defaults.set_enter_from_x(enter_from(self.config).x);
        let defaults = (
            defaults.enter_spec(),
            defaults.exit_spec(),
            defaults.displaced_spec(),
        );
        let result = self.renderer.ensure(&defaults).and_then(|transitions| {
            if let Some((enter, exit, displaced, custom)) = transitions {
                self.list.set_transitions(enter, exit, displaced, custom);
            }
            self.renderer.view(
                &self.services,
                NotificationData {
                    id: notification.id,
                    app: &notification.app,
                    title: &notification.title,
                    body: &notification.body,
                    icon: &notification.icon,
                    urgency: notification.urgency,
                    has_image: notification.image.is_some(),
                    actions: &notification.actions,
                },
            )
        });
        match result {
            Ok(node) => {
                self.renderer.error = None;
                self.trees.insert(notification.id, node);
            }
            Err(error) => {
                self.renderer.note_error(error);
                self.trees.remove(&notification.id);
            }
        }
    }
}
