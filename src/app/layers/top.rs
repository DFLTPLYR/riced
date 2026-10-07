use super::Popup;
use super::background::Background;
use super::listview::{Axis, Transition};
use crate::app::app::{PlotInfo, Plots};
use crate::app::{Plant, TopEvent, WidgetEvent};
use crate::composables::panel_window::top_window;
use crate::config::WidgetDef;
use crate::theme;
use iced::mouse::Button;
use iced::widget::{Space, button, column, container, progress_bar, row, text};
use iced::window;
use iced::{Element, Fill, Point, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use iced_wayland_subscriber::{OutputId, OutputInfo};
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Content inset (px) painted as a transparent gap inside the bar surface.
/// Implemented as widget padding (see `PanelWindow::padding`), so the bar
/// stays edge-pinned and keeps its exclusive zone while the backdrop shrinks.
#[derive(Debug, Clone, Copy, Default)]
pub struct Margins {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

/// Per-corner rounding (px) painted on the bar backdrop.
#[derive(Debug, Clone, Copy, Default)]
pub struct CornerRadius {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_left: f32,
    pub bottom_right: f32,
}

#[derive(Debug, Clone)]
pub struct Top {
    anchor: Anchor,
    /// Index into `Config::bar` (`[[bar]]`) for config-spawned bars,
    /// `usize::MAX` for unpersisted temporaries (set on persist).
    pub bar_index: usize,
    pub local: TopLocal,
}

/// Position of a slot's child along the bar's long axis (`Start` =
/// first, `Center` = middle, `End` = last). The cross axis stays
/// centered so content never hugs the bar's thin edge.
/// Per-slot so e.g. a clock can sit right while the next cell centers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SlotAlign {
    Start,
    #[default]
    Center,
    End,
}

impl SlotAlign {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Center => "center",
            Self::End => "end",
        }
    }

    /// Parse a persisted alignment (`[[bar]] aligns`); unknown or empty
    /// strings fall back to `Center`, never an error.
    pub(crate) fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "start" | "left" | "top" => Self::Start,
            "end" | "right" | "bottom" => Self::End,
            _ => Self::Center,
        }
    }

    fn iced(self) -> iced::Alignment {
        match self {
            Self::Start => iced::Alignment::Start,
            Self::Center => iced::Alignment::Center,
            Self::End => iced::Alignment::End,
        }
    }

    /// `(x, y)` cell alignment for a bar orientation: the preset rides
    /// the long axis while the cross axis stays centered (vertical bars
    /// center horizontally, horizontal bars vertically).
    fn for_bar(self, horizontal: bool) -> (iced::Alignment, iced::Alignment) {
        let align = self.iced();
        if horizontal {
            (align, iced::Alignment::Center)
        } else {
            (iced::Alignment::Center, align)
        }
    }
}

/// Local data for one bar: length %, thickness px, grid slots, floating + margins, rounding.
/// Plain runtime state on `Top`, deliberately outside `Config` so bars stay
/// independent of the global config file and its hot-reload.
#[derive(Debug, Clone)]
pub struct TopLocal {
    pub length_pct: f32,
    pub thickness_px: f32,
    /// Grid cells along the long axis: columns when horizontal
    /// (top/bottom anchor), rows when vertical (left/right anchor).
    pub slots: u32,
    /// Child alignment per slot position (`len == slots`). Resized by
    /// [`TopLocal::ensure_aligns`], persisted as names.
    pub aligns: Vec<SlotAlign>,
    /// Widget names per slot position (`len == slots`), resolved against
    /// `widgets.toml`. Each slot renders its entries together along the
    /// bar axis. Resized by [`TopLocal::ensure_widgets`], persisted as
    /// name lists.
    pub widgets: Vec<Vec<String>>,
    /// Inset inside every slot cell, around the widget content (px).
    pub slot_padding: f32,
    /// Gap between slot cells and between icon/text segments inside one
    /// widget (px).
    pub slot_spacing: f32,
    /// Backdrop opacity, always one of 0.0/0.25/0.5/0.75/1.0.
    pub opacity: f32,
    pub floating: bool,
    /// Inset of the bar backdrop inside its surface (view-live padding).
    pub margins: Margins,
    pub radius: CornerRadius,
}

impl TopLocal {
    /// Hard cap on grid slots (config asks 1..infinite; unbounded widget
    /// counts would freeze the frame).
    pub(crate) const MAX_SLOTS: u32 = 32;

    /// Hard cap for slot padding/spacing (px).
    pub(crate) const MAX_SLOT_GAP: f32 = 64.0;

    /// Estimated button pitch (px) for list displaced glides
    /// (text 13 + padding 12 + border ≈ 27–30; iced can't measure
    /// items in view code, see `ListView`).
    pub(crate) const ROW_PITCH: f32 = 32.0;

    /// Opacity steps: 0/25/50/75/100%. Snaps any value to the nearest step
    /// so config, slider drags, and preset buttons all agree.
    pub(crate) fn snap_opacity(v: f32) -> f32 {
        (v.clamp(0.0, 1.0) * 4.0).round() / 4.0
    }

    /// All five steps, for the settings preset row.
    pub(crate) const OPACITY_STEPS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
}

impl Default for TopLocal {
    fn default() -> Self {
        Self {
            length_pct: 100.0,
            // ~50px, the old fixed thickness.
            thickness_px: 50.0,
            slots: 1,
            aligns: vec![SlotAlign::Center],
            widgets: vec![Vec::new()],
            slot_padding: 0.0,
            slot_spacing: 4.0,
            opacity: 1.0,
            floating: false,
            margins: Margins::default(),
            radius: CornerRadius::default(),
        }
    }
}

impl TopLocal {
    pub(crate) fn px_size(&self, sw: f32, sh: f32, horizontal: bool) -> (u32, u32) {
        let length = self.length_pct.clamp(1.0, 100.0);
        let max_t = Self::max_thickness(sw, sh, horizontal).max(1.0);
        let thick = (self.thickness_px.clamp(1.0, max_t).round() as u32).max(1);
        if horizontal {
            let w = ((sw * length / 100.0).round() as u32).max(1);
            (w, thick)
        } else {
            let h = ((sh * length / 100.0).round() as u32).max(1);
            (thick, h)
        }
    }

    /// Max thickness in px for the given output size + orientation.
    pub(crate) fn max_thickness(sw: f32, sh: f32, horizontal: bool) -> f32 {
        if horizontal { sh } else { sw }
    }

    /// Keep `aligns` aligned with the slot count (truncate extras, pad
    /// with `Center`). Called after every `slots` change and config
    /// load so views can index by position without clamping.
    pub(crate) fn ensure_aligns(&mut self) {
        let n = self.slots.clamp(1, Self::MAX_SLOTS) as usize;
        self.aligns.resize(n, SlotAlign::Center);
    }

    /// Child alignment by position (`Center` past the end).
    pub(crate) fn align_at(&self, pos: usize) -> SlotAlign {
        self.aligns.get(pos).copied().unwrap_or(SlotAlign::Center)
    }

    /// Sentinel for empty slots in `widgets` (`[[bar]]` name).
    pub(crate) const NO_WIDGET: &'static str = "none";

    /// `true` for the empty sentinel (`none`, blank) — renders an
    /// empty cell.
    pub(crate) fn is_empty_widget(name: &str) -> bool {
        let name = name.trim();
        name.is_empty() || name.eq_ignore_ascii_case(Self::NO_WIDGET)
    }

    /// Keep `widgets` aligned with the slot count (truncate extras, pad
    /// empty). Called with [`TopLocal::ensure_aligns`] after every
    /// `slots` change and config load.
    pub(crate) fn ensure_widgets(&mut self) {
        let n = self.slots.clamp(1, Self::MAX_SLOTS) as usize;
        self.widgets.resize(n, Vec::new());
    }

    /// Widget names by position (empty past the end).
    pub(crate) fn widgets_at(&self, pos: usize) -> &[String] {
        self.widgets.get(pos).map(Vec::as_slice).unwrap_or(&[])
    }
}

impl From<&crate::config::TopConfig> for TopLocal {
    fn from(c: &crate::config::TopConfig) -> Self {
        let slots = c.slots.clamp(1, Self::MAX_SLOTS);
        let mut aligns: Vec<SlotAlign> = c.aligns.iter().map(|a| SlotAlign::from_str(a)).collect();
        aligns.resize(slots as usize, SlotAlign::Center);
        let mut widgets: Vec<Vec<String>> = c
            .widgets
            .iter()
            .map(|slot| match slot {
                crate::config::SlotWidgets::One(name) if TopLocal::is_empty_widget(name) => {
                    Vec::new()
                }
                crate::config::SlotWidgets::One(name) => vec![name.clone()],
                crate::config::SlotWidgets::Many(names) => names
                    .iter()
                    .filter(|name| !TopLocal::is_empty_widget(name))
                    .cloned()
                    .collect(),
            })
            .collect();
        widgets.resize(slots as usize, Vec::new());
        Self {
            length_pct: c.length,
            thickness_px: c.thickness,
            slots,
            aligns,
            widgets,
            slot_padding: c.slot_padding.clamp(0.0, Self::MAX_SLOT_GAP),
            slot_spacing: c.slot_spacing.clamp(0.0, Self::MAX_SLOT_GAP),
            opacity: Self::snap_opacity(c.opacity),
            floating: c.floating,
            margins: Margins {
                top: c.margin_top.max(0),
                right: c.margin_right.max(0),
                bottom: c.margin_bottom.max(0),
                left: c.margin_left.max(0),
            },
            radius: CornerRadius {
                top_left: c.radius_top_left.max(0.0),
                top_right: c.radius_top_right.max(0.0),
                bottom_left: c.radius_bottom_left.max(0.0),
                bottom_right: c.radius_bottom_right.max(0.0),
            },
        }
    }
}

/// Lucide icon bytes by name (`{icon:cpu}` in widget text). Lookup is
/// case-insensitive and ignores `-_ ` separators, so `memory-stick`,
/// `memory_stick` and `MemoryStick` all resolve. Covers the full
/// Lucide set via the build-generated lookup (same fresh-icons
/// guarantee as lucide-iced itself — no hand list, no checked-in
/// table) plus short aliases (`mem`, `vol`, `up`...). Unknown names
/// render as literal text so typos stay visible.
fn icon_bytes(name: &str) -> Option<&'static [u8]> {
    let key: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    generated_icon_bytes(&key)
}

// Build-generated full-set lookup (see build.rs): one match arm per
// Lucide icon, derived from lucide-iced's own build output.
include!(concat!(env!("OUT_DIR"), "/lucide_lookup.rs"));

/// One piece of widget text: plain text or an `{icon:name}` reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment<'a> {
    Text(&'a str),
    Icon(&'a str),
}

/// Split widget text on `{icon:name}` placeholders. Unterminated
/// `{icon:` tails stay literal text.
fn icon_segments(output: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut rest = output;
    while let Some(start) = rest.find("{icon:") {
        if start > 0 {
            segments.push(Segment::Text(&rest[..start]));
        }
        let after = &rest[start + "{icon:".len()..];
        match after.find('}') {
            Some(end) => {
                segments.push(Segment::Icon(after[..end].trim()));
                rest = &after[end + 1..];
            }
            None => {
                segments.push(Segment::Text(&rest[start..]));
                rest = "";
            }
        }
    }
    if !rest.is_empty() {
        segments.push(Segment::Text(rest));
    }
    segments
}

/// Widget text with `{icon:name}` placeholders resolved to theme-aware
/// Lucide icons (they inherit the surrounding text color, so they
/// follow the theme like text does). Plain text without placeholders
/// renders as a single text element, exactly like before.
pub(crate) fn rich_text(output: String, size: f32, spacing: f32) -> Element<'static, Plant> {
    let size = size.max(1.0);
    if !output.contains("{icon:") {
        return text(output).size(size).into();
    }
    let mut row = row![]
        .spacing(spacing.max(0.0))
        .align_y(iced::Alignment::Center)
        .width(iced::Length::Shrink)
        .height(iced::Length::Shrink);
    for segment in icon_segments(&output) {
        match segment {
            Segment::Text(text_) if !text_.is_empty() => {
                row = row.push(text(text_.to_owned()).size(size));
            }
            Segment::Icon(name) => match icon_bytes(name) {
                Some(bytes) => row = row.push(lucide_iced::themed_icon(bytes, size)),
                None => row = row.push(text(format!("{{icon:{name}}}")).size(size)),
            },
            _ => {}
        }
    }
    row.into()
}

/// Empty cell for slots with no widget (unknown names land here too).
fn empty_slot() -> Element<'static, Plant> {
    Space::new().into()
}

/// Render one slot's widgets side by side along the bar axis
/// (`none`/unknown names are skipped; no entries = empty cell).
/// Every widget gets its own mouse area (renderer hit-testing), so
/// clicks carry the exact widget — no slot-granularity guessing.
/// Top-level rows/columns get enter/exit transitions on direct
/// button children (see [`super::anim`]); anything else builds plain.
#[allow(clippy::too_many_arguments)]
fn render_slot_widgets(
    bar_id: window::Id,
    pos: usize,
    names: &[String],
    defs: &[WidgetDef],
    outputs: &HashMap<(window::Id, String), String>,
    trees: &HashMap<(window::Id, String), WidgetNode>,
    gap: f32,
    horizontal: bool,
    anim_runtime: &aura_anim::core::runtime::MotionRuntime,
    lists: &std::collections::HashMap<
        String,
        super::listview::ListView<(String, String), WidgetNode>,
    >,
) -> Element<'static, Plant> {
    use iced::widget::mouse_area;
    let mut items = Vec::new();
    for name in names {
        if TopLocal::is_empty_widget(name) {
            continue;
        }
        // Per-widget click target: press records (slot, widget), the
        // matching release dispatches. Inner `ui.button`s capture
        // their own presses, so they never double-fire the widget.
        let area = |el: Element<'static, Plant>, name: &str| -> Element<'static, Plant> {
            mouse_area(el)
                .on_press(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Pressed(
                    bar_id,
                    pos,
                    name.to_string(),
                ))))
                .on_release(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Released(
                    bar_id,
                    pos,
                    name.to_string(),
                ))))
                .into()
        };
        if let Some(node) = trees.get(&(bar_id, name.clone())) {
            let size = defs
                .iter()
                .find(|d| d.name == *name)
                .map(|d| d.size)
                .unwrap_or(13.0);
            // Trees failing to build render nothing (logged at ingest).
            // Buttons arm a per-widget MouseArea: the click carries the
            // owning widget, so on_action routes back to its own state.
            // List owners scope to this bar, so two bars never animate
            // each other.
            let widget = name.clone();
            let scope = Top::list_scope(bar_id, name);
            let msg = move |action: String| {
                Plant::TopPlot(TopEvent::Widget(WidgetEvent::CellAction(
                    bar_id,
                    widget.clone(),
                    action,
                )))
            };
            let built = match node {
                WidgetNode::Row {
                    children,
                    spacing,
                    width,
                    height,
                }
                | WidgetNode::Column {
                    children,
                    spacing,
                    width,
                    height,
                } => {
                    let is_row = matches!(node, WidgetNode::Row { .. });
                    build_anim_list(
                        &scope,
                        children,
                        *spacing,
                        width.clone(),
                        height.clone(),
                        is_row,
                        size,
                        &msg,
                        anim_runtime,
                        lists,
                    )
                }
                _ => build_with_lists(node, &scope, size, Some(&msg), anim_runtime, lists),
            };
            if let Ok(item) = built {
                items.push(area(item, name));
            }
        } else if let Some((output, size)) = lua_cell_text(bar_id, name, defs, outputs) {
            items.push(area(rich_text(output, size, gap), name));
        }
    }
    if items.is_empty() {
        return empty_slot();
    }
    if horizontal {
        let mut row = row![]
            .spacing(gap.max(0.0))
            .align_y(iced::Alignment::Center)
            .width(iced::Length::Shrink)
            .height(iced::Length::Shrink);
        for item in items {
            row = row.push(item);
        }
        row.into()
    } else {
        let mut column = column![]
            .spacing(gap.max(0.0))
            .align_x(iced::Alignment::Center)
            .width(iced::Length::Shrink)
            .height(iced::Length::Shrink);
        for item in items {
            column = column.push(item);
        }
        column.into()
    }
}

/// Build a top-level row/column with QML-style transitions: direct
/// button children (keyed by action) shift by their live motion
/// without disturbing layout; retained ghosts render inert at their
/// old indices. Live build failures skip the widget (mirrors plain
/// `build_node` strictness); ghost build failures skip just that ghost.
#[allow(clippy::too_many_arguments)]
fn build_anim_list(
    widget: &str,
    children: &[WidgetNode],
    spacing: f32,
    width: NodeLength,
    height: NodeLength,
    is_row: bool,
    size: f32,
    button_msg: &dyn Fn(String) -> Plant,
    anim_runtime: &aura_anim::core::runtime::MotionRuntime,
    lists: &std::collections::HashMap<
        String,
        super::listview::ListView<(String, String), WidgetNode>,
    >,
) -> Result<Element<'static, Plant>, String> {
    // This widget's own list (absent on first paint): settled motion.
    let at_rest = crate::app::layers::anim::ItemMotion::settled();
    let shift_of = |action: &str| -> (f32, f32) {
        match lists.get(widget) {
            Some(list) => {
                let m = list.motion_of(anim_runtime, &(widget.to_string(), action.to_string()));
                (m.x, m.y)
            }
            None => (at_rest.x, at_rest.y),
        }
    };
    let mut items = Vec::new();
    for child in children {
        let el = build_with_lists(child, widget, size, Some(button_msg), anim_runtime, lists)?;
        match child {
            WidgetNode::Button { action, .. } => {
                let (x, y) = shift_of(action);
                let opacity = lists
                    .get(widget)
                    .map(|list| {
                        list.motion_of(anim_runtime, &(widget.to_string(), action.clone()))
                            .opacity
                    })
                    .unwrap_or(1.0);
                let el = build_node_opacity(child, size, Some(button_msg), opacity)?;
                items.push(super::motion::shifted(el, x, y, true));
            }
            _ => items.push(el),
        }
    }
    // Exits paint above the live layout and never reserve a slot.
    let mut overlays = Vec::new();
    for ghost in lists
        .get(widget)
        .map(|list| list.ghosts_for(|(w, _)| w == widget))
        .unwrap_or_default()
    {
        let opacity = lists
            .get(widget)
            .map(|list| list.motion_of(anim_runtime, &ghost.key).opacity)
            .unwrap_or(1.0);
        let Ok(el) = build_node_opacity(&ghost.content, size, None, opacity) else {
            continue;
        };
        let (x, y) = match lists.get(widget) {
            Some(list) => {
                let m = list.motion_of(anim_runtime, &ghost.key);
                (m.x, m.y)
            }
            None => (0.0, 0.0),
        };
        let anchor = ghost.index as f32 * TopLocal::ROW_PITCH;
        overlays.push(super::motion::ghost(
            el,
            x + if is_row { anchor } else { 0.0 },
            y + if is_row { 0.0 } else { anchor },
        ));
    }
    if !overlays.is_empty() {
        let base = if items.is_empty() {
            Space::new().height(TopLocal::ROW_PITCH).into()
        } else {
            items.remove(0)
        };
        let mut layer = iced::widget::stack![base];
        for overlay in overlays {
            layer = layer.push(overlay);
        }
        items.insert(0, layer.into());
    }
    if is_row {
        let mut row = row![]
            .spacing(spacing.max(0.0))
            .align_y(iced::Alignment::Center)
            .width(width.iced())
            .height(height.iced());
        for item in items {
            row = row.push(item);
        }
        Ok(row.into())
    } else {
        let mut column = column![]
            .spacing(spacing.max(0.0))
            .align_x(iced::Alignment::Center)
            .width(width.iced())
            .height(height.iced());
        for item in items {
            column = column.push(item);
        }
        Ok(column.into())
    }
}

