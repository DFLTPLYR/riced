use super::background::Background;
use super::top::{
    Top, TopLocal, WidgetNode, call_lua_value, coerce_text, lua_has_func, lua_value_kind,
    parse_node, publish_system_tables, publish_theme_tables, rich_text,
};
use crate::app::app::{PlotInfo, Plots};
use crate::app::{Plant, TopEvent, WidgetEvent};
use crate::theme;
use iced::window;
use iced::{Element, Length, Point, Task as Command};
use iced_exwlshell::actions::IcedNewPopupSettings;
use iced_exwlshell::reexport::{
    Anchor, PixelSize, PopupAnchor, PopupConstraintAdjustment, PopupGravity,
};
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use iced_wayland_subscriber::OutputId;
use std::collections::HashMap;

/// QML-`Menu`-style popup for widget slots, backed by a native
/// `xdg_popup` (`Plant::NewPopUp`): an exactly-sized surface anchored
/// to the clicked slot, with the compositor flipping/sliding it into
/// view on overflow. No fullscreen overlay, no cursor math.
///
/// Opened by clicking a slot whose widget defines `popup()` in
/// `widget.lua`; the box renders that text (icons included). Clicking
/// the same slot toggles it; the compositor dismisses on outside
/// clicks (menu-grab semantics).
#[derive(Debug, Clone)]
pub struct Popup {
    /// Own window id (item clicks address their popup).
    pub win_id: window::Id,
    /// Bar that spawned this popup (toggle handling).
    pub bar_id: window::Id,
    /// Slot index on that bar.
    pub slot: usize,
    /// Widget name whose `popup()` renders the body.
    pub widget: String,
    /// Last rendered `popup()` body text.
    pub body: String,
    /// Last rendered clickable rows.
    pub items: Vec<PopupItem>,
    /// Last rendered composed body (`ui` field).
    pub tree: Option<WidgetNode>,
    /// Text size, from the widget def at open time.
    pub size: f32,
    /// Box size in px.
    pub w: u32,
    pub h: u32,
}

/// One clickable popup row: label plus the `on_action()` key sent on click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PopupItem {
    pub label: String,
    pub action: String,
}

/// Parsed `popup()` return: body text, optional explicit size, and
/// clickable rows. A plain string return is just the text.
#[derive(Debug, Clone, Default)]
pub(crate) struct PopupContent {
    pub text: String,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub items: Vec<PopupItem>,
    /// Composed body from the `ui` field (a `ui.*` tree).
    pub tree: Option<WidgetNode>,
}

impl Popup {
    /// Menu box width in px (height derives from the body line count).
    pub(crate) const WIDTH: f32 = 280.0;

    /// Box size for popup content: explicit dimensions win (clamped
    /// into the output), otherwise width 280 and height estimated from
    /// the text lines, item rows, and the composed tree's real shape.
    pub(crate) fn content_size(sw: f32, sh: f32, content: &PopupContent) -> (u32, u32) {
        let text_h = content.text.lines().count() as f32 * 24.0;
        let items_h = content.items.len() as f32 * 24.0;
        // Estimate the tree so nested menus/lists aren't clipped by the
        // old row-count heuristic (which only saw the `items` shorthand).
        let tree_h = content
            .tree
            .as_ref()
            .map(|tree| super::top::estimate_height(tree, 13.0))
            .unwrap_or(0.0);
        let gaps = if content.tree.is_some() && !content.items.is_empty() {
            8.0
        } else {
            0.0
        };
        let auto_h = (44.0 + text_h + items_h + tree_h + gaps).clamp(80.0, 720.0);
        let w = content
            .width
            .map(|w| w.clamp(80.0, sw))
            .unwrap_or_else(|| sw.min(Self::WIDTH))
            .round() as u32;
        let h = content
            .height
            .map(|h| h.clamp(64.0, sh))
            .unwrap_or(auto_h)
            .round() as u32;
        (w.max(1), h.max(1))
    }

