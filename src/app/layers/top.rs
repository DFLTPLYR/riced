use super::Popup;
use super::background::Background;
use crate::app::app::{PlotInfo, Plots};
use crate::app::{Plant, TopEvent};
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
    outputs: &HashMap<String, String>,
    trees: &HashMap<String, WidgetNode>,
    gap: f32,
    horizontal: bool,
    anim_runtime: &aura_anim::core::runtime::MotionRuntime,
    item_anims: &super::anim::ItemMotions,
    item_ghosts: &super::anim::ItemGhosts,
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
                .on_press(Plant::TopPlot(TopEvent::WidgetPressed(
                    bar_id,
                    pos,
                    name.to_string(),
                )))
                .on_release(Plant::TopPlot(TopEvent::WidgetReleased(
                    bar_id,
                    pos,
                    name.to_string(),
                )))
                .into()
        };
        if let Some(node) = trees.get(name) {
            let size = defs
                .iter()
                .find(|d| d.name == *name)
                .map(|d| d.size)
                .unwrap_or(13.0);
            // Trees failing to build render nothing (logged at ingest).
            // Buttons arm a per-widget MouseArea: the click carries the
            // owning widget, so on_action routes back to its own state.
            let widget = name.clone();
            let msg =
                move |action: String| Plant::TopPlot(TopEvent::CellAction(widget.clone(), action));
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
                        name,
                        children,
                        *spacing,
                        width.clone(),
                        height.clone(),
                        is_row,
                        size,
                        &msg,
                        anim_runtime,
                        item_anims,
                        item_ghosts.get(name).map(Vec::as_slice).unwrap_or(&[]),
                        horizontal,
                    )
                }
                _ => build_node(node, size, Some(&msg)),
            };
            if let Ok(item) = built {
                items.push(area(item, name));
            }
        } else if let Some((output, size)) = lua_cell_text(name, defs, outputs) {
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

/// Build a top-level row/column with enter/exit transitions: direct
/// button children (keyed by action) slide from their motion pad;
/// retained ghosts render inert at their old indices. Live build
/// failures skip the widget (mirrors plain `build_node` strictness);
/// ghost build failures skip just that ghost.
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
    item_anims: &super::anim::ItemMotions,
    ghosts: &[super::anim::GhostItem],
    horizontal: bool,
) -> Result<Element<'static, Plant>, String> {
    let pad_of = |key: &str| -> f32 {
        item_anims
            .get(&(widget.to_string(), key.to_string()))
            .and_then(|m| m.value(anim_runtime).ok())
            .map(|v| v.pad)
            .unwrap_or(0.0)
    };
    let wrap = |el: Element<'static, Plant>, pad: f32| -> Element<'static, Plant> {
        if pad <= 0.01 {
            return el;
        }
        let mut p = iced::Padding::ZERO;
        if horizontal {
            p.left = pad;
        } else {
            p.top = pad;
        }
        container(el).padding(p).into()
    };
    let mut items = Vec::new();
    for child in children {
        let el = build_node(child, size, Some(button_msg))?;
        let pad = match child {
            WidgetNode::Button { action, .. } => pad_of(action),
            _ => 0.0,
        };
        items.push(wrap(el, pad));
    }
    // Ghosts at their old indices (clamped: batch removals shift).
    let mut ordered: Vec<&super::anim::GhostItem> = ghosts.iter().collect();
    ordered.sort_by_key(|g| g.index);
    for ghost in ordered {
        let Ok(el) = build_node(&ghost.node, size, None) else {
            continue;
        };
        let at = ghost.index.min(items.len());
        items.push(wrap(el, pad_of(&ghost.key)));
        // Move the just-pushed ghost into place.
        let last = items.len() - 1;
        if at < last {
            let el = items.remove(last);
            items.insert(at, el);
        }
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

/// Last script output (text, size) by widget name (`None` = empty cell).
/// Split out so the cache lookup stays testable without rendering.
fn lua_cell_text(
    name: &str,
    defs: &[WidgetDef],
    outputs: &HashMap<String, String>,
) -> Option<(String, f32)> {
    let def = defs.iter().find(|d| d.name == name)?;
    outputs.get(name).cloned().map(|text| (text, def.size))
}

/// One composable UI node, built in Lua via the `ui` table and
/// interpreted here into iced widgets. Lua never holds real widgets —
/// it composes these descriptions, which is the entire expressive
/// range (nesting is free; new primitives add one constructor plus one
/// match arm below).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WidgetNode {
    Text {
        content: String,
        size: Option<f32>,
        width: Option<NodeLength>,
        height: Option<NodeLength>,
    },
    Icon(String),
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
    /// popup/`on_press` fallback). In popups buttons render inert
    /// (popup clicks go through item rows + `PopupSelect`).
    Button {
        label: String,
        action: String,
        width: Option<NodeLength>,
        height: Option<NodeLength>,
        padding: Option<f32>,
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
fn opt_number(t: &Table, field: &str, what: &str) -> Result<Option<f32>, String> {
    match t.get::<Value>(field).map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => Ok(None),
        Value::Integer(i) => Ok(Some(i as f32)),
        Value::Number(n) => Ok(Some(n as f32)),
        other => Err(format!(
            "{what} {field} must be a number, got {}",
            lua_value_kind(&other)
        )),
    }
}

/// Optional box size from a node table: numbers are px, `"fill"` /
/// `"shrink"` (any case) are the iced modes, unset (nil — or the
/// setter function sharing the field namespace) means `None`.
/// Anything else errors naming the field.
fn opt_length(t: &Table, field: &str, what: &str) -> Result<Option<NodeLength>, String> {
    match t.get::<Value>(field).map_err(|e| e.to_string())? {
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
                "text" => Ok(WidgetNode::Text {
                    content: coerce_text(
                        t.get::<Value>("text").map_err(|e| e.to_string())?,
                        "ui.text()",
                    )?,
                    // Chained :size(14) overrides the widget default.
                    size: opt_number(t, "size", "ui.text()")?,
                    width: opt_length(t, "width", "ui.text()")?,
                    height: opt_length(t, "height", "ui.text()")?,
                }),
                "icon" => match t.get::<Value>("name").map_err(|e| e.to_string())? {
                    Value::String(s) => Ok(WidgetNode::Icon(s.to_string_lossy())),
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
                    let spacing = match t.get::<Value>("spacing").map_err(|e| e.to_string())? {
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
                other => Err(format!("unknown ui node type {other:?}")),
            }
        }
        _ => Ok(WidgetNode::Text {
            content: coerce_text(value.clone(), "ui node")?,
            size: None,
            width: None,
            height: None,
        }),
    }
}

/// Build an iced element from a node tree. Pure Rust over owned data —
/// views call this per redraw while Lua only runs on its interval.
pub(crate) fn build_node(
    node: &WidgetNode,
    size: f32,
    button_msg: Option<&dyn Fn(String) -> Plant>,
) -> Result<Element<'static, Plant>, String> {
    match node {
        WidgetNode::Text {
            content,
            size: own,
            width,
            height,
        } => {
            let s = own.unwrap_or(size).max(1.0);
            let mut t = text(content.clone()).size(s);
            if let Some(w) = width {
                t = t.width(w.clone().iced());
            }
            if let Some(h) = height {
                t = t.height(h.clone().iced());
            }
            Ok(t.into())
        }
        WidgetNode::Icon(name) => match icon_bytes(name) {
            Some(bytes) => Ok(lucide_iced::themed_icon(bytes, size.max(1.0))),
            None => Ok(text(format!("{{icon:{name}}}")).size(size.max(1.0)).into()),
        },
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
                row = row.push(build_node(child, size, button_msg)?);
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
                column = column.push(build_node(child, size, button_msg)?);
            }
            Ok(column.into())
        }
        WidgetNode::Button {
            label,
            action,
            width,
            height,
            padding,
        } => {
            let mut item = button(rich_text(label.clone(), size, 4.0))
                .padding(padding.unwrap_or(6.0).max(0.0))
                .style(theme::menu_button(theme::RADIUS));
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
            Ok(bar.into())
        }
        // Animated ring: iced has no spinner widget, so a rotating
        // loader icon approximates one (redrawn every frame while
        // visible — popups repaint on cursor/tick activity).
        WidgetNode::Spinner => Ok(lucide_iced::themed_icon(
            lucide_iced::bytes::LOADER_CIRCLE,
            size.max(1.0) * 1.5,
        )),
    }
}