/// Direct button children `(action key, node)` of a top-level row or
/// column, in order. Anything else has no stable key and never
/// transitions.
fn button_children(node: &WidgetNode) -> Vec<(String, WidgetNode)> {
    match node {
        WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => children
            .iter()
            .filter_map(|child| match child {
                WidgetNode::Button { action, .. } => Some((action.clone(), child.clone())),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

type WidgetLists = HashMap<String, super::listview::ListView<(String, String), WidgetNode>>;

fn declared_lists<'a>(
    node: &'a WidgetNode,
    out: &mut HashMap<String, &'a WidgetNode>,
) -> Result<(), String> {
    match node {
        WidgetNode::ListView { id, items, .. } => {
            if out.insert(id.clone(), node).is_some() {
                return Err(format!("duplicate listview id {id:?}"));
            }
            for (_, child) in items {
                declared_lists(child, out)?;
            }
        }
        WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => {
            for child in children {
                declared_lists(child, out)?;
            }
        }
        WidgetNode::Container { child, .. } | WidgetNode::Scrollable { child, .. } => {
            declared_lists(child, out)?
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn sync_declared_lists(
    plots: &mut Plots,
    widget: &str,
    old: Option<&WidgetNode>,
    new: &WidgetNode,
) -> Result<(), String> {
    let mut before = HashMap::new();
    let mut after = HashMap::new();
    if let Some(old) = old {
        declared_lists(old, &mut before)?;
    }
    declared_lists(new, &mut after)?;
    for id in before.keys().filter(|id| !after.contains_key(*id)) {
        if let Some(mut list) = plots.widget_lists.remove(&format!("{widget}/{id}")) {
            list.clear_all(&mut plots.anim_runtime);
        }
    }
    for (id, node) in after {
        let WidgetNode::ListView {
            items,
            horizontal,
            pitch,
            transitions,
            ..
        } = node
        else {
            continue;
        };
        let owner = format!("{widget}/{id}");
        let list = plots
            .widget_lists
            .entry(owner.clone())
            .or_insert_with(|| super::listview::ListView::new(*pitch));
        list.set_pitch(*pitch);
        list.set_transitions(
            transitions.0.clone(),
            transitions.1.clone(),
            transitions.2.clone(),
            true,
        );
        let Some(WidgetNode::ListView {
            items: old_items, ..
        }) = before.get(&id).copied()
        else {
            continue;
        };
        let old_keys: Vec<_> = old_items
            .iter()
            .map(|(key, _)| (owner.clone(), key.clone()))
            .collect();
        let keys: Vec<_> = items
            .iter()
            .map(|(key, _)| (owner.clone(), key.clone()))
            .collect();
        let removed: Vec<_> = old_items
            .iter()
            .enumerate()
            .filter(|(_, (key, _))| !items.iter().any(|(k, _)| k == key))
            .map(|(index, (key, child))| (index, (owner.clone(), key.clone()), child.clone()))
            .collect();
        list.update(
            &mut plots.anim_runtime,
            plots.config.animation.speed.duration(),
            if *horizontal {
                Axis::Horizontal
            } else {
                Axis::Vertical
            },
            &old_keys,
            &keys,
            &removed,
        );
    }
    Ok(())
}

pub(crate) fn build_with_lists(
    node: &WidgetNode,
    widget: &str,
    size: f32,
    message: Option<&dyn Fn(String) -> Plant>,
    runtime: &aura_anim::core::runtime::MotionRuntime,
    lists: &WidgetLists,
) -> Result<Element<'static, Plant>, String> {
    match node {
        WidgetNode::ListView {
            id,
            items,
            horizontal,
            spacing,
            width,
            height,
            ..
        } => {
            let owner = format!("{widget}/{id}");
            let keys: Vec<_> = items
                .iter()
                .map(|(key, _)| (owner.clone(), key.clone()))
                .collect();
            let content = |key: &(String, String)| {
                items
                    .iter()
                    .find(|(k, _)| k == &key.1)
                    .map(|(_, c)| c.clone())
            };
            let render = |_: &(String, String),
                          child: WidgetNode,
                          motion: crate::app::layers::anim::ItemMotion,
                          live: bool| {
                if motion.opacity < 1.0 || !live {
                    build_node_opacity(
                        &child,
                        size,
                        if live { message } else { None },
                        motion.opacity,
                    )
                } else {
                    build_with_lists(&child, widget, size, message, runtime, lists)
                }
                .unwrap_or_else(|error| text(error).into())
            };
            let elements = if let Some(list) = lists.get(&owner) {
                list.items(runtime, &keys, &content, render)
            } else {
                items
                    .iter()
                    .map(|(_, c)| build_with_lists(c, widget, size, message, runtime, lists))
                    .collect::<Result<Vec<_>, _>>()?
            };
            // from_vecs deliberately does not infer sizing from children.
            // Match iced's push/extend semantics before installing the keyed
            // delegates, otherwise a shrink list of fill buttons gets width 0.
            let (list_width, list_height) = elements.iter().fold(
                (width.clone().iced(), height.clone().iced()),
                |(w, h), child| {
                    let hints = child.as_widget().size_hint();
                    (w.enclose(hints.width), h.enclose(hints.height))
                },
            );
            if *horizontal {
                Ok(iced::widget::Row::with_children(elements)
                    .spacing(*spacing)
                    .width(list_width)
                    .height(list_height)
                    .into())
            } else {
                use std::hash::{Hash, Hasher};
                let mut identities: Vec<u64> = keys
                    .iter()
                    .map(|key| {
                        let mut hash = std::collections::hash_map::DefaultHasher::new();
                        key.hash(&mut hash);
                        hash.finish()
                    })
                    .collect();
                if identities.is_empty() && !elements.is_empty() {
                    identities.push(0);
                }
                Ok(iced::widget::keyed::Column::from_vecs(identities, elements)
                    .spacing(*spacing)
                    .width(list_width)
                    .height(list_height)
                    .into())
            }
        }
        WidgetNode::Row {
            children,
            spacing,
            width,
            height,
        } => {
            let elements = children
                .iter()
                .map(|c| build_with_lists(c, widget, size, message, runtime, lists))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(iced::widget::Row::new()
                .spacing(*spacing)
                .width(width.clone().iced())
                .height(height.clone().iced())
                .extend(elements)
                .into())
        }
        WidgetNode::Column {
            children,
            spacing,
            width,
            height,
        } => {
            let elements = children
                .iter()
                .map(|c| build_with_lists(c, widget, size, message, runtime, lists))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(iced::widget::Column::new()
                .spacing(*spacing)
                .width(width.clone().iced())
                .height(height.clone().iced())
                .extend(elements)
                .into())
        }
        WidgetNode::Container {
            child,
            width,
            height,
            padding,
            background,
            radius,
        } => {
            let background = background.map(iced::Background::Color);
            let radius = *radius;
            Ok(container(build_with_lists(
                child, widget, size, message, runtime, lists,
            )?)
            .width(width.clone().iced())
            .height(height.clone().iced())
            .padding(padding.max(0.0))
            .style(move |_| iced::widget::container::Style {
                background,
                border: iced::Border {
                    radius: radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into())
        }
        WidgetNode::Scrollable {
            child,
            width,
            height,
        } => Ok(iced::widget::scrollable(build_with_lists(
            child, widget, size, message, runtime, lists,
        )?)
        .width(width.clone().iced())
        .height(height.clone().iced())
        .into()),
        _ => build_node(node, size, message),
    }
}

/// Parse an optional Lua `transitions()` spec over per-slot defaults
/// (QML `Transition` blocks, declarative):
///
/// ```lua
/// function transitions()
///     return {
///         add = { x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 },
///         remove = { x = { to = -200 }, opacity = { to = 0 }, duration = 250 },
///         displaced = { duration = 250 },
///     }
/// end
/// ```
///
/// Missing `transitions`, a nil return, or missing slots/fields keep
/// the defaults, so partial specs compose. Anything misshaped errors
/// naming the slot (callers log once and keep defaults — never
/// half-applied). Durations are milliseconds, clamped to 0–5000.
/// `displaced` takes only `duration` (distance comes from the layout).
pub(crate) fn parse_transitions(
    lua: &Lua,
    enter: &Transition,
    exit: &Transition,
    displaced: &Transition,
) -> Result<(Transition, Transition, Transition, bool), String> {
    let spec = match lua.named_registry_value::<Table>("riced.widget.app") {
        Ok(app) => match app.get::<Value>("transitions").map_err(|e| e.to_string())? {
            Value::Nil => Value::Nil,
            Value::Function(_) => call_lua_value(lua, "transitions")?,
            _ => return Err("app.transitions must be a function".into()),
        },
        Err(_) => Value::Nil,
    };
    parse_transition_value(spec, enter, exit, displaced)
}

fn parse_transition_value(
    spec: Value,
    enter: &Transition,
    exit: &Transition,
    displaced: &Transition,
) -> Result<(Transition, Transition, Transition, bool), String> {
    fn num(value: &Value) -> Option<f32> {
        match value {
            Value::Integer(i) => Some(*i as f32),
            Value::Number(n) => Some(*n as f32),
            _ => None,
        }
        .filter(|value| value.is_finite())
    }
    fn axis_number(
        slot: &Table,
        slot_name: &str,
        axis: &str,
        field: &str,
        keep: f32,
    ) -> Result<f32, String> {
        let axis_value: Value = slot.get(axis).map_err(|e| e.to_string())?;
        let axis_table = match axis_value {
            Value::Nil => return Ok(keep),
            Value::Table(t) => t,
            other => {
                return Err(format!(
                    "transitions().{slot_name}.{axis} must be a table like {{ from = 0, to = 1 }}, got {}",
                    lua_value_kind(&other)
                ));
            }
        };
        match axis_table.get::<Value>(field).map_err(|e| e.to_string())? {
            Value::Nil => Ok(keep),
            v => num(&v).ok_or_else(|| {
                format!(
                    "transitions().{slot_name}.{axis}.{field} must be a number, got {}",
                    lua_value_kind(&v)
                )
            }),
        }
    }
    fn slot_duration(
        slot: &Table,
        slot_name: &str,
        keep: Option<Duration>,
    ) -> Result<Option<Duration>, String> {
        match slot.get::<Value>("duration").map_err(|e| e.to_string())? {
            Value::Nil => Ok(keep),
            v => num(&v)
                .filter(|n| *n >= 0.0)
                .map(|n| Some(Duration::from_millis(n.clamp(0.0, 5000.0) as u64)))
                .ok_or_else(|| {
                    format!(
                        "transitions().{slot_name}.duration must be a non-negative number of milliseconds, got {}",
                        lua_value_kind(&v)
                    )
                }),
        }
    }
    let spec_table = match spec {
        Value::Nil => return Ok((enter.clone(), exit.clone(), displaced.clone(), false)),
        Value::Table(t) => t,
        other => {
            return Err(format!(
                "transitions() must return a table, got {}",
                lua_value_kind(&other)
            ));
        }
    };
    let mut out_enter = enter.clone();
    let mut out_exit = exit.clone();
    let mut out_displaced = displaced.clone();
    let custom_enter = matches!(
        spec_table.get::<Value>("add").map_err(|e| e.to_string())?,
        Value::Table(_)
    );
    for (slot_name, is_exit, is_displaced) in [
        ("add", false, false),
        ("remove", true, false),
        ("displaced", false, true),
    ] {
        let slot_value: Value = spec_table.get(slot_name).map_err(|e| e.to_string())?;
        let Value::Table(slot) = slot_value else {
            if slot_value == Value::Nil {
                continue;
            }
            return Err(format!(
                "transitions().{slot_name} must be a table, got {}",
                lua_value_kind(&slot_value)
            ));
        };
        if is_displaced {
            out_displaced.duration = slot_duration(&slot, slot_name, displaced.duration)?;
            continue;
        }
        if is_exit {
            out_exit.to.x = axis_number(&slot, slot_name, "x", "to", exit.to.x)?;
            out_exit.to.y = axis_number(&slot, slot_name, "y", "to", exit.to.y)?;
            out_exit.to.opacity = axis_number(&slot, slot_name, "opacity", "to", exit.to.opacity)?;
            out_exit.duration = slot_duration(&slot, slot_name, exit.duration)?;
        } else {
            out_enter.from.x = axis_number(&slot, slot_name, "x", "from", enter.from.x)?;
            out_enter.from.y = axis_number(&slot, slot_name, "y", "from", enter.from.y)?;
            out_enter.from.opacity =
                axis_number(&slot, slot_name, "opacity", "from", enter.from.opacity)?;
            out_enter.to.x = axis_number(&slot, slot_name, "x", "to", enter.to.x)?;
            out_enter.to.y = axis_number(&slot, slot_name, "y", "to", enter.to.y)?;
            out_enter.to.opacity =
                axis_number(&slot, slot_name, "opacity", "to", enter.to.opacity)?;
            out_enter.duration = slot_duration(&slot, slot_name, enter.duration)?;
        }
    }
    Ok((out_enter, out_exit, out_displaced, custom_enter))
}

/// Last script output (text, size) by widget name (`None` = empty cell).
/// Split out so the cache lookup stays testable without rendering.
fn lua_cell_text(
    bar: window::Id,
    name: &str,
    defs: &[WidgetDef],
    outputs: &HashMap<(window::Id, String), String>,
) -> Option<(String, f32)> {
    let def = defs.iter().find(|d| d.name == name)?;
    outputs
        .get(&(bar, name.to_string()))
        .cloned()
        .map(|text| (text, def.size))
}

/// One composable UI node, built in Lua via the `ui` table and
/// interpreted here into iced widgets. Lua never holds real widgets —
/// it composes these descriptions, which is the entire expressive
/// range (nesting is free; new primitives add one constructor plus one
/// match arm below).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WidgetNode {
    ListView {
        id: String,
        items: Vec<(String, WidgetNode)>,
        horizontal: bool,
        pitch: f32,
        spacing: f32,
        width: NodeLength,
        height: NodeLength,
        transitions: Box<(Transition, Transition, Transition)>,
    },
    Container {
        child: Box<WidgetNode>,
        width: NodeLength,
        height: NodeLength,
        padding: f32,
        background: Option<iced::Color>,
        radius: f32,
    },
    Scrollable {
        child: Box<WidgetNode>,
        width: NodeLength,
        height: NodeLength,
    },
    Space {
        width: NodeLength,
        height: NodeLength,
    },
    Image {
        path: String,
        width: NodeLength,
        height: NodeLength,
    },
    Text {
        content: String,
        size: Option<f32>,
        width: Option<NodeLength>,
        height: Option<NodeLength>,
        /// Explicit label color (`:color()`); `None` inherits the theme.
        color: Option<iced::Color>,
    },
    Icon {
        name: String,
        /// Tint (`:color()`); `None` inherits surrounding text color.
        color: Option<iced::Color>,
    },
    Row {
        children: Vec<WidgetNode>,
        spacing: f32,
        width: NodeLength,
        height: NodeLength,
    },
    Column {
        children: Vec<WidgetNode>,
        spacing: f32,
        width: NodeLength,
        height: NodeLength,
    },
    /// Label plus `on_action()` key. In cells each button is a
    /// per-widget MouseArea (`CellAction` carries the owner, so clicks
    /// route to that widget's `on_action` — never the slot-wide
    /// popup/`on_press` fallback). In popups buttons route like `items`
    /// rows (`PopupSelect` carries the popup, same `on_action` key).
    Button {
        label: String,
        action: String,
        width: Option<NodeLength>,
        height: Option<NodeLength>,
        padding: Option<f32>,
        /// Explicit label color (`:color()`); `None` uses the themed
        /// button text (backgrounds always stay themed).
        color: Option<iced::Color>,
    },
    Progress {
        value: f32,
        width: NodeLength,
        height: Option<NodeLength>,
    },
    /// Loading placeholder: animated ring while a slow fetch resolves.
    /// Purely visual (no action) — scripts return it first, then swap
    /// in real content once cached data arrives.
    Spinner,
    /// Horizontal hairline between items (`iced.separator()`), painted
    /// in the theme border color. Always full-width (iced rules fill
    /// their axis); only thickness (`height`, default 1px) chains.
    Separator { height: f32 },
}

/// Box sizing for `ui` nodes: a number is px (`Fixed`), `"fill"` /
/// `"shrink"` are the iced `Length` modes. Unset means `Shrink`
/// everywhere except progress width (`Fixed(120)`), so old scripts
/// render identically.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeLength {
    Fill,
    Shrink,
    Fixed(f32),
}

impl NodeLength {
    pub(crate) fn iced(self) -> iced::Length {
        match self {
            Self::Fill => iced::Length::Fill,
            Self::Shrink => iced::Length::Shrink,
            Self::Fixed(px) => iced::Length::Fixed(px.max(1.0)),
        }
    }
}

/// Optional numeric field from a node table. Unset reads as nil —
/// but so do the chainable setter *methods* (same namespace: `t.size`
/// is the setter function until `:size(v)` overwrites it), so
/// functions read as unset too. Real numbers pass; anything else
/// (strings, tables) errors naming the field. Both constructor args
/// and chained setters share this path.
fn node_property(t: &Table, field: &str) -> mlua::Result<Value> {
    if let Ok(properties) = t.raw_get::<Table>("_properties") {
        return properties.raw_get(field);
    }
    t.get(field)
}

fn opt_number(t: &Table, field: &str, what: &str) -> Result<Option<f32>, String> {
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => Ok(None),
        Value::Integer(i) => Ok(Some(i as f32)),
        Value::Number(n) => Ok(Some(n as f32)),
        other => Err(format!(
            "{what} {field} must be a number, got {}",
            lua_value_kind(&other)
        )),
    }
}

/// Optional color from a node table: `"#rgb"` / `"#rrggbb"` /
/// `"#rrggbbaa"` strings or `{r, g, b[, a]}` 0–1 tables (named or
/// positional keys). Unset (nil — or the setter function sharing the
/// field namespace) means `None`. Anything else errors naming the
/// field, so typos stay visible.
fn opt_color(t: &Table, field: &str, what: &str) -> Result<Option<iced::Color>, String> {
    fn num(v: &Value) -> Option<f32> {
        match v {
            Value::Integer(i) => Some(*i as f32),
            Value::Number(n) => Some(*n as f32),
            _ => None,
        }
    }
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => Ok(None),
        Value::String(s) => {
            let raw = s.to_string_lossy();
            // Expand `#rgb` to `#rrggbb` before the theme parser.
            let expanded = if raw.len() == 4 && raw.starts_with('#') {
                let c: Vec<char> = raw.chars().collect();
                format!("#{}{}{}{}{}{}", c[1], c[1], c[2], c[2], c[3], c[3])
            } else {
                raw.to_string()
            };
            crate::theme::parse_hex(&expanded).ok_or_else(|| {
                format!("{what} {field} must be a hex color like \"#rrggbb\" or an {{r, g, b}} table, got {raw:?}")
            }).map(Some)
        }
        Value::Table(rgb) => {
            let chan = |k: &str, i: i32| -> Option<f32> {
                rgb.get::<Value>(k)
                    .ok()
                    .as_ref()
                    .and_then(num)
                    .or_else(|| rgb.get::<Value>(i).ok().as_ref().and_then(num))
            };
            match (chan("r", 1), chan("g", 2), chan("b", 3)) {
                (Some(r), Some(g), Some(b)) => Ok(Some(iced::Color::from_rgba(
                    r.clamp(0.0, 1.0),
                    g.clamp(0.0, 1.0),
                    b.clamp(0.0, 1.0),
                    chan("a", 4).unwrap_or(1.0).clamp(0.0, 1.0),
                ))),
                _ => Err(format!(
                    "{what} {field} must be a hex color like \"#rrggbb\" or an {{r, g, b}} table"
                )),
            }
        }
        other => Err(format!(
            "{what} {field} must be a hex color like \"#rrggbb\" or an {{r, g, b}} table, got {}",
            lua_value_kind(&other)
        )),
    }
}

/// Optional box size from a node table: numbers are px, `"fill"` /
/// `"shrink"` (any case) are the iced modes, unset (nil — or the
/// setter function sharing the field namespace) means `None`.
/// Anything else errors naming the field.
fn opt_length(t: &Table, field: &str, what: &str) -> Result<Option<NodeLength>, String> {
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => Ok(None),
        Value::Integer(i) => Ok(Some(NodeLength::Fixed(i as f32))),
        Value::Number(n) => Ok(Some(NodeLength::Fixed(n as f32))),
        Value::String(s) => match s.to_string_lossy().to_lowercase().as_str() {
            "fill" => Ok(Some(NodeLength::Fill)),
            "shrink" => Ok(Some(NodeLength::Shrink)),
            other => Err(format!(
                "{what} {field} must be a number, \"fill\", or \"shrink\", got {other:?}"
            )),
        },
        other => Err(format!(
            "{what} {field} must be a number, \"fill\", or \"shrink\", got {}",
            lua_value_kind(&other)
        )),
    }
}

/// Parse a `render()` table return into a node tree. Scalars coerce to
/// text like before; malformed structure is an error (logged
/// once-per-message by the caller, cell renders empty).
pub(crate) fn parse_node(value: &Value) -> Result<WidgetNode, String> {
    match value {
        Value::Table(t) => {
            let kind: String = match t.get::<Value>("type").map_err(|e| e.to_string())? {
                Value::String(s) => s.to_string_lossy(),
                Value::Nil => {
                    return Err("ui node table needs a type field".to_string());
                }
                other => {
                    return Err(format!(
                        "ui node type must be a string, got {}",
                        lua_value_kind(&other)
                    ));
                }
            };
            match kind.as_str() {
                "listview" => {
                    let id = match node_property(t, "id").map_err(|e| e.to_string())? {
                        Value::String(s) => s.to_string_lossy(),
                        _ => return Err("iced.listview needs :id('stable-name')".into()),
                    };
                    let key_field = match node_property(t, "key").map_err(|e| e.to_string())? {
                        Value::String(s) => s.to_string_lossy(),
                        _ => return Err("iced.listview needs :key('field')".into()),
                    };
                    let delegate = match node_property(t, "delegate").map_err(|e| e.to_string())? {
                        Value::Function(f) => f,
                        _ => {
                            return Err(
                                "iced.listview needs :delegate(function(item) ... end)".into()
                            );
                        }
                    };
                    let data: Table = t.get("data").map_err(|e| e.to_string())?;
                    let mut items = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for item in data.sequence_values::<Table>() {
                        let item = item.map_err(|e| e.to_string())?;
                        let key = match item
                            .get::<Value>(key_field.as_str())
                            .map_err(|e| e.to_string())?
                        {
                            Value::String(s) => s.to_string_lossy(),
                            Value::Integer(n) => n.to_string(),
                            _ => {
                                return Err(format!(
                                    "listview {id:?} key must be a string or integer"
                                ));
                            }
                        };
                        if !seen.insert(key.clone()) {
                            return Err(format!("listview {id:?} duplicate key {key:?}"));
                        }
                        let value: Value = delegate.call(item).map_err(|e| e.to_string())?;
                        items.push((key, parse_node(&value)?));
                    }
                    let defaults: super::listview::ListView<String, WidgetNode> =
                        super::listview::ListView::default();
                    let (enter, exit, displaced, _) = parse_transition_value(
                        t.get("transitions").map_err(|e| e.to_string())?,
                        &defaults.enter_spec(),
                        &defaults.exit_spec(),
                        &defaults.displaced_spec(),
                    )?;
                    Ok(WidgetNode::ListView {
                        id,
                        items,
                        horizontal: matches!(node_property(t, "axis").map_err(|e| e.to_string())?, Value::String(s) if s.to_string_lossy() == "horizontal"),
                        pitch: opt_number(t, "pitch", "iced.listview()")?
                            .unwrap_or(32.0)
                            .max(1.0),
                        spacing: opt_number(t, "spacing", "iced.listview()")?
                            .unwrap_or(4.0)
                            .max(0.0),
                        width: opt_length(t, "width", "iced.listview()")?
                            .unwrap_or(NodeLength::Shrink),
                        height: opt_length(t, "height", "iced.listview()")?
                            .unwrap_or(NodeLength::Shrink),
                        transitions: Box::new((enter, exit, displaced)),
                    })
                }
                "container" => Ok(WidgetNode::Container {
                    child: Box::new(parse_node(
                        &t.get::<Value>("child").map_err(|e| e.to_string())?,
                    )?),
                    width: opt_length(t, "width", "iced.container()")?
                        .unwrap_or(NodeLength::Shrink),
                    height: opt_length(t, "height", "iced.container()")?
                        .unwrap_or(NodeLength::Shrink),
                    padding: opt_number(t, "padding", "iced.container()")?.unwrap_or(0.0),
                    radius: opt_number(t, "radius", "iced.container()")?.unwrap_or(0.0),
                    background: opt_color(t, "background", "iced.container()")?,
                }),
                "scrollable" => Ok(WidgetNode::Scrollable {
                    child: Box::new(parse_node(
                        &t.get::<Value>("child").map_err(|e| e.to_string())?,
                    )?),
                    width: opt_length(t, "width", "iced.scrollable()")?.unwrap_or(NodeLength::Fill),
                    height: opt_length(t, "height", "iced.scrollable()")?
                        .unwrap_or(NodeLength::Fill),
                }),
                "space" => Ok(WidgetNode::Space {
                    width: opt_length(t, "width", "iced.space()")?.unwrap_or(NodeLength::Shrink),
                    height: opt_length(t, "height", "iced.space()")?.unwrap_or(NodeLength::Shrink),
                }),
                "image" => Ok(WidgetNode::Image {
                    path: t.get("path").map_err(|e| e.to_string())?,
                    width: opt_length(t, "width", "iced.image()")?.unwrap_or(NodeLength::Shrink),
                    height: opt_length(t, "height", "iced.image()")?.unwrap_or(NodeLength::Shrink),
                }),
                "text" => Ok(WidgetNode::Text {
                    content: coerce_text(
                        t.get::<Value>("text").map_err(|e| e.to_string())?,
                        "ui.text()",
                    )?,
                    // Chained :size(14) overrides the widget default.
                    size: opt_number(t, "size", "ui.text()")?,
                    width: opt_length(t, "width", "ui.text()")?,
                    height: opt_length(t, "height", "ui.text()")?,
                    color: opt_color(t, "color", "ui.text()")?,
                }),
                "icon" => match t.get::<Value>("name").map_err(|e| e.to_string())? {
                    Value::String(s) => Ok(WidgetNode::Icon {
                        name: s.to_string_lossy(),
                        color: opt_color(t, "color", "ui.icon()")?,
                    }),
                    Value::Nil => Err("ui.icon() needs a name".to_string()),
                    other => Err(format!(
                        "ui.icon() name must be a string, got {}",
                        lua_value_kind(&other)
                    )),
                },
                "row" | "column" => {
                    let children = match t.get::<Value>("children").map_err(|e| e.to_string())? {
                        Value::Table(list) => list
                            .sequence_values::<Value>()
                            .map(|child| {
                                child
                                    .map_err(|e| e.to_string())
                                    .and_then(|child| parse_node(&child))
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                        Value::Nil => Vec::new(),
                        other => {
                            return Err(format!(
                                "ui.{}() children must be an array, got {}",
                                kind,
                                lua_value_kind(&other)
                            ));
                        }
                    };
                    let spacing = match node_property(t, "spacing").map_err(|e| e.to_string())? {
                        Value::Nil | Value::Function(_) => 4.0,
                        Value::Integer(i) => i as f32,
                        Value::Number(n) => n as f32,
                        other => {
                            return Err(format!(
                                "ui.{}() spacing must be a number, got {}",
                                kind,
                                lua_value_kind(&other)
                            ));
                        }
                    };
                    if kind == "row" {
                        Ok(WidgetNode::Row {
                            children,
                            spacing,
                            width: opt_length(t, "width", "ui.row()")?
                                .unwrap_or(NodeLength::Shrink),
                            height: opt_length(t, "height", "ui.row()")?
                                .unwrap_or(NodeLength::Shrink),
                        })
                    } else {
                        Ok(WidgetNode::Column {
                            children,
                            spacing,
                            width: opt_length(t, "width", "ui.column()")?
                                .unwrap_or(NodeLength::Shrink),
                            height: opt_length(t, "height", "ui.column()")?
                                .unwrap_or(NodeLength::Shrink),
                        })
                    }
                }
                "button" => {
                    let label = coerce_text(
                        t.get::<Value>("label").map_err(|e| e.to_string())?,
                        "ui.button() label",
                    )?;
                    let action = coerce_text(
                        t.get::<Value>("action").map_err(|e| e.to_string())?,
                        "ui.button() action",
                    )?;
                    Ok(WidgetNode::Button {
                        label,
                        action,
                        width: opt_length(t, "width", "ui.button()")?,
                        height: opt_length(t, "height", "ui.button()")?,
                        padding: opt_number(t, "padding", "ui.button()")?,
                        color: opt_color(t, "color", "ui.button()")?,
                    })
                }
                "progress" => {
                    let value = match t.get::<Value>("value").map_err(|e| e.to_string())? {
                        Value::Nil => 0.0,
                        Value::Integer(i) => i as f32,
                        Value::Number(n) => n as f32,
                        other => {
                            return Err(format!(
                                "ui.progress() value must be a number, got {}",
                                lua_value_kind(&other)
                            ));
                        }
                    };
                    // Width via constructor arg or :width() chain
                    // (number, "fill", or "shrink"). Unset reads as
                    // the setter *function* (same namespace via
                    // __index) — treat functions as unset, not error.
                    let width = match opt_length(t, "width", "ui.progress()")? {
                        Some(l) => l,
                        None => NodeLength::Fixed(120.0),
                    };
                    Ok(WidgetNode::Progress {
                        value,
                        width,
                        height: opt_length(t, "height", "ui.progress()")?,
                    })
                }
                "spinner" => Ok(WidgetNode::Spinner),
                "separator" => Ok(WidgetNode::Separator {
                    height: opt_number(t, "height", "ui.separator()")?
                        .unwrap_or(1.0)
                        .max(1.0),
                }),
                other => Err(format!("unknown ui node type {other:?}")),
            }
        }
        _ => Ok(WidgetNode::Text {
            content: coerce_text(value.clone(), "ui node")?,
            size: None,
            width: None,
            height: None,
            color: None,
        }),
    }
}

/// Rough vertical extent of a node tree in px, for popup window sizing.
/// Layer surfaces need an upfront height and iced can't measure text, so
/// this mirrors the built geometry closely enough to avoid clipping.
pub(crate) fn estimate_height(node: &WidgetNode, size: f32) -> f32 {
    let base = size.max(1.0);
    match node {
        WidgetNode::Text { size: own, .. } => owned_size(*own, base) * 1.4,
        WidgetNode::Icon { .. } | WidgetNode::Spinner => base * 1.5,
        WidgetNode::Separator { height } => height.max(1.0),
        WidgetNode::Button { padding, .. } => base * 1.4 + 2.0 * padding.unwrap_or(6.0).max(0.0),
        WidgetNode::Progress { height, .. } => {
            height.as_ref().map(|h| length_px(h, base)).unwrap_or(12.0)
        }
        WidgetNode::Space { height, .. } => length_px(height, base),
        WidgetNode::Image { height, .. } => length_px(height, base).max(base),
        WidgetNode::Row { children, .. } => children
            .iter()
            .map(|c| estimate_height(c, base))
            .fold(0.0, f32::max),
        WidgetNode::Column {
            children, spacing, ..
        } => {
            let sum: f32 = children.iter().map(|c| estimate_height(c, base)).sum();
            sum + spacing.max(0.0) * children.len().saturating_sub(1) as f32
        }
        WidgetNode::ListView {
            items,
            horizontal,
            spacing,
            ..
        } => {
            let heights: Vec<f32> = items
                .iter()
                .map(|(_, c)| estimate_height(c, base))
                .collect();
            if *horizontal {
                heights.into_iter().fold(0.0, f32::max)
            } else {
                let sum: f32 = heights.iter().sum();
                sum + spacing.max(0.0) * items.len().saturating_sub(1) as f32
            }
        }
        WidgetNode::Container { child, padding, .. } => {
            estimate_height(child, base) + 2.0 * padding.max(0.0)
        }
        WidgetNode::Scrollable { child, .. } => estimate_height(child, base),
    }
}

/// Node text size, or the inherited `base` when unset.
fn owned_size(own: Option<f32>, base: f32) -> f32 {
    own.unwrap_or(base).max(1.0)
}

/// Resolve a [`NodeLength`] to a px estimate (`Fill`/`Shrink` fall back).
fn length_px(length: &NodeLength, fallback: f32) -> f32 {
    match length {
        NodeLength::Fixed(px) => *px,
        NodeLength::Fill | NodeLength::Shrink => fallback,
    }
}

/// Build an iced element from a node tree. Pure Rust over owned data —
/// views call this per redraw while Lua only runs on its interval.
pub(crate) fn build_node(
    node: &WidgetNode,
    size: f32,
    button_msg: Option<&dyn Fn(String) -> Plant>,
) -> Result<Element<'static, Plant>, String> {
    build_node_opacity(node, size, button_msg, 1.0)
}

/// Render supported primitives with alpha applied to explicit colors
/// as well as inherited colors. This is primitive alpha, not group compositing.
pub(crate) fn build_node_opacity(
    node: &WidgetNode,
    size: f32,
    button_msg: Option<&dyn Fn(String) -> Plant>,
    opacity: f32,
) -> Result<Element<'static, Plant>, String> {
    match node {
        WidgetNode::ListView {
            items,
            horizontal,
            spacing,
            width,
            height,
            ..
        } => {
            let children = items.iter().map(|(_, child)| child.clone()).collect();
            let layout = if *horizontal {
                WidgetNode::Row {
                    children,
                    spacing: *spacing,
                    width: width.clone(),
                    height: height.clone(),
                }
            } else {
                WidgetNode::Column {
                    children,
                    spacing: *spacing,
                    width: width.clone(),
                    height: height.clone(),
                }
            };
            build_node_opacity(&layout, size, button_msg, opacity)
        }
        WidgetNode::Container {
            child,
            width,
            height,
            padding,
            background,
            radius,
        } => {
            let background =
                background.map(|color| iced::Background::Color(color.scale_alpha(opacity)));
            let radius = *radius;
            Ok(
                container(build_node_opacity(child, size, button_msg, opacity)?)
                    .width(width.clone().iced())
                    .height(height.clone().iced())
                    .padding(padding.max(0.0))
                    .style(move |_| iced::widget::container::Style {
                        background,
                        border: iced::Border {
                            radius: radius.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into(),
            )
        }
        WidgetNode::Scrollable {
            child,
            width,
            height,
        } => Ok(
            iced::widget::scrollable(build_node_opacity(child, size, button_msg, opacity)?)
                .width(width.clone().iced())
                .height(height.clone().iced())
                .into(),
        ),
        WidgetNode::Space { width, height } => Ok(Space::new()
            .width(width.clone().iced())
            .height(height.clone().iced())
            .into()),
        WidgetNode::Image {
            path,
            width,
            height,
        } => {
            let path = path.strip_prefix("file://").unwrap_or(path);
            match crate::config::decode_handle(std::path::Path::new(path)) {
                Some((_, _, handle)) => Ok(iced::widget::image::Image::new(handle)
                    .width(width.clone().iced())
                    .height(height.clone().iced())
                    .opacity(opacity)
                    .into()),
                None => Ok(Space::new()
                    .width(width.clone().iced())
                    .height(height.clone().iced())
                    .into()),
            }
        }
        WidgetNode::Text {
            content,
            size: own,
            width,
            height,
            color,
        } => {
            let s = own.unwrap_or(size).max(1.0);
            let mut t = text(content.clone()).size(s);
            if let Some(w) = width {
                t = t.width(w.clone().iced());
            }
            if let Some(h) = height {
                t = t.height(h.clone().iced());
            }
            if color.is_some() || opacity < 1.0 {
                t = t.color(color.unwrap_or_else(theme::text).scale_alpha(opacity));
            }
            Ok(t.into())
        }
        WidgetNode::Icon { name, color } => {
            let base: Element<'static, Plant> = match icon_bytes(name) {
                Some(bytes) => lucide_iced::themed_icon(bytes, size.max(1.0)),
                None => text(format!("{{icon:{name}}}")).size(size.max(1.0)).into(),
            };
            match color {
                // Icons inherit text color as their tint: a
                // shrink-wrapped text_color scope tints without
                // disturbing layout.
                _ if color.is_some() || opacity < 1.0 => {
                    let tint = color.unwrap_or_else(theme::text).scale_alpha(opacity);
                    Ok(container(base)
                        .width(iced::Length::Shrink)
                        .height(iced::Length::Shrink)
                        .style(move |_| iced::widget::container::Style {
                            text_color: Some(tint),
                            ..Default::default()
                        })
                        .into())
                }
                _ => Ok(base),
            }
        }
        WidgetNode::Row {
            children,
            spacing,
            width,
            height,
        } => {
            let mut row = row![]
                .spacing(spacing.max(0.0))
                .align_y(iced::Alignment::Center)
                .width(width.clone().iced())
                .height(height.clone().iced());
            for child in children {
                row = row.push(build_node_opacity(child, size, button_msg, opacity)?);
            }
            Ok(row.into())
        }
        WidgetNode::Column {
            children,
            spacing,
            width,
            height,
        } => {
            let mut column = column![]
                .spacing(spacing.max(0.0))
                .align_x(iced::Alignment::Center)
                .width(width.clone().iced())
                .height(height.clone().iced());
            for child in children {
                column = column.push(build_node_opacity(child, size, button_msg, opacity)?);
            }
            Ok(column.into())
        }
        WidgetNode::Button {
            label,
            action,
            width,
            height,
            padding,
            color,
        } => {
            let mut item = button(rich_text(label.clone(), size, 4.0))
                .padding(padding.unwrap_or(6.0).max(0.0));
            // Label tint only: surfaces stay themed in every status.
            if let Some(c) = color {
                item = item.style(theme::menu_button_tinted(theme::RADIUS, *c));
            } else {
                item = item.style(theme::menu_button(theme::RADIUS));
            }
            if opacity < 1.0 {
                let tint = *color;
                item = item.style(move |theme, status| {
                    let mut style = theme::menu_button(theme::RADIUS)(theme, status);
                    style.text_color = tint.unwrap_or(style.text_color).scale_alpha(opacity);
                    style.background = style.background.map(|bg| bg.scale_alpha(opacity));
                    style.border.color = style.border.color.scale_alpha(opacity);
                    style.shadow.color = style.shadow.color.scale_alpha(opacity);
                    style
                });
            }
            if let Some(w) = width {
                // Buttons keep a 20px floor on Fixed widths so chained
                // typos can't collapse the hit area; Fill/Shrink pass.
                let w = match w {
                    NodeLength::Fixed(px) => iced::Length::Fixed(px.max(20.0)),
                    other => other.clone().iced(),
                };
                item = item.width(w);
            }
            if let Some(h) = height {
                item = item.height(h.clone().iced());
            }
            if let Some(make_msg) = button_msg {
                item = item.on_press(make_msg(action.clone()));
            }
            Ok(item.into())
        }
        WidgetNode::Progress {
            value,
            width,
            height,
        } => {
            // iced's progress_bar has length + girth (thickness), no
            // height: :height() maps to girth so Lua stays iced-spelled
            // in intent (vertical size) if not in method name.
            let length = match width {
                NodeLength::Fixed(px) => iced::Length::Fixed(px.max(20.0)),
                other => other.clone().iced(),
            };
            let mut bar = progress_bar(0.0..=1.0, value.clamp(0.0, 1.0)).length(length);
            if let Some(h) = height {
                bar = bar.girth(h.clone().iced());
            }
            bar = bar.style(move |theme| {
                let mut style = iced::widget::progress_bar::primary(theme);
                style.background = style.background.scale_alpha(opacity);
                style.bar = style.bar.scale_alpha(opacity);
                style.border.color = style.border.color.scale_alpha(opacity);
                style
            });
            Ok(bar.into())
        }
        // Animated ring: iced has no spinner widget, so a rotating
        // loader icon approximates one (redrawn every frame while
        // visible — popups repaint on cursor/tick activity).
        WidgetNode::Spinner => Ok(lucide_iced::ThemedIcon::new(
            iced::advanced::svg::Handle::from_memory(lucide_iced::bytes::LOADER_CIRCLE),
            size.max(1.0) * 1.5,
        )
        .opacity(opacity)
        .into()),
        // Hairline: theme border color, full-width by rule design.
        WidgetNode::Separator { height } => Ok(iced::widget::rule::horizontal(height.max(1.0))
            .style(move |_| iced::widget::rule::Style {
                color: theme::border_color().scale_alpha(opacity),
                radius: 0.0.into(),
                fill_mode: iced::widget::rule::FillMode::Full,
                snap: true,
            })
            .into()),
    }
}

/// Lua state for one widget: string/table/math/os/io with native
/// shell (`os.execute`, `io.popen` live — owner-accepted risk, no
/// allowlist). `os.exit`/`os.remove`/`os.rename` stay nil'd, as do
/// `dofile`/`loadfile`/`require`. `print` stays for daemon logs.
///
/// Shared with the notification renderer (same sandbox, separate state).
pub(crate) fn new_widget_lua() -> mlua::Result<Lua> {
    let lua = empty_widget_lua()?;
    inject_ui(&lua)?;
    Ok(lua)
}

fn empty_widget_lua() -> mlua::Result<Lua> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::OS | StdLib::IO,
        LuaOptions::default(),
    )?;
    let globals = lua.globals();
    for key in ["dofile", "loadfile", "require"] {
        globals.set(key, Value::Nil)?;
    }
    let os: Table = globals.get("os")?;
    for key in ["exit", "remove", "rename", "setlocale"] {
        os.set(key, Value::Nil)?;
    }
    Ok(lua)
}

/// The `iced` constructors table (plus legacy `ui` alias), present in
/// every widget state next to the service tables. Each call builds a
/// plain description table — no iced objects cross into Lua;
/// [`parse_node`] interprets them.
///
/// Every node type gets its own metatable with iced-spelled chainable
/// setters, so Lua reads like iced builders: `ui.progress(0.5)`
/// `:width(200):height(12)`, `ui.text("hi"):size(14)`, `ui.row({...})`
/// `:spacing(8)`, `ui.button("go", "run"):width(120):padding(4)`.
/// Each setter writes its field and returns the node. Calling a
/// setter the node type doesn't own (e.g. `:width()` on text) is a
/// Lua error naming the type — typos stay visible. Setter names never
/// collide with parsed fields: setters live on the metatable while
/// real data (`t.width`) reads raw first.
///
/// Shared with the notification renderer (same constructors, same
/// sandbox, separate Lua state).
pub(crate) fn inject_ui(lua: &Lua) -> mlua::Result<()> {
    type Library = Vec<(std::path::PathBuf, String)>;
    static LAST_GOOD: std::sync::LazyLock<std::sync::Mutex<Library>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));
    static LAST_REJECTED: std::sync::LazyLock<std::sync::Mutex<Option<Library>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(None));
    inject_ui_base(lua)?;
    let candidate = crate::config::component_files();
    let mut previous = LAST_GOOD.lock().expect("component cache");
    let mut rejected = LAST_REJECTED.lock().expect("rejected component cache");
    let selected = if candidate == *previous {
        candidate
    } else if rejected.as_ref() == Some(&candidate) {
        previous.clone()
    } else {
        let trial = empty_widget_lua()?;
        inject_ui_base(&trial)?;
        let validation = candidate.iter().try_for_each(|(path, source)| {
            trial
                .load(source)
                .set_name(format!("@{}", path.display()))
                .exec()
        });
        match validation {
            Ok(()) => {
                *rejected = None;
                *previous = candidate.clone();
                candidate
            }
            Err(error) => {
                *rejected = Some(candidate);
                eprintln!("components: keeping last working library: {error}");
                previous.clone()
            }
        }
    };
    drop(previous);
    drop(rejected);
    for (path, source) in selected {
        lua.load(source)
            .set_name(format!("@{}", path.display()))
            .exec()?;
    }
    Ok(())
}