    /// Parse a `popup()` return into content: a plain string is just
    /// body text, a table carries `text`/`width`/`height`/`items` plus
    /// an optional `ui` tree (any `ui.*` constructor result) rendered
    /// above the items.
    pub(crate) fn parse_popup_content(value: mlua::Value) -> Result<PopupContent, String> {
        use mlua::Value;
        match value {
            Value::Table(t) => {
                let text = match t.get::<Value>("text").map_err(|e| e.to_string())? {
                    Value::Nil => String::new(),
                    other => coerce_text(other, "popup().text")?,
                };
                let number = |key: &str| -> Result<Option<f32>, String> {
                    match t.get::<Value>(key).map_err(|e| e.to_string())? {
                        Value::Nil => Ok(None),
                        Value::Integer(i) => Ok(Some(i as f32)),
                        Value::Number(n) => Ok(Some(n as f32)),
                        other => Err(format!(
                            "popup().{key} must be a number, got {}",
                            lua_value_kind(&other)
                        )),
                    }
                };
                let width = number("width")?;
                let height = number("height")?;
                let tree = match t.get::<Value>("ui").map_err(|e| e.to_string())? {
                    Value::Nil => None,
                    other => Some(parse_node(&other)?),
                };
                let mut items = Vec::new();
                match t.get::<Value>("items").map_err(|e| e.to_string())? {
                    Value::Nil => {}
                    Value::Table(list) => {
                        for entry in list.sequence_values::<mlua::Table>() {
                            let entry = entry.map_err(|e| e.to_string())?;
                            let label =
                                match entry.get::<Value>("label").map_err(|e| e.to_string())? {
                                    Value::Nil => String::new(),
                                    other => coerce_text(other, "popup() item label")?,
                                };
                            let action =
                                match entry.get::<Value>("action").map_err(|e| e.to_string())? {
                                    Value::Nil => String::new(),
                                    other => coerce_text(other, "popup() item action")?,
                                };
                            items.push(PopupItem { label, action });
                        }
                    }
                    other => {
                        return Err(format!(
                            "popup().items must be an array, got {}",
                            lua_value_kind(&other)
                        ));
                    }
                }
                Ok(PopupContent {
                    text,
                    width,
                    height,
                    items,
                    tree,
                })
            }
            other => Ok(PopupContent {
                text: coerce_text(other, "popup()")?,
                ..Default::default()
            }),
        }
    }

    /// Slot cell rect in bar-local px: `content` is the bar rect minus
    /// floating margins, `n` the slot count, `gap` the inter-cell
    /// spacing. Used as the popup anchor rect.
    pub(crate) fn slot_rect(
        content: (f32, f32, f32, f32),
        n: usize,
        gap: f32,
        horizontal: bool,
        pos: usize,
    ) -> (i32, i32, u32, u32) {
        let (ox, oy, ow, oh) = content;
        if n == 0 {
            return (ox as i32, oy as i32, 1, 1);
        }
        let gap = gap.max(0.0);
        let pos = pos.min(n - 1) as f32;
        if horizontal {
            let cell = ((ow - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
            (
                (ox + pos * (cell + gap)).round() as i32,
                oy.round() as i32,
                cell.round().max(1.0) as u32,
                oh.round().max(1.0) as u32,
            )
        } else {
            let cell = ((oh - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
            (
                ox.round() as i32,
                (oy + pos * (cell + gap)).round() as i32,
                ow.round().max(1.0) as u32,
                cell.round().max(1.0) as u32,
            )
        }
    }

    /// Slot index under a point, or `None` in gaps/outside. `content`
    /// is the bar rect minus floating margins, `n` the slot count,
    /// `gap` the inter-cell spacing. The point is bar-local (cursor
    /// positions already are).
    pub(crate) fn slot_at_point(
        content: (f32, f32, f32, f32),
        n: usize,
        gap: f32,
        horizontal: bool,
        p: Point,
    ) -> Option<usize> {
        let (ox, oy, ow, oh) = content;
        if n == 0 {
            return None;
        }
        let gap = gap.max(0.0);
        let (u, len) = if horizontal {
            (p.x - ox, ow)
        } else {
            (p.y - oy, oh)
        };
        if u < 0.0 || u >= len {
            return None;
        }
        let cell = (len - gap * (n as f32 - 1.0)) / n as f32;
        if cell <= 0.0 {
            return None;
        }
        let idx = (u / (cell + gap)).floor() as usize;
        if idx >= n {
            return None;
        }
        let within = u - idx as f32 * (cell + gap);
        if within <= cell { Some(idx) } else { None }
    }

    /// GPU busy % from DRM sysfs (AMD + Intel expose `gpu_busy_percent`
    /// per card; NVIDIA needs NVML and reads as unavailable). Busiest
    /// card wins on multi-GPU setups.
    pub(crate) fn gpu_usage_percent() -> Option<f32> {
        Self::gpu_usage_in(std::path::Path::new("/sys/class/drm"))
    }

    pub(crate) fn gpu_usage_in(drm: &std::path::Path) -> Option<f32> {
        std::fs::read_dir(drm)
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    name.strip_prefix("card").is_some_and(|rest| {
                        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                    })
                })
            })
            .filter_map(|entry| {
                std::fs::read_to_string(entry.path().join("device/gpu_busy_percent")).ok()
            })
            .filter_map(|text| text.trim().parse::<f32>().ok())
            .fold(None, |busiest: Option<f32>, usage| {
                Some(busiest.map_or(usage, |peak| peak.max(usage)))
            })
    }