/// Refresh the `sysinfo`/`gfxinfo` globals of one Lua state from live
/// system data. Scripts see `sysinfo.cpu_usage` (%, all cores),
/// `sysinfo.cpu_count`, `sysinfo.mem_used`/`mem_total` (bytes),
/// `sysinfo.mem_usage` (%), and `gfxinfo.usage` (% or nil when the
/// GPU exposes nothing readable).
pub(crate) fn publish_system_tables(
    lua: &Lua,
    sys: &sysinfo::System,
    gpu: Option<f32>,
) -> mlua::Result<()> {
    let globals = lua.globals();
    let info = lua.create_table()?;
    info.set("cpu_usage", sys.global_cpu_usage())?;
    info.set("cpu_count", sys.cpus().len())?;
    info.set("mem_used", sys.used_memory())?;
    info.set("mem_total", sys.total_memory())?;
    let total = sys.total_memory();
    info.set(
        "mem_usage",
        if total > 0 {
            sys.used_memory() as f32 / total as f32 * 100.0
        } else {
            0.0
        },
    )?;
    globals.set("sysinfo", info)?;
    let gfx = lua.create_table()?;
    match gpu {
        Some(usage) => gfx.set("usage", usage)?,
        None => gfx.set("usage", Value::Nil)?,
    }
    globals.set("gfxinfo", gfx)?;
    Ok(())
}