pub(crate) fn inject_ui_base(lua: &Lua) -> mlua::Result<()> {
    /// One setter: store properties separately from methods so repeated
    /// calls never shadow the method itself.
    /// NOTE: the type gate below is near-dead — method lookup via
    /// __index fails first for foreign setters (nil method = eval
    /// error before the closure runs). Kept as defense in depth for
    /// shared metatables (button+progress share `:width`).
    fn setter(lua: &Lua, field: &str, types: &'static [&'static str]) -> mlua::Result<Function> {
        let key = field.to_string();
        lua.create_function(move |_, (node, v): (Table, Value)| {
            let kind: String = node.get("type")?;
            if !types.contains(&kind.as_str()) {
                return Err(mlua::Error::RuntimeError(format!(
                    "ui {kind} has no :{key}() setter"
                )));
            }
            let properties: Table = node.raw_get("_properties")?;
            properties.set(key.clone(), v)?;
            Ok(node)
        })
    }
    /// Metatable for one node type: methods table as `__index`.
    /// NOTE: pre-seeding fields as nil does NOT shadow __index (Lua
    /// treats nil slots as absent), so setter names and field names
    /// share one namespace by necessity. Parse therefore reads via a
    /// helper that skips functions: real numbers pass, unset-or-method
    /// reads as nil (unset). `opt_number` below implements this.
    fn mt_for(lua: &Lua, methods: &[(&str, Function)]) -> mlua::Result<Table> {
        let mt = lua.create_table()?;
        let index = lua.create_table()?;
        for (name, f) in methods {
            index.set(*name, f.clone())?;
        }
        mt.set("__index", index)?;
        Ok(mt)
    }
    fn node(
        lua: &Lua,
        node_type: &str,
        mt: Table,
        build: impl FnOnce(&mlua::Table) -> mlua::Result<()>,
    ) -> mlua::Result<mlua::Table> {
        let t = lua.create_table()?;
        t.set("type", node_type)?;
        build(&t)?;
        let properties = lua.create_table()?;
        for field in [
            "size",
            "width",
            "height",
            "spacing",
            "padding",
            "color",
            "background",
            "radius",
            "id",
            "key",
            "delegate",
            "axis",
            "pitch",
        ] {
            properties.set(field, t.raw_get::<Value>(field)?)?;
            t.raw_set(field, Value::Nil)?;
        }
        t.raw_set("_properties", properties)?;
        t.set_metatable(Some(mt))?;
        Ok(t)
    }
    // One metatable per setter shape (shared across types that allow
    // the same setters). Width/height now span text/row/column/button/
    // progress — every layout type chains iced-style.
    let mt_text = mt_for(
        lua,
        &[
            ("size", setter(lua, "size", &["text"])?),
            (
                "width",
                setter(
                    lua,
                    "width",
                    &["text", "row", "column", "button", "progress"],
                )?,
            ),
            (
                "height",
                setter(
                    lua,
                    "height",
                    &["text", "row", "column", "button", "progress"],
                )?,
            ),
            ("color", setter(lua, "color", &["text", "icon", "button"])?),
        ],
    )?;
    let mt_icon = mt_for(
        lua,
        &[("color", setter(lua, "color", &["text", "icon", "button"])?)],
    )?;
    let mt_rowcol = mt_for(
        lua,
        &[
            ("spacing", setter(lua, "spacing", &["row", "column"])?),
            (
                "width",
                setter(
                    lua,
                    "width",
                    &["text", "row", "column", "button", "progress"],
                )?,
            ),
            (
                "height",
                setter(
                    lua,
                    "height",
                    &["text", "row", "column", "button", "progress"],
                )?,
            ),
        ],
    )?;
    let mt_button = mt_for(
        lua,
        &[
            (
                "width",
                setter(
                    lua,
                    "width",
                    &["text", "row", "column", "button", "progress"],
                )?,
            ),
            (
                "height",
                setter(
                    lua,
                    "height",
                    &["text", "row", "column", "button", "progress"],
                )?,
            ),
            ("padding", setter(lua, "padding", &["button"])?),
            ("color", setter(lua, "color", &["text", "icon", "button"])?),
        ],
    )?;
    let mt_progress = mt_for(
        lua,
        &[
            (
                "width",
                setter(
                    lua,
                    "width",
                    &["text", "row", "column", "button", "progress"],
                )?,
            ),
            (
                "height",
                setter(
                    lua,
                    "height",
                    &["text", "row", "column", "button", "progress"],
                )?,
            ),
        ],
    )?;
    let mt_bare = mt_for(lua, &[])?;
    let mt_separator = mt_for(lua, &[("height", setter(lua, "height", &["separator"])?)])?;
    lua.globals().set("_riced_ui_mt_text", mt_text.clone())?;
    lua.globals()
        .set("_riced_ui_mt_rowcol", mt_rowcol.clone())?;
    lua.globals()
        .set("_riced_ui_mt_button", mt_button.clone())?;
    lua.globals()
        .set("_riced_ui_mt_progress", mt_progress.clone())?;
    lua.globals().set("_riced_ui_mt_bare", mt_bare.clone())?;
    lua.globals()
        .set("_riced_ui_mt_separator", mt_separator.clone())?;
    lua.globals().set("_riced_ui_mt_icon", mt_icon.clone())?;
    // Move clones into the constructor closures (mlua closures are
    // 'static): each captures only its own metatable.
    let (mt_text_c, mt_rowcol_c, mt_button_c, mt_progress_c, mt_icon_c) = (
        mt_text.clone(),
        mt_rowcol.clone(),
        mt_button.clone(),
        mt_progress.clone(),
        mt_icon.clone(),
    );
    let ui = lua.create_table()?;
    let mut list_methods = Vec::new();
    for field in [
        "id", "key", "delegate", "axis", "pitch", "spacing", "width", "height",
    ] {
        list_methods.push((field, setter(lua, field, &["listview"])?));
    }
    for (method, slot) in [
        ("onEntered", "add"),
        ("onExit", "remove"),
        ("onDisplaced", "displaced"),
    ] {
        list_methods.push((
            method,
            lua.create_function(move |_, (node, spec): (Table, Table)| {
                let transitions: Table = node.get("transitions")?;
                transitions.set(slot, spec)?;
                Ok(node)
            })?,
        ));
    }
    let list_mt = mt_for(lua, &list_methods)?;
    ui.set(
        "listview",
        lua.create_function(move |lua, data: Table| {
            node(lua, "listview", list_mt.clone(), |table| {
                table.set("data", data)?;
                table.set("transitions", lua.create_table()?)
            })
        })?,
    )?;
    for kind in ["container", "scrollable", "space", "image"] {
        let mut methods = vec![
            (
                "width",
                setter(lua, "width", &["container", "scrollable", "space", "image"])?,
            ),
            (
                "height",
                setter(
                    lua,
                    "height",
                    &["container", "scrollable", "space", "image"],
                )?,
            ),
        ];
        if kind == "container" {
            methods.extend([
                ("padding", setter(lua, "padding", &["container"])?),
                ("background", setter(lua, "background", &["container"])?),
                ("radius", setter(lua, "radius", &["container"])?),
            ]);
        }
        let mt = mt_for(lua, &methods)?;
        ui.set(
            kind,
            lua.create_function(move |lua, value: Value| {
                node(lua, kind, mt.clone(), |t| match kind {
                    "container" | "scrollable" => t.set("child", value),
                    "image" => t.set("path", value),
                    _ => Ok(()),
                })
            })?,
        )?;
    }
    ui.set(
        "text",
        lua.create_function(move |lua, text: Value| {
            node(lua, "text", mt_text_c.clone(), |t| t.set("text", text))
        })?,
    )?;
    ui.set(
        "icon",
        lua.create_function(move |lua, name: Value| {
            node(lua, "icon", mt_icon_c.clone(), |t| t.set("name", name))
        })?,
    )?;
    ui.set(
        "row",
        lua.create_function(move |lua, (children, spacing): (Table, Value)| {
            node(lua, "row", mt_rowcol_c.clone(), |t| {
                t.set("children", children)?;
                t.set("spacing", spacing)
            })
        })?,
    )?;
    let mt_rowcol_c2 = mt_rowcol.clone();
    ui.set(
        "column",
        lua.create_function(move |lua, (children, spacing): (Table, Value)| {
            node(lua, "column", mt_rowcol_c2.clone(), |t| {
                t.set("children", children)?;
                t.set("spacing", spacing)
            })
        })?,
    )?;
    ui.set(
        "button",
        lua.create_function(move |lua, (label, action): (Value, Value)| {
            node(lua, "button", mt_button_c.clone(), |t| {
                t.set("label", label)?;
                t.set("action", action)
            })
        })?,
    )?;
    ui.set(
        "progress",
        lua.create_function(move |lua, (value, width): (Value, Value)| {
            node(lua, "progress", mt_progress_c.clone(), |t| {
                t.set("value", value)?;
                t.set("width", width)
            })
        })?,
    )?;
    let mt_bare_c2 = mt_bare.clone();
    ui.set(
        "spinner",
        lua.create_function(move |lua, _: Value| {
            node(lua, "spinner", mt_bare_c2.clone(), |_| Ok(()))
        })?,
    )?;
    let mt_separator_c = mt_separator.clone();
    ui.set(
        "separator",
        lua.create_function(move |lua, _: Value| {
            node(lua, "separator", mt_separator_c.clone(), |_| Ok(()))
        })?,
    )?;
    // Named component registry (per state): `iced.define(name, fn)`
    // stores a builder, `iced.use(name, props)` calls it and returns
    // the node table (metatable intact, so chaining still works).
    // Non-table or typeless returns error naming the component.
    lua.globals()
        .set("_riced_components", lua.create_table()?)?;
    ui.set(
        "define",
        lua.create_function(|lua, (name, func): (String, Function)| {
            let namespace: Table = lua.globals().get("iced")?;
            if namespace.raw_get::<Value>(name.as_str())? != Value::Nil {
                return Err(mlua::Error::RuntimeError(format!(
                    "component {name:?} conflicts with an iced constructor"
                )));
            }
            let registry: Table = lua.globals().get("_riced_components")?;
            if registry.get::<Value>(name.clone())? != Value::Nil {
                eprintln!("iced: component {name:?} redefined (last wins)");
            }
            registry.set(name, func)?;
            Ok(())
        })?,
    )?;
    ui.set(
        "use",
        lua.create_function(|lua, (name, props): (String, Value)| {
            let registry: Table = lua.globals().get("_riced_components")?;
            let func: Function = match registry.get::<Value>(name.clone())? {
                Value::Function(f) => f,
                _ => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "unknown component {name:?} — iced.define it first"
                    )));
                }
            };
            let out: Value = func.call(props)?;
            match &out {
                Value::Table(t) => match t.get::<Value>("type")? {
                    Value::String(_) => Ok(out),
                    other => Err(mlua::Error::RuntimeError(format!(
                        "component {name:?} must return a ui node table, got type {}",
                        lua_value_kind(&other)
                    ))),
                },
                _ => Err(mlua::Error::RuntimeError(format!(
                    "component {name:?} must return a ui node table, got {}",
                    lua_value_kind(&out)
                ))),
            }
        })?,
    )?;
    let component_methods = lua.create_table()?;
    component_methods.set(
        "__index",
        lua.create_function(|lua, (_table, name): (Table, String)| {
            let registry: Table = lua.globals().get("_riced_components")?;
            if registry.get::<Value>(name.as_str())? == Value::Nil {
                return Ok(Value::Nil);
            }
            Ok(Value::Function(lua.create_function(
                move |lua, props: Value| {
                    let namespace: Table = lua.globals().get("iced")?;
                    let use_component: Function = namespace.raw_get("use")?;
                    use_component.call::<Value>((name.clone(), props))
                },
            )?))
        })?,
    )?;
    ui.set_metatable(Some(component_methods))?;
    // Canonical namespace: `iced` owns every constructor above (plus
    // `define`/`use`). Bare `ui` stays as the same table so existing
    // scripts keep working untouched.
    lua.globals().set("iced", ui.clone())?;
    lua.globals().set("ui", ui)?;
    // Shared component library (`components/*.lua`, see
    // `config::components_source`): runs in every state so widgets and
    // the notification renderer share `iced.define` components. The
    // registry above is fresh per injection, so re-injection redefines
    // silently. A broken library logs once and leaves bare `iced` —
    // widgets still render, components just stay undefined.
    Ok(())
}

