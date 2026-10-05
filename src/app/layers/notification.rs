//! Lua-configured notification layer (D-Bus server + internal events).
//!
//! One layer-shell window per output that currently shows anything
//! (usually just the mouse output). Each window renders that output's
//! stack newest-first; windows close when their stack empties.
//! Bodies render from `notifications.lua` once Stage 3 lands — until
//! then, [`default_card`] draws a title + up-to-three body lines.
//!
//! Placement honors `[notifications] output`: `"mouse"` resolves the
//! output under the last-known global cursor (or the compositor's
//! `LastOutput` equivalent via `OutputOption`), any other value pins
//! by output name (`OutputInfo.name`, e.g. `"DP-1"`).

use super::background::Background;
use super::top::{WidgetNode, build_node, inject_ui, new_widget_lua, parse_node};
use crate::app::app::{PlotInfo, Plots};
use crate::app::{NotifyEvent, Plant};
use crate::config::NotificationConfig;
use crate::theme;
use iced::window;
use iced::{Element, Length, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings,
    OutputOption,
};
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use iced_wayland_subscriber::OutputId;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Fixed card geometry (px). Bodies clip past three lines — layer
/// surfaces need an upfront size and Lua can't measure text.
const CARD_H: f32 = 100.0;
const CARD_GAP: f32 = 8.0;
const WINDOW_PAD: f32 = 16.0;

/// One notification: plain data in, layout out. `timeout` of `None`
/// persists until clicked (critical urgency).
#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u32,
    pub output: Option<OutputId>,
    pub app: String,
    pub title: String,
    pub body: String,
    pub icon: String,
    /// freedesktop urgency: 0 low, 1 normal, 2 critical.
    pub urgency: u8,
    pub received_at: Instant,
    pub timeout: Option<Duration>,
}

/// One output's geometry for hit-testing: `(id, (x, y, w, h))`.
pub(crate) type OutputGeom = (OutputId, (f32, f32, f32, f32));

/// D-Bus `Notify` payload before timeout/output resolution.
pub(crate) struct Incoming {
    pub id: u32,
    pub app: String,
    pub icon: String,
    pub title: String,
    pub body: String,
    pub urgency: u8,
    /// Client-requested timeout; `None` means server default.
    pub timeout_ms: Option<u64>,
}

impl Notification {
    pub fn expired(&self, now: Instant) -> bool {
        self.timeout
            .is_some_and(|t| now.duration_since(self.received_at) >= t)
    }

    /// Internal event notification (`app = "riced"`). Critical urgency
    /// persists until clicked; anything else uses the configured
    /// default timeout. Caller assigns output via `handle_arrived`.
    pub(crate) fn internal(
        cfg: &NotificationConfig,
        title: &str,
        body: String,
        urgency: u8,
    ) -> Self {
        let timeout = if urgency >= 2 {
            None
        } else {
            Some(Duration::from_millis(cfg.timeout_ms))
        };
        Self {
            id: 0,
            output: None,
            app: "riced".to_string(),
            title: title.to_string(),
            body,
            icon: String::new(),
            urgency: urgency.min(2),
            received_at: Instant::now(),
            timeout,
        }
    }

    /// Build from a D-Bus `Notify` call. `timeout_ms` of `None` means
    /// server default; critical urgency persists until clicked.
    pub(crate) fn from_dbus(incoming: Incoming, default_ms: u64) -> Self {
        let timeout = if incoming.urgency >= 2 {
            None
        } else {
            Some(Duration::from_millis(
                incoming.timeout_ms.unwrap_or(default_ms),
            ))
        };
        Self {
            id: incoming.id,
            output: None,
            app: incoming.app,
            title: incoming.title,
            body: incoming.body,
            icon: incoming.icon,
            urgency: incoming.urgency.min(2),
            received_at: Instant::now(),
            timeout,
        }
    }
}