/// Lua state for one widget: string/table/math/os/io with native
/// shell (`os.execute`, `io.popen` live — owner-accepted risk, no
/// allowlist). `os.exit`/`os.remove`/`os.rename` stay nil'd, as do
/// `dofile`/`loadfile`/`require`. `print` stays for daemon logs.
fn new_widget_lua() -> mlua::Result<Lua> {
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
    inject_ui(&lua)?;
    Ok(lua)
}

/// The `ui` constructors table, present in every widget state next to
/// `sysinfo`/`gfxinfo`. Each call builds a plain description table —
/// no iced objects cross into Lua; [`parse_node`] interprets them.
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
fn inject_ui(lua: &Lua) -> mlua::Result<()> {
    /// One setter: `node:name(v)` writes field, returns node.
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
            node.set(key.clone(), v)?;
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
        ],
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
    lua.globals().set("_riced_ui_mt_text", mt_text.clone())?;
    lua.globals()
        .set("_riced_ui_mt_rowcol", mt_rowcol.clone())?;
    lua.globals()
        .set("_riced_ui_mt_button", mt_button.clone())?;
    lua.globals()
        .set("_riced_ui_mt_progress", mt_progress.clone())?;
    lua.globals().set("_riced_ui_mt_bare", mt_bare.clone())?;
    // Move clones into the constructor closures (mlua closures are
    // 'static): each captures only its own metatable.
    let (mt_text_c, mt_bare_c, mt_rowcol_c, mt_button_c, mt_progress_c) = (
        mt_text.clone(),
        mt_bare.clone(),
        mt_rowcol.clone(),
        mt_button.clone(),
        mt_progress.clone(),
    );
    let ui = lua.create_table()?;
    ui.set(
        "text",
        lua.create_function(move |lua, text: Value| {
            node(lua, "text", mt_text_c.clone(), |t| t.set("text", text))
        })?,
    )?;
    ui.set(
        "icon",
        lua.create_function(move |lua, name: Value| {
            node(lua, "icon", mt_bare_c.clone(), |t| t.set("name", name))
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
    lua.globals().set("ui", ui)
}

/// Load a widget script into its state and verify it defines `render`.
fn load_widget_script(lua: &Lua, label: &str, source: &str) -> mlua::Result<()> {
    lua.load(source).set_name(format!("@{label}")).exec()?;
    let _: Function = lua.globals().get("render")?;
    Ok(())
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

/// Call a widget script function (`render`, `popup`, ...) and get the
/// raw return value.
pub(crate) fn call_lua_value(lua: &Lua, func: &str) -> Result<Value, String> {
    let func_value: Function = lua.globals().get(func).map_err(|e| e.to_string())?;
    func_value.call::<Value>(()).map_err(|e| e.to_string())
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
        lua.globals()
            .get::<Function>(func)
            .map(|_| true)
            .unwrap_or(false)
    })
}

/// Run a widget's `on_press()` click action. The return value is ignored;
/// scripts signal through globals that the next `render()` reads.
fn call_lua_action(lua: &Lua) -> Result<(), String> {
    let action: Function = lua.globals().get("on_press").map_err(|e| e.to_string())?;
    action.call::<()>(()).map_err(|e| e.to_string())
}

/// Run a widget's `on_action(key)` (cell buttons and popup items share
/// it). Missing `on_action` is a silent no-op so plain-text widgets
/// coexist with button trees.
fn call_lua_named_action(lua: &Lua, action: &str) -> Result<(), String> {
    let func: Function = match lua.globals().get("on_action") {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };
    func.call::<()>(action.to_string())
        .map_err(|e| e.to_string())
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
        outputs: &HashMap<String, String>,
        trees: &HashMap<String, WidgetNode>,
        anim_runtime: &aura_anim::core::runtime::MotionRuntime,
        item_anims: &super::anim::ItemMotions,
        item_ghosts: &super::anim::ItemGhosts,
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
                item_anims,
                item_ghosts,
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

    /// Set one slot's child alignment (`TopEvent::SetSlotAlign`): single
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

    /// Check/uncheck one slot widget (`TopEvent::SetSlotWidget`):
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

    /// Set the slot cell padding (`TopEvent::SetSlotPadding`): single
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

    /// Set the slot gaps (`TopEvent::SetSlotSpacing`): single commit per
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

    /// Publish fresh system tables and call one widget's `render()`,
    /// returning the raw value. Errors are returned for
    /// once-per-message logging by the caller.
    fn render_lua_value(
        plots: &mut Plots,
        def: &crate::config::WidgetDef,
        gpu: Option<f32>,
    ) -> Result<Value, String> {
        Self::sync_script_state(plots, def);
        Self::ensure_widget_lua(plots, def)?;
        let lua = plots
            .widget_lua
            .get(&def.name)
            .ok_or_else(|| "runtime missing".to_string())?;
        publish_system_tables(lua, &plots.sysinfo, gpu).map_err(|e| e.to_string())?;
        call_lua_value(lua, "render")
    }

    /// Store one `render()` result: tables become [`WidgetNode`] trees,
    /// scalars become cached text. Switching shapes clears the other
    /// cache so nothing stale renders.
    fn ingest_render_value(
        plots: &mut Plots,
        def: &crate::config::WidgetDef,
        result: Result<Value, String>,
    ) {
        match result {
            Ok(Value::Table(t)) => {
                plots.widget_last_error.remove(&def.name);
                plots.widget_outputs.remove(&def.name);
                match parse_node(&Value::Table(t)) {
                    Ok(node) => {
                        // Diff button lists for enter/exit transitions
                        // before replacing the cached tree.
                        let old = plots.widget_trees.get(&def.name).cloned();
                        let duration = plots.config.animation.speed.duration();
                        super::anim::sync_list_anims(
                            &mut plots.anim_runtime,
                            &mut plots.item_anims,
                            &mut plots.item_ghosts,
                            &def.name,
                            old.as_ref(),
                            &node,
                            duration,
                        );
                        plots.widget_trees.insert(def.name.clone(), node);
                    }
                    Err(e) => {
                        plots.widget_trees.remove(&def.name);
                        Self::note_widget_error(plots, &def.name, e);
                    }
                }
            }
            Ok(value) => {
                plots.widget_last_error.remove(&def.name);
                plots.widget_trees.remove(&def.name);
                match coerce_text(value, "render()") {
                    Ok(text) => {
                        plots.widget_outputs.insert(def.name.clone(), text);
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
    pub(crate) fn init_widget_lua(plots: &mut Plots) {
        plots.widget_lua.clear();
        plots.widget_outputs.clear();
        plots.widget_trees.clear();
        plots.widget_last_run.clear();
        plots.widget_last_error.clear();
        plots.widget_script_mtime.clear();
        // Fresh Lua states mean fresh lists: drop in-flight transitions
        // so the first post-reload paint settles instantly.
        plots.item_anims.clear();
        plots.item_ghosts.clear();
        plots.sysinfo.refresh_cpu_usage();
        plots.sysinfo.refresh_memory();
        let gpu = Popup::gpu_usage_percent();
        let defs = plots.widgets.clone();
        let now = Instant::now();
        for def in &defs {
            plots.widget_last_run.insert(def.name.clone(), now);
            let result = Self::render_lua_value(plots, def, gpu);
            Self::ingest_render_value(plots, def, result);
        }
        Self::warn_unknown_slot_widgets(plots);
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

    /// Re-render one widget now and report whether visible output
    /// moved (text or tree). Shared by the interval tick, the
    /// `on_press()` click path, cell actions, and popup selects.
    pub(crate) fn refresh_widget(
        plots: &mut Plots,
        def: &crate::config::WidgetDef,
        gpu: Option<f32>,
    ) -> bool {
        let before = (
            plots.widget_outputs.get(&def.name).cloned(),
            plots.widget_trees.get(&def.name).cloned(),
        );
        let result = Self::render_lua_value(plots, def, gpu);
        Self::ingest_render_value(plots, def, result);
        let after = (
            plots.widget_outputs.get(&def.name).cloned(),
            plots.widget_trees.get(&def.name).cloned(),
        );
        before != after
    }

    /// Re-render due Lua widgets (`TopEvent::WidgetTick`): emits
    /// `WidgetsChanged` only when an output moved (which repaints).
    pub(crate) fn handle_widget_tick(plots: &mut Plots) -> Command<Plant> {
        if Self::run_due_widgets(plots) {
            return Command::done(Plant::TopPlot(TopEvent::WidgetsChanged));
        }
        Command::none()
    }

    /// Advance list enter/exit transitions (`TopEvent::WidgetAnim`):
    /// ticks the aura runtime and sweeps settled motions + ghosts.
    /// Repaint comes from the `Scope::All` redraw scope, not here.
    pub(crate) fn handle_anim_frame(plots: &mut Plots) -> Command<Plant> {
        super::anim::sweep_anims(
            &mut plots.anim_runtime,
            &mut plots.item_anims,
            &mut plots.item_ghosts,
            Instant::now(),
        );
        Command::none()
    }

    /// Click a cell button: run the owning widget's `on_action(key)`
    /// (the view closure stamps the owner, so the key routes to its
    /// own Lua state — no slot-wide popup/`on_press` fallback), then
    /// re-render that widget like the click path does.
    pub(crate) fn handle_cell_action(
        plots: &mut Plots,
        widget: String,
        action: String,
    ) -> Command<Plant> {
        let gpu = Popup::gpu_usage_percent();
        let outcome = plots.widget_lua.get(&widget).map(|lua| {
            publish_system_tables(lua, &plots.sysinfo, gpu)
                .map_err(|e| e.to_string())
                .and_then(|()| call_lua_named_action(lua, &action))
        });
        match outcome {
            Some(Ok(())) => {
                plots.widget_last_error.remove(&widget);
                if let Some(def) = plots.widgets.iter().find(|d| d.name == widget).cloned()
                    && Self::refresh_widget(plots, &def, gpu)
                {
                    return Command::done(Plant::TopPlot(TopEvent::WidgetsChanged));
                }
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
            cmds.push(Self::run_on_press(plots, widget));
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
            return Self::run_on_press(plots, &name);
        }
        Command::none()
    }

    /// Run one widget's `on_press()` click action, then re-render it
    /// (a toggle flips its next output). Errors log once-per-message.
    fn run_on_press(plots: &mut Plots, name: &str) -> Command<Plant> {
        let gpu = Popup::gpu_usage_percent();
        let outcome = match plots.widget_lua.get(name) {
            Some(lua) => {
                let acted = publish_system_tables(lua, &plots.sysinfo, gpu)
                    .map_err(|e| e.to_string())
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
                    return Command::done(Plant::TopPlot(TopEvent::WidgetsChanged));
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

    #[test]
    fn lua_sandbox_runs_render() {
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(&lua, "test", "function render() return 'hi' end").expect("load");
        assert_eq!(call_lua_text(&lua, "render").unwrap(), "hi");
    }

    #[test]
    fn lua_return_values_coerce_to_text() {
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(&lua, "test", "function render() return 42 end").expect("load");
        assert_eq!(call_lua_text(&lua, "render").unwrap(), "42");
        load_widget_script(&lua, "test", "function render() return true end").expect("load");
        assert_eq!(call_lua_text(&lua, "render").unwrap(), "true");
        load_widget_script(&lua, "test", "function render() return nil end").expect("load");
        assert_eq!(call_lua_text(&lua, "render").unwrap(), "");
        load_widget_script(&lua, "test", "function render() return {} end").expect("load");
        assert!(call_lua_text(&lua, "render").is_err());
    }

    #[test]
    fn lua_missing_render_is_rejected() {
        let lua = new_widget_lua().expect("sandbox");
        assert!(load_widget_script(&lua, "test", "x = 1").is_err());
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
        let defs = vec![lua_widget("w")];
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(lua_cell_text("w", &defs, &empty), None);
        let mut outputs = HashMap::new();
        outputs.insert("w".to_string(), "hi".to_string());
        assert_eq!(
            lua_cell_text("w", &defs, &outputs),
            Some(("hi".to_string(), 13.0))
        );
        // Unknown names never read the cache.
        assert_eq!(lua_cell_text("nope", &defs, &outputs), None);
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

    #[test]
    fn system_tables_expose_cpu_memory_and_gpu() {
        let lua = new_widget_lua().expect("sandbox");
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        publish_system_tables(&lua, &sys, Some(42.0)).expect("publish");
        let cpu: f32 = lua.load("return sysinfo.cpu_usage").eval().expect("eval");
        assert!(cpu >= 0.0);
        assert!(sys.total_memory() > 0);
        let gfx: f32 = lua.load("return gfxinfo.usage").eval().expect("eval");
        assert_eq!(gfx, 42.0);
        // Missing GPUs read as nil, not an error.
        publish_system_tables(&lua, &sys, None).expect("publish");
        let nil: Value = lua.load("return gfxinfo.usage").eval().expect("eval");
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
            WidgetNode::Icon(_) => true,
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
            publish_system_tables(&lua, &sys, None).expect("publish");
            let value = call_lua_value(&lua, "render").expect("render");
            let node = parse_node(&value).expect("parse");
            assert!(!node_has_icon(&node), "usage seeds stay icon-free");
        }
    }

    /// Seeds render through the full pipeline: parse plus build.
    /// The hypr seed is excluded — it needs a compositor socket.
    #[test]
    fn seed_scripts_parse_and_build() {
        use crate::config::{
            SEED_CLINEPASS_LUA, SEED_CLOCK_LUA, SEED_CPU_LUA, SEED_GPU_LUA, SEED_HELLO_LUA,
            SEED_RAM_LUA, SEED_STATS_LUA, SEED_SYSTEM_LUA,
        };
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        for source in [
            SEED_CLOCK_LUA,
            SEED_HELLO_LUA,
            SEED_STATS_LUA,
            SEED_CPU_LUA,
            SEED_RAM_LUA,
            SEED_GPU_LUA,
            SEED_CLINEPASS_LUA,
            SEED_SYSTEM_LUA,
        ] {
            let lua = new_widget_lua().expect("sandbox");
            load_widget_script(&lua, "seed", source).expect("load");
            publish_system_tables(&lua, &sys, None).expect("publish");
            let value = call_lua_value(&lua, "render").expect("render");
            let node = parse_node(&value).expect("parse");
            let _ = build_node(&node, 13.0, None).expect("builds");
        }
        // Blank-key clinepass renders the connect hint popup (ui tree).
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(&lua, "clinepass", SEED_CLINEPASS_LUA).expect("load");
        let popup: mlua::Value = lua.load("return popup()").eval().expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("popup parses");
        assert!(content.tree.is_some());
        // System seed: power cell plus a four-row session menu.
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(&lua, "system", SEED_SYSTEM_LUA).expect("load");
        let popup: mlua::Value = lua.load("return popup()").eval().expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("popup parses");
        assert_eq!(content.items.len(), 4);
        assert!(content.items.iter().all(|i| !i.action.is_empty()));
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
        let _: mlua::Value = lua
            .load(r#"return on_action("suspend")"#)
            .eval()
            .expect("action");
        // A hostile key never reaches the shell.
        let _: mlua::Value = lua
            .load(r#"return on_action("x; rm -rf ~")"#)
            .eval()
            .expect("action");
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
        // Stub io.popen: return the canned body regardless of command.
        // Method-call form: handle:read("*a") passes the handle as
        // first arg, mode second — accept both.
        let io: mlua::Table = lua.globals().get("io").expect("io");
        let body_owned = body.to_string();
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
        lua.load(SEED_CLINEPASS_LUA.replace(r#"local API_KEY = """#, r#"local API_KEY = "x""#))
            .exec()
            .expect("load");
        // First popup (cold cache) is the spinner, not the data.
        let popup: mlua::Value = lua.load("return popup()").eval().expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("popup parses");
        let tree = content.tree.expect("spinner tree");
        assert!(matches!(
            tree,
            WidgetNode::Column { children, .. } if children.len() == 2
        ));
        // render() fetches into _usage; second popup shows the rows.
        let _: mlua::Value = lua.load("return render()").eval().expect("render");
        let popup: mlua::Value = lua.load("return popup()").eval().expect("popup");
        let content = crate::app::layers::Popup::parse_popup_content(popup).expect("popup parses");
        let tree = content.tree.expect("usage tree");
        let built = build_node(&tree, 13.0, None).expect("builds");
        let _ = built;
        // One row per window: header + 3 label/bar/pct rows.
        match tree {
            WidgetNode::Column { children, .. } => {
                assert_eq!(children.len(), 4);
                for row in &children[1..] {
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
    fn hypr_seed_shows_output_local_workspaces_by_position() {
        use crate::config::SEED_HYPR_LUA;
        // Ids 3 (DP-1), 4 (HDMI-1), 5 (DP-1); focused is 5 on DP-1.
        // Positions must be 1..2 over DP-1 only — never raw ids.
        let workspaces = r#"[{"id":3,"name":"3","monitor":"DP-1","monitorID":0,"windows":1},{"id":4,"name":"4","monitor":"HDMI-1","monitorID":1,"windows":0},{"id":5,"name":"5","monitor":"DP-1","monitorID":0,"windows":2}]"#;
        let active = r#"{"id":5,"name":"5","monitor":"DP-1","monitorID":0,"windows":2}"#;
        let lua = new_widget_lua().expect("sandbox");
        // Stub io.popen by command (method-call read form).
        let io: mlua::Table = lua.globals().get("io").expect("io");
        let (ws, act) = (workspaces.to_string(), active.to_string());
        io.set(
            "popen",
            lua.create_function(move |lua, cmd: String| {
                let body = if cmd.contains("activeworkspace") {
                    act.clone()
                } else {
                    ws.clone()
                };
                let h = lua.create_table().expect("handle");
                h.set(
                    "read",
                    lua.create_function(move |_, (_h, _m): (mlua::Value, mlua::Value)| {
                        Ok(body.clone())
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
        // Record dispatches instead of spawning hyprctl.
        let os: mlua::Table = lua.globals().get("os").expect("os");
        os.set(
            "execute",
            lua.create_function(|lua, cmd: String| {
                lua.globals().set("_dispatched", cmd)?;
                Ok(true)
            })
            .expect("exec"),
        )
        .expect("set execute");
        load_widget_script(&lua, "hypr", SEED_HYPR_LUA).expect("load");
        let value: mlua::Value = lua.load("return render()").eval().expect("render");
        let node = parse_node(&value).expect("parse");
        match node {
            WidgetNode::Row { children, .. } => {
                assert_eq!(children.len(), 2);
                match &children[0] {
                    WidgetNode::Button { label, action, .. } => {
                        assert_eq!((label.as_str(), action.as_str()), ("1", "ws:1"));
                    }
                    other => panic!("expected button, got {other:?}"),
                }
                match &children[1] {
                    WidgetNode::Button { label, action, .. } => {
                        assert_eq!((label.as_str(), action.as_str()), ("[2]", "ws:2"));
                    }
                    other => panic!("expected button, got {other:?}"),
                }
            }
            other => panic!("expected row, got {other:?}"),
        }
        // Position 1 maps back to workspace id 3 (not "workspace 1").
        let _: mlua::Value = lua
            .load(r#"return on_action("ws:1")"#)
            .eval()
            .expect("action");
        let dispatched: String = lua.load("return _dispatched").eval().expect("dispatched");
        assert!(
            dispatched.contains("workspace = 3"),
            "dispatches mapped id, got {dispatched:?}"
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
            "function render() return 'x' end\nfunction popup() return 'menu' end",
        )
        .expect("load");
        assert_eq!(call_lua_text(&lua, "popup").unwrap(), "menu");
        // on_press() mutates script state; the next render reflects it.
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(
            &lua,
            "test",
            "flag = false\nfunction render() return flag and 1 or 0 end\nfunction on_press() flag = true end",
        )
        .expect("load");
        call_lua_action(&lua).expect("action");
        assert_eq!(call_lua_text(&lua, "render").unwrap(), "1");
        // Missing functions are absent, never errors at lookup.
        let mut states = HashMap::new();
        states.insert("w".to_string(), new_widget_lua().expect("sandbox"));
        load_widget_script(&states["w"], "w", "function render() return 'x' end").expect("load");
        assert!(!lua_has_func(&states, "w", "popup"));
        assert!(!lua_has_func(&states, "w", "on_press"));
        assert!(!lua_has_func(&states, "missing", "render"));
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
    }

    #[test]
    fn lua_on_action_receives_the_item_key() {
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(
            &lua,
            "test",
            "seen = {}\nfunction render() return '' end\nfunction on_action(name) seen[#seen + 1] = name end",
        )
        .expect("load");
        let on_action: Function = lua.globals().get("on_action").expect("fn");
        on_action.call::<()>("toggle".to_string()).expect("call");
        let seen: String = lua.load("return seen[1]").eval().expect("eval");
        assert_eq!(seen, "toggle");
        // Missing on_action is a silent no-op (plain-text widgets
        // coexist with cell buttons without erroring).
        let lua = new_widget_lua().expect("sandbox");
        load_widget_script(&lua, "plain", "function render() return 'x' end").expect("load");
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
        assert_eq!(node.get::<f64>("spacing").expect("spacing"), 8.0);
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
                    },
                    WidgetNode::Button {
                        label: "go".to_string(),
                        action: "run".to_string(),
                        width: None,
                        height: None,
                        padding: None,
                    },
                    WidgetNode::Text {
                        content: "7".to_string(),
                        size: None,
                        width: None,
                        height: None,
                    },
                    WidgetNode::Icon("cpu".to_string()),
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
            },
            WidgetNode::Icon("cpu".to_string()),
            WidgetNode::Icon("typo".to_string()),
            WidgetNode::Row {
                children: vec![WidgetNode::Text {
                    content: "a".to_string(),
                    size: None,
                    width: None,
                    height: None,
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
            },
            WidgetNode::Progress {
                value: 1.5,
                width: NodeLength::Fixed(120.0),
                height: None,
            },
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
        };
        let _ = build_node(&node, size, Some(&|_| Plant::Tend)).expect("builds");
    }

    #[test]
    fn build_anim_list_merges_live_and_ghosts() {
        use crate::app::layers::anim::{ENTER_OFFSET, GhostItem, ItemMotions, ItemSlide};
        use aura_anim::core::runtime::MotionRuntime;
        // Live row like the hypr seed renders: two buttons, the first
        // mid-enter, plus one exiting ghost at index 1.
        let children = vec![
            WidgetNode::Button {
                label: "1".to_string(),
                action: "ws:1".to_string(),
                width: None,
                height: None,
                padding: None,
            },
            WidgetNode::Button {
                label: "[2]".to_string(),
                action: "ws:2".to_string(),
                width: None,
                height: None,
                padding: None,
            },
        ];
        let mut rt = MotionRuntime::new();
        let timing = aura_anim::core::timing::Timing::ease_out(Duration::from_millis(150));
        let mut motions: ItemMotions = HashMap::new();
        for (key, pad) in [("ws:1", ENTER_OFFSET), ("ws:9", 0.0)] {
            let m = rt.motion_with(ItemSlide { pad }, timing);
            let target = if key == "ws:1" { 0.0 } else { ENTER_OFFSET };
            let _ = m.transition_to(ItemSlide { pad: target }, &mut rt);
            motions.insert(("hypr".to_string(), key.to_string()), m);
        }
        let ghosts = vec![GhostItem {
            index: 1,
            key: "ws:9".to_string(),
            node: WidgetNode::Button {
                label: "9".to_string(),
                action: "ws:9".to_string(),
                width: None,
                height: None,
                padding: None,
            },
        }];
        let msg = |_: String| Plant::Tend;
        // Live + ghost merge builds (ghost renders inert).
        let _ = build_anim_list(
            "hypr",
            &children,
            4.0,
            NodeLength::Shrink,
            NodeLength::Shrink,
            true,
            13.0,
            &msg,
            &rt,
            &motions,
            &ghosts,
            true,
        )
        .expect("builds");
        // Vertical bars pad the top instead of the left: same build.
        let _ = build_anim_list(
            "hypr",
            &children,
            4.0,
            NodeLength::Shrink,
            NodeLength::Shrink,
            false,
            13.0,
            &msg,
            &rt,
            &motions,
            &ghosts,
            false,
        )
        .expect("builds");
    }
}