/// Load a module script. Only a returned app table with app:view is accepted.
pub(crate) fn load_widget_script(lua: &Lua, label: &str, source: &str) -> mlua::Result<()> {
    let returned: Value = lua.load(source).set_name(format!("@{label}")).eval()?;
    match returned {
        Value::Table(app) => {
            let _: Function = app.get("view").map_err(|_| {
                mlua::Error::RuntimeError(format!(
                    "{label}: returned app table needs a view method"
                ))
            })?;
            for method in ["view", "popup", "on_action", "on_press", "transitions"] {
                match app.get::<Value>(method)? {
                    Value::Function(_) | Value::Nil => {}
                    _ => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "{label}: app.{method} must be a function"
                        )));
                    }
                }
            }
            lua.set_named_registry_value("riced.widget.app", app)?;
            Ok(())
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "{label}: script must return an app table with a view method"
        ))),
    }
}

pub(crate) fn lua_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Nil => "nil",
        Value::Boolean(_) => "boolean",
        Value::LightUserData(_) => "light userdata",
        Value::Integer(_) => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Table(_) => "table",
        Value::Function(_) => "function",
        Value::Thread(_) => "thread",
        Value::UserData(_) => "userdata",
        Value::Error(_) => "error",
        _ => "other",
    }
}

/// Invoke a registered app method with self bound; return values are
/// decoded immediately and never stored in the Rust IR.
pub(crate) fn call_widget_method(
    lua: &Lua,
    method: &str,
    mut args: mlua::MultiValue,
) -> Result<Value, String> {
    let app: Table = lua
        .named_registry_value("riced.widget.app")
        .map_err(|e| e.to_string())?;
    let func: Function = app.get(method).map_err(|e| e.to_string())?;
    args.push_front(Value::Table(app));
    func.call(args).map_err(|e| e.to_string())
}

pub(crate) fn call_lua_value(lua: &Lua, func: &str) -> Result<Value, String> {
    call_widget_method(lua, func, mlua::MultiValue::new())
}

/// Coerce a Lua return value to cell text (numbers and booleans
/// stringify, `nil` is empty). Anything else is an error naming `what`.
pub(crate) fn coerce_text(value: Value, what: &str) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(s.to_string_lossy()),
        Value::Integer(i) => Ok(i.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Boolean(b) => Ok(b.to_string()),
        Value::Nil => Ok(String::new()),
        other => Err(format!(
            "{what} must return a string, got {}",
            lua_value_kind(&other)
        )),
    }
}

/// Call a widget script function, tolerantly coerced to text.
/// Test-only since the popup-select refresh went tree-aware (prod
/// paths use `call_lua_value` + `ingest_render_value` instead).
#[cfg(test)]
pub(crate) fn call_lua_text(lua: &Lua, func: &str) -> Result<String, String> {
    let value = call_lua_value(lua, func)?;
    coerce_text(value, &format!("{func}()"))
}

/// Does a widget state define a callable global (`popup`, `on_press`)?
/// Missing states and non-function globals read as absent, never an error.
pub(crate) fn lua_has_func(states: &HashMap<String, mlua::Lua>, name: &str, func: &str) -> bool {
    states.get(name).is_some_and(|lua| {
        lua.named_registry_value::<Table>("riced.widget.app")
            .and_then(|app| app.get::<Function>(func))
            .is_ok()
    })
}

/// Run a widget's `on_press()` click action. The return value is ignored;
/// scripts signal through globals that the next `render()` reads.
fn call_lua_action(lua: &Lua) -> Result<(), String> {
    call_lua_value(lua, "on_press").map(|_| ())
}

/// Run a widget's `on_action(key)` (cell buttons and popup items share
/// it). Missing `on_action` is a silent no-op so plain-text widgets
/// coexist with button trees. Returns the raw Lua value: handlers map
/// `{ dismiss = id }` / `{ invoke = { id, key } }` onto notification
/// commands (see `notification::command_from_action`), anything else
/// just refreshes the widget.
pub(crate) fn call_lua_named_action(lua: &Lua, action: &str) -> Result<Value, String> {
    let app: Table = lua
        .named_registry_value("riced.widget.app")
        .map_err(|e| e.to_string())?;
    if matches!(
        app.get::<Value>("on_action").map_err(|e| e.to_string())?,
        Value::Nil
    ) {
        return Ok(Value::Nil);
    }
    let key = lua.create_string(action).map_err(|e| e.to_string())?;
    call_widget_method(
        lua,
        "on_action",
        mlua::MultiValue::from_vec(vec![Value::String(key)]),
    )
}

impl Top {
    /// Hold threshold: press held >= this on release counts as hold.
    const HOLD_THRESHOLD: Duration = Duration::from_millis(500);

    pub fn new() -> Self {
        Self {
            anchor: Anchor::Top,
            bar_index: usize::MAX,
            local: TopLocal::default(),
        }
    }

    pub fn with_anchor(anchor: Anchor) -> Self {
        // Length defaults to full, thickness to ~50px, for any edge.
        Self {
            anchor,
            bar_index: usize::MAX,
            local: TopLocal::default(),
        }
    }

    /// Bar seeded from a `[[bar]]` config entry.
    pub fn with_config(bar_index: usize, anchor: Anchor, local: TopLocal) -> Self {
        Self {
            anchor,
            bar_index,
            local,
        }
    }

    /// Parse a config `anchor` value (`top`/`bottom`/`left`/`right`,
    /// case-insensitive). `None` logs nothing — the caller warns.
    pub fn parse_anchor(s: &str) -> Option<Anchor> {
        match s.trim().to_lowercase().as_str() {
            "top" => Some(Anchor::Top),
            "bottom" => Some(Anchor::Bottom),
            "left" => Some(Anchor::Left),
            "right" => Some(Anchor::Right),
            _ => None,
        }
    }

    pub fn anchor(&self) -> Anchor {
        self.anchor
    }

    pub(crate) fn is_horizontal(&self) -> bool {
        !(self.anchor == Anchor::Left || self.anchor == Anchor::Right)
    }

    fn output_size(output_infos: &HashMap<OutputId, OutputInfo>, output: OutputId) -> (f32, f32) {
        output_infos
            .get(&output)
            .map(|info| {
                let (_, _, sw, sh) = Background::output_geometry(info);
                (sw, sh)
            })
            .unwrap_or((1920.0, 1080.0))
    }

    fn exclusive_px(top: &Top, w: u32, h: u32) -> i32 {
        if top.is_horizontal() {
            h as i32
        } else {
            w as i32
        }
    }