/// Pure: output whose rect contains the point (global coords).
pub(crate) fn output_at(geoms: &[OutputGeom], point: iced::Point) -> Option<OutputId> {
    geoms.iter().find_map(|(id, (x, y, w, h))| {
        (point.x >= *x && point.x < x + w && point.y >= *y && point.y < y + h).then_some(*id)
    })
}

/// Pure: drop expired entries in place.
pub(crate) fn sweep_expired(queue: &mut VecDeque<Notification>, now: Instant) {
    queue.retain(|n| !n.expired(now));
}

/// Insert, replacing the same id in place (D-Bus `replaces_id`).
pub(crate) fn enqueue(queue: &mut VecDeque<Notification>, item: Notification) {
    if let Some(slot) = queue.iter_mut().find(|n| n.id == item.id) {
        *slot = item;
    } else {
        queue.push_back(item);
    }
}

/// Resolve the target output: configured name first (`"mouse"`
/// means cursor output), then cursor output, then first output.
pub(crate) fn resolve_output(plots: &Plots, cfg: &NotificationConfig) -> Option<OutputId> {
    if cfg.output != "mouse" {
        let wanted = cfg.output.to_lowercase();
        if let Some((id, _)) = plots.output_infos.iter().find(|(_, info)| {
            info.name
                .as_deref()
                .is_some_and(|n| n.to_lowercase() == wanted)
        }) {
            return Some(*id);
        }
    }
    if let Some(cursor) = plots.last_cursor_global {
        let geoms: Vec<_> = plots
            .output_infos
            .iter()
            .map(|(id, info)| (*id, Background::output_geometry(info)))
            .collect();
        if let Some(hit) = output_at(&geoms, cursor) {
            return Some(hit);
        }
    }
    plots.output_infos.keys().copied().next()
}

/// Corner anchor + outward margins for the configured position.
fn placement(cfg: &NotificationConfig) -> (Anchor, (i32, i32, i32, i32)) {
    // Margin order is (top, right, bottom, left); only the two edges
    // facing the corner get the offset.
    const M: i32 = 16;
    match cfg.position.as_str() {
        "top-left" => (Anchor::Top | Anchor::Left, (M, 0, 0, M)),
        "bottom-right" => (Anchor::Bottom | Anchor::Right, (0, M, M, 0)),
        "bottom-left" => (Anchor::Bottom | Anchor::Left, (0, 0, M, M)),
        _ => (Anchor::Top | Anchor::Right, (M, M, 0, 0)),
    }
}

/// Estimated window height for `visible` cards.
pub(crate) fn window_height(visible: usize) -> u32 {
    (WINDOW_PAD + visible.max(1) as f32 * (CARD_H + CARD_GAP)).round() as u32
}

impl Notification {
    /// Open (or reuse) the layer window for `output` and show this
    /// stack on it. Windows are per output; the view slices the
    /// newest `max_visible` on top. Returns the spawn command when a
    /// window was created (`None` on reuse — the repaint covers it).
    pub(crate) fn ensure_window(
        plots: &mut Plots,
        output: OutputId,
        cfg: &NotificationConfig,
    ) -> Option<Command<Plant>> {
        if plots.notif_windows.contains_key(&output) {
            return None;
        }
        let id = window::Id::unique();
        let (anchor, margin) = placement(cfg);
        let visible = plots
            .notifications
            .iter()
            .filter(|n| n.output == Some(output))
            .count();
        let settings = NewLayerShellSettings {
            anchor,
            layer: Layer::Overlay,
            exclusive_zone: None,
            size: LayerSize::px(
                cfg.width.max(200.0) as u32,
                window_height(visible.min(cfg.max_visible.max(1) as usize)),
            ),
            output_option: OutputOption::GlobalName(output.0),
            margin: Some(margin),
            namespace: Some(format!("Riced - Notifications {output:?}")),
            keyboard_interactivity: KeyboardInteractivity::None,
            blur_option: BlurOption::None,
            ..Default::default()
        };
        plots.notif_windows.insert(output, id);
        plots.ids.insert(id, PlotInfo::Notification(output));
        Some(Command::done(Plant::NewLayerShell { settings, id }))
    }
}

