//! Notification delivery, timeout, output assignment, and D-Bus action policy.
//! The whole visual list is a configured Lua scene in the generic overlay host.
use crate::{
    config::NotificationConfig,
    shell::{
        Plant, Plots,
        overlay::{self, Spec},
    },
};
use iced::Task;
use iced_wayland_subscriber::OutputId;
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u32,
    pub output: Option<OutputId>,
    pub app: String,
    pub title: String,
    pub body: String,
    pub icon: String,
    pub urgency: u8,
    pub received_at: Instant,
    pub timeout: Option<Duration>,
    pub actions: Vec<(String, String)>,
    pub image: Option<iced::widget::image::Handle>,
}
pub(crate) type OutputGeom = (OutputId, (f32, f32, f32, f32));
pub(crate) struct Incoming {
    pub id: u32,
    pub app: String,
    pub icon: String,
    pub title: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub image: Option<iced::widget::image::Handle>,
    pub urgency: u8,
    pub timeout_ms: Option<u64>,
}
pub(crate) fn output_at(geometries: &[OutputGeom], point: iced::Point) -> Option<OutputId> {
    geometries.iter().find_map(|(id, (x, y, w, h))| {
        (point.x >= *x && point.y >= *y && point.x < x + w && point.y < y + h).then_some(*id)
    })
}
pub(crate) fn enqueue(queue: &mut VecDeque<Notification>, notification: Notification) {
    if let Some(previous) = queue
        .iter_mut()
        .find(|previous| previous.id == notification.id)
    {
        *previous = notification;
    } else {
        queue.push_back(notification);
    }
}
pub(crate) fn resolve_output(plots: &Plots, config: &NotificationConfig) -> Option<OutputId> {
    if config.output != "mouse"
        && let Some((id, _)) = plots.windows.output_infos.iter().find(|(_, info)| {
            info.name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(&config.output))
        })
    {
        return Some(*id);
    }
    if let Some(cursor) = plots.input.global {
        let geometry: Vec<_> = plots
            .windows
            .output_infos
            .iter()
            .map(|(id, info)| (*id, crate::shared::geometry::output_geometry(info)))
            .collect();
        if let Some(output) = output_at(&geometry, cursor) {
            return Some(output);
        }
    }
    plots.windows.output_infos.keys().copied().next()
}
pub(crate) fn scene_spec(plots: &Plots, output: OutputId) -> Spec {
    let records: Vec<_> = plots
        .notification
        .queue
        .iter()
        .rev()
        .filter(|item| item.output == Some(output))
        .cloned()
        .collect();
    Spec {
        id: "notification".into(),
        source: plots.config.notifications.source.clone(),
        input: Arc::new(move |lua| {
            let props = lua.create_table()?;
            let notifications = lua.create_table()?;
            for (index, notification) in records.iter().enumerate() {
                let item = lua.create_table()?;
                item.set("id", notification.id)?;
                item.set("app", notification.app.as_str())?;
                item.set("title", notification.title.as_str())?;
                item.set("body", notification.body.as_str())?;
                item.set("icon", notification.icon.as_str())?;
                item.set("urgency", notification.urgency)?;
                item.set("has_image", notification.image.is_some())?;
                item.set(
                    "actions",
                    crate::lua::notifications::actions_table(lua, &notification.actions)
                        .map_err(mlua::Error::RuntimeError)?,
                )?;
                if let Some(image) = &notification.image {
                    item.set(
                        "image",
                        lua.create_userdata(crate::ui::image_value::ImageValue(image.clone()))?,
                    )?;
                }
                notifications.set(index + 1, item)?;
            }
            props.set("notifications", notifications)?;
            Ok(props)
        }),
    }
}
pub(crate) fn command_from_action(value: &mlua::Value, plots: &mut Plots) -> Task<Plant> {
    use crate::lua::notifications::{NotificationAction, notification_action};
    match notification_action(value) {
        Some(NotificationAction::Dismiss(id)) => Notification::handle_dismissed(plots, id),
        Some(NotificationAction::Invoke(id, key)) => Notification::handle_invoke(plots, id, key),
        None => Task::none(),
    }
}
fn changed(plots: &mut Plots) -> Task<Plant> {
    plots.placements.last_run.clear();
    Task::batch([
        overlay::refresh(plots, true),
        super::top::Top::handle_widget_tick(plots),
        super::popup::Popup::refresh_bodies(plots),
    ])
}