    pub(crate) fn anchor_label(&self) -> &'static str {
        if self.anchor == Anchor::Top {
            "TOP"
        } else if self.anchor == Anchor::Bottom {
            "BOTTOM"
        } else if self.anchor == Anchor::Left {
            "LEFT"
        } else if self.anchor == Anchor::Right {
            "RIGHT"
        } else {
            "BAR"
        }
    }

    /// Open a Top bar for a specific output (GlobalName) at `w`x`h` px
    /// (resolved from the `%` fields against the output size by the caller).
    /// Called on `WayEvent::OutputInsert` which fires at startup for each
    /// active output when `StartMode::AllScreens`.
    pub fn open(&self, output: u32, w: u32, h: u32) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();
        let edge = Self::exclusive_px(self, w, h);

        let settings = NewLayerShellSettings {
            anchor: self.anchor,
            layer: Layer::Top,
            exclusive_zone: Some(edge),
            size: LayerSize::px(w, h),
            output_option: OutputOption::GlobalName(output),
            margin: None,
            namespace: Some(format!("Riced - {} {}", self.anchor_label(), output)),
            // Bars never blur (frost fights the `opacity` fade).
            blur_option: BlurOption::None,
            ..Default::default()
        };

        (id, settings)
    }

    /// Fallback for startup when no OutputId is known yet (uses Active output).
    /// Fixed 50px strip like the old default; replaced once outputs arrive.
    pub fn open_active(&self) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();

        let settings = NewLayerShellSettings {
            anchor: self.anchor,
            layer: Layer::Top,
            exclusive_zone: Some(50),
            size: LayerSize::fill_width(50),
            output_option: OutputOption::Active,
            namespace: Some(format!("Riced - {} Active", self.anchor_label())),
            margin: None,
            // Bars never blur: frosted glass fights the `opacity` alpha fade
            // (blur would frost the desktop behind a faded fill and the
            // transparent margins), so the fill composites cleanly.
            blur_option: BlurOption::None,
            ..Default::default()
        };

        (id, settings)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn view(
        &self,
        id: window::Id,
        widgets: &[WidgetDef],
        outputs: &HashMap<(window::Id, String), String>,
        trees: &HashMap<(window::Id, String), WidgetNode>,
        anim_runtime: &aura_anim::core::runtime::MotionRuntime,
        lists: &std::collections::HashMap<
            String,
            super::listview::ListView<(String, String), WidgetNode>,
        >,
    ) -> Element<'_, Plant> {
        let n = self.local.slots.clamp(1, TopLocal::MAX_SLOTS) as usize;
        let gap = self.local.slot_spacing.clamp(0.0, TopLocal::MAX_SLOT_GAP);
        let pad = self.local.slot_padding.clamp(0.0, TopLocal::MAX_SLOT_GAP);
        let horizontal = self.is_horizontal();
        let cell = |pos: usize| -> Element<'_, Plant> {
            let body: Element<'_, Plant> = render_slot_widgets(
                id,
                pos,
                self.local.widgets_at(pos),
                widgets,
                outputs,
                trees,
                gap,
                horizontal,
                anim_runtime,
                lists,
            );
            let (align_x, align_y) = self.local.align_at(pos).for_bar(horizontal);
            container(body)
                .width(Fill)
                .height(Fill)
                .align_x(align_x)
                .align_y(align_y)
                .padding(pad)
                .into()
        };
        let content: Element<'_, Plant> = if self.is_horizontal() {
            let mut r = row![].width(Fill).height(Fill).spacing(gap);
            for pos in 0..n {
                r = r.push(cell(pos));
            }
            r.into()
        } else {
            let mut c = column![].width(Fill).height(Fill).spacing(gap);
            for pos in 0..n {
                c = c.push(cell(pos));
            }
            c.into()
        };
        let radius = self.local.radius;
        let opacity = TopLocal::snap_opacity(self.local.opacity);
        let m = self.local.margins;
        let padding = if self.local.floating {
            iced::Padding {
                top: m.top.max(0) as f32,
                right: m.right.max(0) as f32,
                bottom: m.bottom.max(0) as f32,
                left: m.left.max(0) as f32,
            }
        } else {
            iced::Padding::ZERO
        };
        top_window(id)
            .padding(padding)
            .content(
                container(content)
                    .width(Fill)
                    .height(Fill)
                    .center_x(Fill)
                    .center_y(Fill)
                    .style(move |theme: &iced::Theme| {
                        let mut s = theme::bar(theme);
                        if let Some(iced::Background::Color(c)) = s.background {
                            s.background = Some(iced::Background::Color(iced::Color {
                                a: c.a * opacity,
                                ..c
                            }));
                        }
                        s.border.radius = iced::border::Radius {
                            top_left: radius.top_left,
                            top_right: radius.top_right,
                            bottom_right: radius.bottom_right,
                            bottom_left: radius.bottom_left,
                        };
                        s
                    }),
            )
            .into()
    }

    // ------------------------------------------------------------------
    // Event handling — press/release arrive via PanelWindow (mouse_area);
    // CursorMoved still arrives via Plant::Graft for cursor bookkeeping.
    // Measures press duration for hold detection: press stores Instant,
    // release compares against HOLD_THRESHOLD and clears the entry.
    // ------------------------------------------------------------------

    fn handle_cursor_moved(plots: &mut Plots, id: window::Id, position: Point) -> Command<Plant> {
        plots.last_cursor.insert(id, position);
        Command::none()
    }

    pub(crate) fn handle_press(
        plots: &mut Plots,
        id: window::Id,
        button: Button,
    ) -> Command<Plant> {
        if !matches!(plots.id_info(id), Some(PlotInfo::Top(_))) {
            return Command::none();
        }
        // Outer presses only arrive from gaps (widget areas capture
        // their own). Record a gap target so the matching release runs
        // the slot fallback; unresolvable slots clear stale targets.
        // Non-left buttons never click: drop any target.
        if button != Button::Left {
            plots.press_targets.remove(&id);
            return Command::none();
        }
        match Self::cursor_slot(plots, id) {
            Some(pos) => {
                plots.press_targets.insert(id, (pos, None, Instant::now()));
            }
            None => {
                plots.press_targets.remove(&id);
            }
        }
        Command::none()
    }

    /// Left press on one widget's own mouse area: record the widget
    /// target. The matching release — and only it — dispatches.
    pub(crate) fn handle_widget_press(
        plots: &mut Plots,
        bar_id: window::Id,
        pos: usize,
        widget: String,
    ) -> Command<Plant> {
        if !matches!(plots.id_info(bar_id), Some(PlotInfo::Top(_))) {
            return Command::none();
        }
        plots
            .press_targets
            .insert(bar_id, (pos, Some(widget), Instant::now()));
        Command::none()
    }

    /// Slot under the last known cursor, if any.
    /// Shared by press recording (releases match against the recorded
    /// target instead of re-resolving).
    fn cursor_slot(plots: &Plots, bar_id: window::Id) -> Option<usize> {
        let top = plots.tops.get(&bar_id)?.clone();
        let output = match plots.ids.get(&bar_id).copied() {
            Some(PlotInfo::Top(o)) => o,
            _ => return None,
        };
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
        plots.last_cursor.get(&bar_id).copied().and_then(|p| {
            Popup::slot_at_point(
                (pl, pt, bw as f32 - pl - pr, bh as f32 - pt - pb),
                n,
                gap,
                horizontal,
                p,
            )
        })
    }

    /// Whether a release completes the recorded press as a click: same
    /// slot and widget (gap releases carry `None`), left button, under
    /// the hold threshold. Pure for testing.
    fn release_matches_press(
        target: Option<(usize, Option<String>, Instant)>,
        pos: usize,
        widget: Option<&str>,
        button: Button,
        now: Instant,
    ) -> bool {
        button == Button::Left
            && matches!(target, Some((p, ref w, t))
                if p == pos && w.as_deref() == widget && now.duration_since(t) < Self::HOLD_THRESHOLD)
    }

    /// Size/float/margin edits apply live to the layer window every tick, so
    /// sliders stay smooth (radius is view-live and needs nothing). Every
    /// edit also stages a coalesced write to the bar's `[[bar]]`
    /// entry, so bars survive restarts.
    pub(crate) fn handle_set_length(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.length_pct = value.clamp(1.0, 100.0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_thickness(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        // Thickness is px, bound to 1..=thin output axis.
        let max = plots
            .ids
            .get(&id)
            .copied()
            .and_then(|info| match info {
                PlotInfo::Top(o) => plots.output_infos.get(&o),
                _ => None,
            })
            .map(|info| {
                let (_, _, sw, sh) = Background::output_geometry(info);
                let horizontal = plots.tops.get(&id).is_none_or(|t| t.is_horizontal());
                TopLocal::max_thickness(sw, sh, horizontal).max(1.0)
            })
            .unwrap_or(1080.0);
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.thickness_px = value.clamp(1.0, max);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_slots(
        plots: &mut Plots,
        id: window::Id,
        value: u32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.slots = value.clamp(1, TopLocal::MAX_SLOTS);
            // Count change resizes the per-slot rows (extras drop, new
            // cells start centered with no widget) — placement survives
            // by position.
            top.local.ensure_aligns();
            top.local.ensure_widgets();
        }
        // Window size is unchanged (cells share the bar) — persist only.
        Self::persist_bar(plots, id)
    }

    /// Set one slot's child alignment (`TopEvent::Bar(BarEvent::SlotAlign)`): single
    /// commit per press (preset buttons, not a drag stream).
    pub(crate) fn handle_set_slot_align(
        plots: &mut Plots,
        id: window::Id,
        pos: usize,
        align: SlotAlign,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.ensure_aligns();
            if pos < top.local.aligns.len() {
                top.local.aligns[pos] = align;
            }
        }
        // Window size is unchanged (alignment only moves the child
        // inside its cell) — persist only.
        Self::persist_bar(plots, id)
    }

    /// Check/uncheck one slot widget (`TopEvent::Bar(BarEvent::SlotWidget)`):
    /// checking appends the name (no duplicates, `none` never stored),
    /// unchecking removes it. Single commit per toggle.
    pub(crate) fn handle_set_slot_widget(
        plots: &mut Plots,
        id: window::Id,
        pos: usize,
        widget: String,
        enabled: bool,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.ensure_widgets();
            if pos < top.local.widgets.len() {
                let slot = &mut top.local.widgets[pos];
                if enabled {
                    if !TopLocal::is_empty_widget(&widget) && !slot.iter().any(|w| w == &widget) {
                        slot.push(widget);
                    }
                } else {
                    slot.retain(|w| w != &widget);
                }
            }
        }
        // Window size is unchanged (the cell keeps its size, only its
        // content swaps) — persist only.
        Self::persist_bar(plots, id)
    }

    pub(crate) fn handle_widget_layout(
        plots: &mut Plots,
        id: window::Id,
        expected: Vec<Vec<String>>,
        widgets: Vec<Vec<String>>,
    ) -> Command<Plant> {
        let Some(top) = plots.tops.get_mut(&id) else {
            return Command::none();
        };
        if !Self::valid_widget_layout(&top.local.widgets, &expected, &widgets, &plots.widgets) {
            return Command::none();
        }
        top.local.widgets = widgets;
        Self::render_bar_widgets(plots, id);
        Self::persist_bar(plots, id)
    }

    fn valid_widget_layout(
        live: &[Vec<String>],
        expected: &[Vec<String>],
        widgets: &[Vec<String>],
        defs: &[WidgetDef],
    ) -> bool {
        if live != expected || widgets.len() != expected.len() || widgets == expected {
            return false;
        }
        if widgets.iter().any(|slot| {
            slot.iter()
                .enumerate()
                .any(|(i, name)| TopLocal::is_empty_widget(name) || slot[..i].contains(name))
        }) {
            return false;
        }
        // Existing missing definitions can be moved/removed; newly added
        // names must still exist after a widgets.toml reload.
        if widgets.iter().flatten().any(|name| {
            !expected.iter().flatten().any(|old| old == name)
                && !defs.iter().any(|def| &def.name == name)
        }) {
            return false;
        }
        true
    }

    /// Set the slot cell padding (`TopEvent::Bar(BarEvent::SlotPadding)`): single
    /// commit per press (slider, not a drag stream — still coalesced).
    pub(crate) fn handle_set_slot_padding(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.slot_padding = value.clamp(0.0, TopLocal::MAX_SLOT_GAP);
        }
        // Window size is unchanged (padding lives inside the cells) —
        // persist only.
        Self::persist_bar(plots, id)
    }

    /// Set the slot gaps (`TopEvent::Bar(BarEvent::SlotSpacing)`): single commit per
    /// press, like padding.
    pub(crate) fn handle_set_slot_spacing(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.slot_spacing = value.clamp(0.0, TopLocal::MAX_SLOT_GAP);
        }
        Self::persist_bar(plots, id)
    }

    /// Minimum seconds between two `render()` calls of one Lua widget
    /// (keeps a `interval = 0` typo from hot-looping the update thread).
    const MIN_WIDGET_INTERVAL: f32 = 0.25;

    /// Script file for a Lua def, resolved against the widgets dir
    /// (absolute paths pass through). An empty `file` defaults to
    /// `<name>.lua`, so bare `name`-only entries just work.
    pub(crate) fn widget_script_path(def: &crate::config::WidgetDef) -> std::path::PathBuf {
        let trimmed = def.file.trim();
        if trimmed.is_empty() {
            return crate::config::widgets_dir().join(format!("{}.lua", def.name.trim()));
        }
        let path = std::path::PathBuf::from(trimmed);
        if path.is_absolute() {
            path
        } else {
            crate::config::widgets_dir().join(path)
        }
    }

    /// Log a widget error once per message (a broken 1s script must not
    /// flood the log every tick; fixing the file logs nothing new until
    /// it breaks differently).
    pub(crate) fn note_widget_error(plots: &mut Plots, name: &str, err: String) {
        if plots
            .widget_last_error
            .get(name)
            .is_some_and(|last| *last == err)
        {
            return;
        }
        eprintln!("riced: widget {name:?}: {err}");
        plots.widget_last_error.insert(name.to_string(), err);
    }

    /// Ensure a sandboxed runtime for one Lua def (load + `render`
    /// check). Retried on later ticks while missing, so fixing the
    /// file recovers without a restart.
    pub(crate) fn ensure_widget_lua(
        plots: &mut Plots,
        def: &crate::config::WidgetDef,
    ) -> Result<(), String> {
        if plots.widget_lua.contains_key(&def.name) {
            return Ok(());
        }
        let path = Self::widget_script_path(def);
        let source = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let lua = new_widget_lua().map_err(|e| e.to_string())?;
        load_widget_script(&lua, &def.name, &source)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        plots.widget_lua.insert(def.name.clone(), lua);
        Ok(())
    }

    /// Owner scope for one bar's widget instance: legacy button lists
    /// and declared `ui.listview`s key runtimes (and motion keys) under
    /// this, so two bars never animate each other.
    fn list_scope(bar: window::Id, widget: &str) -> String {
        format!("{bar:?}/{widget}")
    }

    /// Bars whose slots reference `widget`: its per-bar render targets.
    /// Widgets in no bar render nowhere (nothing displays them).
    fn bars_with_widget(plots: &Plots, widget: &str) -> Vec<window::Id> {
        plots
            .tops
            .iter()
            .filter(|(_, top)| {
                top.local
                    .widgets
                    .iter()
                    .any(|slot| slot.iter().any(|name| name == widget))
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Publish fresh service tables plus this bar's `bar.output`, then
    /// call one widget's `render()`, returning the raw value. Errors
    /// are returned for once-per-message logging by the caller.
    fn render_lua_value(
        plots: &mut Plots,
        def: &crate::config::WidgetDef,
        gpu: Option<f32>,
        bar: window::Id,
    ) -> Result<Value, String> {
        Self::sync_script_state(plots, def);
        Self::ensure_widget_lua(plots, def)?;
        let lua = plots
            .widget_lua
            .get(&def.name)
            .ok_or_else(|| "runtime missing".to_string())?;
        let ctx = crate::services::ServiceCtx::from_plots(plots, gpu);
        crate::services::publish_all(&ctx, lua).map_err(|e| e.to_string())?;
        let output = Self::output_name(plots, bar);
        crate::services::publish_bar(lua, &output).map_err(|e| e.to_string())?;
        call_lua_value(lua, "view")
    }

    /// Store one `render()` result: tables become [`WidgetNode`] trees,
    /// scalars become cached text. Switching shapes clears the other
    /// cache so nothing stale renders. Trees/text cache per bar (same
    /// widget renders per-bar `bar.output`); the Lua state stays
    /// shared, so `self` is per widget, not per bar.
    fn ingest_render_value(
        plots: &mut Plots,
        def: &crate::config::WidgetDef,
        bar: window::Id,
        result: Result<Value, String>,
    ) {
        let tree_key = (bar, def.name.clone());
        let scope = Self::list_scope(bar, &def.name);
        match result {
            Ok(Value::Table(t)) => {
                plots.widget_last_error.remove(&def.name);
                plots.widget_outputs.remove(&tree_key);
                match parse_node(&Value::Table(t)) {
                    Ok(node) => {
                        // Diff button lists for enter/exit/displaced
                        // transitions before replacing the cached tree,
                        // applying this widget's Lua `transitions()`
                        // spec (or the shared defaults) to its own list.
                        let old = plots.widget_trees.get(&tree_key).cloned();
                        if let Err(error) = sync_declared_lists(plots, &scope, old.as_ref(), &node)
                        {
                            Self::note_widget_error(plots, &def.name, error);
                            return;
                        }
                        let duration = plots.config.animation.speed.duration();
                        let list = plots
                            .widget_lists
                            .entry(scope.clone())
                            .or_insert_with(|| super::listview::ListView::new(TopLocal::ROW_PITCH));
                        if let Some(lua) = plots.widget_lua.get(&def.name) {
                            let defaults: super::listview::ListView<(String, String), WidgetNode> =
                                super::listview::ListView::new(TopLocal::ROW_PITCH);
                            let current = (
                                defaults.enter_spec(),
                                defaults.exit_spec(),
                                defaults.displaced_spec(),
                            );
                            match parse_transitions(lua, &current.0, &current.1, &current.2) {
                                Ok((enter, exit, displaced, custom)) => {
                                    list.set_transitions(enter, exit, displaced, custom);
                                }
                                Err(e) => {
                                    list.clear_all(&mut plots.anim_runtime);
                                    Self::note_widget_error(plots, &def.name, e);
                                    plots.widget_lists.remove(&scope);
                                    plots.widget_trees.insert(tree_key.clone(), node);
                                    return;
                                }
                            }
                        }
                        let old_is_list = matches!(
                            old,
                            Some(WidgetNode::Row { .. } | WidgetNode::Column { .. })
                        );
                        let new_is_list =
                            matches!(node, WidgetNode::Row { .. } | WidgetNode::Column { .. });
                        if !old_is_list || !new_is_list {
                            // First paint or shape flip: settle instantly.
                            if let Some(list) = plots.widget_lists.get_mut(&scope) {
                                list.clear_scope(&mut plots.anim_runtime, move |(w, _)| {
                                    *w == scope
                                });
                            }
                        } else {
                            let old_node = old.as_ref().expect("list checked");
                            let old_kids = button_children(old_node);
                            let new_kids = button_children(&node);
                            let old_keys: Vec<(String, String)> = old_kids
                                .iter()
                                .map(|(k, _)| (scope.clone(), k.clone()))
                                .collect();
                            let new_keys: Vec<(String, String)> = new_kids
                                .iter()
                                .map(|(k, _)| (scope.clone(), k.clone()))
                                .collect();
                            let by_key: std::collections::HashMap<&str, &WidgetNode> =
                                old_kids.iter().map(|(k, n)| (k.as_str(), n)).collect();
                            let removed: Vec<(usize, (String, String), WidgetNode)> = old_keys
                                .iter()
                                .enumerate()
                                .filter(|(_, k)| !new_keys.contains(k))
                                .filter_map(|(i, k)| {
                                    by_key
                                        .get(k.1.as_str())
                                        .map(|n| (i, k.clone(), (*n).clone()))
                                })
                                .collect();
                            let axis = if matches!(node, WidgetNode::Row { .. }) {
                                Axis::Horizontal
                            } else {
                                Axis::Vertical
                            };
                            if let Some(list) = plots.widget_lists.get_mut(&scope) {
                                list.update(
                                    &mut plots.anim_runtime,
                                    duration,
                                    axis,
                                    &old_keys,
                                    &new_keys,
                                    &removed,
                                );
                            }
                        }
                        plots.widget_trees.insert(tree_key, node);
                    }
                    Err(e) => {
                        plots.widget_trees.remove(&tree_key);
                        Self::note_widget_error(plots, &def.name, e);
                    }
                }
            }
            Ok(value) => {
                plots.widget_last_error.remove(&def.name);
                plots.widget_trees.remove(&tree_key);
                match coerce_text(value, "render()") {
                    Ok(text) => {
                        plots.widget_outputs.insert(tree_key, text);
                    }
                    Err(e) => Self::note_widget_error(plots, &def.name, e),
                }
            }
            Err(e) => Self::note_widget_error(plots, &def.name, e),
        }
    }

    /// Drop a widget's runtime when its script file changed on disk, so
    /// the next render reloads it (live widget development without
    /// touching `widgets.toml`). Unreadable files keep the old state.
    /// Returns true when the runtime was dropped (callers force a
    /// refresh: reloaded content may differ even if `render()` output
    /// doesn't, e.g. `popup()` edits on a slow-interval widget).
    pub(crate) fn sync_script_state(plots: &mut Plots, def: &crate::config::WidgetDef) -> bool {
        let path = Self::widget_script_path(def);
        let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            return false;
        };
        match plots.widget_script_mtime.get(&def.name) {
            Some(known) if *known == mtime => false,
            _ => {
                plots.widget_lua.remove(&def.name);
                plots.widget_script_mtime.insert(def.name.clone(), mtime);
                true
            }
        }
    }

    /// (Re)build runtimes for every def and render once, so bars
    /// populate immediately. Called at startup and after every
    /// `widgets.toml` hot-reload (which clears the old states).
    /// Per-bar trees render when bars appear (`render_bar_widgets`)
    /// or on the next due tick; with no bars yet this only resets.
    pub(crate) fn init_widget_lua(plots: &mut Plots) {
        plots.widget_lua.clear();
        plots.widget_outputs.clear();
        plots.widget_trees.clear();
        plots.widget_last_run.clear();
        plots.widget_last_error.clear();
        plots.widget_script_mtime.clear();
        // Fresh Lua states mean fresh lists: drop every per-widget
        // list (their runtime slots free on the next sweep; the first
        // post-reload paint re-enters from scratch and settles).
        for list in plots.widget_lists.values_mut() {
            list.clear_all(&mut plots.anim_runtime);
        }
        plots.widget_lists.clear();
        plots.sysinfo.refresh_cpu_usage();
        plots.sysinfo.refresh_memory();
        let gpu = Popup::gpu_usage_percent();
        let defs = plots.widgets.clone();
        let now = Instant::now();
        for def in &defs {
            plots.widget_last_run.insert(def.name.clone(), now);
            for bar in Self::bars_with_widget(plots, &def.name) {
                let result = Self::render_lua_value(plots, def, gpu, bar);
                Self::ingest_render_value(plots, def, bar, result);
            }
        }
        Self::warn_unknown_slot_widgets(plots);
    }

    /// Render every widget slotted on a bar (bar creation at startup
    /// or output hotplug): without this a new bar paints empty until
    /// each widget's interval elapses.
    pub(crate) fn render_bar_widgets(plots: &mut Plots, bar: window::Id) {
        plots.sysinfo.refresh_cpu_usage();
        plots.sysinfo.refresh_memory();
        let gpu = Popup::gpu_usage_percent();
        let now = Instant::now();
        let names: Vec<String> = plots
            .tops
            .get(&bar)
            .map(|top| top.local.widgets.iter().flatten().cloned().collect())
            .unwrap_or_default();
        for def in plots.widgets.clone() {
            if !names.iter().any(|name| name == &def.name) {
                continue;
            }
            plots.widget_last_run.insert(def.name.clone(), now);
            let result = Self::render_lua_value(plots, &def, gpu, bar);
            Self::ingest_render_value(plots, &def, bar, result);
        }
    }

    /// Warn about slot names that resolve to no registry entry (typos
    /// and commented-out defs render blank with no other trace).
    /// Runs at startup and on every hot-reload, when configs change.
    fn warn_unknown_slot_widgets(plots: &Plots) {
        for (bar_id, top) in &plots.tops {
            for (pos, slot) in top.local.widgets.iter().enumerate() {
                for name in slot {
                    if !TopLocal::is_empty_widget(name)
                        && !plots.widgets.iter().any(|d| d.name == *name)
                    {
                        eprintln!(
                            "riced: bar {bar_id:?} slot {} references unknown widget {name:?} (not in widgets.toml)",
                            pos + 1,
                        );
                    }
                }
            }
        }
    }

    /// Run every `render()` whose interval elapsed. Returns `true`
    /// when any output moved (caller repaints).
    fn run_due_widgets(plots: &mut Plots) -> bool {
        let now = Instant::now();
        plots.sysinfo.refresh_cpu_usage();
        plots.sysinfo.refresh_memory();
        let gpu = Popup::gpu_usage_percent();
        let defs = plots.widgets.clone();
        let mut changed = false;
        for def in &defs {
            let interval = def.interval.max(Self::MIN_WIDGET_INTERVAL);
            let due = plots
                .widget_last_run
                .get(&def.name)
                .is_none_or(|t| now.duration_since(*t) >= Duration::from_secs_f32(interval));
            // Open popups bypass the render interval: a script edit
            // must refresh the menu now, not on the next (maybe 60s)
            // tick. `sync` drops the stale runtime; the refresh below
            // reloads it, and `changed` forces the repaint + body pass
            // even when `render()` output is identical.
            let live_popup = plots.popups.values().any(|p| p.widget == def.name)
                && Self::sync_script_state(plots, def);
            if !due && !live_popup {
                continue;
            }
            plots.widget_last_run.insert(def.name.clone(), now);
            changed |= Self::refresh_widget(plots, def, gpu) | live_popup;
        }
        changed
    }

    /// Re-render one widget on every bar showing it, and report whether
    /// any visible output moved (text or tree). Shared by the interval
    /// tick, the `on_press()` click path, cell actions, and popup selects.
    pub(crate) fn refresh_widget(
        plots: &mut Plots,
        def: &crate::config::WidgetDef,
        gpu: Option<f32>,
    ) -> bool {
        let bars = Self::bars_with_widget(plots, &def.name);
        let before: Vec<_> = bars
            .iter()
            .map(|bar| {
                (
                    plots.widget_outputs.get(&(*bar, def.name.clone())).cloned(),
                    plots.widget_trees.get(&(*bar, def.name.clone())).cloned(),
                )
            })
            .collect();
        for bar in &bars {
            let result = Self::render_lua_value(plots, def, gpu, *bar);
            Self::ingest_render_value(plots, def, *bar, result);
        }
        let after: Vec<_> = bars
            .iter()
            .map(|bar| {
                (
                    plots.widget_outputs.get(&(*bar, def.name.clone())).cloned(),
                    plots.widget_trees.get(&(*bar, def.name.clone())).cloned(),
                )
            })
            .collect();
        before != after
    }

    /// Re-render due Lua widgets (`TopEvent::Widget(WidgetEvent::Tick)`):
    /// emits `Widget(Changed)` only when an output moved (which repaints).
    pub(crate) fn handle_widget_tick(plots: &mut Plots) -> Command<Plant> {
        if Self::run_due_widgets(plots) {
            return Command::done(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Changed)));
        }
        Command::none()
    }

    /// Advance list enter/exit transitions (`TopEvent::Widget(WidgetEvent::Anim)`):
    /// ticks the shared aura runtime once, then sweeps the widget and
    /// notification transition sets. Repaint comes from the `Scope::All`
    /// redraw scope, not here.
    pub(crate) fn handle_anim_frame(plots: &mut Plots) -> Command<Plant> {
        plots.anim_runtime.tick_at(Instant::now());
        for list in plots.widget_lists.values_mut() {
            list.sweep(&mut plots.anim_runtime);
        }
        super::notification::sweep_noti_anims(plots)
    }

    /// Click a cell button: run the owning widget's `on_action(key)`
    /// (the view closure stamps the owner, so the key routes to its
    /// own Lua state — no slot-wide popup/`on_press` fallback), then
    /// re-render that widget like the click path does. `on_action` sees
    /// the clicking bar's `bar.output`.
    pub(crate) fn handle_cell_action(
        plots: &mut Plots,
        bar: window::Id,
        widget: String,
        action: String,
    ) -> Command<Plant> {
        let gpu = Popup::gpu_usage_percent();
        let outcome = plots.widget_lua.get(&widget).map(|lua| {
            let output = Self::output_name(plots, bar);
            let ctx = crate::services::ServiceCtx::from_plots(plots, gpu);
            crate::services::publish_all(&ctx, lua)
                .map_err(|e| e.to_string())
                .and_then(|()| {
                    crate::services::publish_bar(lua, &output).map_err(|e| e.to_string())
                })
                .and_then(|()| call_lua_named_action(lua, &action))
        });
        match outcome {
            Some(Ok(value)) => {
                plots.widget_last_error.remove(&widget);
                let notif = super::notification::command_from_action(&value, plots);
                if let Some(def) = plots.widgets.iter().find(|d| d.name == widget).cloned()
                    && Self::refresh_widget(plots, &def, gpu)
                {
                    return Command::batch(vec![
                        Command::done(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Changed))),
                        notif,
                    ]);
                }
                return notif;
            }
            Some(Err(e)) => Self::note_widget_error(plots, &widget, e),
            // Unknown widget: ignore (stale message after hot-reload).
            None => {}
        }
        Command::none()
    }

    pub(crate) fn handle_set_opacity(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            // Single commit per press (preset buttons, not a drag stream).
            top.local.opacity = TopLocal::snap_opacity(value);
        }
        Self::persist_bar(plots, id)
    }

    pub(crate) fn handle_set_floating(
        plots: &mut Plots,
        id: window::Id,
        value: bool,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.floating = value;
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_margin_top(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.top = value.max(0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_margin_right(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.right = value.max(0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_margin_bottom(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.bottom = value.max(0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_margin_left(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.left = value.max(0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_radius_tl(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.top_left = value.max(0.0);
        }
        Self::persist_bar(plots, id)
    }

    pub(crate) fn handle_set_radius_tr(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.top_right = value.max(0.0);
        }
        Self::persist_bar(plots, id)
    }

    pub(crate) fn handle_set_radius_bl(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.bottom_left = value.max(0.0);
        }
        Self::persist_bar(plots, id)
    }

    pub(crate) fn handle_set_radius_br(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.bottom_right = value.max(0.0);
        }
        Self::persist_bar(plots, id)
    }

    /// Connector name for a bar's output (`""` when unknown/sentinel).
    pub(crate) fn output_name(plots: &Plots, bar_id: window::Id) -> String {
        plots
            .ids
            .get(&bar_id)
            .copied()
            .and_then(|info| match info {
                PlotInfo::Top(o) => plots.output_infos.get(&o),
                _ => None,
            })
            .and_then(|info| info.name.clone())
            .unwrap_or_default()
    }

    /// Write the bar's current state back to its `[[bar]]` entry and
    /// arm a coalesced config save (same idle-write as `ConfigPatch`
    /// drags). Entries are matched by index; out-of-range indices push.
    pub(crate) fn persist_bar(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        let (index, anchor, local) = match plots.tops.get(&bar_id) {
            Some(t) => (
                t.bar_index,
                t.anchor_label().to_lowercase(),
                t.local.clone(),
            ),
            None => return Command::none(),
        };
        let output = Self::output_name(plots, bar_id);
        let aligns: Vec<String> = local
            .aligns
            .iter()
            .map(|a| a.as_str().to_string())
            .collect();
        let widgets: Vec<crate::config::SlotWidgets> = local
            .widgets
            .iter()
            .map(|slot| crate::config::SlotWidgets::Many(slot.clone()))
            .collect();
        let entry = crate::config::TopConfig {
            anchor,
            output,
            length: local.length_pct,
            thickness: local.thickness_px,
            slots: local.slots.clamp(1, TopLocal::MAX_SLOTS),
            aligns,
            widgets,
            slot_padding: local.slot_padding,
            slot_spacing: local.slot_spacing,
            opacity: TopLocal::snap_opacity(local.opacity),
            floating: local.floating,
            margin_top: local.margins.top,
            margin_right: local.margins.right,
            margin_bottom: local.margins.bottom,
            margin_left: local.margins.left,
            radius_top_left: local.radius.top_left,
            radius_top_right: local.radius.top_right,
            radius_bottom_left: local.radius.bottom_left,
            radius_bottom_right: local.radius.bottom_right,
        };
        if index == usize::MAX || index >= plots.config.bar.len() {
            plots.config.bar.push(entry);
            if let Some(top) = plots.tops.get_mut(&bar_id) {
                top.bar_index = plots.config.bar.len() - 1;
            }
        } else {
            plots.config.bar[index] = entry;
        }
        plots.arm_config_save()
    }

    /// Remove a bar (`TopEvent::Remove`): drop tracking + cursor state,
    /// close its window, and delete its `[[bar]]` entry (persisted
    /// immediately) so it stays gone after restart.
    pub(crate) fn handle_remove(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        plots.last_cursor.remove(&bar_id);
        plots.press_targets.remove(&bar_id);
        let mut cmds = Vec::new();
        // A bar going away takes its popup with it.
        let popup_id = plots
            .popups
            .iter()
            .find(|(_, p)| p.bar_id == bar_id)
            .map(|(id, _)| *id);
        if let Some(pid) = popup_id {
            cmds.push(super::Popup::handle_dismiss(plots, pid));
        }
        if let Some(top) = plots.tops.remove(&bar_id) {
            plots.ids.remove(&bar_id);
            if top.bar_index < plots.config.bar.len() {
                plots.config.bar.remove(top.bar_index);
                // Indices after the hole shift down by one.
                for other in plots.tops.values_mut() {
                    if other.bar_index > top.bar_index {
                        other.bar_index -= 1;
                    }
                }
            }
            // flush_config_save only writes when dirty — mark it first
            // (same for persist_new below).
            plots.config_dirty = true;
            plots.flush_config_save();
        } else {
            plots.ids.remove(&bar_id);
        }
        cmds.push(iced_runtime::task::effect(Action::Window(
            WindowAction::Close(bar_id),
        )));
        Command::batch(cmds)
    }

    /// Push the bar's current size/exclusive/margins to its live window.
    /// Same window id throughout — no close/reopen flicker. Skips sentinel
    /// windows (fixed fallback until outputs arrive and replace them).
    pub(crate) fn apply_layout(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        // Clamp stale values: length 1–100%, thickness 1..=thin output axis.
        if let Some(top) = plots.tops.get_mut(&bar_id) {
            top.local.length_pct = top.local.length_pct.clamp(1.0, 100.0);
        }
        let (output, top) = match plots.ids.get(&bar_id).copied() {
            Some(PlotInfo::Top(o)) => match plots.tops.get(&bar_id).cloned() {
                Some(t) => (o, t),
                None => return Command::none(),
            },
            _ => return Command::none(),
        };
        if output == OutputId(u32::MAX) {
            return Command::none();
        }
        let (sw, sh) = Self::output_size(&plots.output_infos, output);
        let mut top = top;
        let max = TopLocal::max_thickness(sw, sh, top.is_horizontal()).max(1.0);
        top.local.thickness_px = top.local.thickness_px.clamp(1.0, max);
        if let Some(stored) = plots.tops.get_mut(&bar_id) {
            stored.local.thickness_px = top.local.thickness_px;
            stored.local.length_pct = top.local.length_pct;
        }
        let horizontal = top.is_horizontal();
        let (w, h) = top.local.px_size(sw, sh, horizontal);
        // Margins are widget padding (see `view`): the surface stays
        // edge-pinned with its full exclusive zone. Always clear the
        // compositor-side margins so no stale layer offset lingers.
        let cmds = vec![
            Command::done(Plant::LayoutChange {
                id: bar_id,
                anchor: top.anchor,
                size: LayerSize::px(w, h),
            }),
            Command::done(Plant::ExclusiveZoneChange {
                id: bar_id,
                zone_size: Self::exclusive_px(&top, w, h),
            }),
            Command::done(Plant::MarginChange {
                id: bar_id,
                margin: (0, 0, 0, 0),
            }),
        ];
        Command::batch(cmds)
    }

    /// Re-apply every bar on `output` (resolution/scale changed geometry:
    /// `%` sizes now resolve to different px). Called on `OutputUpdated`.
    pub(crate) fn reapply_for_output(plots: &mut Plots, output: OutputId) -> Command<Plant> {
        let bars: Vec<window::Id> = plots
            .ids
            .iter()
            .filter_map(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == output => Some(*wid),
                _ => None,
            })
            .collect();
        Command::batch(
            bars.into_iter()
                .map(|wid| Self::apply_layout(plots, wid))
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn handle_release(
        plots: &mut Plots,
        id: window::Id,
        button: Button,
    ) -> Command<Plant> {
        if !matches!(plots.id_info(id), Some(PlotInfo::Top(_))) {
            return Command::none();
        }
        // Outer releases bubble up from everywhere, including widget
        // areas that already dispatched their own click: only a
        // recorded *gap* target acts here (the slot fallback below).
        // Anything else is either a widget click (handled) or stale.
        let target = plots.press_targets.remove(&id);
        // Silent: the global Graft release safety net in update can
        // clear the press first when release lands on another window,
        // and a same-window release fires both PanelWindow and Graft
        // paths. Only matched gap clicks act.
        let gap_clicked = button == Button::Left
            && matches!(target, Some((_, None, t)) if Instant::now().duration_since(t) < Self::HOLD_THRESHOLD);
        if gap_clicked {
            return Self::handle_slot_click(plots, id);
        }
        Command::none()
    }

    /// Release on one widget's own mouse area: clicks only when the
    /// press target matches (same slot/widget, under hold). A press
    /// captured by an inner `ui.button` records no target, so cell
    /// buttons never also trigger the widget click.
    pub(crate) fn handle_widget_release(
        plots: &mut Plots,
        bar_id: window::Id,
        pos: usize,
        widget: String,
    ) -> Command<Plant> {
        if !matches!(plots.id_info(bar_id), Some(PlotInfo::Top(_))) {
            return Command::none();
        }
        let target = plots.press_targets.remove(&bar_id);
        if Self::release_matches_press(target, pos, Some(&widget), Button::Left, Instant::now()) {
            return Self::handle_widget_click(plots, bar_id, pos, &widget);
        }
        Command::none()
    }

    /// Left-click on a bar under the hold threshold: resolve the slot
    /// under the cursor and either toggle its widget popup or run its
    /// `on_press()` action. A slot holding both prefers the popup.
    pub(crate) fn handle_slot_click(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        let (output, top) = match plots.ids.get(&bar_id).copied() {
            Some(PlotInfo::Top(o)) => match plots.tops.get(&bar_id).cloned() {
                Some(t) => (o, t),
                None => return Command::none(),
            },
            _ => return Command::none(),
        };
        let Some((_, _, sw, sh)) = Background::available_rect(output, &plots.output_infos) else {
            return Command::none();
        };
        let horizontal = top.is_horizontal();
        let (bw, bh) = top.local.px_size(sw, sh, horizontal);
        // Content rect: floating margins inset the painted cells. Cursor
        // positions are bar-local, so no output translation is needed.
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
        let cursor = plots.last_cursor.get(&bar_id).copied();
        let pos = cursor
            .and_then(|p| {
                Popup::slot_at_point(
                    (pl, pt, bw as f32 - pl - pr, bh as f32 - pt - pb),
                    n,
                    gap,
                    horizontal,
                    p,
                )
            })
            .or_else(|| {
                // No cursor (or gap click): first slot holding a popup widget.
                (0..n).find(|pos| {
                    top.local
                        .widgets_at(*pos)
                        .iter()
                        .any(|w| lua_has_func(&plots.widget_lua, w, "popup"))
                })
            });
        let Some(pos) = pos else {
            return Command::none();
        };
        // Toggle: a popup already open for this bar closes first; same
        // slot means it was just a close (per-widget clicks handle
        // their own toggles via `handle_widget_click`).
        let mut cmds = Vec::new();
        if let Some(pid) = plots
            .popups
            .iter()
            .find(|(_, p)| p.bar_id == bar_id)
            .map(|(id, _)| *id)
        {
            let same = plots.popups.get(&pid).is_some_and(|p| p.slot == pos);
            cmds.push(Popup::handle_dismiss(plots, pid));
            if same {
                return Command::batch(cmds);
            }
        }
        let names = top.local.widgets_at(pos).to_vec();
        cmds.push(Self::handle_slot_fallback(
            plots,
            bar_id,
            output,
            pos,
            &names,
            cursor.map(|p| (p.x, p.y)),
        ));
        if cmds.is_empty() {
            return Command::none();
        }
        Command::batch(cmds)
    }

    /// Click on one widget's own mouse area (renderer hit-tested, so
    /// every widget in a slot gets its own calls): toggle its popup,
    /// run its `on_press()`, or fall back to the slot when it defines
    /// neither. A popup open for another widget is replaced.
    pub(crate) fn handle_widget_click(
        plots: &mut Plots,
        bar_id: window::Id,
        pos: usize,
        widget: &str,
    ) -> Command<Plant> {
        let output = match plots.ids.get(&bar_id).copied() {
            Some(PlotInfo::Top(o)) => o,
            _ => return Command::none(),
        };
        let cursor = plots.last_cursor.get(&bar_id).copied().map(|p| (p.x, p.y));
        let mut cmds = Vec::new();
        if let Some(pid) = plots
            .popups
            .iter()
            .find(|(_, p)| p.bar_id == bar_id)
            .map(|(id, _)| *id)
        {
            let same_widget = plots.popups.get(&pid).is_some_and(|p| p.widget == widget);
            cmds.push(Popup::handle_dismiss(plots, pid));
            if same_widget {
                return Command::batch(cmds);
            }
        }
        if lua_has_func(&plots.widget_lua, widget, "popup") {
            if let Some(cmd) = Popup::open_for(plots, bar_id, output, pos, cursor, Some(widget)) {
                cmds.push(cmd);
            }
        } else if lua_has_func(&plots.widget_lua, widget, "on_press") {
            cmds.push(Self::run_on_press(plots, bar_id, widget));
        } else {
            let names = plots
                .tops
                .get(&bar_id)
                .map(|t| t.local.widgets_at(pos).to_vec())
                .unwrap_or_default();
            cmds.push(Self::handle_slot_fallback(
                plots, bar_id, output, pos, &names, cursor,
            ));
        }
        if cmds.is_empty() {
            return Command::none();
        }
        Command::batch(cmds)
    }

    /// Slot fallback for gap clicks and action-less widgets: open the
    /// first popup in the slot, else run the first `on_press()`.
    fn handle_slot_fallback(
        plots: &mut Plots,
        bar_id: window::Id,
        output: OutputId,
        pos: usize,
        names: &[String],
        cursor: Option<(f32, f32)>,
    ) -> Command<Plant> {
        if names
            .iter()
            .any(|w| lua_has_func(&plots.widget_lua, w, "popup"))
        {
            if let Some(cmd) = Popup::open_for(plots, bar_id, output, pos, cursor, None) {
                return cmd;
            }
            return Command::none();
        }
        // No menu: run the first `on_press()` action in the slot, then
        // re-render that widget (a toggle flips its next output).
        if let Some(name) = names
            .iter()
            .find(|w| lua_has_func(&plots.widget_lua, w, "on_press"))
            .cloned()
        {
            return Self::run_on_press(plots, bar_id, &name);
        }
        Command::none()
    }

    /// Run one widget's `on_press()` click action, then re-render it
    /// (a toggle flips its next output). Errors log once-per-message.
    /// `on_action` sees the clicking bar's `bar.output`.
    fn run_on_press(plots: &mut Plots, bar: window::Id, name: &str) -> Command<Plant> {
        let gpu = Popup::gpu_usage_percent();
        let outcome = match plots.widget_lua.get(name) {
            Some(lua) => {
                let output = Self::output_name(plots, bar);
                let ctx = crate::services::ServiceCtx::from_plots(plots, gpu);
                let acted = crate::services::publish_all(&ctx, lua)
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        crate::services::publish_bar(lua, &output).map_err(|e| e.to_string())
                    })
                    .and_then(|()| call_lua_action(lua));
                Some(acted)
            }
            None => None,
        };
        match outcome {
            Some(Ok(())) => {
                if let Some(def) = plots.widgets.iter().find(|d| d.name == name).cloned()
                    && Self::refresh_widget(plots, &def, gpu)
                {
                    return Command::done(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Changed)));
                }
            }
            Some(Err(e)) => Self::note_widget_error(plots, name, e),
            None => {}
        }
        Command::none()
    }

    /// Cursor bookkeeping for Top windows — press/release come from PanelWindow.
    pub(crate) fn handle_graft(
        plots: &mut Plots,
        id: window::Id,
        event: &iced::Event,
    ) -> Command<Plant> {
        if let iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) = event {
            return Self::handle_cursor_moved(plots, id, *position);
        }
        Command::none()
    }

    /// Cleanup sentinel tops (OutputId::MAX) created before outputs were known.
    /// Returns the window ids that were removed (caller should close them and clean last_cursor).
    pub(crate) fn cleanup_sentinels(
        tops: &mut HashMap<window::Id, Top>,
        ids: &mut HashMap<window::Id, PlotInfo>,
    ) -> Vec<window::Id> {
        let sentinel = OutputId(u32::MAX);
        let sentinel_ids: Vec<window::Id> = ids
            .iter()
            .filter_map(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == sentinel => Some(*wid),
                _ => None,
            })
            .collect();
        for id in &sentinel_ids {
            tops.remove(id);
            ids.remove(id);
        }
        sentinel_ids
    }

    /// Remove all tops for `output_id` (supports multiple per output).
    /// Returns the window ids that were removed.
    pub(crate) fn remove_for_output(
        tops: &mut HashMap<window::Id, Top>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        output_id: OutputId,
    ) -> Vec<window::Id> {
        let to_remove: Vec<window::Id> = ids
            .iter()
            .filter_map(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == output_id => Some(*wid),
                _ => None,
            })
            .collect();
        for wid in &to_remove {
            tops.remove(wid);
            ids.remove(wid);
        }
        to_remove
    }

    /// Handle `TopPlot(TopEvent::Sow)` – detect output and closest edge (Left/Right/Top/Bottom) where the
    /// context menu was opened and spawn a new bar there. Returns a `NewLayerShell` command.
    pub(crate) fn handle_add(
        plots: &mut Plots,
        menu_pos: Option<Point>,
        menu_output: Option<OutputId>,
    ) -> Option<Command<Plant>> {
        // Geometry helpers live on Background so Top bars and Background
        // windows agree on output coordinates.
        use Background as Geo;
        fn closest_anchor(mp: Point, info: &OutputInfo) -> Anchor {
            let (sx, sy, sw, sh) = Geo::output_geometry(info);
            let left_dist = mp.x - sx;
            let right_dist = (sx + sw) - mp.x;
            let top_dist = mp.y - sy;
            let bottom_dist = (sy + sh) - mp.y;
            let mut best = (top_dist, Anchor::Top);
            if bottom_dist < best.0 {
                best = (bottom_dist, Anchor::Bottom);
            }
            if left_dist < best.0 {
                best = (left_dist, Anchor::Left);
            }
            if right_dist < best.0 {
                best = (right_dist, Anchor::Right);
            }
            best.1
        }
        fn anchor_name(a: Anchor) -> &'static str {
            if a == Anchor::Top {
                "Top"
            } else if a == Anchor::Bottom {
                "Bottom"
            } else if a == Anchor::Left {
                "Left"
            } else if a == Anchor::Right {
                "Right"
            } else {
                "Unknown"
            }
        }
        /// Persist a fresh bar as a `[[bar]]` entry right away (a
        /// click, not a drag — no coalescing needed) and record its index.
        fn persist_new(plots: &mut Plots, top: &mut Top, anchor: Anchor, output: String) {
            let l = top.local.clone();
            let aligns: Vec<String> = l.aligns.iter().map(|a| a.as_str().to_string()).collect();
            let widgets: Vec<crate::config::SlotWidgets> = l
                .widgets
                .iter()
                .map(|slot| crate::config::SlotWidgets::Many(slot.clone()))
                .collect();
            plots.config.bar.push(crate::config::TopConfig {
                anchor: anchor_name(anchor).to_lowercase(),
                output,
                length: l.length_pct,
                thickness: l.thickness_px,
                slots: l.slots.clamp(1, TopLocal::MAX_SLOTS),
                aligns,
                widgets,
                slot_padding: l.slot_padding,
                slot_spacing: l.slot_spacing,
                opacity: TopLocal::snap_opacity(l.opacity),
                floating: l.floating,
                margin_top: l.margins.top,
                margin_right: l.margins.right,
                margin_bottom: l.margins.bottom,
                margin_left: l.margins.left,
                radius_top_left: l.radius.top_left,
                radius_top_right: l.radius.top_right,
                radius_bottom_left: l.radius.bottom_left,
                radius_bottom_right: l.radius.bottom_right,
            });
            top.bar_index = plots.config.bar.len() - 1;
            plots.config_dirty = true;
            plots.flush_config_save();
        }

        let (target_output, target_anchor): (Option<OutputId>, Anchor) = {
            if let Some(output) = menu_output {
                if let Some(info) = plots.output_infos.get(&output) {
                    let anchor = menu_pos.map_or(Anchor::Top, |mp| closest_anchor(mp, info));
                    (Some(output), anchor)
                } else {
                    (Some(output), Anchor::Top)
                }
            } else if let Some(mp) = menu_pos {
                let mut found = plots.output_infos.iter().find(|(_, info)| {
                    let (sx, sy, sw, sh) = Geo::output_geometry(info);
                    mp.x >= sx && mp.x < sx + sw && mp.y >= sy && mp.y < sy + sh
                });
                if found.is_none() && !plots.output_infos.is_empty() {
                    let mut best: Option<(&OutputId, &OutputInfo, f32)> = None;
                    for (oid, info) in &plots.output_infos {
                        let (sx, sy, sw, sh) = Geo::output_geometry(info);
                        let cx = sx + sw / 2.0;
                        let cy = sy + sh / 2.0;
                        let dx = mp.x - cx;
                        let dy = mp.y - cy;
                        let dist2 = dx * dx + dy * dy;
                        if best.is_none() || dist2 < best.unwrap().2 {
                            best = Some((oid, info, dist2));
                        }
                    }
                    found = best.map(|(oid, info, _)| (oid, info));
                }
                if let Some((oid, info)) = found {
                    (Some(*oid), closest_anchor(mp, info))
                } else {
                    (None, Anchor::Top)
                }
            } else if let Some(oid) = plots.output_infos.keys().next().copied() {
                (Some(oid), Anchor::Top)
            } else {
                (None, Anchor::Top)
            }
        };

        if let Some(output_id) = target_output {
            let anchor = target_anchor;
            let duplicate = plots.ids.iter().any(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == output_id => {
                    plots.tops.get(wid).is_some_and(|t| t.anchor() == anchor)
                }
                _ => false,
            });
            if duplicate {
                println!(
                    "Note: {} bar already exists for output {output_id:?}, not spawning another",
                    anchor_name(anchor),
                );
                return None;
            }
            let mut top = Top::with_anchor(anchor);
            let (sw, sh) = Self::output_size(&plots.output_infos, output_id);
            let (w, h) = top.local.px_size(sw, sh, top.is_horizontal());
            let (win_id, settings) = top.open(output_id.0, w, h);
            let output = plots
                .output_infos
                .get(&output_id)
                .and_then(|info| info.name.clone())
                .unwrap_or_default();
            persist_new(plots, &mut top, anchor, output);
            plots.tops.insert(win_id, top);
            plots.ids.insert(win_id, PlotInfo::Top(output_id));
            Self::render_bar_widgets(plots, win_id);
            println!(
                "Added {} bar for output {output_id:?} window {win_id:?} (closest to {:?} @ {menu_pos:?} stored_output {menu_output:?}) — calling top.open() and spawning NewLayerShell",
                anchor_name(anchor),
                anchor
            );
            Some(Command::done(Plant::NewLayerShell {
                settings,
                id: win_id,
            }))
        } else {
            let sentinel = OutputId(u32::MAX);
            let duplicate = plots
                .ids
                .iter()
                .any(|(_, info)| matches!(info, PlotInfo::Top(o) if *o == sentinel));
            if duplicate {
                println!("Note: sentinel bar already exists (no output yet), not spawning another");
                return None;
            }
            let mut top = Top::new();
            let (win_id, settings) = top.open_active();
            persist_new(plots, &mut top, Anchor::Top, String::new());
            plots.tops.insert(win_id, top);
            plots.ids.insert(win_id, PlotInfo::Top(sentinel));
            Self::render_bar_widgets(plots, win_id);
            println!(
                "Added sentinel Top window {win_id:?} (no output yet) — calling top.open_active()"
            );
            Some(Command::done(Plant::NewLayerShell {
                settings,
                id: win_id,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_widget(name: &str) -> WidgetDef {
        WidgetDef {
            name: name.to_string(),
            size: 13.0,
            ..Default::default()
        }
    }

    /// Load the vendored seed components into a test state (production
    /// injects them via `components_source()` from disk; tests seed
    /// them directly so seed widgets using `iced.use` resolve).
    fn load_seed_components(lua: &Lua) {
        use crate::config::{SEED_COMPONENT_CARD, SEED_COMPONENT_DEFINE, SEED_COMPONENT_MENU};
        for source in [
            SEED_COMPONENT_DEFINE,
            SEED_COMPONENT_CARD,
            SEED_COMPONENT_MENU,
        ] {
            lua.load(source).exec().expect("seed component");
        }
    }

    #[test]
    fn lua_sandbox_runs_app_view() {
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(&lua, "test", "return { view = function() return 'hi' end }")
            .expect("load");
        assert_eq!(call_lua_text(&lua, "view").unwrap(), "hi");
    }

    #[test]
    fn lua_return_values_coerce_to_text() {
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(&lua, "test", "return { view = function() return 42 end }")
            .expect("load");
        assert_eq!(call_lua_text(&lua, "view").unwrap(), "42");
        load_widget_script(&lua, "test", "return { view = function() return true end }")
            .expect("load");
        assert_eq!(call_lua_text(&lua, "view").unwrap(), "true");
        load_widget_script(&lua, "test", "return { view = function() return nil end }")
            .expect("load");
        assert_eq!(call_lua_text(&lua, "view").unwrap(), "");
        load_widget_script(&lua, "test", "return { view = function() return {} end }")
            .expect("load");
        assert!(call_lua_text(&lua, "view").is_err());
    }

    #[test]
    fn lua_module_without_view_or_with_legacy_render_is_rejected() {
        let lua = new_widget_lua().expect("sandbox");
        assert!(load_widget_script(&lua, "test", "x = 1").is_err());
        assert!(load_widget_script(&lua, "test", "function render() return 'old' end").is_err());
        assert!(load_widget_script(&lua, "test", "return {}").is_err());
    }

    #[test]
    fn module_methods_bind_self_without_exporting_globals() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "module", "return {count=0, view=function(self) return ui.text(tostring(self.count)) end, on_press=function(self) self.count=self.count+1 end, on_action=function(self,key) return {dismiss=self.count, key=key} end}").unwrap();
        for name in ["render", "view", "on_press", "on_action"] {
            assert_eq!(lua.globals().get::<Value>(name).unwrap(), Value::Nil);
        }
        call_lua_action(&lua).unwrap();
        assert_eq!(
            parse_node(&call_lua_value(&lua, "view").unwrap()).unwrap(),
            WidgetNode::Text {
                content: "1".into(),
                size: None,
                width: None,
                height: None,
                color: None,
            }
        );
        let Value::Table(action) = call_lua_named_action(&lua, "dismiss").unwrap() else {
            panic!("table");
        };
        assert_eq!(action.get::<u32>("dismiss").unwrap(), 1);
        assert_eq!(action.get::<String>("key").unwrap(), "dismiss");
        load_widget_script(&lua, "replacement", crate::config::SEED_HELLO_LUA).unwrap();
        assert_eq!(call_lua_named_action(&lua, "dismiss").unwrap(), Value::Nil);
    }

    #[test]
    fn lua_sandbox_blocks_escapes_but_keeps_time() {
        // os.execute + io.popen are native shell (owner-accepted);
        // os.exit/remove/rename + require stay blocked.
        let lua = new_widget_lua().expect("sandbox");
        let execute: Value = lua.load("return os.execute").eval().expect("eval");
        assert!(matches!(execute, Value::Function(_)));
        for key in ["exit", "remove", "rename"] {
            let v: Value = lua.load(format!("return os.{key}")).eval().expect("eval");
            assert!(matches!(v, Value::Nil), "{key} blocked");
        }
        let require: Value = lua.load("return require").eval().expect("eval");
        assert!(matches!(require, Value::Nil));
        let hour: String = lua.load("return os.date('%H')").eval().expect("eval");
        assert_eq!(hour.len(), 2);
    }

    #[test]
    fn os_execute_is_native_shell() {
        // Native shell: exit codes, stdout capture, and `;` chaining
        // all work (documents the accepted risk — no allowlist).
        let lua = new_widget_lua().expect("sandbox");
        let ok: bool = lua
            .load(r#"return os.execute("true")"#)
            .eval()
            .expect("eval");
        assert!(ok);
        let out: String = lua
            .load(r#"local h = io.popen("echo hi"); local s = h:read("*a"); h:close(); return s"#)
            .eval()
            .expect("eval");
        assert_eq!(out.trim(), "hi");
        let chained: String = lua
            .load(r#"local h = io.popen("echo a; echo b"); local s = h:read("*a"); h:close(); return s"#)
            .eval()
            .expect("eval");
        assert!(chained.contains('a') && chained.contains('b'));
    }

    #[test]
    fn lua_cell_text_reads_cache_with_size() {
        let bar = window::Id::unique();
        let defs = vec![lua_widget("w")];
        let empty: HashMap<(window::Id, String), String> = HashMap::new();
        assert_eq!(lua_cell_text(bar, "w", &defs, &empty), None);
        let mut outputs = HashMap::new();
        outputs.insert((bar, "w".to_string()), "hi".to_string());
        assert_eq!(
            lua_cell_text(bar, "w", &defs, &outputs),
            Some(("hi".to_string(), 13.0))
        );
        // Unknown names never read the cache.
        assert_eq!(lua_cell_text(bar, "nope", &defs, &outputs), None);
        // Another bar's cache never leaks across.
        assert_eq!(
            lua_cell_text(window::Id::unique(), "w", &defs, &outputs),
            None
        );
    }

    #[test]
    fn preview_drop_rejects_stale_layout_and_removed_pool_definitions() {
        let old = vec![vec!["missing".to_string()], vec![]];
        let moved = vec![vec![], vec!["missing".to_string()]];
        assert!(Top::valid_widget_layout(&old, &old, &moved, &[]));
        assert!(!Top::valid_widget_layout(&moved, &old, &moved, &[]));
        let added = vec![vec!["missing".into(), "clock".into()], vec![]];
        assert!(!Top::valid_widget_layout(&old, &old, &added, &[]));
        assert!(Top::valid_widget_layout(
            &old,
            &old,
            &added,
            &[lua_widget("clock")]
        ));
        let duplicates = vec![vec!["missing".into(), "missing".into()], vec![]];
        assert!(!Top::valid_widget_layout(&old, &old, &duplicates, &[]));
    }

    #[test]
    fn release_matches_press_pairs_clicks() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let target = |pos: usize, widget: Option<&str>, age_ms: u64| {
            Some((
                pos,
                widget.map(str::to_string),
                now - Duration::from_millis(age_ms),
            ))
        };
        // Same slot+widget, left, under hold: click.
        assert!(Top::release_matches_press(
            target(1, Some("clock"), 10),
            1,
            Some("clock"),
            Button::Left,
            now
        ));
        // Widget mismatch (dragged off, or bubbled outer release after
        // a widget click): no double-fire.
        assert!(!Top::release_matches_press(
            target(1, Some("clock"), 10),
            1,
            Some("stats"),
            Button::Left,
            now
        ));
        // Widget release never answers a gap press and vice versa.
        assert!(!Top::release_matches_press(
            target(1, None, 10),
            1,
            Some("clock"),
            Button::Left,
            now
        ));
        assert!(!Top::release_matches_press(
            target(1, Some("clock"), 10),
            1,
            None,
            Button::Left,
            now
        ));
        // Slot mismatch, hold expiry, non-left, and stale targets: none.
        assert!(!Top::release_matches_press(
            target(1, None, 10),
            2,
            None,
            Button::Left,
            now
        ));
        assert!(!Top::release_matches_press(
            target(1, None, 900),
            1,
            None,
            Button::Left,
            now
        ));
        assert!(!Top::release_matches_press(
            target(1, None, 10),
            1,
            None,
            Button::Right,
            now
        ));
        assert!(!Top::release_matches_press(
            None,
            1,
            None,
            Button::Left,
            now
        ));
    }

    #[test]
    fn icon_segments_split_placeholders() {
        use Segment::{Icon, Text};
        assert_eq!(icon_segments("12%"), vec![Text("12%")]);
        assert_eq!(
            icon_segments("{icon:cpu} 12%"),
            vec![Icon("cpu"), Text(" 12%")]
        );
        assert_eq!(
            icon_segments("{icon:cpu}{icon:mem}"),
            vec![Icon("cpu"), Icon("mem")]
        );
        // Unterminated tails and empty names stay structured but render
        // literal (lookup misses on "").
        assert_eq!(icon_segments("{icon:cpu"), vec![Text("{icon:cpu")]);
        assert_eq!(icon_segments("{icon:}"), vec![Icon("")]);
        assert_eq!(icon_segments(""), Vec::new());
    }

    #[test]
    fn icon_bytes_resolves_names_case_insensitively() {
        assert!(icon_bytes("cpu").is_some());
        assert!(icon_bytes("CPU").is_some());
        assert!(icon_bytes("Heart").is_some());
        assert!(icon_bytes("memory-stick").is_some());
        assert!(icon_bytes("memory_stick").is_some());
        assert!(icon_bytes("MemoryStick").is_some());
        assert!(icon_bytes("mem").is_some());
        assert!(icon_bytes("disk").is_some());
        // Full set, not just the hand-match: generated fallback.
        assert!(icon_bytes("bot").is_some());
        assert!(icon_bytes("Bot").is_some());
        assert!(icon_bytes("robot-vacuum").is_some());
        assert!(icon_bytes("house").is_some());
        assert!(icon_bytes("power").is_some());
        assert!(icon_bytes("nope").is_none());
        assert!(icon_bytes("").is_none());
    }

    /// Publish every service table from throwaway fixtures (mirrors
    /// what `render_lua_value` does from live `Plots`, including the
    /// nil `bar.output` for output-agnostic seeds).
    fn publish_test_services(lua: &mlua::Lua, sys: &sysinfo::System, gpu: Option<f32>) {
        let theme = crate::config::ThemeConfig::default();
        let outputs = std::collections::HashMap::new();
        let queue = std::collections::VecDeque::new();
        let toplevels = crate::services::ToplevelCache::default();
        let workspaces = crate::services::WorkspaceCache::default();
        let ctx = crate::services::ServiceCtx {
            sys,
            gpu,
            theme: &theme,
            outputs: &outputs,
            notifications: &queue,
            toplevels: &toplevels,
            workspaces: &workspaces,
        };
        crate::services::publish_all(&ctx, lua).expect("publish");
        crate::services::publish_bar(lua, "").expect("bar");
    }

    #[test]
    fn system_service_exposes_cpu_memory_and_gpu() {
        let lua = new_widget_lua().expect("sandbox");
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        publish_test_services(&lua, &sys, Some(42.0));
        let cpu: f32 = lua.load("return system.cpu_usage").eval().expect("eval");
        assert!(cpu >= 0.0);
        assert!(sys.total_memory() > 0);
        let count: u64 = lua.load("return system.cpu_count").eval().expect("eval");
        assert_eq!(count as usize, sys.cpus().len());
        let gpu: f32 = lua.load("return system.gpu_usage").eval().expect("eval");
        assert_eq!(gpu, 42.0);
        // Missing GPUs read as nil, not an error.
        publish_test_services(&lua, &sys, None);
        let nil: Value = lua.load("return system.gpu_usage").eval().expect("eval");
        assert!(matches!(nil, Value::Nil));
    }

    #[test]
    fn slot_gaps_clamp_to_range() {
        use crate::config::TopConfig;
        let cfg = TopConfig {
            slot_padding: -5.0,
            slot_spacing: 500.0,
            ..Default::default()
        };
        let local = TopLocal::from(&cfg);
        assert_eq!(local.slot_padding, 0.0);
        assert_eq!(local.slot_spacing, TopLocal::MAX_SLOT_GAP);
    }

    #[test]
    fn slot_widgets_normalize_flat_nested_and_none() {
        use crate::config::{SlotWidgets, TopConfig};
        let cfg = TopConfig {
            slots: 3,
            widgets: vec![
                SlotWidgets::One("clock".to_string()),
                SlotWidgets::One("none".to_string()),
                SlotWidgets::Many(vec![
                    "cpu".to_string(),
                    "none".to_string(),
                    "ram".to_string(),
                ]),
            ],
            ..Default::default()
        };
        let local = TopLocal::from(&cfg);
        assert_eq!(
            local.widgets,
            vec![
                vec!["clock".to_string()],
                Vec::<String>::new(),
                vec!["cpu".to_string(), "ram".to_string()],
            ]
        );
    }

    fn node_has_icon(node: &WidgetNode) -> bool {
        match node {
            WidgetNode::Icon { .. } => true,
            WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => {
                children.iter().any(node_has_icon)
            }
            _ => false,
        }
    }

    #[test]
    fn seed_usage_scripts_render_icon_free_text() {
        use crate::config::{SEED_CPU_LUA, SEED_GPU_LUA, SEED_RAM_LUA};
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        for source in [SEED_CPU_LUA, SEED_RAM_LUA, SEED_GPU_LUA] {
            let lua = new_widget_lua().expect("sandbox");
            load_widget_script(&lua, "seed", source).expect("load");
            publish_test_services(&lua, &sys, None);
            let value = call_lua_value(&lua, "view").expect("view");
            let node = parse_node(&value).expect("parse");
            assert!(!node_has_icon(&node), "usage seeds stay icon-free");
        }
    }

    /// Seeds render through the full pipeline: parse plus build.
    /// Off-compositor services (Hyprland socket, outputs) degrade to
    /// empty tables, so every seed renders — including workspaces ("--").
    #[test]
    fn seed_scripts_parse_and_build() {
        use crate::config::{
            SEED_CLINEPASS_LUA, SEED_CLOCK_LUA, SEED_CPU_LUA, SEED_GPU_LUA, SEED_HELLO_LUA,
            SEED_RAM_LUA, SEED_STATS_LUA, SEED_SYSTEM_LUA, SEED_WORKSPACES_LUA,
        };
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        // Services publish the real theme palette (same as prod); no
        // stub needed.
        for source in [
            SEED_CLOCK_LUA,
            SEED_HELLO_LUA,
            SEED_STATS_LUA,
            SEED_CPU_LUA,
            SEED_RAM_LUA,
            SEED_GPU_LUA,
            SEED_WORKSPACES_LUA,
            SEED_CLINEPASS_LUA,
            SEED_SYSTEM_LUA,
        ] {
            let lua = new_widget_lua().expect("sandbox");
            load_widget_script(&lua, "seed", source).expect("load");
            publish_test_services(&lua, &sys, None);
            let value = call_lua_value(&lua, "view").expect("view");
            let node = parse_node(&value).expect("parse");
            let _ = build_node(&node, 13.0, None).expect("builds");
        }
        // Blank-key clinepass renders the connect hint popup (card tree).
        let lua = new_widget_lua().expect("sandbox");
        load_seed_components(&lua);
        load_widget_script(&lua, "clinepass", SEED_CLINEPASS_LUA).expect("load");
        publish_test_services(&lua, &sys, None);
        let popup = call_lua_value(&lua, "popup").expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("popup parses");
        assert!(content.tree.is_some());
        // System seed: power cell plus a card with a four-row session menu.
        let lua = new_widget_lua().expect("sandbox");
        load_seed_components(&lua);
        load_widget_script(&lua, "system", SEED_SYSTEM_LUA).expect("load");
        let popup = call_lua_value(&lua, "popup").expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("popup parses");
        // Menu component now owns the rows (no popup `items` shorthand).
        assert!(content.items.is_empty());
        fn button_actions(node: &WidgetNode, out: &mut Vec<String>) {
            match node {
                WidgetNode::ListView { items, .. } => {
                    for (_, child) in items {
                        button_actions(child, out);
                    }
                }
                WidgetNode::Button { action, .. } => out.push(action.clone()),
                WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => {
                    for child in children {
                        button_actions(child, out);
                    }
                }
                _ => {}
            }
        }
        let mut actions = Vec::new();
        if let Some(tree) = &content.tree {
            button_actions(tree, &mut actions);
        }
        assert_eq!(actions, vec!["suspend", "hibernate", "reboot", "poweroff"]);
    }

    #[test]
    fn system_menu_popup_is_sized_to_fit_all_rows() {
        use crate::config::SEED_SYSTEM_LUA;
        let lua = new_widget_lua().expect("sandbox");
        load_seed_components(&lua);
        load_widget_script(&lua, "system", SEED_SYSTEM_LUA).expect("load");
        let popup = call_lua_value(&lua, "popup").expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("parse");
        let tree = content.tree.as_ref().expect("card tree");
        // The card embeds a four-row menu; the window must be tall enough
        // to show every row instead of clipping to a fixed row count.
        let (_, height) = crate::app::layers::Popup::content_size(1920.0, 1080.0, &content);
        let needed = estimate_height(tree, 13.0);
        assert!(
            height as f32 >= needed,
            "popup {height}px clips {needed}px of content"
        );
        assert!(height > 150, "expected room for four rows, got {height}");
    }

    #[test]
    fn system_seed_whitelists_systemctl_actions() {
        use crate::config::SEED_SYSTEM_LUA;
        // Record os.execute calls instead of running systemctl.
        let lua = new_widget_lua().expect("sandbox");
        let os: mlua::Table = lua.globals().get("os").expect("os");
        os.set(
            "execute",
            lua.create_function(|lua, cmd: String| {
                let seen: mlua::Table = lua.globals().get("_seen").expect("seen");
                seen.set(seen.len().unwrap_or(0) + 1, cmd)?;
                Ok(true)
            })
            .expect("exec"),
        )
        .expect("set execute");
        lua.globals()
            .set("_seen", lua.create_table().expect("table"))
            .expect("seen");
        load_widget_script(&lua, "system", SEED_SYSTEM_LUA).expect("load");
        call_lua_named_action(&lua, "suspend").expect("action");
        // A hostile key never reaches the shell.
        call_lua_named_action(&lua, "x; rm -rf ~").expect("action");
        let seen: mlua::Table = lua.globals().get("_seen").expect("seen");
        assert_eq!(seen.len().unwrap_or(0), 1);
        let first: String = seen.get(1).expect("first");
        assert_eq!(first, "systemctl suspend");
    }

    #[test]
    fn clinepass_popup_parses_real_usage_shape() {
        use crate::config::SEED_CLINEPASS_LUA;
        // Shape from the live endpoint (values redacted): three
        // percentUsed bars, one per window. render() fills the cache
        // first (popup reads _usage, never curls directly).
        let body = r#"{"data":{"limits":[{"type":"five_hour","percentUsed":14,"resetsAt":"2026-10-03T20:48:05Z"},{"type":"weekly","percentUsed":5,"resetsAt":"2026-10-10T15:48:05Z"},{"type":"monthly","percentUsed":2,"resetsAt":"2026-11-02T15:48:05Z"}]},"success":true}"#;
        let lua = new_widget_lua().expect("sandbox");
        load_seed_components(&lua);
        // Main-branch popup tints through theme.* (republished live).
        let theme = lua.create_table().expect("theme");
        theme.set("primary", "#00ff00").expect("set");
        lua.globals().set("theme", theme).expect("theme");
        // Stub io.popen: return the canned body regardless of command.
        // Method-call form: handle:read("*a") passes the handle as
        // first arg, mode second — accept both.
        let body_owned = body.to_string();
        let io: mlua::Table = lua.globals().get("io").expect("io");
        io.set(
            "popen",
            lua.create_function(move |lua, _: String| {
                let h = lua.create_table().expect("handle");
                let b = body_owned.clone();
                h.set(
                    "read",
                    lua.create_function(move |_, (handle, mode): (mlua::Value, mlua::Value)| {
                        let _ = (&handle, &mode);
                        Ok(b.clone())
                    })
                    .expect("read"),
                )
                .expect("set read");
                h.set(
                    "close",
                    lua.create_function(|_, _: mlua::Value| Ok(true))
                        .expect("close"),
                )
                .expect("set close");
                Ok(h)
            })
            .expect("popen"),
        )
        .expect("set popen");
        // Key must be non-empty or popup() takes the hint branch.
        load_widget_script(
            &lua,
            "clinepass",
            &SEED_CLINEPASS_LUA.replace(r#"local API_KEY = """#, r#"local API_KEY = "x""#),
        )
        .expect("load");
        // First popup (cold cache) is the spinner card, not the data:
        // card column is [head, separator, body].
        let popup = call_lua_value(&lua, "popup").expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("popup parses");
        let tree = content.tree.expect("spinner tree");
        assert!(matches!(
            tree,
            WidgetNode::Column { children, .. } if children.len() == 3
        ));
        // render() fetches into _usage; second popup shows the rows.
        call_lua_value(&lua, "view").expect("view");
        let popup = call_lua_value(&lua, "popup").expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("popup parses");
        let tree = content.tree.expect("usage tree");
        let built = build_node(&tree, 13.0, None).expect("builds");
        let _ = built;
        // Card column is [head, separator, body]: the body holds one
        // label/bar/pct row per window.
        match tree {
            WidgetNode::Column { children, .. } => {
                assert_eq!(children.len(), 3);
                let Some(WidgetNode::Column { children: rows, .. }) = children.get(2) else {
                    panic!("expected body column, got {children:?}");
                };
                assert_eq!(rows.len(), 3);
                for row in rows {
                    assert!(matches!(
                        row,
                        WidgetNode::Row { children, .. } if children.len() == 3
                    ));
                }
            }
            other => panic!("expected column, got {other:?}"),
        }
    }

    #[test]
    fn workspaces_seed_filters_bar_output_and_dispatches_actual_name() {
        use crate::config::SEED_WORKSPACES_LUA;
        use crate::services::{Toplevel, ToplevelCache, Workspace, WorkspaceCache};
        let lua = new_widget_lua().expect("sandbox");
        // Feed the seed through the real `wayland` service from a
        // canned native snapshot (no compositor needed).
        let sys = sysinfo::System::new();
        let theme = crate::config::ThemeConfig::default();
        let outputs = std::collections::HashMap::new();
        let queue = std::collections::VecDeque::new();
        let workspaces = WorkspaceCache::from_rows(vec![
            Workspace {
                name: "web".to_string(),
                monitor: "".to_string(),
                active: false,
                rects: vec![],
            },
            Workspace {
                name: "code".to_string(),
                monitor: "DP-1".to_string(),
                active: true,
                rects: vec![[0.0, 0.0, 2560.0, 1440.0]],
            },
            Workspace {
                name: "mail".to_string(),
                monitor: "HDMI-1".to_string(),
                active: false,
                rects: vec![[2560.0, 0.0, 1920.0, 1080.0]],
            },
        ]);
        let toplevels = ToplevelCache::from_rows(vec![
            Toplevel {
                app_id: "foot".to_string(),
                title: "shell".to_string(),
            },
            Toplevel {
                app_id: "firefox".to_string(),
                title: "".to_string(),
            },
        ]);
        let ctx = crate::services::ServiceCtx {
            sys: &sys,
            gpu: None,
            theme: &theme,
            outputs: &outputs,
            notifications: &queue,
            toplevels: &toplevels,
            workspaces: &workspaces,
        };
        crate::services::publish_all(&ctx, &lua).expect("publish");
        // This bar lives on DP-1 (`OutputInfo` needs a live
        // compositor; the service's own shape test covers the outputs
        // table itself).
        crate::services::publish_bar(&lua, "DP-1").expect("bar");
        load_widget_script(&lua, "workspaces", SEED_WORKSPACES_LUA).expect("load");
        // Bar is a horizontal strip over the local output: `code`
        // passes by monitor; `web` (unassigned) and `mail` (HDMI-1)
        // remain excluded from this strip.
        let value = call_lua_value(&lua, "view").expect("view");
        let node = parse_node(&value).expect("parse");
        match node {
            WidgetNode::ListView {
                id,
                horizontal,
                items,
                ..
            } => {
                assert_eq!(id, "wayland-strip");
                assert!(horizontal, "strip runs along the bar");
                let labels: Vec<&str> = items
                    .iter()
                    .map(|(_, item)| match item {
                        WidgetNode::Button { label, action, .. } => {
                            assert_eq!(action, "code");
                            label.as_str()
                        }
                        other => panic!("expected button, got {other:?}"),
                    })
                    .collect();
                assert_eq!(labels, ["1"]);
            }
            other => panic!("expected listview, got {other:?}"),
        }
        // Record the action rather than launching hyprctl in tests.
        let os: mlua::Table = lua.globals().get("os").expect("os");
        os.set(
            "execute",
            lua.create_function(|lua, command: String| lua.globals().set("dispatched", command))
                .expect("execute"),
        )
        .expect("stub");
        call_lua_named_action(&lua, "1001").expect("action");
        let command: String = lua.globals().get("dispatched").expect("command");
        assert_eq!(
            command,
            "hyprctl dispatch 'hl.dsp.focus({workspace = 1001})' >/dev/null 2>&1"
        );
    }

    #[test]
    fn ui_spinner_parses_and_builds() {
        let lua = new_widget_lua().expect("sandbox");
        let value: mlua::Value = lua.load("return ui.spinner()").eval().expect("eval");
        let node = parse_node(&value).expect("parse");
        assert_eq!(node, WidgetNode::Spinner);
        let _ = build_node(&node, 13.0, None).expect("builds");
        // Nests like any node.
        let value: mlua::Value = lua
            .load("return ui.row({ ui.spinner(), ui.text(\"x\") })")
            .eval()
            .expect("eval");
        let node = parse_node(&value).expect("parse");
        assert!(matches!(
            node,
            WidgetNode::Row { children, .. } if children.len() == 2
        ));
    }

    #[test]
    fn ui_with_chains_like_iced_builders() {
        // Iced-spelled setters: node:width(v) sets + returns self.
        // Wrong-type calls error loudly at eval, naming the type.
        let lua = new_widget_lua().expect("sandbox");
        let value: mlua::Value = lua
            .load(r#"return ui.progress(0.5):width(200)"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&value).expect("parse"),
            WidgetNode::Progress {
                value: 0.5,
                width: NodeLength::Fixed(200.0),
                height: None,
            }
        );
        let value: mlua::Value = lua
            .load(r#"return ui.text("hi"):size(14)"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&value).expect("parse"),
            WidgetNode::Text {
                content: "hi".to_string(),
                size: Some(14.0),
                width: None,
                height: None,
                color: None,
            }
        );
        // Multi-chain + nesting.
        let value: mlua::Value = lua
            .load(r#"return ui.button("go", "run"):width(120):padding(4)"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&value).expect("parse"),
            WidgetNode::Button {
                label: "go".to_string(),
                action: "run".to_string(),
                width: Some(NodeLength::Fixed(120.0)),
                height: None,
                padding: Some(4.0),
                color: None,
            }
        );
        // Chained setter AFTER a failing pcall in the same state:
        // setmetatable caching means the metatable must survive
        // error paths (regression probe).
        let value: mlua::Value = lua
            .load(r#"return ui.row({ ui.text("x") }):spacing(8)"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&value).expect("parse"),
            WidgetNode::Row {
                children: vec![WidgetNode::Text {
                    content: "x".to_string(),
                    size: None,
                    width: None,
                    height: None,
                    color: None,
                }],
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                spacing: 8.0,
            }
        );
        // Wrong-type call errors at eval, naming the type. Uses a
        // pcall-contained failures don't poison the state (probed).
        // NOTE: the raw method-miss error surfaces here, not the
        // setter message — the setter never runs (method nil at
        // lookup). assert(not ok) inside proves it errored.
        // (:padding() is button-only, so progress rejects it.)
        let ok: bool = lua
            .load(
                r#"local ok, _ = pcall(function() return ui.progress(0.5):padding(4) end) return ok"#,
            )
            .eval()
            .expect("eval");
        assert!(!ok, "padding on progress must fail");
        // Wrong-typed chain value errors at parse with the field name.
        let value: mlua::Value = lua
            .load(r#"return ui.progress(0.5):width("wide")"#)
            .eval()
            .expect("eval");
        assert!(parse_node(&value).is_err());
        // :height() on all three height-bearing types.
        let value: mlua::Value = lua
            .load(r#"return ui.progress(0.5):width(200):height(12)"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&value).expect("parse"),
            WidgetNode::Progress {
                value: 0.5,
                width: NodeLength::Fixed(200.0),
                height: Some(NodeLength::Fixed(12.0)),
            }
        );
        let value: mlua::Value = lua
            .load(r#"return ui.button("go", "run"):height(36)"#)
            .eval()
            .expect("eval");
        assert!(matches!(
            parse_node(&value).expect("parse"),
            WidgetNode::Button {
                height: Some(_),
                ..
            }
        ));
        let value: mlua::Value = lua
            .load(r#"return ui.text("hi"):height(20)"#)
            .eval()
            .expect("eval");
        assert!(matches!(
            parse_node(&value).expect("parse"),
            WidgetNode::Text {
                height: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn ui_length_strings_map_to_fill_and_shrink() {
        // Numbers stay px (backward-compat); "fill"/"shrink" map to
        // iced Length modes; anything else errors naming the field.
        let lua = new_widget_lua().expect("sandbox");
        let value: mlua::Value = lua
            .load(r#"return ui.progress(0.5):width("fill")"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&value).expect("parse"),
            WidgetNode::Progress {
                value: 0.5,
                width: NodeLength::Fill,
                height: None,
            }
        );
        // Case-insensitive; chains on row/text too.
        let value: mlua::Value = lua
            .load(r#"return ui.row({ ui.text("x") }):width("FILL"):height("shrink")"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&value).expect("parse"),
            WidgetNode::Row {
                children: vec![WidgetNode::Text {
                    content: "x".to_string(),
                    size: None,
                    width: None,
                    height: None,
                    color: None,
                }],
                width: NodeLength::Fill,
                height: NodeLength::Shrink,
                spacing: 4.0,
            }
        );
        let value: mlua::Value = lua
            .load(r#"return ui.text("hi"):width("fill")"#)
            .eval()
            .expect("eval");
        assert!(matches!(
            parse_node(&value).expect("parse"),
            WidgetNode::Text {
                width: Some(NodeLength::Fill),
                ..
            }
        ));
        // Unknown mode errors at parse with the field name.
        let value: mlua::Value = lua
            .load(r#"return ui.progress(0.5):width("huge")"#)
            .eval()
            .expect("eval");
        assert!(parse_node(&value).is_err());
        // Defaults unchanged: unset row/col shrink, progress 120px.
        let value: mlua::Value = lua
            .load(r#"return ui.row({ ui.text("x") })"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&value).expect("parse"),
            WidgetNode::Row {
                children: vec![WidgetNode::Text {
                    content: "x".to_string(),
                    size: None,
                    width: None,
                    height: None,
                    color: None,
                }],
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                spacing: 4.0,
            }
        );
        // Fill builds without a renderer.
        let fill = WidgetNode::Progress {
            value: 0.5,
            width: NodeLength::Fill,
            height: Some(NodeLength::Fill),
        };
        let _ = build_node(&fill, 13.0, None).expect("builds");
    }

    #[test]
    fn slot_align_rides_the_long_axis() {
        use iced::Alignment as A;
        assert_eq!(SlotAlign::Start.for_bar(true), (A::Start, A::Center));
        assert_eq!(SlotAlign::End.for_bar(true), (A::End, A::Center));
        assert_eq!(SlotAlign::Center.for_bar(true), (A::Center, A::Center));
        assert_eq!(SlotAlign::Start.for_bar(false), (A::Center, A::Start));
        assert_eq!(SlotAlign::End.for_bar(false), (A::Center, A::End));
        assert_eq!(SlotAlign::Center.for_bar(false), (A::Center, A::Center));
    }

    #[test]
    fn lua_popup_and_press_contract() {
        // popup() renders the menu body.
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(
            &lua,
            "test",
            "return { view=function() return 'x' end, popup=function() return 'menu' end }",
        )
        .expect("load");
        assert_eq!(call_lua_text(&lua, "popup").unwrap(), "menu");
        // on_press() mutates script state; the next render reflects it.
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(
            &lua,
            "test",
            "return {flag=false, view=function(self) return self.flag and 1 or 0 end, on_press=function(self) self.flag=true end}",
        )
        .expect("load");
        call_lua_action(&lua).expect("action");
        assert_eq!(call_lua_text(&lua, "view").unwrap(), "1");
        // Missing functions are absent, never errors at lookup.
        let mut states = HashMap::new();
        states.insert("w".to_string(), new_widget_lua().expect("sandbox"));
        load_widget_script(&states["w"], "w", "return {view=function() return 'x' end}")
            .expect("load");
        assert!(!lua_has_func(&states, "w", "popup"));
        assert!(!lua_has_func(&states, "w", "on_press"));
        assert!(!lua_has_func(&states, "missing", "view"));
        assert!(call_lua_action(&states["w"]).is_err());
    }

    #[test]
    fn widget_script_path_defaults_to_name_lua() {
        use crate::config::WidgetDef;
        let bare = WidgetDef {
            name: "clock".to_string(),
            ..Default::default()
        };
        assert_eq!(
            Top::widget_script_path(&bare),
            crate::config::widgets_dir().join("clock.lua")
        );
        let relative = WidgetDef {
            name: "x".to_string(),
            file: "sub/y.lua".to_string(),
            ..Default::default()
        };
        assert_eq!(
            Top::widget_script_path(&relative),
            crate::config::widgets_dir().join("sub/y.lua")
        );
        let absolute = WidgetDef {
            name: "x".to_string(),
            file: "/tmp/abs.lua".to_string(),
            ..Default::default()
        };
        assert_eq!(
            Top::widget_script_path(&absolute),
            std::path::PathBuf::from("/tmp/abs.lua")
        );
    }

    #[test]
    fn lua_popup_content_parses_string_and_table_forms() {
        use crate::app::layers::Popup;
        use crate::app::layers::top::WidgetNode;
        // Plain strings are just text with defaults.
        let lua = new_widget_lua().expect("sandbox");
        let value: Value = lua.load(r#"return "hi""#).eval().expect("eval");
        let content = Popup::parse_popup_content(value).unwrap();
        assert_eq!(content.text, "hi");
        assert!(content.width.is_none() && content.items.is_empty());
        // Tables carry text/size/items (scalars coerce like render).
        let value: Value = lua
            .load(
                r#"return { text = "head", width = 300, height = 200,
                    items = { { label = "a", action = "go" }, { label = 7, action = "n" } } }"#,
            )
            .eval()
            .expect("eval");
        let content = Popup::parse_popup_content(value).unwrap();
        assert_eq!(content.text, "head");
        assert_eq!((content.width, content.height), (Some(300.0), Some(200.0)));
        assert_eq!(content.items.len(), 2);
        assert_eq!(content.items[0].action, "go");
        assert_eq!(content.items[1].label, "7");
        // Malformed items are an error, not a silent empty menu.
        let value: Value = lua
            .load(r#"return { items = "nope" }"#)
            .eval()
            .expect("eval");
        assert!(Popup::parse_popup_content(value).is_err());
        // A ui tree parses into the composed body.
        let value: Value = lua
            .load(r#"return { ui = ui.row({ ui.icon("cpu"), ui.text("x") }) }"#)
            .eval()
            .expect("eval");
        let content = Popup::parse_popup_content(value).unwrap();
        assert!(content.text.is_empty() && content.items.is_empty());
        let tree = content.tree.expect("tree");
        assert!(matches!(tree, WidgetNode::Row { .. }));
        // A ui.button inside the body keeps its key (routes like an
        // item row once the popup view wires it up).
        let value: Value = lua
            .load(r#"return { ui = ui.button("Reboot", "reboot") }"#)
            .eval()
            .expect("eval");
        let content = Popup::parse_popup_content(value).unwrap();
        assert!(matches!(
            content.tree,
            Some(WidgetNode::Button { action, .. }) if action == "reboot"
        ));
    }

    #[test]
    fn lua_on_action_receives_the_item_key() {
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(
            &lua,
            "test",
            "seen={}; return {view=function() return '' end, on_action=function(self,name) seen[#seen+1]=name end}",
        )
        .expect("load");
        call_lua_named_action(&lua, "toggle").expect("call");
        let seen: String = lua.load("return seen[1]").eval().expect("eval");
        assert_eq!(seen, "toggle");
        // Missing on_action is a silent no-op (plain-text widgets
        // coexist with cell buttons without erroring).
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(&lua, "plain", "return {view=function() return 'x' end}").expect("load");
        call_lua_named_action(&lua, "ws:1").expect("noop");
    }

    #[test]
    fn rich_text_builds_without_a_renderer() {
        // Element construction is pure — smoke-test all three shapes.
        let _ = rich_text("12%".to_string(), 13.0, 4.0);
        let _ = rich_text("{icon:cpu} 12%".to_string(), 13.0, 4.0);
        let _ = rich_text("{icon:nope}".to_string(), 13.0, 4.0);
    }

    #[test]
    fn ui_constructors_build_description_tables() {
        let lua = new_widget_lua().expect("sandbox");
        let node: Table = lua
            .load(r#"return ui.row({ ui.icon("cpu"), ui.text("42%") }, 8)"#)
            .eval()
            .expect("eval");
        assert_eq!(node.get::<String>("type").expect("type"), "row".to_string());
        assert_eq!(
            node_property(&node, "spacing").expect("spacing"),
            Value::Number(8.0)
        );
        let kids: Vec<Table> = node
            .get::<Table>("children")
            .expect("children")
            .sequence_values()
            .collect::<Result<_, _>>()
            .expect("sequence");
        assert_eq!(kids.len(), 2);
        // Omitted spacing defaults at parse time, not construction.
        let bare: Table = lua
            .load(r#"return ui.column({ ui.progress(0.5) })"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&Value::Table(bare)).expect("parse"),
            WidgetNode::Column {
                children: vec![WidgetNode::Progress {
                    value: 0.5,
                    width: NodeLength::Fixed(120.0),
                    height: None,
                }],
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                spacing: 4.0,
            }
        );
        // Optional width second arg; malformed width errors.
        let wide: Table = lua
            .load(r#"return ui.progress(0.5, 200)"#)
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&Value::Table(wide)).expect("parse"),
            WidgetNode::Progress {
                value: 0.5,
                width: NodeLength::Fixed(200.0),
                height: None,
            }
        );
        let bad: Table = lua
            .load(r#"return ui.progress(0.5, 'wide')"#)
            .eval()
            .expect("eval");
        assert!(parse_node(&Value::Table(bad)).is_err());
    }

    #[test]
    fn listview_delegate_accepts_mixed_nodes_and_components() {
        let lua = new_widget_lua().expect("sandbox");
        lua.load(
            r#"iced.define('wsbtn', function(p) return iced.button(p.label, p.action):width(120) end)"#,
        )
        .exec()
        .expect("define");
        let value: Value = lua
            .load(
                r#"return ui.listview({{key='a',label='x'},{key='b',label='y'}}):id('s'):key('key')
                    :delegate(function(item)
                        if item.key == 'a' then
                            return ui.text(item.label)
                        else
                            return ui.wsbtn({label=item.label, action='go'})
                        end
                    end)"#,
            )
            .eval()
            .expect("eval");
        let node = parse_node(&value).expect("parse");
        match node {
            WidgetNode::ListView { items, .. } => {
                assert_eq!(items.len(), 2);
                assert!(matches!(items[0].1, WidgetNode::Text { .. }));
                assert!(matches!(
                    items[1].1,
                    WidgetNode::Button { ref label, ref action, .. }
                    if label == "y" && action == "go"
                ));
            }
            other => panic!("expected listview, got {other:?}"),
        }
    }

    #[test]
    fn lua_setters_are_repeatable_and_components_are_directly_callable() {
        let lua = empty_widget_lua().expect("lua");
        inject_ui_base(&lua).expect("constructors");
        lua.load(r#"iced.define('label', function(p) return iced.text(p.text) end)"#)
            .exec()
            .unwrap();
        let value: Value = lua.load(r##"return iced.label({text='hello'}):width(10):width(30):color('#f00'):color('#0f0')"##).eval().unwrap();
        assert!(matches!(parse_node(&value).unwrap(), WidgetNode::Text {
            width: Some(NodeLength::Fixed(30.0)), color: Some(color), ..
        } if color.g == 1.0 && color.r == 0.0));
        let value: Value = lua
            .load("return iced.row({}, 4):spacing(8):spacing(12)")
            .eval()
            .unwrap();
        assert!(matches!(
            parse_node(&value).unwrap(),
            WidgetNode::Row { spacing: 12.0, .. }
        ));
        assert!(
            lua.load("iced.define('text', function() return iced.text('x') end)")
                .exec()
                .is_err()
        );
    }

    #[test]
    fn lua_listview_delegates_have_stable_keys_and_local_transitions() {
        let lua = empty_widget_lua().unwrap();
        inject_ui_base(&lua).unwrap();
        let value: Value = lua.load(r#"
            return iced.scrollable(iced.listview({{id=1,title='one'}, {id=2,title='two'}})
                :id('center'):key('id'):pitch(108):spacing(8)
                :delegate(function(n) return iced.container(iced.text(n.title)):padding(10):radius(6) end)
                :onEntered({x={from=200,to=0},opacity={from=0,to=1},duration=250})
                :onExit({x={to=-200},opacity={to=0},duration=250})
                :onDisplaced({duration=250}))
        "#).eval().unwrap();
        let tree = parse_node(&value).unwrap();
        let WidgetNode::Scrollable { child, .. } = &tree else {
            panic!("scrollable");
        };
        let WidgetNode::ListView {
            items,
            transitions,
            pitch,
            ..
        } = &**child
        else {
            panic!("list");
        };
        assert_eq!(
            items
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2"]
        );
        assert_eq!(*pitch, 108.0);
        assert_eq!(transitions.0.from.x, 200.0);
        assert_eq!(transitions.1.to.x, -200.0);
        assert_eq!(transitions.2.duration, Some(Duration::from_millis(250)));
        let mut rt = aura_anim::core::runtime::MotionRuntime::new();
        let mut lists = WidgetLists::new();
        let list = super::super::listview::ListView::new(108.0);
        lists.insert("clock/center".into(), list);
        build_with_lists(&tree, "clock", 13.0, None, &rt, &lists).unwrap();
        for list in lists.values_mut() {
            list.clear_all(&mut rt);
        }
        assert_eq!(rt.motion_count(), 0);
    }

    #[test]
    fn parse_node_reads_full_trees_and_rejects_junk() {
        let lua = new_widget_lua().expect("sandbox");
        let value: Value = lua
            .load(
                r#"return ui.row({ ui.text("hi"), ui.button("go", "run"), 7, { type = "icon", name = "cpu" } })"#,
            )
            .eval()
            .expect("eval");
        assert_eq!(
            parse_node(&value).expect("parse"),
            WidgetNode::Row {
                children: vec![
                    WidgetNode::Text {
                        content: "hi".to_string(),
                        size: None,
                        width: None,
                        height: None,
                        color: None,
                    },
                    WidgetNode::Button {
                        label: "go".to_string(),
                        action: "run".to_string(),
                        width: None,
                        height: None,
                        padding: None,
                        color: None,
                    },
                    WidgetNode::Text {
                        content: "7".to_string(),
                        size: None,
                        width: None,
                        height: None,
                        color: None,
                    },
                    WidgetNode::Icon {
                        name: "cpu".to_string(),
                        color: None,
                    },
                ],
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                spacing: 4.0,
            }
        );
        for bad in ["return {}", "return { type = 'nope' }"] {
            let value: Value = lua.load(bad).eval().expect("eval");
            assert!(parse_node(&value).is_err(), "rejects {bad}");
        }
        // Wrong-typed constructor args fail at eval time instead
        // (missing ones default: action "" is inert).
        assert!(lua.load("return ui.row('flat')").eval::<Value>().is_err());
    }

    #[test]
    fn build_node_builds_every_primitive_without_a_renderer() {
        let size = 13.0;
        let no_msg: Option<&dyn Fn(String) -> Plant> = None;
        for node in [
            WidgetNode::Text {
                content: "hi".to_string(),
                size: None,
                width: None,
                height: None,
                color: None,
            },
            WidgetNode::Icon {
                name: "cpu".to_string(),
                color: None,
            },
            WidgetNode::Icon {
                name: "typo".to_string(),
                color: None,
            },
            WidgetNode::Row {
                children: vec![WidgetNode::Text {
                    content: "a".to_string(),
                    size: None,
                    width: None,
                    height: None,
                    color: None,
                }],
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                spacing: 2.0,
            },
            WidgetNode::Column {
                children: vec![],
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                spacing: 2.0,
            },
            WidgetNode::Button {
                label: "go".to_string(),
                action: "run".to_string(),
                width: None,
                height: None,
                padding: None,
                color: None,
            },
            WidgetNode::Progress {
                value: 1.5,
                width: NodeLength::Fixed(120.0),
                height: None,
            },
            WidgetNode::Separator { height: 2.0 },
        ] {
            let _ = build_node(&node, size, no_msg).expect("builds");
        }
        // Buttons carry their action into the message when asked.
        let node = WidgetNode::Button {
            label: "go".to_string(),
            action: "run".to_string(),
            width: None,
            height: None,
            padding: None,
            color: None,
        };
        let _ = build_node(&node, size, Some(&|_| Plant::Tend)).expect("builds");
    }

    #[test]
    fn separator_parses_defaults_and_chains() {
        let lua = new_widget_lua().expect("sandbox");
        // `iced` is the canonical table; bare `ui` is the same table.
        let same: bool = lua.load("return ui == iced").eval().expect("alias");
        assert!(same);
        for (src, height) in [
            ("return iced.separator()", 1.0),
            ("return iced.separator():height(3)", 3.0),
            ("return ui.separator()", 1.0),
        ] {
            let value: Value = lua.load(src).eval().expect("eval");
            let node = parse_node(&value).expect("parse");
            assert_eq!(node, WidgetNode::Separator { height }, "{src}");
        }
    }

    #[test]
    fn iced_define_and_use_round_trip_with_chaining() {
        let lua = new_widget_lua().expect("sandbox");
        lua.load(
            r#"
            iced.define("__t_stat", function(props)
                return iced.row({ iced.icon(props.icon), iced.text(props.value) })
            end)
            "#,
        )
        .exec()
        .expect("define");
        // Chaining survives `use` (component returns a constructor value).
        let value: Value = lua
            .load(r#"return iced.use("__t_stat", { icon = "cpu", value = "42%" }):width("fill")"#)
            .eval()
            .expect("use");
        match parse_node(&value).expect("parse") {
            WidgetNode::Row {
                width: NodeLength::Fill,
                children,
                ..
            } => assert_eq!(children.len(), 2),
            other => panic!("unexpected {other:?}"),
        }
        // Unknown names and non-node returns error naming the component.
        let err = lua
            .load(r#"return iced.use("__t_missing", {})"#)
            .eval::<Value>()
            .expect_err("missing");
        assert!(err.to_string().contains("__t_missing"), "{err}");
        lua.load(r#"iced.define("__t_bad", function() return 42 end)"#)
            .exec()
            .expect("define bad");
        let err = lua
            .load(r#"return iced.use("__t_bad", {})"#)
            .eval::<Value>()
            .expect_err("bad");
        assert!(err.to_string().contains("__t_bad"), "{err}");
    }

    #[test]
    fn color_setter_accepts_hex_and_rgba_tables() {
        let lua = new_widget_lua().expect("sandbox");
        let red = iced::Color::from_rgb(1.0, 0.0, 0.0);
        for (src, expect) in [
            (r##"return iced.text("hi"):color("#ff0000")"##, red),
            (r##"return iced.text("hi"):color("#f00")"##, red),
            (
                r#"return iced.text("hi"):color({ r = 1, g = 0, b = 0 })"#,
                red,
            ),
            (
                r#"return iced.text("hi"):color({ 1, 0, 0, 0.5 })"#,
                iced::Color::from_rgba(1.0, 0.0, 0.0, 0.5),
            ),
        ] {
            let value: Value = lua.load(src).eval().expect("eval");
            match parse_node(&value).expect("parse") {
                WidgetNode::Text { color: Some(c), .. } => {
                    assert!(
                        (c.r - expect.r).abs() < 0.01
                            && (c.g - expect.g).abs() < 0.01
                            && (c.b - expect.b).abs() < 0.01
                            && (c.a - expect.a).abs() < 0.01,
                        "{src} -> {c:?}"
                    );
                }
                other => panic!("{src} -> unexpected {other:?}"),
            }
        }
        // Unset stays themed; garbage names the field at parse time
        // (setters store, parse validates — same as sizes).
        let value: Value = lua.load(r#"return iced.text("hi")"#).eval().expect("eval");
        assert!(matches!(
            parse_node(&value).expect("parse"),
            WidgetNode::Text { color: None, .. }
        ));
        let value: Value = lua
            .load(r#"return iced.text("hi"):color("nope")"#)
            .eval()
            .expect("setter stores");
        let err = parse_node(&value).expect_err("bad hex");
        assert!(err.contains("color"), "{err}");
        // Buttons and icons take colors too; buttons keep the tinted style.
        let value: Value = lua
            .load(r##"return iced.button("go", "run"):color("#00ff00")"##)
            .eval()
            .expect("eval");
        let node = parse_node(&value).expect("parse");
        assert!(matches!(node, WidgetNode::Button { color: Some(_), .. }));
        let _ = build_node(&node, 13.0, None).expect("builds tinted");
        let value: Value = lua
            .load(r##"return iced.icon("cpu"):color("#00ff00")"##)
            .eval()
            .expect("eval");
        let node = parse_node(&value).expect("parse");
        assert!(matches!(node, WidgetNode::Icon { color: Some(_), .. }));
        let _ = build_node(&node, 13.0, None).expect("builds tinted");
    }

    #[test]
    fn seed_components_define_working_builders() {
        use crate::config::{SEED_COMPONENT_CARD, SEED_COMPONENT_DEFINE, SEED_COMPONENT_MENU};
        let lua = new_widget_lua().expect("sandbox");
        for source in [
            SEED_COMPONENT_DEFINE,
            SEED_COMPONENT_CARD,
            SEED_COMPONENT_MENU,
        ] {
            lua.load(source).exec().expect("seed loads");
        }
        // spacer from the docs file, card and menu from their files.
        for src in [
            r#"return iced.use("spacer", { h = 4 })"#,
            r#"return iced.use("card", { title = "T", body = "b" })"#,
            r#"return iced.use("menu", { items = { { label = "Go", action = "go" } } })"#,
        ] {
            let value: Value = lua.load(src).eval().expect("use");
            parse_node(&value).expect("parses");
        }
        // menu carries the action key through to a button.
        let value: Value = lua
            .load(r#"return iced.use("menu", { items = { { label = "Go", action = "go" } } })"#)
            .eval()
            .expect("menu");
        match parse_node(&value).expect("parse") {
            WidgetNode::ListView { items, .. } => {
                assert!(matches!(
                    &items[..],
                    [(_, WidgetNode::Button { action, .. })] if action == "go"
                ));
            }
            other => panic!("unexpected {other:?}"),
        }
        // card color prop tints the title text.
        let value: Value = lua
            .load(r##"return iced.use("card", { title = "T", color = "#ff0000" })"##)
            .eval()
            .expect("card color");
        match parse_node(&value).expect("parse") {
            WidgetNode::Column { children, .. } => {
                assert!(matches!(
                    &children[..],
                    [WidgetNode::Text { color: Some(_), .. }, _, _]
                ));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_transitions_reads_qml_subset_and_defaults() {
        use super::super::listview::Transition;
        let enter = Transition::slide_fade(16.0);
        let exit = Transition::slide_fade_out(-16.0);
        let displaced = Transition {
            from: crate::app::layers::anim::ItemMotion::settled(),
            to: crate::app::layers::anim::ItemMotion::settled(),
            duration: None,
        };
        // No transitions() at all: defaults pass through untouched.
        let lua = new_widget_lua().expect("sandbox");
        let (e, x, d, _) = parse_transitions(&lua, &enter, &exit, &displaced).expect("parse");
        assert_eq!(e.from.x, 16.0);
        assert_eq!(x.to.x, -16.0);
        assert!(d.duration.is_none());
        // Full spec overrides from/to/duration per slot.
        load_widget_script(&lua, "transitions",
            r#"
            local app = {view=function() return ui.text('x') end}
            function app:transitions()
                return {
                    add = { x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 },
                    remove = { x = { to = -200 }, duration = 250 },
                    displaced = { duration = 300 },
                }
            end
            return app
            "#,
        )
        .expect("load");
        let (e, x, d, custom) = parse_transitions(&lua, &enter, &exit, &displaced).expect("parse");
        assert!(custom, "a transitions() spec marks the list customized");
        assert_eq!((e.from.x, e.to.x, e.from.opacity), (200.0, 0.0, 0.0));
        assert_eq!(x.to.x, -200.0);
        assert_eq!(
            (e.duration, x.duration, d.duration),
            (
                Some(Duration::from_millis(250)),
                Some(Duration::from_millis(250)),
                Some(Duration::from_millis(300)),
            )
        );
        // Partial spec keeps unspecified fields at defaults (exit y is
        // untouched, add.to.x untouched here since only from given).
        load_widget_script(&lua, "transitions", r#"return {view=function() return '' end, transitions=function() return {add={x={from=50}}} end}"#)
            .expect("load");
        let (e, x, d, _) = parse_transitions(&lua, &enter, &exit, &displaced).expect("parse");
        assert_eq!(e.from.x, 50.0);
        assert_eq!(e.to.x, 0.0); // default settle
        assert_eq!(x.to.x, -16.0); // exit untouched
        assert!(d.duration.is_none());
        // Malformed specs error naming the slot; nil passes through.
        load_widget_script(&lua, "transitions", r#"return {view=function() return '' end, transitions=function() return {add={x='nope'}} end}"#)
            .expect("load");
        let err = parse_transitions(&lua, &enter, &exit, &displaced).expect_err("bad");
        assert!(err.contains("add"), "{err}");
        load_widget_script(
            &lua,
            "transitions",
            "return {view=function() return '' end, transitions=function() return nil end}",
        )
        .expect("load");
        assert!(parse_transitions(&lua, &enter, &exit, &displaced).is_ok());
    }

    #[test]
    fn notify_center_seed_reads_queue_and_dismisses() {
        use crate::config::SEED_NOTIFY_CENTER_LUA;
        let lua = new_widget_lua().expect("sandbox");
        load_seed_components(&lua);
        load_widget_script(&lua, "notifycenter", SEED_NOTIFY_CENTER_LUA).expect("load");
        // Empty queue: bell-only cell, "No notifications" popup.
        lua.globals()
            .set("notifications", lua.create_table().expect("t"))
            .expect("set");
        let value = call_lua_value(&lua, "view").expect("view");
        let _ = build_node(&parse_node(&value).expect("parse"), 13.0, None).expect("build");
        let popup = call_lua_value(&lua, "popup").expect("popup");
        let _ = crate::app::layers::Popup::parse_popup_content(popup).expect("popup");
        // One queued item: count cell + a dismiss row.
        lua.load(
            r#"notifications = { { id = 42, app = "mako", title = "hi", body = "b", urgency = 1, has_image = false } }"#,
        )
        .exec()
        .expect("seed queue");
        let value = call_lua_value(&lua, "view").expect("view");
        assert!(matches!(
            parse_node(&value).expect("parse"),
            WidgetNode::Row { .. }
        ));
        let popup = call_lua_value(&lua, "popup").expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("popup");
        let mut actions = Vec::new();
        fn walk(node: &WidgetNode, out: &mut Vec<String>) {
            match node {
                WidgetNode::ListView { items, .. } => {
                    for (_, child) in items {
                        walk(child, out);
                    }
                }
                WidgetNode::Button { action, .. } => out.push(action.clone()),
                WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => {
                    for c in children {
                        walk(c, out);
                    }
                }
                _ => {}
            }
        }
        if let Some(tree) = &content.tree {
            walk(tree, &mut actions);
        }
        assert_eq!(actions, vec!["dismiss:42"]);
        // The dismiss key maps back through on_action to a table.
        let value = call_lua_named_action(&lua, "dismiss:42").expect("action");
        match value {
            Value::Table(t) => assert_eq!(t.get::<u32>("dismiss").expect("dismiss"), 42),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn build_anim_list_merges_live_and_ghosts() {
        use super::super::listview::{Axis, ListView};
        use aura_anim::core::runtime::MotionRuntime;
        // Live row of two buttons, the first mid-enter, plus one
        // exiting ghost at index 1.
        let children = vec![
            WidgetNode::Button {
                label: "1".to_string(),
                action: "go:1".to_string(),
                width: None,
                height: None,
                padding: None,
                color: None,
            },
            WidgetNode::Button {
                label: "[2]".to_string(),
                action: "go:2".to_string(),
                width: None,
                height: None,
                padding: None,
                color: None,
            },
        ];
        let mut rt = MotionRuntime::new();
        let duration = Duration::from_millis(150);
        let mut list: ListView<(String, String), WidgetNode> = ListView::new(32.0);
        let ws1 = ("demo".to_string(), "go:1".to_string());
        let ws2 = ("demo".to_string(), "go:2".to_string());
        let ws9 = ("demo".to_string(), "go:9".to_string());
        // First button mid-enter, plus one exiting ghost at index 1.
        list.update(
            &mut rt,
            duration,
            Axis::Horizontal,
            &[],
            &[ws1.clone(), ws2.clone()],
            &[],
        );
        list.update(
            &mut rt,
            duration,
            Axis::Horizontal,
            &[ws1.clone(), ws2.clone(), ws9.clone()],
            &[ws1.clone(), ws2.clone()],
            &[(
                1,
                ws9.clone(),
                WidgetNode::Button {
                    label: "9".to_string(),
                    action: "ws:9".to_string(),
                    width: None,
                    height: None,
                    padding: None,
                    color: None,
                },
            )],
        );
        assert_eq!(list.ghosts_for(|_| true).len(), 1);
        let msg = |_: String| Plant::Tend;
        let mut lists = std::collections::HashMap::new();
        lists.insert("demo".to_string(), list);
        // Live + ghost merge builds (ghost renders inert), rows and
        // columns alike.
        for is_row in [true, false] {
            let _ = build_anim_list(
                "demo",
                &children,
                4.0,
                NodeLength::Shrink,
                NodeLength::Shrink,
                is_row,
                13.0,
                &msg,
                &rt,
                &lists,
            )
            .expect("builds");
        }
    }
}
