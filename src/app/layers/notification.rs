//! Lua-configured notification layer (D-Bus server + internal events).
//!
//! One layer-shell window per output that currently shows anything
//! (usually just the mouse output). Each window spans the full output
//! height and renders that output's whole stack newest-first in a
//! scrollable (listview, no cap); windows close when their stack
//! empties.
//! Bodies render from `notifications.lua`, falling back to
//! [`default_tree`] when the script is missing or broken.
//!
//! Cards animate on the shared [`crate::app::layers::anim::TransSet`]
//! machine keyed `(output, id)`: arrivals slide in from the anchored
//! edge, dismissals linger as inert ghosts sliding out. Placement
//! honors `[notifications] output`: `"mouse"` resolves the output
//! under the last-known global cursor (or the compositor's `LastOutput`
//! equivalent via `OutputOption`), any other value pins by output name
//! (`OutputInfo.name`, e.g. `"DP-1"`).

use super::background::Background;
use super::top::{NodeLength, WidgetNode, build_node, inject_ui, new_widget_lua, parse_node};
use crate::app::app::{PlotInfo, Plots};
use crate::app::{NotifyEvent, Plant};
use crate::config::NotificationConfig;
use crate::theme;
use aura_anim::core::timing::Timing;
use iced::window;
use iced::{Element, Length, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings,
    OutputOption, WlRegion,
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
/// Corner margin left at each anchored edge (matches `placement`'s
/// `M`): the window spans the full output height minus both margins.
const WINDOW_MARGIN: f32 = 16.0;

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
    /// Action `(key, label)` pairs from the D-Bus `actions` array.
    pub actions: Vec<(String, String)>,
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
    pub actions: Vec<(String, String)>,
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
            actions: Vec::new(),
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
            actions: incoming.actions,
        }
    }
}

/// Pure: output whose rect contains the point (global coords).
pub(crate) fn output_at(geoms: &[OutputGeom], point: iced::Point) -> Option<OutputId> {
    geoms.iter().find_map(|(id, (x, y, w, h))| {
        (point.x >= *x && point.x < x + w && point.y >= *y && point.y < y + h).then_some(*id)
    })
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
    const M: i32 = WINDOW_MARGIN as i32;
    match cfg.position.as_str() {
        "top-left" => (Anchor::Top | Anchor::Left, (M, 0, 0, M)),
        "bottom-right" => (Anchor::Bottom | Anchor::Right, (0, M, M, 0)),
        "bottom-left" => (Anchor::Bottom | Anchor::Left, (0, 0, M, M)),
        _ => (Anchor::Top | Anchor::Right, (M, M, 0, 0)),
    }
}

/// Estimated window height for `visible` cards (floor for the
/// output-height window when geometry is unknown or tiny).
pub(crate) fn window_height(visible: usize) -> u32 {
    (WINDOW_PAD + visible.max(1) as f32 * (CARD_H + CARD_GAP)).round() as u32
}

/// Listview window height: the full output height minus both corner
/// margins, so the whole stack shows (scrolling) instead of capping
/// at `max_visible`. Never smaller than one card.
pub(crate) fn window_height_for_output(output_h: f32) -> u32 {
    (output_h - 2.0 * WINDOW_MARGIN)
        .max(window_height(1) as f32)
        .round() as u32
}

/// Output height behind `output` (full geometry, ignores bars),
/// falling back to HD when the compositor hasn't reported it yet.
fn output_height(plots: &Plots, output: OutputId) -> f32 {
    plots
        .output_infos
        .get(&output)
        .map(|info| Background::output_geometry(info).3)
        .unwrap_or(1080.0)
}