impl Notification {
    pub fn expired(&self, now: Instant) -> bool {
        self.timeout
            .is_some_and(|timeout| now.duration_since(self.received_at) >= timeout)
    }
    pub(crate) fn internal(
        config: &NotificationConfig,
        title: &str,
        body: String,
        urgency: u8,
    ) -> Self {
        Self {
            id: 0,
            output: None,
            app: "riced".into(),
            title: title.into(),
            body,
            icon: String::new(),
            urgency: urgency.min(2),
            received_at: Instant::now(),
            timeout: (urgency < 2).then(|| Duration::from_millis(config.timeout_ms)),
            actions: Vec::new(),
            image: None,
        }
    }
    pub(crate) fn from_dbus(incoming: Incoming, default_ms: u64) -> Self {
        Self {
            id: incoming.id,
            output: None,
            app: incoming.app,
            title: incoming.title,
            body: incoming.body,
            icon: incoming.icon,
            urgency: incoming.urgency.min(2),
            received_at: Instant::now(),
            timeout: (incoming.urgency < 2)
                .then(|| Duration::from_millis(incoming.timeout_ms.unwrap_or(default_ms))),
            actions: incoming.actions,
            image: incoming.image,
        }
    }
    pub(crate) fn handle_arrived(plots: &mut Plots, mut notification: Notification) -> Task<Plant> {
        if !plots.config.notifications.enabled {
            return Task::none();
        }
        let Some(output) = notification
            .output
            .or_else(|| resolve_output(plots, &plots.config.notifications))
        else {
            return Task::none();
        };
        notification.output = Some(output);
        if notification.id == 0 {
            notification.id = plots.notification.allocate_id();
        }
        enqueue(&mut plots.notification.queue, notification);
        changed(plots)
    }
    pub(crate) fn handle_dismissed(plots: &mut Plots, id: u32) -> Task<Plant> {
        if !plots.notification.queue.iter().any(|item| item.id == id) {
            return Task::none();
        }
        plots.notification.queue.retain(|item| item.id != id);
        Task::batch([crate::notify::emit_closed(id, 2), changed(plots)])
    }
    pub(crate) fn handle_peer_closed(plots: &mut Plots, id: u32) -> Task<Plant> {
        if !plots.notification.queue.iter().any(|item| item.id == id) {
            return Task::none();
        }
        plots.notification.queue.retain(|item| item.id != id);
        changed(plots)
    }
    pub(crate) fn handle_invoke(plots: &mut Plots, id: u32, key: String) -> Task<Plant> {
        if !plots
            .notification
            .queue
            .iter()
            .any(|item| item.id == id && item.actions.iter().any(|(action, _)| action == &key))
        {
            return Task::none();
        }
        plots.notification.queue.retain(|item| item.id != id);
        Task::batch([
            crate::notify::emit_action_invoked(id, key),
            crate::notify::emit_closed(id, 2),
            changed(plots),
        ])
    }
    pub(crate) fn handle_tick(plots: &mut Plots) -> Task<Plant> {
        let now = Instant::now();
        let ids: Vec<_> = plots
            .notification
            .queue
            .iter()
            .filter(|item| item.expired(now))
            .map(|item| item.id)
            .collect();
        if ids.is_empty() {
            return Task::none();
        }
        plots
            .notification
            .queue
            .retain(|item| !ids.contains(&item.id));
        let mut commands: Vec<_> = ids
            .into_iter()
            .map(|id| crate::notify::emit_closed(id, 1))
            .collect();
        commands.push(changed(plots));
        Task::batch(commands)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_replacement_output_and_action_policy_stay_in_the_service() {
        let config = NotificationConfig::default();
        let mut queue = VecDeque::new();
        let mut item = Notification::internal(&config, "first", "body".into(), 1);
        item.id = 4;
        item.output = Some(OutputId(1));
        enqueue(&mut queue, item.clone());
        item.title = "replacement".into();
        enqueue(&mut queue, item);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].title, "replacement");
        assert!(queue[0].expired(Instant::now() + Duration::from_secs(10)));
        assert!(
            !Notification::internal(&config, "critical", "body".into(), 2)
                .expired(Instant::now() + Duration::from_secs(100))
        );
        assert_eq!(
            output_at(
                &[(OutputId(1), (-100.0, 0.0, 100.0, 100.0))],
                iced::Point::new(-1.0, 20.0)
            ),
            Some(OutputId(1))
        );
    }
}
