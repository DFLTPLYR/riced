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
    pub generation: u64,
    pub popup_visible: bool,
    pub closed: bool,
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
        .filter(|item| item.popup_visible && item.output == Some(output))
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
                item.set("_notification_generation", notification.generation)?;
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
        Some(NotificationAction::DismissAll) => Notification::handle_dismiss_all(plots),
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
pub(crate) fn callback_is_current(item: &mlua::Table, queue: &VecDeque<Notification>) -> bool {
    if let (Ok(id), Ok(generation)) = (
        item.get::<u32>("id"),
        item.get::<u64>("_notification_generation"),
    ) {
        return !queue
            .iter()
            .any(|record| record.id == id && record.generation != generation);
    }
    true
}

impl Notification {
    pub fn expired(&self, now: Instant) -> bool {
        self.popup_visible
            && !self.closed
            && self
                .timeout
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
            generation: 0,
            popup_visible: true,
            closed: false,
            output: None,
            app: "riced".into(),
            title: title.into(),
            body,
            icon: String::new(),
            urgency: urgency.min(2),
            received_at: Instant::now(),
            timeout: (urgency < 2 && config.timeout_ms > 0)
                .then(|| Duration::from_millis(config.timeout_ms)),
            actions: Vec::new(),
            image: None,
        }
    }
    pub(crate) fn from_dbus(incoming: Incoming, default_ms: u64) -> Self {
        Self {
            id: incoming.id,
            generation: 0,
            popup_visible: true,
            closed: false,
            output: None,
            app: incoming.app,
            title: incoming.title,
            body: incoming.body,
            icon: incoming.icon,
            urgency: incoming.urgency.min(2),
            received_at: Instant::now(),
            timeout: (incoming.urgency < 2 && incoming.timeout_ms.unwrap_or(default_ms) > 0)
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
        notification.generation = plots.notification.allocate_generation();
        enqueue(&mut plots.notification.queue, notification);
        changed(plots)
    }
    pub(crate) fn handle_dismissed(plots: &mut Plots, id: u32) -> Task<Plant> {
        let Some(item) = plots.notification.queue.iter().find(|item| item.id == id) else {
            return Task::none();
        };
        let closed = item.closed;
        plots.notification.queue.retain(|item| item.id != id);
        Task::batch([
            if closed {
                Task::none()
            } else {
                crate::notify::emit_closed(id, 2)
            },
            changed(plots),
        ])
    }
    pub(crate) fn handle_dismiss_all(plots: &mut Plots) -> Task<Plant> {
        if plots.notification.queue.is_empty() {
            return Task::none();
        }
        let mut tasks: Vec<_> = plots
            .notification
            .queue
            .drain(..)
            .filter(|item| !item.closed)
            .map(|item| crate::notify::emit_closed(item.id, 2))
            .collect();
        tasks.push(changed(plots));
        Task::batch(tasks)
    }
    pub(crate) fn handle_peer_closed(plots: &mut Plots, id: u32) -> Task<Plant> {
        let Some(item) = plots.notification.queue.iter().find(|item| item.id == id) else {
            return Task::none();
        };
        let closed = item.closed;
        plots.notification.queue.retain(|item| item.id != id);
        Task::batch([
            if closed {
                Task::none()
            } else {
                crate::notify::emit_closed(id, 3)
            },
            changed(plots),
        ])
    }
    pub(crate) fn handle_invoke(plots: &mut Plots, id: u32, key: String) -> Task<Plant> {
        if !plots.notification.queue.iter().any(|item| {
            item.id == id && !item.closed && item.actions.iter().any(|(action, _)| action == &key)
        }) {
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
        for item in &mut plots.notification.queue {
            if ids.contains(&item.id) {
                item.popup_visible = false;
                item.closed = true;
            }
        }
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
    use iced::futures::StreamExt;

    fn plots() -> Plots {
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        Plots {
            apps: Default::default(),
            windows: Default::default(),
            shell_events: receiver,
            input: Default::default(),
            desktop: Default::default(),
            config: Default::default(),
            config_mtime: None,
            catalog: crate::shell::catalog::WidgetCatalog::new(Vec::new()),
            placements: Default::default(),
            animation: Default::default(),
            services: crate::services::state::ServiceState {
                system: sysinfo::System::new(),
                toplevels: Default::default(),
                workspaces: Default::default(),
                wayland_changes: Default::default(),
                tray: Default::default(),
                started: std::time::Instant::now(),
                sampled: 0.0,
                gpu: None,
            },
            config_jobs: Default::default(),
            theme_mtime: None,
            theme_jobs: Default::default(),
            notification: Default::default(),
        }
    }
    fn messages(task: Task<Plant>) -> usize {
        iced_runtime::task::into_stream(task)
            .map(|stream| iced::futures::executor::block_on(stream.collect::<Vec<_>>()).len())
            .unwrap_or(0)
    }
    fn incoming(id: u32, output: u32) -> Notification {
        let mut item =
            Notification::internal(&NotificationConfig::default(), "title", "body".into(), 1);
        item.id = id;
        item.output = Some(OutputId(output));
        item
    }

    #[test]
    fn timeout_hides_popup_but_keeps_history_and_only_closes_once() {
        let mut plots = plots();
        let mut item = incoming(7, 1);
        item.received_at -= Duration::from_secs(10);
        let _ = Notification::handle_arrived(&mut plots, item);
        assert!(plots.notification.has_deadlines());
        assert_eq!(messages(Notification::handle_tick(&mut plots)), 1);
        assert_eq!(plots.notification.queue.len(), 1);
        assert!(!plots.notification.queue[0].popup_visible);
        assert!(plots.notification.queue[0].closed);
        assert!(!plots.notification.has_deadlines());
        assert_eq!(messages(Notification::handle_tick(&mut plots)), 0);
        let lua = mlua::Lua::new();
        let visible = (scene_spec(&plots, OutputId(1)).input)(&lua).unwrap();
        assert_eq!(
            visible
                .get::<mlua::Table>("notifications")
                .unwrap()
                .raw_len(),
            0
        );
        crate::services::publish_all(&crate::services::ServiceCtx::from_plots(&plots, None), &lua)
            .unwrap();
        assert_eq!(
            lua.globals()
                .get::<mlua::Table>("notifications")
                .unwrap()
                .raw_len(),
            1
        );
        assert_eq!(messages(Notification::handle_dismissed(&mut plots, 7)), 0);
        assert!(plots.notification.queue.is_empty());
    }

    #[test]
    fn dismiss_all_clears_every_output_without_reclosing_history() {
        let mut plots = plots();
        for (id, output) in [(1, 1), (2, 2), (3, 1)] {
            let _ = Notification::handle_arrived(&mut plots, incoming(id, output));
        }
        plots.notification.queue[0].closed = true;
        plots.notification.queue[0].popup_visible = false;
        let lua = mlua::Lua::new();
        crate::lua::notifications::install(&lua).unwrap();
        let command = lua.load("return notification.dismissAll()").eval().unwrap();
        assert_eq!(messages(command_from_action(&command, &mut plots)), 2);
        assert!(plots.notification.queue.is_empty());
        assert_eq!(messages(command_from_action(&command, &mut plots)), 0);
    }

    #[test]
    fn replacement_reshows_hidden_record_and_invalidates_its_old_completion() {
        let mut plots = plots();
        let _ = Notification::handle_arrived(&mut plots, incoming(7, 1));
        let generation = plots.notification.queue[0].generation;
        plots.notification.queue[0].popup_visible = false;
        plots.notification.queue[0].closed = true;
        let _ = Notification::handle_arrived(&mut plots, incoming(7, 2));
        assert_eq!(plots.notification.queue.len(), 1);
        assert!(plots.notification.queue[0].popup_visible);
        assert!(!plots.notification.queue[0].closed);
        assert_eq!(plots.notification.queue[0].output, Some(OutputId(2)));
        let lua = mlua::Lua::new();
        let old = lua.create_table().unwrap();
        old.set("id", 7).unwrap();
        old.set("_notification_generation", generation).unwrap();
        assert!(!callback_is_current(&old, &plots.notification.queue));
        plots.notification.forget_output(OutputId(2));
        assert_eq!(plots.notification.queue.len(), 1);
        assert!(!plots.notification.queue[0].popup_visible);
    }

    #[test]
    fn peer_close_and_action_validation_do_not_duplicate_closed_signals() {
        let mut plots = plots();
        let mut item = incoming(7, 1);
        item.actions.push(("open".into(), "Open".into()));
        let _ = Notification::handle_arrived(&mut plots, item);
        assert_eq!(
            messages(Notification::handle_invoke(&mut plots, 7, "unknown".into())),
            0
        );
        plots.notification.queue[0].closed = true;
        assert_eq!(
            messages(Notification::handle_invoke(&mut plots, 7, "open".into())),
            0
        );
        assert_eq!(messages(Notification::handle_peer_closed(&mut plots, 7)), 0);
        let _ = Notification::handle_arrived(&mut plots, incoming(8, 1));
        assert_eq!(messages(Notification::handle_peer_closed(&mut plots, 8)), 1);
        assert_eq!(messages(Notification::handle_peer_closed(&mut plots, 8)), 0);
    }

    #[test]
    fn explicit_zero_timeout_never_expires() {
        let notification = Notification::from_dbus(
            Incoming {
                id: 1,
                app: "app".into(),
                icon: String::new(),
                title: "title".into(),
                body: "body".into(),
                actions: Vec::new(),
                image: None,
                urgency: 1,
                timeout_ms: Some(0),
            },
            5000,
        );
        assert!(notification.timeout.is_none());
        assert!(!notification.expired(Instant::now() + Duration::from_secs(1000)));
    }
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