impl Notification {
    /// Open (or reuse) the layer window for `output` and show this
    /// stack on it. Windows are per output and span the full output
    /// height (listview: the whole stack scrolls); the view slices
    /// nothing. Returns the spawn command when a window was created
    /// (`None` on reuse — the repaint covers it).
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
        let settings = NewLayerShellSettings {
            anchor,
            layer: Layer::Overlay,
            exclusive_zone: None,
            size: LayerSize::px(
                cfg.width.max(200.0) as u32,
                window_height_for_output(output_height(plots, output)),
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

/// Slide timing for notification cards (shared animation speed).
fn noti_timing(plots: &Plots) -> Timing {
    Timing::ease_out(plots.config.animation.speed.duration())
}

/// Retire one notification into a ghost on the shared transition set:
/// drop it from the queue but keep its tree, sliding out from its
/// current pad (mid-enter dismissals retarget smoothly instead of
/// jumping). Returns false when the id was already gone.
fn retire_noti(plots: &mut Plots, id: u32) -> bool {
    let order = visible_order(&plots.notifications);
    let Some(pos) = order.iter().position(|(nid, _)| *nid == id) else {
        plots.notif_trees.remove(&id);
        return false;
    };
    let output = order[pos].1;
    let cached = plots.notif_trees.remove(&id);
    let fallback = plots.notifications.iter().find(|n| n.id == id);
    let node = cached.unwrap_or_else(|| default_tree(fallback));
    plots.notifications.retain(|n| n.id != id);
    let timing = noti_timing(plots);
    plots
        .notif_trans
        .retire(&mut plots.anim_runtime, timing, (output, id), pos, node);
    true
}

/// Visible `(id, output)` in newest-first, per-output view order —
/// the single ordering both view and retire share. Uncapped: the
/// window spans the output height and the stack scrolls.
fn visible_order(notifications: &VecDeque<Notification>) -> Vec<(u32, OutputId)> {
    let mut by_output: std::collections::HashMap<OutputId, Vec<u32>> = Default::default();
    for n in notifications {
        if let Some(output) = n.output {
            by_output.entry(output).or_default().push(n.id);
        }
    }
    let mut ordered = Vec::new();
    for (output, mut ids) in by_output {
        ids.reverse();
        ordered.extend(ids.into_iter().map(|id| (id, output)));
    }
    ordered
}

/// Drop settled notification exits (called from the shared 16ms
/// animation loop after it ticks the runtime).
pub(crate) fn sweep_noti_anims(plots: &mut Plots) {
    plots.notif_trans.sweep(&mut plots.anim_runtime);
}

/// Default card tree: title + up-to-three body lines + one button
/// row per action. Used when `notifications.lua` is missing/broken —
/// and as ghost content, so every visible card (live or ghost) is
/// always a `WidgetNode`.
fn default_tree(n: Option<&Notification>) -> WidgetNode {
    let (title, body, actions) = n
        .map(|n| (n.title.clone(), n.body.clone(), n.actions.clone()))
        .unwrap_or_default();
    let mut children = vec![WidgetNode::Text {
        content: title,
        size: Some(14.0),
        width: None,
        height: None,
    }];
    for line in body.lines().take(3) {
        children.push(WidgetNode::Text {
            content: line.to_string(),
            size: Some(12.0),
            width: None,
            height: None,
        });
    }
    if !actions.is_empty() {
        children.push(WidgetNode::Row {
            children: actions
                .into_iter()
                .map(|(key, label)| WidgetNode::Button {
                    label,
                    action: key,
                    width: None,
                    height: None,
                    padding: None,
                })
                .collect(),
            spacing: 4.0,
            width: NodeLength::Shrink,
            height: NodeLength::Shrink,
        });
    }
    WidgetNode::Column {
        children,
        spacing: 2.0,
        width: NodeLength::Shrink,
        height: NodeLength::Shrink,
    }
}

/// Stack view for one output's window: all live cards newest-first
/// plus ghosts merged at their old indices, in a scrollable the
/// height of the output. Bodies come from cached
/// `notifications.lua` trees, falling back to [`default_tree`] when
/// the script is missing or broken. Cards slide from the anchored edge
/// (down for top corners, up for bottom ones). Action buttons route to
/// `on_action`-style `Invoke` messages; the rest of a live card is one
/// dismiss area while ghosts stay inert.
pub fn view(plots: &Plots, output: OutputId) -> Element<'_, Plant> {
    use iced::widget::{column, container, mouse_area, scrollable};
    let cfg = &plots.config.notifications;
    let width = cfg.width.max(200.0) - WINDOW_PAD * 2.0;
    let from_bottom = cfg.position.starts_with("bottom");
    struct Card {
        id: u32,
        node: WidgetNode,
        live: bool,
    }
    let mut cards: Vec<Card> = visible_order(&plots.notifications)
        .into_iter()
        .filter(|(_, o)| *o == output)
        .map(|(id, _)| {
            let node =
                plots.notif_trees.get(&id).cloned().unwrap_or_else(|| {
                    default_tree(plots.notifications.iter().find(|n| n.id == id))
                });
            Card {
                id,
                node,
                live: true,
            }
        })
        .collect();
    for ghost in plots.notif_trans.ghosts_for(|(o, _)| *o == output) {
        let at = ghost.index.min(cards.len());
        let card = Card {
            id: ghost.key.1,
            node: ghost.node.clone(),
            live: false,
        };
        cards.push(card);
        let last = cards.len() - 1;
        if at < last {
            let card = cards.remove(last);
            cards.insert(at, card);
        }
    }
    let mut stack = column![].spacing(CARD_GAP);
    for card in &cards {
        let pad = plots
            .notif_trans
            .pad(&plots.anim_runtime, &(output, card.id));
        // Live buttons invoke actions; ghosts (and build failures
        // falling back to the default tree) stay inert.
        let msg = |action: String| {
            let id = card.id;
            Plant::Notify(NotifyEvent::Invoke(id, action))
        };
        let body: Element<'_, Plant> = build_node(
            &card.node,
            13.0,
            card.live.then_some(&msg as &dyn Fn(String) -> Plant),
        )
        .unwrap_or_else(|_| {
            build_node(
                &default_tree(plots.notifications.iter().find(|n| n.id == card.id)),
                13.0,
                None,
            )
            .expect("default tree builds")
        });
        let (top_extra, bottom_extra) = if from_bottom { (0.0, pad) } else { (pad, 0.0) };
        let chrome = container(body)
            .width(Length::Fixed(width))
            .height(Length::Fixed(CARD_H))
            .padding(iced::Padding {
                top: 10.0 + top_extra,
                right: 10.0,
                bottom: 10.0 + bottom_extra,
                left: 10.0,
            })
            .clip(true)
            .style(theme::menu_box);
        // Skip the wrapper when settled (fewer layout nodes).
        let el: Element<'_, Plant> = if card.live {
            mouse_area(chrome)
                .on_press(Plant::Notify(NotifyEvent::Dismissed(card.id)))
                .into()
        } else {
            chrome.into()
        };
        stack = stack.push(el);
    }
    scrollable(stack)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Script file for the Lua renderer, next to the widget scripts
/// (`~/.config/riced/widgets/notifications.lua`). Never overwritten
/// once seeded; missing file falls back to [`default_tree`].
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

/// `n.actions` for Lua: 1-based array of `{key, label}` tables.
fn actions_table(lua: &mlua::Lua, actions: &[(String, String)]) -> Result<mlua::Table, String> {
    let table = lua.create_table().map_err(|e| e.to_string())?;
    for (i, (key, label)) in actions.iter().enumerate() {
        let entry = lua.create_table().map_err(|e| e.to_string())?;
        entry.set("key", key.clone()).map_err(|e| e.to_string())?;
        entry
            .set("label", label.clone())
            .map_err(|e| e.to_string())?;
        table.set(i + 1, entry).map_err(|e| e.to_string())?;
    }
    Ok(table)
}

/// Render one notification through `render(n)` and cache the tree.
/// Missing/broken scripts fall back to [`default_tree`] (and clear
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
        table
            .set("actions", actions_table(lua, &n.actions)?)
            .map_err(|e| e.to_string())?;
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
        // Fresh card slides in on the shared transition set.
        let timing = Timing::ease_out(plots.config.animation.speed.duration());
        plots
            .notif_trans
            .enter(&mut plots.anim_runtime, timing, (output, id));
        Self::reconcile(plots, output)
    }

    /// Dismiss by id (card click). Unknown ids are ignored. Reports
    /// reason 2 (`dismissed by user`) back over D-Bus.
    pub(crate) fn handle_dismissed(plots: &mut Plots, id: u32) -> Command<Plant> {
        if !retire_noti(plots, id) {
            return Command::none();
        }
        Command::batch(vec![
            Self::reconcile_all(plots),
            crate::notify::emit_closed(id, 2),
        ])
    }

    /// Silent drop for peer-initiated closes (D-Bus
    /// `CloseNotification`): the `NotificationClosed` signal (reason 3)
    /// already went out on the bus, so no second signal here.
    pub(crate) fn handle_peer_closed(plots: &mut Plots, id: u32) -> Command<Plant> {
        if !retire_noti(plots, id) {
            return Command::none();
        }
        Self::reconcile_all(plots)
    }

    /// Fire one action button: emit `ActionInvoked` for valid keys,
    /// then dismiss like a click (reason 2). Unknown ids — or keys the
    /// notification doesn't offer (stale trees) — are ignored.
    pub(crate) fn handle_invoke(plots: &mut Plots, id: u32, key: String) -> Command<Plant> {
        let known = plots
            .notifications
            .iter()
            .find(|n| n.id == id)
            .is_some_and(|n| n.actions.iter().any(|(k, _)| *k == key));
        if !known {
            return Command::none();
        }
        let mut cmds = vec![crate::notify::emit_action_invoked(id, key)];
        if retire_noti(plots, id) {
            cmds.push(Self::reconcile_all(plots));
            cmds.push(crate::notify::emit_closed(id, 2));
        }
        Command::batch(cmds)
    }

    /// Expiry sweep tick (+ script hot-reload while visible): retire
    /// timed-out notifications (reason 1 each), close emptied windows.
    /// Repaint comes from the `Scope::All` redraw scope.
    pub(crate) fn handle_tick(plots: &mut Plots) -> Command<Plant> {
        if sync_notify_lua(plots) {
            render_all_notis(plots);
        }
        let now = Instant::now();
        let mut cmds = Vec::new();
        for id in plots
            .notifications
            .iter()
            .filter(|n| n.expired(now))
            .map(|n| n.id)
            .collect::<Vec<_>>()
        {
            if retire_noti(plots, id) {
                cmds.push(crate::notify::emit_closed(id, 1));
            }
        }
        if cmds.is_empty() {
            return Command::none();
        }
        cmds.push(Self::reconcile_all(plots));
        Command::batch(cmds)
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

    /// Make the window match the stack: open when non-empty, close +
    /// drop mappings when empty. Ghosts keep the window alive until
    /// their exit settles. Height follows the output (listview), so
    /// arrivals and dismissals never resize — only output geometry
    /// changes do (see [`Self::reapply_for_output`]). Every path that
    /// changes the stack also re-pushes the input region so only card
    /// rects stay clickable (see [`Self::push_input_region`]).
    fn reconcile(plots: &mut Plots, output: OutputId) -> Command<Plant> {
        let cfg = plots.config.notifications.clone();
        let live = plots
            .notifications
            .iter()
            .filter(|n| n.output == Some(output))
            .count();
        let ghosts = plots.notif_trans.ghosts_for(|(o, _)| *o == output).len();
        let visible = live + ghosts;
        match plots.notif_windows.get(&output).copied() {
            None => {
                if visible == 0 {
                    return Command::none();
                }
                // ensure_window registers mappings; record its size.
                let h = window_height_for_output(output_height(plots, output));
                let cmd = Self::ensure_window(plots, output, &cfg);
                plots.notif_sizes.insert(output, h);
                let open = cmd.unwrap_or_else(Command::none);
                Command::batch(vec![open, Self::push_input_region(plots, output)])
            }
            Some(id) => {
                if visible == 0 {
                    plots.notif_windows.remove(&output);
                    plots.notif_sizes.remove(&output);
                    plots.ids.remove(&id);
                    return iced_runtime::task::effect(Action::Window(WindowAction::Close(id)));
                }
                let h = window_height_for_output(output_height(plots, output));
                if plots.notif_sizes.get(&output).copied() == Some(h) {
                    return Self::push_input_region(plots, output);
                }
                plots.notif_sizes.insert(output, h);
                let (anchor, _) = placement(&cfg);
                Command::batch(vec![
                    Command::done(Plant::LayoutChange {
                        id,
                        anchor,
                        size: LayerSize::px(cfg.width.max(200.0) as u32, h),
                    }),
                    Self::push_input_region(plots, output),
                ])
            }
        }
    }

    /// Re-push the output-height window size after a geometry change
    /// (mirrors `Top::reapply_for_output`). No-op when no window is
    /// showing on the output or the height didn't move.
    pub(crate) fn reapply_for_output(plots: &mut Plots, output: OutputId) -> Command<Plant> {
        let Some(id) = plots.notif_windows.get(&output).copied() else {
            return Command::none();
        };
        let cfg = plots.config.notifications.clone();
        let h = window_height_for_output(output_height(plots, output));
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

    /// Card rects for the compositor input region (Quickshell-mask
    /// equivalent): one `(x, y, w, h)` per visible card, in window
    /// coords. Everything outside these rects clicks through — the
    /// tall listview window never swallows background clicks. Y
    /// offsets mirror the `view` stack (pad + chrome + gap); the
    /// scroll offset is unknown here, so cards past the viewport map
    /// to their unscrolled position (harmless: still inside the
    /// window, still clickable area).
    pub(crate) fn input_rects(plots: &Plots, output: OutputId) -> Vec<(i32, i32, i32, i32)> {
        let count = visible_order(&plots.notifications)
            .into_iter()
            .filter(|(_, o)| *o == output)
            .count()
            + plots.notif_trans.ghosts_for(|(o, _)| *o == output).len();
        let cfg = &plots.config.notifications;
        let width = cfg.width.max(200.0);
        let mut rects = Vec::with_capacity(count);
        let mut y = 0.0f32;
        for _ in 0..count {
            // Settled chrome height: padding 10 + body 100 + gap 8.
            // In-flight pads only shift cards within the window; the
            // rect union still covers them.
            rects.push((
                0,
                y.round() as i32,
                width.round() as i32,
                (CARD_H + 20.0).round() as i32,
            ));
            y += CARD_H + 20.0 + CARD_GAP;
        }
        rects
    }

    /// Push [`input_rects`] to the compositor for `output`'s window
    /// (no-op when no window is showing). Called on every reconcile
    /// path that changes the stack so the mask tracks arrivals,
    /// dismissals, and ghost exits. The runtime clears the region
    /// first (`subtract` of the whole surface), so only the card
    /// rects stay clickable — everything else passes through.
    pub(crate) fn push_input_region(plots: &Plots, output: OutputId) -> Command<Plant> {
        let Some(id) = plots.notif_windows.get(&output).copied() else {
            return Command::none();
        };
        let rects = Self::input_rects(plots, output);
        Command::done(Plant::SetInputRegion {
            id,
            callback: iced_exwlshell::actions::ActionCallback::new(move |region: &WlRegion| {
                for (x, y, w, h) in &rects {
                    region.add(*x, *y, *w, *h);
                }
            }),
        })
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
        // Output gone: settle its transitions instantly (no window left
        // to animate in).
        plots
            .notif_trans
            .clear_scope(&mut plots.anim_runtime, |(o, _)| *o == output);
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
            actions: Vec::new(),
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
    fn expiry_honours_timeout_and_critical_persist() {
        let now = Instant::now();
        assert!(noti(1, 10_000, Some(5_000), 1).expired(now)); // stale normal
        assert!(!noti(2, 1_000, Some(5_000), 1).expired(now)); // fresh normal
        assert!(!noti(3, 60_000, None, 2).expired(now)); // critical persists
        assert!(noti(4, 10_000, Some(5_000), 0).expired(now)); // stale low
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
    fn window_height_for_output_spans_minus_margins() {
        // 1080p output minus both 16px corner margins.
        assert_eq!(window_height_for_output(1080.0), 1048);
        // Tiny/unknown geometry never collapses below one card.
        assert_eq!(
            window_height_for_output(0.0),
            window_height_for_output(10.0)
        );
        assert_eq!(window_height_for_output(10.0), window_height(1));
    }

    #[test]
    fn input_rects_cover_each_card_and_nothing_else() {
        use crate::app::Plots;
        use iced_wayland_subscriber::shell::channel;
        let (_tx, rx) = channel();
        let mut plots = Plots::new(rx);
        // Empty stack: no rects, fully click-through.
        assert!(Notification::input_rects(&plots, OutputId(1)).is_empty());
        // Two cards on output 1, one on output 2.
        for (id, output) in [(1, OutputId(1)), (2, OutputId(1)), (3, OutputId(2))] {
            let mut n = noti(id, 0, Some(5_000), 1);
            n.output = Some(output);
            plots.notifications.push_back(n);
        }
        let rects = Notification::input_rects(&plots, OutputId(1));
        assert_eq!(rects.len(), 2);
        let w = plots.config.notifications.width.max(200.0).round() as i32;
        let card = (CARD_H + 20.0).round() as i32;
        assert_eq!(rects[0], (0, 0, w, card));
        assert_eq!(rects[1], (0, card + CARD_GAP as i32, w, card));
        // Output 2 sees only its own card.
        assert_eq!(Notification::input_rects(&plots, OutputId(2)).len(), 1);
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

    #[test]
    fn visible_order_is_newest_first_per_output() {
        let mk = |id: u32, output: Option<OutputId>| Notification {
            id,
            output,
            app: String::new(),
            title: String::new(),
            body: String::new(),
            icon: String::new(),
            urgency: 1,
            received_at: Instant::now(),
            timeout: Some(Duration::from_secs(5)),
            actions: Vec::new(),
        };
        let queue = VecDeque::from([
            mk(1, Some(OutputId(1))),
            mk(2, Some(OutputId(1))),
            mk(3, Some(OutputId(1))),
            mk(4, Some(OutputId(1))),
            mk(5, Some(OutputId(2))),
            mk(6, None),
        ]);
        // Newest first, uncapped (the window scrolls), unplaced excluded.
        let mut order = visible_order(&queue);
        order.sort();
        assert_eq!(
            order,
            vec![
                (1, OutputId(1)),
                (2, OutputId(1)),
                (3, OutputId(1)),
                (4, OutputId(1)),
                (5, OutputId(2)),
            ]
        );
        // Within one output the newest id sorts last after the sort,
        // but insertion order is newest-first: check directly.
        let out1: Vec<u32> = visible_order(&queue)
            .into_iter()
            .filter(|(_, o)| *o == OutputId(1))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(out1, vec![4, 3, 2, 1]);
    }

    #[test]
    fn notification_transitions_share_the_generic_set() {
        use super::super::anim::{ENTER_OFFSET, TransSet};
        use aura_anim::core::{runtime::MotionRuntime, timing::Timing};
        // Same TransSet machine as widget rows, keyed (output, id).
        let mut rt = MotionRuntime::new();
        let timing = Timing::ease_out(Duration::from_millis(150));
        let mut set: TransSet<(OutputId, u32)> = TransSet::default();
        set.enter(&mut rt, timing, (OutputId(1), 7));
        assert_eq!(set.pad(&rt, &(OutputId(1), 7)), ENTER_OFFSET);
        set.retire(
            &mut rt,
            timing,
            (OutputId(1), 7),
            0,
            WidgetNode::Text {
                content: "hi".to_string(),
                size: None,
                width: None,
                height: None,
            },
        );
        assert_eq!(set.ghosts_for(|(o, _)| *o == OutputId(1)).len(), 1);
        // Other outputs untouched by scope clearing.
        set.enter(&mut rt, timing, (OutputId(2), 9));
        set.clear_scope(&mut rt, |(o, _)| *o == OutputId(1));
        assert!(set.ghosts_for(|_| true).is_empty());
        assert!(set.pad(&rt, &(OutputId(2), 9)) > 0.0);
    }

    #[test]
    fn default_tree_renders_title_and_body_lines() {
        let n = noti(1, 0, Some(5_000), 1);
        let tree = default_tree(Some(&n));
        assert!(matches!(tree, WidgetNode::Column { .. }));
        let _ = tree;
        let empty = default_tree(None);
        assert!(matches!(empty, WidgetNode::Column { .. }));
    }

    #[test]
    fn default_tree_appends_one_button_row_per_action() {
        let mut n = noti(1, 0, Some(5_000), 1);
        // No actions: text only, no trailing row.
        let bare = default_tree(Some(&n));
        assert!(matches!(bare, WidgetNode::Column { .. }));
        n.actions = vec![
            ("default".to_string(), "Activate".to_string()),
            ("mute".to_string(), "Mute".to_string()),
        ];
        let tree = default_tree(Some(&n));
        let WidgetNode::Column { children, .. } = tree else {
            panic!("expected column");
        };
        let Some(WidgetNode::Row { children: btns, .. }) = children.last() else {
            panic!("expected trailing button row");
        };
        let keys: Vec<&str> = btns
            .iter()
            .filter_map(|b| match b {
                WidgetNode::Button { action, label, .. } => {
                    assert!(!label.is_empty());
                    Some(action.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(keys, vec!["default", "mute"]);
    }

    #[test]
    fn from_dbus_carries_actions_through() {
        let n = Notification::from_dbus(
            Incoming {
                id: 7,
                app: "firefox".to_string(),
                icon: "firefox".to_string(),
                title: "t".to_string(),
                body: "b".to_string(),
                actions: vec![("default".to_string(), "Activate".to_string())],
                urgency: 1,
                timeout_ms: None,
            },
            5_000,
        );
        assert_eq!(n.id, 7);
        assert_eq!(
            n.actions,
            vec![("default".to_string(), "Activate".to_string())]
        );
    }

    #[test]
    fn lua_sees_actions_as_key_label_tables() {
        let lua = new_widget_lua().expect("sandbox");
        let made = actions_table(
            &lua,
            &[
                ("default".to_string(), "Activate".to_string()),
                ("mute".to_string(), "Mute".to_string()),
            ],
        )
        .expect("table");
        lua.globals().set("got", made).expect("set");
        // 1-based array of {key, label} — the shape the seed consumes.
        let seen: String = lua
            .load(r#"return got[1].key .. "=" .. got[1].label .. "," .. got[2].key"#)
            .eval()
            .expect("eval");
        assert_eq!(seen, "default=Activate,mute");
        let empty = actions_table(&lua, &[]).expect("empty");
        let len: i64 = empty.len().expect("len");
        assert_eq!(len, 0);
    }
}