/// Default card (Stage 1): title + up-to-three body lines. Stage 3
/// replaces the body with `notifications.lua` output.
fn default_card(n: &Notification, width: f32) -> Element<'static, Plant> {
    use iced::widget::{column, container, mouse_area, text};
    let mut lines = column![text(n.title.clone()).size(14.0)].spacing(2);
    for line in n.body.lines().take(3) {
        lines = lines.push(text(line.to_string()).size(12.0));
    }
    let card = container(lines)
        .width(Length::Fixed(width))
        .height(Length::Fixed(CARD_H))
        .padding(10)
        .clip(true)
        .style(theme::menu_box);
    mouse_area(card)
        .on_press(Plant::Notify(NotifyEvent::Dismissed(n.id)))
        .into()
}

/// Stack view for one output's window: newest first, capped. Bodies
/// come from cached `notifications.lua` trees, falling back to
/// [`default_card`] when the script is missing or broken. v1 has no
/// actions: the whole card is one dismiss area.
pub fn view(plots: &Plots, output: OutputId) -> Element<'_, Plant> {
    use iced::widget::{column, container, mouse_area};
    let cfg = &plots.config.notifications;
    let width = cfg.width.max(200.0) - WINDOW_PAD * 2.0;
    let mut stack = column![].spacing(CARD_GAP);
    for n in plots
        .notifications
        .iter()
        .filter(|n| n.output == Some(output))
        .rev()
        .take(cfg.max_visible.max(1) as usize)
    {
        let body: Element<'_, Plant> = match plots.notif_trees.get(&n.id) {
            Some(tree) => build_node(tree, 13.0, None).unwrap_or_else(|_| default_card(n, width)),
            None => default_card(n, width),
        };
        let card = container(body)
            .width(Length::Fixed(width))
            .height(Length::Fixed(CARD_H))
            .padding(10)
            .clip(true)
            .style(theme::menu_box);
        let el: Element<'_, Plant> = mouse_area(card)
            .on_press(Plant::Notify(NotifyEvent::Dismissed(n.id)))
            .into();
        stack = stack.push(el);
    }
    stack.into()
}

/// Script file for the Lua renderer, next to the widget scripts
/// (`~/.config/riced/widgets/notifications.lua`). Never overwritten
/// once seeded; missing file falls back to [`default_card`].
pub(crate) fn script_path() -> std::path::PathBuf {
    crate::config::widgets_dir().join("notifications.lua")
}

/// Drop the renderer state when the script changed on disk (same
/// live-edit story as widgets). Returns true on change.
fn sync_notify_lua(plots: &mut Plots) -> bool {
    let Ok(mtime) = std::fs::metadata(script_path()).and_then(|m| m.modified()) else {
        return false;
    };
    if plots.notify_mtime == Some(mtime) {
        return false;
    }
    plots.notify_lua = None;
    plots.notify_mtime = Some(mtime);
    true
}

fn note_error(plots: &mut Plots, err: String) {
    if plots.notify_last_error.as_deref() != Some(err.as_str()) {
        eprintln!("riced: notifications.lua: {err}");
        plots.notify_last_error = Some(err);
    }
}

fn ensure_notify_lua(plots: &mut Plots) -> Result<(), String> {
    if plots.notify_lua.is_some() {
        return Ok(());
    }
    let source = std::fs::read_to_string(script_path()).map_err(|e| format!("cannot read {e}"))?;
    let lua = new_widget_lua().map_err(|e| e.to_string())?;
    inject_ui(&lua).map_err(|e| e.to_string())?;
    lua.load(&source)
        .set_name("@notifications.lua")
        .exec()
        .map_err(|e| e.to_string())?;
    let _: mlua::Function = lua.globals().get("render").map_err(|e| e.to_string())?;
    plots.notify_lua = Some(lua);
    Ok(())
}