    /// Anchor point (bar-local px) for the popup: centered on the
    /// cursor along the bar axis and pinned to the bar's outer edge on
    /// the cross axis, so the menu grows off the edge under the pointer.
    /// Without a cursor, falls back to the slot center (edge-pinned the
    /// same way). The compositor slides the box into the output.
    pub(crate) fn popup_anchor(
        bar: (f32, f32),
        horizontal: bool,
        first_side: bool,
        slot: (i32, i32, u32, u32),
        cursor: Option<(f32, f32)>,
    ) -> (i32, i32) {
        let (bw, bh) = bar;
        let (rx, ry, rw, rh) = slot;
        let (cx, cy) = cursor.unwrap_or((rx as f32 + rw as f32 / 2.0, ry as f32 + rh as f32 / 2.0));
        if horizontal {
            let x = cx.clamp(0.0, (bw - 1.0).max(0.0)).round() as i32;
            let y = if first_side { bh as i32 - 1 } else { 0 };
            (x, y.max(0))
        } else {
            let y = cy.clamp(0.0, (bh - 1.0).max(0.0)).round() as i32;
            let x = if first_side { bw as i32 - 1 } else { 0 };
            (x.max(0), y)
        }
    }

    /// Open a popup for a slot's `popup()`-capable widget, anchored
    /// to the slot rect with the menu growing off the bar edge
    /// (flipped/slid into view by the compositor on overflow).
    /// `prefer` selects which widget when the caller hit-tested one
    /// (per-widget mouse areas); otherwise the first capable widget
    /// wins. Returns `None` when nothing in the slot can pop up.
    pub(crate) fn open_for(
        plots: &mut Plots,
        bar_id: window::Id,
        output: OutputId,
        pos: usize,
        cursor: Option<(f32, f32)>,
        prefer: Option<&str>,
    ) -> Option<Command<Plant>> {
        let top = plots.tops.get(&bar_id)?.clone();
        let names: Vec<String> = top.local.widgets_at(pos).to_vec();
        let mut capable: Vec<String> = names
            .iter()
            .filter(|w| lua_has_func(&plots.widget_lua, w, "popup"))
            .cloned()
            .collect();
        // Slow path: the file may have changed since the cached state
        // was built (popup paths don't tick like `render()` does) —
        // sync candidate defs and re-check, so a newly added `popup()`
        // opens on the first click after saving.
        if capable.is_empty() || prefer.is_some_and(|p| !capable.iter().any(|w| w == p)) {
            for w in &names {
                if let Some(def) = plots.widgets.iter().find(|d| d.name == *w).cloned() {
                    Top::sync_script_state(plots, &def);
                    // Errors surface below when the chosen widget fails
                    // to load; other candidates just stay unusable.
                    let _ = Top::ensure_widget_lua(plots, &def);
                }
            }
            capable = names
                .iter()
                .filter(|w| lua_has_func(&plots.widget_lua, w, "popup"))
                .cloned()
                .collect();
        }
        let name = prefer
            .filter(|p| capable.iter().any(|w| w == p))
            .map(str::to_string)
            .or_else(|| capable.into_iter().next())?;
        // Fresh state for the chosen widget: edits apply on open, not
        // on the widget's next (maybe 60s) render tick.
        if let Some(def) = plots.widgets.iter().find(|d| d.name == name).cloned() {
            Top::sync_script_state(plots, &def);
            if let Err(e) = Top::ensure_widget_lua(plots, &def) {
                Top::note_widget_error(plots, &name, e);
                return None;
            }
        }
        let body = match plots.widget_lua.get(&name) {
            Some(lua) => match call_lua_value(lua, "popup").and_then(Self::parse_popup_content) {
                Ok(content) => content,
                Err(e) => {
                    Top::note_widget_error(plots, &name, e);
                    return None;
                }
            },
            None => return None,
        };
        // An empty menu (no text, tree, or items) opens nothing.
        if body.text.trim().is_empty() && body.items.is_empty() && body.tree.is_none() {
            return None;
        }
        let (_, _, sw, sh) = Background::available_rect(output, &plots.output_infos)?;
        let horizontal = top.is_horizontal();
        let (bw, bh) = top.local.px_size(sw, sh, horizontal);
        let (pl, pt, pr, pb) = if top.local.floating {
            let m = top.local.margins;
            (
                m.left.max(0) as f32,
                m.top.max(0) as f32,
                m.right.max(0) as f32,
                m.bottom.max(0) as f32,
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        let gap = top.local.slot_spacing.clamp(0.0, TopLocal::MAX_SLOT_GAP);
        let n = top.local.slots.clamp(1, TopLocal::MAX_SLOTS) as usize;
        let (rx, ry, rw, rh) = Self::slot_rect(
            (pl, pt, bw as f32 - pl - pr, bh as f32 - pt - pb),
            n,
            gap,
            horizontal,
            pos,
        );
        let (w, h) = Self::content_size(sw, sh, &body);
        let size = PixelSize::try_px(w, h)?;
        let first_side = top.anchor() == Anchor::Top || top.anchor() == Anchor::Left;
        let anchor_at = Self::popup_anchor(
            (bw as f32, bh as f32),
            horizontal,
            first_side,
            (rx, ry, rw, rh),
            cursor,
        );
        let anchor_size = PixelSize::try_px(1, 1)?;
        let (anchor, gravity) = if horizontal {
            if first_side {
                (PopupAnchor::Bottom, PopupGravity::Bottom)
            } else {
                (PopupAnchor::Top, PopupGravity::Top)
            }
        } else if first_side {
            (PopupAnchor::Right, PopupGravity::Right)
        } else {
            (PopupAnchor::Left, PopupGravity::Left)
        };
        let settings = IcedNewPopupSettings::new(bar_id, size, anchor_at, anchor_size)
            .anchor(anchor)
            .gravity(gravity)
            .constraint_adjustment(
                PopupConstraintAdjustment::FlipX
                    | PopupConstraintAdjustment::FlipY
                    | PopupConstraintAdjustment::SlideX
                    | PopupConstraintAdjustment::SlideY,
            );
        let win_id = window::Id::unique();
        if let Some(tree) = &body.tree
            && let Err(error) =
                super::top::sync_declared_lists(plots, &format!("popup:{win_id:?}"), None, tree)
        {
            Top::note_widget_error(plots, &name, error);
            return None;
        }
        let size_text = plots
            .widgets
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.size.max(1.0))
            .unwrap_or(13.0);
        plots.popups.insert(
            win_id,
            Popup {
                win_id,
                bar_id,
                slot: pos,
                widget: name,
                body: body.text,
                items: body.items,
                tree: body.tree,
                size: size_text,
                w,
                h,
            },
        );
        plots.ids.insert(win_id, PlotInfo::Popup(output));
        Some(Command::done(Plant::NewPopUp {
            settings,
            id: win_id,
        }))
    }

    pub fn view(&self, plots: &Plots) -> Element<'static, Plant> {
        use crate::app::TopEvent;
        use iced::widget::{button, column, container};
        let win_id = self.win_id;
        let mut content = column![].spacing(4);
        if !self.body.trim().is_empty() {
            content = content.push(rich_text(self.body.clone(), self.size, 4.0));
        }
        if let Some(tree) = &self.tree {
            // Trees failing to build were rejected at parse/refresh.
            // Buttons route like item rows: the click carries this
            // popup, so `on_action` receives the key.
            let msg = move |action: String| {
                Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupSelect(win_id, action)))
            };
            if let Ok(node) = super::top::build_with_lists(
                tree,
                &format!("popup:{win_id:?}"),
                self.size,
                Some(&msg),
                &plots.anim_runtime,
                &plots.widget_lists,
            ) {
                content = content.push(node);
            }
        }
        for item in &self.items {
            let action = item.action.clone();
            content = content.push(
                button(rich_text(item.label.clone(), self.size, 4.0))
                    .width(Length::Fill)
                    .padding(6)
                    .on_press(Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupSelect(
                        win_id, action,
                    ))))
                    .style(theme::menu_button(theme::RADIUS)),
            );
        }
        container(content)
            .width(Length::Fixed(self.w as f32))
            .height(Length::Fixed(self.h as f32))
            .padding(12)
            .clip(true)
            .style(theme::menu_box)
            .into()
    }

    /// Dismiss a popup: drop tracking + cursor state and close its window
    /// (idempotent, like every other layer close).
    pub(crate) fn handle_dismiss(plots: &mut Plots, id: window::Id) -> Command<Plant> {
        let prefix = format!("popup:{id:?}/");
        let keys: Vec<_> = plots
            .widget_lists
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(mut list) = plots.widget_lists.remove(&key) {
                list.clear_all(&mut plots.anim_runtime);
            }
        }
        plots.last_cursor.remove(&id);
        plots.popups.remove(&id);
        plots.ids.remove(&id);
        iced_runtime::task::effect(Action::Window(WindowAction::Close(id)))
    }

    /// Re-render open popup bodies with live data (called when widget
    /// outputs move, so menus tick too). Each body re-syncs its script
    /// first, so `popup()` edits apply without waiting out the
    /// widget's render interval. Bodies that break keep their last good
    /// content; popups whose widget left `widgets.toml` are dismissed.
    /// Returns close commands for dismissals (possibly none).
    pub(crate) fn refresh_bodies(plots: &mut Plots) -> Command<Plant> {
        let open: Vec<(window::Id, String)> = plots
            .popups
            .iter()
            .map(|(id, popup)| (*id, popup.widget.clone()))
            .collect();
        if open.is_empty() {
            return Command::none();
        }
        plots.sysinfo.refresh_cpu_usage();
        plots.sysinfo.refresh_memory();
        let gpu = Self::gpu_usage_percent();
        let mut cmds = Vec::new();
        for (pid, name) in open {
            // Gone from the config: dismiss. Anything else keeps its
            // last good body on error (a typo mid-save must not close
            // the menu you're editing).
            let Some(def) = plots.widgets.iter().find(|d| d.name == name).cloned() else {
                cmds.push(Self::handle_dismiss(plots, pid));
                continue;
            };
            Top::sync_script_state(plots, &def);
            let content = match Top::ensure_widget_lua(plots, &def) {
                Err(e) => {
                    Top::note_widget_error(plots, &name, e);
                    continue;
                }
                Ok(()) => match plots.widget_lua.get(&name) {
                    Some(lua) => {
                        match publish_system_tables(lua, &plots.sysinfo, gpu)
                            .map_err(|e| e.to_string())
                            .and_then(|()| {
                                publish_theme_tables(lua, &plots.config.theme)
                                    .map_err(|e| e.to_string())
                            })
                            .and_then(|()| {
                                super::notification::publish_notification_list(lua, plots)
                                    .map_err(|e| e.to_string())
                            })
                            .and_then(|()| call_lua_value(lua, "popup"))
                            .and_then(Self::parse_popup_content)
                        {
                            Ok(content) => Some(content),
                            Err(e) => {
                                Top::note_widget_error(plots, &name, e);
                                continue;
                            }
                        }
                    }
                    None => continue,
                },
            };
            match content {
                Some(content) => {
                    // Recompute the box: explicit width/height may have
                    // changed (e.g. 300 -> 200), and the window keeps its
                    // creation size unless told otherwise.
                    let output = plots.ids.get(&pid).and_then(|info| match info {
                        PlotInfo::Popup(o) => plots.output_infos.get(o).map(|_| *o),
                        _ => None,
                    });
                    let (nw, nh) = match output {
                        Some(o) => match Background::available_rect(o, &plots.output_infos) {
                            Some((_, _, sw, sh)) => Self::content_size(sw, sh, &content),
                            None => Self::content_size(1920.0, 1080.0, &content),
                        },
                        None => Self::content_size(1920.0, 1080.0, &content),
                    };
                    let old = plots.popups.get(&pid).and_then(|popup| popup.tree.clone());
                    if let Some(tree) = &content.tree
                        && let Err(error) = super::top::sync_declared_lists(
                            plots,
                            &format!("popup:{pid:?}"),
                            old.as_ref(),
                            tree,
                        )
                    {
                        Top::note_widget_error(plots, &name, error);
                        continue;
                    }
                    if let Some(popup) = plots.popups.get_mut(&pid)
                        && (popup.body != content.text
                            || popup.items != content.items
                            || popup.tree != content.tree
                            || popup.w != nw
                            || popup.h != nh)
                    {
                        popup.body = content.text;
                        popup.items = content.items;
                        popup.tree = content.tree;
                        popup.w = nw;
                        popup.h = nh;
                        cmds.push(iced_runtime::task::effect(Action::Window(
                            WindowAction::Resize(pid, iced::Size::new(nw as f32, nh as f32)),
                        )));
                    }
                }
                None => cmds.push(Self::handle_dismiss(plots, pid)),
            }
        }
        if cmds.is_empty() {
            Command::none()
        } else {
            Command::batch(cmds)
        }
    }

    /// Click a popup item: run the widget's `on_action()` with the item
    /// key, then re-render the menu and the cell (a toggle flips both).
    /// Unknown popups are ignored.
    pub(crate) fn handle_select(
        plots: &mut Plots,
        id: window::Id,
        action: String,
    ) -> Command<Plant> {
        let Some(widget) = plots.popups.get(&id).map(|p| p.widget.clone()) else {
            return Command::none();
        };
        // Fresh handler for the click: an edited `on_action()` applies
        // immediately (in-memory toggle state resets on reload, same
        // as editing any widget script).
        if let Some(def) = plots.widgets.iter().find(|d| d.name == widget).cloned() {
            Top::sync_script_state(plots, &def);
            if let Err(e) = Top::ensure_widget_lua(plots, &def) {
                Top::note_widget_error(plots, &widget, e);
                return Command::none();
            }
        }
        let outcome = match plots.widget_lua.get(&widget) {
            Some(lua) => {
                let acted = publish_system_tables(lua, &plots.sysinfo, Self::gpu_usage_percent())
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        publish_theme_tables(lua, &plots.config.theme).map_err(|e| e.to_string())
                    })
                    .and_then(|()| {
                        super::notification::publish_notification_list(lua, plots)
                            .map_err(|e| e.to_string())
                    })
                    .and_then(|()| super::top::call_lua_named_action(lua, &action));
                Some(acted)
            }
            None => None,
        };
        match outcome {
            Some(Ok(value)) => {
                plots.widget_last_error.remove(&widget);
                let notif = super::notification::command_from_action(&value, plots);
                let refresh = Self::refresh_bodies(plots);
                // Tree-aware cell refresh (same as the cell-action
                // path): a tree render() must update widget_trees, not
                // just the text cache.
                if let Some(def) = plots.widgets.iter().find(|d| d.name == widget).cloned() {
                    let gpu = Self::gpu_usage_percent();
                    if Top::refresh_widget(plots, &def, gpu) {
                        return Command::batch(vec![
                            refresh,
                            Command::done(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Changed))),
                            notif,
                        ]);
                    }
                }
                return Command::batch(vec![refresh, notif]);
            }
            Some(Err(e)) => Top::note_widget_error(plots, &widget, e),
            None => {}
        }
        Command::none()
    }

    /// Remove all popups for `output_id` (mirrors `Top::remove_for_output`).
    /// Returns the window ids that were removed.
    pub(crate) fn remove_for_output(
        popups: &mut HashMap<window::Id, Popup>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        output_id: OutputId,
    ) -> Vec<window::Id> {
        let to_remove: Vec<window::Id> = ids
            .iter()
            .filter_map(|(wid, info)| match info {
                PlotInfo::Popup(o) if *o == output_id => Some(*wid),
                _ => None,
            })
            .collect();
        for wid in &to_remove {
            popups.remove(wid);
            ids.remove(wid);
        }
        to_remove
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::layers::top::NodeLength;

    #[test]
    fn slot_at_point_maps_cells_and_gaps() {
        // 3 cells across 300px with 4px gaps: cells are ~97.33px.
        let at = |x: f32| {
            Popup::slot_at_point((0.0, 0.0, 300.0, 50.0), 3, 4.0, true, Point::new(x, 25.0))
        };
        assert_eq!(at(0.0), Some(0));
        assert_eq!(at(50.0), Some(0));
        // Gap between cell 0 and 1 starts at ~97.33.
        assert_eq!(at(97.0), Some(0));
        assert_eq!(at(98.0), None);
        assert_eq!(at(102.0), Some(1));
        assert_eq!(at(299.0), Some(2));
        assert_eq!(at(300.0), None);
        assert_eq!(at(-1.0), None);
        // Vertical bars read y instead.
        let at_y = |y: f32| {
            Popup::slot_at_point((0.0, 0.0, 50.0, 300.0), 3, 4.0, false, Point::new(25.0, y))
        };
        assert_eq!(at_y(10.0), Some(0));
        assert_eq!(at_y(150.0), Some(1));
        assert_eq!(at_y(290.0), Some(2));
        // Zero slots never match.
        assert_eq!(
            Popup::slot_at_point(
                (0.0, 0.0, 300.0, 50.0),
                0,
                4.0,
                true,
                Point::new(10.0, 10.0)
            ),
            None
        );
    }

    #[test]
    fn slot_rect_matches_slot_at_point() {
        // Anchor rects tile the content exactly like hit-testing.
        let content = (0.0, 0.0, 300.0, 50.0);
        for pos in 0..3 {
            let (x, y, w, h) = Popup::slot_rect(content, 3, 4.0, true, pos);
            assert_eq!((y, h), (0, 50));
            let center = Point::new(x as f32 + w as f32 / 2.0, 25.0);
            assert_eq!(
                Popup::slot_at_point(content, 3, 4.0, true, center),
                Some(pos)
            );
        }
        // First cell starts at the content origin with the even share.
        assert_eq!(
            Popup::slot_rect((8.0, 6.0, 300.0, 50.0), 3, 4.0, true, 0),
            (8, 6, 97, 50)
        );
    }

    #[test]
    fn popup_size_defaults_and_honors_overrides() {
        let plain = PopupContent {
            text: "one".to_string(),
            ..Default::default()
        };
        assert_eq!(Popup::content_size(1920.0, 1080.0, &plain), (280, 80));
        let tall = PopupContent {
            text: "one\ntwo\nthree".to_string(),
            ..Default::default()
        };
        assert_eq!(Popup::content_size(1920.0, 1080.0, &tall), (280, 116));
        // Items add rows; narrow outputs shrink the box.
        let listed = PopupContent {
            text: "head".to_string(),
            items: vec![
                PopupItem {
                    label: "a".to_string(),
                    action: "a".to_string(),
                },
                PopupItem {
                    label: "b".to_string(),
                    action: "b".to_string(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(Popup::content_size(1920.0, 1080.0, &listed), (280, 116));
        assert_eq!(Popup::content_size(200.0, 1080.0, &plain), (200, 80));
        // Explicit dimensions win, clamped into the output.
        let custom = PopupContent {
            width: Some(500.0),
            height: Some(10.0),
            ..Default::default()
        };
        assert_eq!(Popup::content_size(1920.0, 1080.0, &custom), (500, 64));
        assert_eq!(Popup::content_size(200.0, 100.0, &custom), (200, 64));
        // A composed tree sizes to its real shape, not a fixed row count.
        let single = PopupContent {
            tree: Some(WidgetNode::Text {
                content: "hi".to_string(),
                size: None,
                width: None,
                height: None,
                color: None,
            }),
            ..Default::default()
        };
        assert_eq!(Popup::content_size(1920.0, 1080.0, &single), (280, 80));
        // A four-row menu tree reserves enough height for every button.
        let menu = PopupContent {
            tree: Some(WidgetNode::ListView {
                id: "session".to_string(),
                items: (0..4)
                    .map(|i| {
                        (
                            format!("a{i}"),
                            WidgetNode::Button {
                                label: format!("Row {i}"),
                                action: format!("a{i}"),
                                width: Some(NodeLength::Fill),
                                height: None,
                                padding: None,
                                color: None,
                            },
                        )
                    })
                    .collect(),
                horizontal: false,
                pitch: 32.0,
                spacing: 4.0,
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                transitions: Box::new((
                    crate::app::layers::listview::Transition::slide_fade(16.0),
                    crate::app::layers::listview::Transition::slide_fade_out(-16.0),
                    crate::app::layers::listview::Transition {
                        from: crate::app::layers::anim::ItemMotion::settled(),
                        to: crate::app::layers::anim::ItemMotion::settled(),
                        duration: None,
                    },
                )),
            }),
            ..Default::default()
        };
        let (_, menu_h) = Popup::content_size(1920.0, 1080.0, &menu);
        assert!(menu_h > 150, "menu needs room for all rows, got {menu_h}");
    }

    #[test]
    fn popup_anchor_centers_on_cursor_at_bar_edge() {
        // Top bar: x follows the cursor, y pins to the bottom edge.
        assert_eq!(
            Popup::popup_anchor(
                (1920.0, 50.0),
                true,
                true,
                (0, 0, 100, 50),
                Some((100.2, 20.0))
            ),
            (100, 49)
        );
        // Bottom bar: pins to the top edge instead.
        assert_eq!(
            Popup::popup_anchor(
                (1920.0, 50.0),
                true,
                false,
                (0, 0, 100, 50),
                Some((100.0, 20.0))
            ),
            (100, 0)
        );
        // Left bar: y follows the cursor, x pins to the right edge.
        assert_eq!(
            Popup::popup_anchor(
                (50.0, 1080.0),
                false,
                true,
                (0, 0, 50, 100),
                Some((20.0, 100.0))
            ),
            (49, 100)
        );
        // Cursor clamps into the bar; missing cursor falls back to the
        // slot center, edge-pinned the same way.
        assert_eq!(
            Popup::popup_anchor(
                (1920.0, 50.0),
                true,
                true,
                (0, 0, 100, 50),
                Some((-5.0, 20.0))
            ),
            (0, 49)
        );
        assert_eq!(
            Popup::popup_anchor((1920.0, 50.0), true, true, (0, 0, 100, 50), None),
            (50, 49)
        );
    }

    #[test]
    fn gpu_reader_picks_busiest_card() {
        let dir = std::env::temp_dir().join(format!("riced-popup-gpu-{}", std::process::id()));
        let card0 = dir.join("card0/device");
        let card1 = dir.join("card1/device");
        let _ = std::fs::create_dir_all(&card0);
        let _ = std::fs::create_dir_all(&card1);
        // Connectors look like cards but are not all-digit suffixes.
        let _ = std::fs::create_dir_all(dir.join("card0-DP-1"));
        std::fs::write(card0.join("gpu_busy_percent"), "12\n").unwrap();
        std::fs::write(card1.join("gpu_busy_percent"), "78\n").unwrap();
        assert_eq!(Popup::gpu_usage_in(&dir), Some(78.0));
        assert_eq!(Popup::gpu_usage_in(&dir.join("missing")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