/// Render one notification through `render(n)` and cache the tree.
/// Missing/broken scripts fall back to [`default_card`] (and clear
/// any stale cached tree); errors log once per message.
pub(crate) fn render_noti(plots: &mut Plots, n: &Notification) {
    if let Err(e) = ensure_notify_lua(plots) {
        note_error(plots, e);
        plots.notif_trees.remove(&n.id);
        return;
    }
    let result: Result<WidgetNode, String> = (|| {
        let lua = plots.notify_lua.as_ref().ok_or("runtime missing")?;
        let table = lua.create_table().map_err(|e| e.to_string())?;
        table.set("id", n.id).map_err(|e| e.to_string())?;
        table.set("app", n.app.clone()).map_err(|e| e.to_string())?;
        table
            .set("title", n.title.clone())
            .map_err(|e| e.to_string())?;
        table
            .set("body", n.body.clone())
            .map_err(|e| e.to_string())?;
        table
            .set("icon", n.icon.clone())
            .map_err(|e| e.to_string())?;
        table.set("urgency", n.urgency).map_err(|e| e.to_string())?;
        let render: mlua::Function = lua.globals().get("render").map_err(|e| e.to_string())?;
        let value: mlua::Value = render.call(table).map_err(|e| e.to_string())?;
        parse_node(&value)
    })();
    // Borrow dance: result computed, plots free again.
    match result {
        Ok(node) => {
            plots.notify_last_error = None;
            plots.notif_trees.insert(n.id, node);
        }
        Err(e) => {
            note_error(plots, e);
            plots.notif_trees.remove(&n.id);
        }
    }
}

/// Render every cached notification (script edit while visible).
fn render_all_notis(plots: &mut Plots) {
    let ids: Vec<u32> = plots.notifications.iter().map(|n| n.id).collect();
    for id in ids {
        if let Some(n) = plots.notifications.iter().find(|n| n.id == id).cloned() {
            render_noti(plots, &n);
        }
    }
}

impl Notification {
    /// Ingest an arrival: resolve output, assign id when zero, enqueue
    /// (same id replaces in place), ensure the window. Disabled layer
    /// drops everything silently.
    pub(crate) fn handle_arrived(plots: &mut Plots, mut n: Notification) -> Command<Plant> {
        if !plots.config.notifications.enabled {
            return Command::none();
        }
        let output = n
            .output
            .or_else(|| resolve_output(plots, &plots.config.notifications));
        let Some(output) = output else {
            return Command::none();
        };
        n.output = Some(output);
        if n.id == 0 {
            n.id = plots.notif_next_id.max(1);
            plots.notif_next_id = plots.notif_next_id.max(1).wrapping_add(1);
        }
        let id = n.id;
        enqueue(&mut plots.notifications, n);
        if let Some(stored) = plots.notifications.iter().find(|n| n.id == id).cloned() {
            render_noti(plots, &stored);
        }
        Self::reconcile(plots, output)
    }

    /// Dismiss by id (click, D-Bus close, sweep). Unknown ids are ignored.
    pub(crate) fn handle_dismissed(plots: &mut Plots, id: u32) -> Command<Plant> {
        let before = plots.notifications.len();
        plots.notifications.retain(|n| n.id != id);
        plots.notif_trees.remove(&id);
        if plots.notifications.len() == before {
            return Command::none();
        }
        Self::reconcile_all(plots)
    }

    /// Expiry sweep tick (+ script hot-reload while visible): drop
    /// timed-out notifications, close emptied windows. Repaint comes
    /// from the `Scope::All` redraw scope.
    pub(crate) fn handle_tick(plots: &mut Plots) -> Command<Plant> {
        if sync_notify_lua(plots) {
            render_all_notis(plots);
        }
        let now = Instant::now();
        let before = plots.notifications.len();
        sweep_expired(&mut plots.notifications, now);
        for gone in plots
            .notif_trees
            .keys()
            .filter(|id| !plots.notifications.iter().any(|n| &n.id == *id))
            .copied()
            .collect::<Vec<_>>()
        {
            plots.notif_trees.remove(&gone);
        }
        if plots.notifications.len() == before {
            return Command::none();
        }
        Self::reconcile_all(plots)
    }

    /// Reconcile every showing output (used after dismiss/sweep).
    fn reconcile_all(plots: &mut Plots) -> Command<Plant> {
        let outputs: Vec<OutputId> = plots.notif_windows.keys().copied().collect();
        let mut cmds = Vec::new();
        for output in outputs {
            cmds.push(Self::reconcile(plots, output));
        }
        if cmds.is_empty() {
            Command::none()
        } else {
            Command::batch(cmds)
        }
    }

    /// Make the window match the stack: open when non-empty, resize on
    /// count change, close + drop mappings when empty.
    fn reconcile(plots: &mut Plots, output: OutputId) -> Command<Plant> {
        let cfg = plots.config.notifications.clone();
        let visible = plots
            .notifications
            .iter()
            .filter(|n| n.output == Some(output))
            .count()
            .min(cfg.max_visible.max(1) as usize);
        match plots.notif_windows.get(&output).copied() {
            None => {
                if visible == 0 {
                    return Command::none();
                }
                // ensure_window registers mappings; record its size.
                let h = window_height(visible);
                let cmd = Self::ensure_window(plots, output, &cfg);
                plots.notif_sizes.insert(output, h);
                cmd.unwrap_or_else(Command::none)
            }
            Some(id) => {
                if visible == 0 {
                    plots.notif_windows.remove(&output);
                    plots.notif_sizes.remove(&output);
                    plots.ids.remove(&id);
                    return iced_runtime::task::effect(Action::Window(WindowAction::Close(id)));
                }
                let h = window_height(visible);
                if plots.notif_sizes.get(&output).copied() == Some(h) {
                    return Command::none();
                }
                plots.notif_sizes.insert(output, h);
                let (anchor, _) = placement(&cfg);
                Command::done(Plant::LayoutChange {
                    id,
                    anchor,
                    size: LayerSize::px(cfg.width.max(200.0) as u32, h),
                })
            }
        }
    }

    /// Drop one output's windows + notifications (compositor close or
    /// output removal). Transient by nature: nothing migrates.
    pub(crate) fn remove_for_output(plots: &mut Plots, output: OutputId) -> Command<Plant> {
        let mut cmds = Vec::new();
        if let Some(id) = plots.notif_windows.remove(&output) {
            plots.ids.remove(&id);
            cmds.push(iced_runtime::task::effect(Action::Window(
                WindowAction::Close(id),
            )));
        }
        plots.notif_sizes.remove(&output);
        plots.notifications.retain(|n| n.output != Some(output));
        let live: std::collections::HashSet<u32> =
            plots.notifications.iter().map(|n| n.id).collect();
        plots.notif_trees.retain(|id, _| live.contains(id));
        if cmds.is_empty() {
            Command::none()
        } else {
            Command::batch(cmds)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::layers::top::{WidgetNode, inject_ui, new_widget_lua, parse_node};

    fn noti(id: u32, age_ms: u64, timeout_ms: Option<u64>, urgency: u8) -> Notification {
        Notification {
            id,
            output: Some(OutputId(1)),
            app: "test".to_string(),
            title: "t".to_string(),
            body: "b".to_string(),
            icon: String::new(),
            urgency,
            received_at: Instant::now() - Duration::from_millis(age_ms),
            timeout: timeout_ms.map(Duration::from_millis),
        }
    }

    #[test]
    fn output_at_hits_rects_and_edges() {
        let geoms = vec![
            (OutputId(1), (0.0, 0.0, 1920.0, 1080.0)),
            (OutputId(2), (1920.0, 0.0, 1920.0, 1080.0)),
        ];
        assert_eq!(
            output_at(&geoms, iced::Point::new(100.0, 100.0)),
            Some(OutputId(1))
        );
        assert_eq!(
            output_at(&geoms, iced::Point::new(2000.0, 500.0)),
            Some(OutputId(2))
        );
        // Left/top edges inclusive, right/bottom exclusive.
        assert_eq!(
            output_at(&geoms, iced::Point::new(1920.0, 0.0)),
            Some(OutputId(2))
        );
        assert_eq!(output_at(&geoms, iced::Point::new(5000.0, 5000.0)), None);
        assert_eq!(output_at(&[], iced::Point::new(0.0, 0.0)), None);
    }

    #[test]
    fn sweep_expired_keeps_fresh_and_critical() {
        let mut q = VecDeque::from([
            noti(1, 10_000, Some(5_000), 1), // stale normal
            noti(2, 1_000, Some(5_000), 1),  // fresh normal
            noti(3, 60_000, None, 2),        // critical persists
            noti(4, 10_000, Some(5_000), 0), // stale low
        ]);
        sweep_expired(&mut q, Instant::now());
        let ids: Vec<u32> = q.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![2, 3]);
    }

    #[test]
    fn enqueue_replaces_same_id_in_place() {
        let mut q = VecDeque::from([noti(1, 0, Some(5_000), 1), noti(2, 0, Some(5_000), 1)]);
        let mut rep = noti(1, 0, Some(5_000), 1);
        rep.title = "new".to_string();
        enqueue(&mut q, rep);
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].title, "new");
        enqueue(&mut q, noti(7, 0, Some(5_000), 1));
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn window_height_scales_with_visible_cards() {
        assert_eq!(window_height(0), window_height(1));
        assert!(window_height(3) > window_height(1));
    }

    #[test]
    fn placement_covers_all_corners() {
        for (pos, anchor) in [
            ("top-right", Anchor::Top | Anchor::Right),
            ("top-left", Anchor::Top | Anchor::Left),
            ("bottom-right", Anchor::Bottom | Anchor::Right),
            ("bottom-left", Anchor::Bottom | Anchor::Left),
            ("nonsense", Anchor::Top | Anchor::Right),
        ] {
            let cfg = crate::config::NotificationConfig {
                position: pos.to_string(),
                ..Default::default()
            };
            assert_eq!(placement(&cfg).0, anchor, "{pos}");
        }
    }

    #[test]
    fn seed_renders_a_card() {
        use crate::config::SEED_NOTIFICATIONS_LUA;
        let lua = new_widget_lua().expect("sandbox");
        inject_ui(&lua).expect("ui");
        lua.load(SEED_NOTIFICATIONS_LUA).exec().expect("load");
        let table = lua.create_table().expect("table");
        table.set("id", 1).expect("set");
        table.set("app", "test").expect("set");
        table.set("title", "hello").expect("set");
        table.set("body", "world").expect("set");
        table.set("icon", "").expect("set");
        table.set("urgency", 1).expect("set");
        let render: mlua::Function = lua.globals().get("render").expect("render");
        let value: mlua::Value = render.call(table).expect("call");
        let node = parse_node(&value).expect("parse");
        assert!(matches!(node, WidgetNode::Column { .. }));
        // Critical urgency renders without error too.
        let table2 = lua.create_table().expect("table");
        table2.set("id", 2).expect("set");
        for (k, v) in [("app", "t"), ("title", "h"), ("body", "b"), ("icon", "")] {
            table2.set(k, v).expect("set");
        }
        table2.set("urgency", 2).expect("set");
        let value: mlua::Value = render.call(table2).expect("call");
        assert!(parse_node(&value).is_ok());
    }
}
