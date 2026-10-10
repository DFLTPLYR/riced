pub(crate) mod animation;
mod edit;
mod input;
mod render;
mod runtime;
mod scheduling;
use super::Popup;
use super::background::Background;
use crate::config::WidgetDefinition;
use crate::shell::{Plant, TopEvent, WidgetEvent};
use crate::shell::{state::Plots, windows::PlotInfo};
use iced::mouse::Button;
use iced::widget::{Space, column, container, row, text};
use iced::window;
use iced::{Element, Point, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use iced_wayland_subscriber::{OutputId, OutputInfo};
use mlua::Value;
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
    /// Widget placements per slot position (`len == slots`), resolved
    /// against discovered `widgets/*.lua` definitions. Each slot
    /// renders its entries together along the bar axis. Resized by
    /// [`TopLocal::ensure_widgets`], persisted as names-or-tables.
    pub widgets: Vec<Vec<crate::config::WidgetPlacement>>,
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

    /// Widget placements by position (empty past the end).
    pub(crate) fn widgets_at(&self, pos: usize) -> &[crate::config::WidgetPlacement] {
        self.widgets.get(pos).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Fully-resolved render inputs for one placement: definition
/// defaults overlaid with placement overrides, script path included.
/// Built fresh per render so edits apply without restarts.
use crate::lua::widgets::{ResolvedWidget, load_widget_script, lua_has_func, new_widget_lua};

impl From<&crate::config::TopConfig> for TopLocal {
    fn from(c: &crate::config::TopConfig) -> Self {
        let slots = c.slots.clamp(1, Self::MAX_SLOTS);
        let mut aligns: Vec<SlotAlign> = c.aligns.iter().map(|a| SlotAlign::from_str(a)).collect();
        aligns.resize(slots as usize, SlotAlign::Center);
        let mut widgets: Vec<Vec<crate::config::WidgetPlacement>> = c
            .widgets
            .iter()
            .map(|slot| match slot {
                crate::config::SlotWidgets::One(name) if TopLocal::is_empty_widget(name) => {
                    Vec::new()
                }
                crate::config::SlotWidgets::One(name) => {
                    vec![crate::config::WidgetPlacement {
                        name: name.clone(),
                        ..Default::default()
                    }]
                }
                crate::config::SlotWidgets::Many(entries) => entries
                    .iter()
                    .filter_map(|entry| match entry {
                        crate::config::SlotEntry::Name(name) if TopLocal::is_empty_widget(name) => {
                            None
                        }
                        crate::config::SlotEntry::Name(name) => {
                            Some(crate::config::WidgetPlacement {
                                name: name.clone(),
                                ..Default::default()
                            })
                        }
                        crate::config::SlotEntry::Full(placement) => {
                            if TopLocal::is_empty_widget(&placement.name) {
                                None
                            } else {
                                Some(placement.clone())
                            }
                        }
                    })
                    .collect(),
            })
            .collect();
        widgets.resize(slots as usize, Vec::new());
        crate::config::ensure_placement_ids(&mut widgets);
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
        crate::ui::listview::ListView<(String, String), WidgetNode>,
    >,
) -> Result<Element<'static, Plant>, String> {
    // This widget's own list (absent on first paint): settled motion.
    let at_rest = crate::ui::anim::ItemMotion::settled();
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
                items.push(crate::ui::motion::shifted(el, x, y, true));
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
        overlays.push(crate::ui::motion::ghost(
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

use animation::WidgetLists;

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
                          motion: crate::ui::anim::ItemMotion,
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
            border,
            border_width,
        } => {
            let background = background.map(iced::Background::Color);
            let radius = *radius;
            let border = *border;
            let border_width = *border_width;
            Ok(container(build_with_lists(
                child, widget, size, message, runtime, lists,
            )?)
            .width(width.clone().iced())
            .height(height.clone().iced())
            .padding(padding.max(0.0))
            .style(move |_| iced::widget::container::Style {
                background,
                border: iced::Border {
                    color: border.unwrap_or_default(),
                    width: border_width,
                    radius: radius.into(),
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

/// Last script output (text, size) by placement id (`None` = empty
/// cell). Split out so the cache lookup stays testable without rendering.
fn lua_cell_text(
    bar: window::Id,
    placement: &crate::config::WidgetPlacement,
    defs: &[crate::config::WidgetDefinition],
    outputs: &HashMap<(window::Id, String), String>,
) -> Option<(String, f32)> {
    let def = defs.iter().find(|d| d.name == placement.name)?;
    outputs
        .get(&(bar, placement.id.clone()))
        .cloned()
        .map(|text| (text, placement.effective_size(&def.defaults)))
}

use crate::lua::metadata::load_widget_meta;
use crate::ui::build::{build_node, build_node_opacity};
use crate::ui::node::{NodeLength, WidgetNode};

impl Top {
    /// Hold threshold: press held >= this on release counts as hold.
    const HOLD_THRESHOLD: Duration = input::HOLD_THRESHOLD;

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
            keyboard_interactivity: iced_exwlshell::reexport::KeyboardInteractivity::None,
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
            keyboard_interactivity: iced_exwlshell::reexport::KeyboardInteractivity::None,
            margin: None,
            // Bars never blur: frosted glass fights the `opacity` alpha fade
            // (blur would frost the desktop behind a faded fill and the
            // transparent margins), so the fill composites cleanly.
            blur_option: BlurOption::None,
            ..Default::default()
        };

        (id, settings)
    }

    pub fn view(
        &self,
        id: window::Id,
        widgets: &[WidgetDefinition],
        placements: &crate::lua::widgets::WidgetState,
        anim_runtime: &aura_anim::core::runtime::MotionRuntime,
        lists: &std::collections::HashMap<
            String,
            crate::ui::listview::ListView<(String, String), WidgetNode>,
        >,
    ) -> Element<'_, Plant> {
        render::view(
            self,
            id,
            render::BarViewContext {
                catalog: widgets,
                placements,
                motion: anim_runtime,
                lists,
            },
        )
    }

    // ------------------------------------------------------------------
    // Event handling — press/release arrive via PanelWindow (mouse_area);
    // CursorMoved still arrives via Plant::Graft for cursor bookkeeping.
    // Measures press duration for hold detection: press stores Instant,
    // release compares against HOLD_THRESHOLD and clears the entry.
    // ------------------------------------------------------------------

    fn handle_cursor_moved(plots: &mut Plots, id: window::Id, position: Point) -> Command<Plant> {
        plots.input.cursors.insert(id, position);
        Command::none()
    }

    pub(crate) fn handle_press(
        plots: &mut Plots,
        id: window::Id,
        button: Button,
    ) -> Command<Plant> {
        input::InputContext {
            windows: &plots.windows,
            input: &mut plots.input,
        }
        .press(id, button, Instant::now());
        Command::none()
    }

    /// Left press on one widget's own mouse area: record the widget
    /// target. The matching release — and only it — dispatches.
    pub(crate) fn handle_widget_press(
        plots: &mut Plots,
        bar_id: window::Id,
        pos: usize,
        placement_id: String,
    ) -> Command<Plant> {
        input::InputContext {
            windows: &plots.windows,
            input: &mut plots.input,
        }
        .widget_press(bar_id, pos, placement_id, Instant::now());
        Command::none()
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
        input::release_matches_press(target, pos, widget, button, now)
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
        Self::change_bar(plots, id, true, |local, _| {
            local.length_pct = value.clamp(1.0, 100.0)
        })
    }

    pub(crate) fn handle_set_thickness(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, true, |local, max| {
            local.thickness_px = value.clamp(1.0, max)
        })
    }

    pub(crate) fn handle_set_slots(
        plots: &mut Plots,
        id: window::Id,
        value: u32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, false, |local, _| {
            local.slots = value.clamp(1, TopLocal::MAX_SLOTS);
            local.ensure_aligns();
            local.ensure_widgets();
        })
    }

    /// Set one slot's child alignment (`TopEvent::Bar(BarEvent::SlotAlign)`): single
    /// commit per press (preset buttons, not a drag stream).
    pub(crate) fn handle_set_slot_align(
        plots: &mut Plots,
        id: window::Id,
        pos: usize,
        align: SlotAlign,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, false, |local, _| {
            local.ensure_aligns();
            if let Some(stored) = local.aligns.get_mut(pos) {
                *stored = align;
            }
        })
    }

    pub(crate) fn handle_widget_layout(
        plots: &mut Plots,
        id: window::Id,
        expected: Vec<Vec<crate::config::WidgetPlacement>>,
        widgets: Vec<Vec<crate::config::WidgetPlacement>>,
    ) -> Command<Plant> {
        if !Self::edit_context(plots).widget_layout(id, &expected, widgets) {
            return Command::none();
        }
        Self::render_bar_widgets(plots, id);
        Self::persist_bar(plots, id)
    }

    /// Pure override application behind [`handle_widget_prop`]
    /// (tested directly; the handler only locates the placement).
    /// Blank keys are ignored — they can only arrive from hand-edited
    /// TOML, never from the editor.
    pub(crate) fn apply_prop_patch(
        placement: &mut crate::config::WidgetPlacement,
        patch: &crate::shell::PlacementProp,
    ) {
        use crate::shell::PlacementProp;
        match patch {
            PlacementProp::Interval(value) => placement.interval = *value,
            PlacementProp::Size(value) => placement.size = *value,
            PlacementProp::Prop { key, value } => {
                if key.trim().is_empty() {
                    return;
                }
                match value {
                    Some(value) => {
                        placement.props.insert(key.clone(), value.clone());
                    }
                    None => {
                        placement.props.remove(key);
                    }
                }
            }
        }
    }

    /// Apply one placement property override (`None` clears back to
    /// inherit), then re-render the bar live and persist. Unknown
    /// placements are ignored (stale editor after a drag or reload).
    pub(crate) fn handle_widget_prop(
        plots: &mut Plots,
        id: window::Id,
        placement_id: &str,
        patch: crate::shell::PlacementProp,
    ) -> Command<Plant> {
        if !Self::edit_context(plots).widget_prop(id, placement_id, &patch) {
            return Command::none();
        }
        Self::render_bar_widgets(plots, id);
        Self::persist_bar(plots, id)
    }

    fn valid_widget_layout(
        live: &[Vec<crate::config::WidgetPlacement>],
        expected: &[Vec<crate::config::WidgetPlacement>],
        widgets: &[Vec<crate::config::WidgetPlacement>],
        defs: &[crate::config::WidgetDefinition],
    ) -> bool {
        if live != expected || widgets.len() != expected.len() || widgets == expected {
            return false;
        }
        if widgets.iter().any(|slot| {
            slot.iter().enumerate().any(|(i, placement)| {
                TopLocal::is_empty_widget(&placement.name)
                    || slot[..i].iter().any(|other| other.name == placement.name)
            })
        }) {
            return false;
        }
        // Existing missing definitions can be moved/removed; newly added
        // names must still exist after a discovery rescan.
        if widgets.iter().flatten().any(|placement| {
            !expected.iter().flatten().any(|old| old.id == placement.id)
                && !defs.iter().any(|def| def.name == placement.name)
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
        Self::change_bar(plots, id, false, |local, _| {
            local.slot_padding = value.clamp(0.0, TopLocal::MAX_SLOT_GAP)
        })
    }

    /// Set the slot gaps (`TopEvent::Bar(BarEvent::SlotSpacing)`): single commit per
    /// press, like padding.
    pub(crate) fn handle_set_slot_spacing(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, false, |local, _| {
            local.slot_spacing = value.clamp(0.0, TopLocal::MAX_SLOT_GAP)
        })
    }

    /// Minimum seconds between two `render()` calls of one Lua widget
    /// (keeps a `interval = 0` typo from hot-looping the update thread).
    const MIN_WIDGET_INTERVAL: f32 = 0.25;

    /// Resolve `(name, file-override)` to a script path (absolute
    /// paths pass through; empty `file` defaults to `<name>.lua`).
    pub(crate) fn widget_file_path(name: &str, file: &str) -> std::path::PathBuf {
        crate::config::discovery::widget_file_path(name, file)
    }

    /// Discover `widgets/*.lua` definitions with author metadata.
    /// Seeds missing scripts first (never overwrites). Each file loads
    /// in a throwaway sandbox — top-level code runs, but `view()` and
    /// friends are never called. Broken files log and resolve to
    /// fallback defaults (their states fail the same way later, with
    /// the usual once-per-message errors).
    pub(crate) fn discover_widget_defs() -> Vec<crate::config::WidgetDefinition> {
        use crate::config::WidgetsFile;
        WidgetsFile::seed_widget_scripts();
        let mut defs = Vec::new();
        for (name, path) in crate::config::discover_widget_files() {
            let (defaults, schema) = match std::fs::read_to_string(&path) {
                Err(e) => {
                    eprintln!(
                        "riced: widget {name:?}: cannot read {}: {e}",
                        path.display()
                    );
                    Default::default()
                }
                Ok(source) => match new_widget_lua() {
                    Err(e) => {
                        eprintln!("riced: widget {name:?}: cannot sandbox metadata load: {e}");
                        Default::default()
                    }
                    Ok(lua) => match load_widget_script(&lua, &name, &source) {
                        Err(e) => {
                            eprintln!("riced: widget {name:?}: metadata load failed: {e}");
                            Default::default()
                        }
                        Ok(()) => load_widget_meta(&lua, &name),
                    },
                },
            };
            defs.push(WidgetDefinition {
                name,
                file: path,
                defaults,
                schema,
            });
        }
        defs
    }

    /// One-time retirement of `widgets.toml`: fold per-def
    /// `interval`/`size`/`file` values into bar placement overrides
    /// (compared against Lua author defaults), persist the bars, and
    /// rename the file so later edits warn instead of silently doing
    /// nothing. Missing or unparseable files are left alone (the
    /// latter warns once per process via `widgets_toml_warned`).
    /// Runs once at startup, after discovery (needs Lua metadata).
    pub(crate) fn migrate_widgets_toml(plots: &mut Plots) {
        use crate::config::{WidgetDefaults, WidgetsFile};
        let path = crate::config::widgets_path();
        let content = match std::fs::read_to_string(&path) {
            Err(_) => return,
            Ok(content) => content,
        };
        let old_defs = match WidgetsFile::parse(&content) {
            Err(e) => {
                eprintln!(
                    "widgets: {} unparseable, leaving it in place: {e}",
                    path.display()
                );
                plots.catalog.retired_file_warned = true;
                return;
            }
            Ok(defs) => defs,
        };
        let lua_defaults: std::collections::HashMap<String, WidgetDefaults> = plots
            .catalog
            .definitions
            .iter()
            .map(|def| (def.name.clone(), def.defaults.clone()))
            .collect();
        let stamped =
            WidgetsFile::migrate_placements(&mut plots.config.bar, &old_defs, &lua_defaults);
        let before = std::fs::metadata(crate::config::config_path())
            .and_then(|m| m.modified())
            .ok();
        plots.config_mtime = plots.config.save().or(plots.config_mtime);
        let wrote = std::fs::metadata(crate::config::config_path())
            .and_then(|m| m.modified())
            .ok()
            != before;
        if !wrote {
            eprintln!(
                "widgets: could not persist migrated bars; leaving {} in place",
                path.display()
            );
            return;
        }
        let migrated = crate::config::widgets_migrated_path();
        match std::fs::rename(&path, &migrated) {
            Ok(()) => eprintln!(
                "widgets: retired {} → {} ({} placement(s) kept custom values)",
                path.display(),
                migrated.display(),
                stamped
            ),
            Err(e) => eprintln!(
                "widgets: migrated bars saved but cannot rename {}: {e}",
                path.display()
            ),
        }
    }

    /// Log a widget error once per message (a broken 1s script must not
    /// flood the log every tick; fixing the file logs nothing new until
    /// it breaks differently).
    pub(crate) fn note_widget_error(plots: &mut Plots, name: &str, err: String) {
        crate::lua::error::report_keyed(
            &mut plots.placements.errors,
            name,
            &format!("riced: widget {name:?}: "),
            err,
        );
    }

    /// Definition for a widget name (cloned for borrowck).
    fn def_for(plots: &Plots, widget: &str) -> Option<crate::config::WidgetDefinition> {
        plots
            .catalog
            .definitions
            .iter()
            .find(|d| d.name == widget)
            .cloned()
    }

    /// Resolve one placement to render inputs: definition defaults
    /// overlaid with placement overrides. `None` when the widget has
    /// no discovered definition (renders blank + warns, like a typo).
    pub(crate) fn resolve_placement(
        plots: &Plots,
        placement: &crate::config::WidgetPlacement,
    ) -> Option<ResolvedWidget> {
        let def = Self::def_for(plots, &placement.name)?;
        Some(ResolvedWidget {
            id: placement.id.clone(),
            name: placement.name.clone(),
            path: Self::widget_file_path(&placement.name, placement.file.as_deref().unwrap_or("")),
            interval: placement
                .effective_interval(&def.defaults)
                .max(Self::MIN_WIDGET_INTERVAL),
            size: placement.effective_size(&def.defaults),
            props: placement.resolve_props(&def.defaults.props),
        })
    }

    /// Ensure a sandboxed runtime for one placement (load + `view`
    /// check). Retried on later ticks while missing, so fixing the
    /// file recovers without a restart. States are independent per
    /// placement — two clocks never share `self`.
    pub(crate) fn ensure_widget_lua(
        plots: &mut Plots,
        resolved: &ResolvedWidget,
    ) -> Result<(), String> {
        plots.placements.ensure_instance(resolved)
    }

    /// Bars whose slots contain a placement (by id): its per-bar render
    /// targets. Placements in no bar render nowhere.
    fn bars_with_placement(plots: &Plots, id: &str) -> Vec<window::Id> {
        plots
            .windows
            .tops
            .iter()
            .filter(|(_, top)| {
                top.local
                    .widgets
                    .iter()
                    .any(|slot| slot.iter().any(|p| p.id == id))
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Every placement on every bar, resolved (unknown widgets skipped;
    /// they warn + render blank via the normal path).
    fn all_resolved(plots: &Plots) -> Vec<(window::Id, ResolvedWidget)> {
        let mut out = Vec::new();
        // Deterministic bar order keeps tick behavior stable.
        let mut bars: Vec<window::Id> = plots.windows.tops.keys().copied().collect();
        bars.sort_by_key(|id| format!("{id:?}"));
        for bar in bars {
            if let Some(top) = plots.windows.tops.get(&bar) {
                for slot in &top.local.widgets {
                    for placement in slot {
                        if let Some(resolved) = Self::resolve_placement(plots, placement) {
                            out.push((bar, resolved));
                        }
                    }
                }
            }
        }
        out
    }

    /// Publish fresh service tables plus this bar's `bar.output` and
    /// the placement's resolved `self.props`, then call one widget's
    /// `view()`, returning the raw value. Errors are returned for
    /// once-per-message logging by the caller.
    fn render_lua_value(
        plots: &mut Plots,
        resolved: &ResolvedWidget,
        gpu: Option<f32>,
        bar: window::Id,
    ) -> Result<Value, String> {
        Self::runtime_context(plots, gpu).render(resolved, bar)
    }

    fn runtime_context(plots: &mut Plots, gpu: Option<f32>) -> runtime::RuntimeContext<'_> {
        let outputs = plots
            .windows
            .tops
            .keys()
            .map(|bar| (*bar, Self::output_name(plots, *bar)))
            .collect();
        let paths = plots
            .windows
            .tops
            .values()
            .flat_map(|top| top.local.widgets.iter().flatten())
            .filter(|placement| !placement.id.is_empty())
            .map(|placement| {
                (
                    placement.id.clone(),
                    Self::widget_file_path(
                        &placement.name,
                        placement.file.as_deref().unwrap_or(""),
                    ),
                )
            })
            .collect();
        runtime::RuntimeContext {
            placements: &mut plots.placements,
            catalog: &mut plots.catalog.definitions,
            animation: animation::ListContext {
                lists: &mut plots.animation.widgets,
                motion: &mut plots.animation.motion,
                duration: plots.config.animation.speed.duration(),
            },
            services: crate::services::ServiceCtx {
                sys: &plots.services.system,
                gpu,
                theme: &plots.config.theme,
                outputs: &plots.windows.output_infos,
                notifications: &plots.notification.queue,
                toplevels: &plots.services.toplevels,
                workspaces: &plots.services.workspaces,
            },
            outputs,
            paths,
        }
    }

    /// Store one `render()` result: tables become [`WidgetNode`] trees,
    /// scalars become cached text. Switching shapes clears the other
    /// cache so nothing stale renders. Trees/text cache per (bar,
    /// placement); each placement animates under its own list scope.
    fn ingest_render_value(
        plots: &mut Plots,
        resolved: &ResolvedWidget,
        bar: window::Id,
        result: Result<Value, String>,
    ) {
        Self::runtime_context(plots, None).ingest(resolved, bar, result);
    }

    /// Drop a placement's runtime when its script file changed on
    /// disk, so the next render reloads it (live widget development).
    /// Unreadable files keep the old state.
    /// Returns true when runtimes were dropped (callers force a
    /// refresh: reloaded content may differ even if `view()` output
    /// doesn't, e.g. `popup()` edits on a slow-interval widget).
    /// Drop states (and refresh author metadata) when a script file
    /// changed under us. Mtime is tracked per resolved path; every
    /// placement rendering that file drops its state. Returns whether
    /// anything changed.
    pub(crate) fn sync_script_state(plots: &mut Plots, resolved: &ResolvedWidget) -> bool {
        Self::runtime_context(plots, None).sync_revision(resolved)
    }

    /// Re-resolve every live bar from its `[[bar]]` config entry
    /// (slot widgets, aligns, geometry). Bars whose entry vanished keep
    /// their current layout. Returns the touched bar ids. Pure
    /// state-sync (no rendering); callers follow with `apply_layout`
    /// + `init_widget_lua` since placement ids regenerate.
    pub(crate) fn resync_bars_from_config(plots: &mut Plots) -> Vec<window::Id> {
        let bars: Vec<window::Id> = plots.windows.tops.keys().copied().collect();
        for bar_id in &bars {
            let entry = plots
                .windows
                .tops
                .get(bar_id)
                .and_then(|top| plots.config.bar.get(top.bar_index).cloned());
            if let (Some(entry), Some(top)) = (entry, plots.windows.tops.get_mut(bar_id)) {
                top.local = TopLocal::from(&entry);
            }
        }
        bars
    }

    /// (Re)build runtimes for every placement and render once, so
    /// bars populate immediately. Called at startup and after every
    /// definition rescan (which clears the old states).
    /// Per-bar trees render when bars appear (`render_bar_widgets`)
    /// or on the next due tick; with no bars yet this only resets.
    pub(crate) fn init_widget_lua(plots: &mut Plots) {
        plots.placements.reset();
        // Fresh Lua states mean fresh lists: drop every per-widget
        // list (their runtime slots free on the next sweep; the first
        // post-reload paint re-enters from scratch and settles).
        plots.animation.reset_widgets();
        plots.services.refresh_system();
        let gpu = Popup::gpu_usage_percent();
        let now = Instant::now();
        for (bar, resolved) in Self::all_resolved(plots) {
            plots.placements.last_run.insert(resolved.id.clone(), now);
            let result = Self::render_lua_value(plots, &resolved, gpu, bar);
            Self::ingest_render_value(plots, &resolved, bar, result);
        }
        Self::warn_unknown_slot_widgets(plots);
    }

    /// Render every placement slotted on a bar (bar creation at startup
    /// or output hotplug): without this a new bar paints empty until
    /// each placement's interval elapses.
    pub(crate) fn render_bar_widgets(plots: &mut Plots, bar: window::Id) {
        plots.services.refresh_system();
        let gpu = Popup::gpu_usage_percent();
        let now = Instant::now();
        let placements: Vec<crate::config::WidgetPlacement> = plots
            .windows
            .tops
            .get(&bar)
            .map(|top| {
                top.local
                    .widgets
                    .iter()
                    .flatten()
                    .filter(|p| !p.id.is_empty())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for placement in &placements {
            let Some(resolved) = Self::resolve_placement(plots, placement) else {
                continue;
            };
            plots.placements.last_run.insert(resolved.id.clone(), now);
            let result = Self::render_lua_value(plots, &resolved, gpu, bar);
            Self::ingest_render_value(plots, &resolved, bar, result);
        }
    }

    /// Warn about slot placements that resolve to no discovered
    /// definition (typos and deleted files render blank).
    /// Runs at startup and on every hot-reload, when configs change.
    fn warn_unknown_slot_widgets(plots: &Plots) {
        for (bar_id, top) in &plots.windows.tops {
            for (pos, slot) in top.local.widgets.iter().enumerate() {
                for placement in slot {
                    if !TopLocal::is_empty_widget(&placement.name)
                        && !plots
                            .catalog
                            .definitions
                            .iter()
                            .any(|d| d.name == placement.name)
                    {
                        eprintln!(
                            "riced: bar {bar_id:?} slot {} references unknown widget {:?} (no widgets/{}.lua)",
                            pos + 1,
                            placement.name,
                            placement.name,
                        );
                    }
                }
            }
        }
    }

    /// Run every `view()` whose interval elapsed. Returns `true`
    /// when any output moved (caller repaints). Intervals are
    /// per-placement (placement override else definition default).
    fn run_due_widgets(plots: &mut Plots) -> bool {
        let jobs = Self::all_resolved(plots);
        let now = Instant::now();
        plots.services.refresh_system();
        let gpu = crate::services::gpu::gpu_usage_percent();
        let open_popups = plots
            .windows
            .popups
            .values()
            .map(|popup| popup.placement.clone())
            .collect();
        scheduling::run_due(
            &mut Self::runtime_context(plots, gpu),
            jobs,
            &open_popups,
            now,
        )
    }

    /// Re-render one widget on every bar showing it, and report whether
    /// any visible output moved (text or tree). Shared by the interval
    /// tick, the `on_press()` click path, cell actions, and popup selects.
    pub(crate) fn refresh_widget(plots: &mut Plots, placement_id: &str, gpu: Option<f32>) -> bool {
        let bars = Self::bars_with_placement(plots, placement_id);
        let before: Vec<_> = bars
            .iter()
            .map(|bar| {
                (
                    plots
                        .placements
                        .outputs
                        .get(&(*bar, placement_id.to_string()))
                        .cloned(),
                    plots
                        .placements
                        .trees
                        .get(&(*bar, placement_id.to_string()))
                        .cloned(),
                )
            })
            .collect();
        for bar in &bars {
            let Some(resolved) = Self::resolve_placement_id(plots, placement_id) else {
                continue;
            };
            let result = Self::render_lua_value(plots, &resolved, gpu, *bar);
            Self::ingest_render_value(plots, &resolved, *bar, result);
        }
        let after: Vec<_> = bars
            .iter()
            .map(|bar| {
                (
                    plots
                        .placements
                        .outputs
                        .get(&(*bar, placement_id.to_string()))
                        .cloned(),
                    plots
                        .placements
                        .trees
                        .get(&(*bar, placement_id.to_string()))
                        .cloned(),
                )
            })
            .collect();
        before != after
    }

    /// Resolve one placement to render inputs by id, searching every
    /// bar's slots. `None` when the id is gone (stale message after a
    /// drag or hot-reload).
    pub(crate) fn resolve_placement_id(
        plots: &Plots,
        placement_id: &str,
    ) -> Option<ResolvedWidget> {
        plots
            .windows
            .tops
            .values()
            .flat_map(|top| top.local.widgets.iter().flatten())
            .find(|placement| placement.id == placement_id)
            .and_then(|placement| Self::resolve_placement(plots, placement))
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
        plots.animation.advance(Instant::now());
        super::notification::sweep_noti_anims(plots)
    }

    /// Click a cell button: run the owning placement's `on_action(key)`
    /// (the view closure stamps the placement id, so the key routes to
    /// its own Lua state — no slot-wide popup/`on_press` fallback), then
    /// re-render that placement like the click path does. `on_action`
    /// sees the clicking bar's `bar.output` and the placement's
    /// resolved `self.props`.
    pub(crate) fn handle_cell_action(
        plots: &mut Plots,
        bar: window::Id,
        placement_id: String,
        action: String,
    ) -> Command<Plant> {
        let gpu = Popup::gpu_usage_percent();
        let resolved = Self::resolve_placement_id(plots, &placement_id);
        let outcome = resolved.as_ref().and_then(|resolved| {
            let output = Self::output_name(plots, bar);
            plots.placements.action(
                resolved,
                &crate::lua::widgets::EntryContext {
                    services: crate::services::ServiceCtx::from_plots(plots, gpu),
                    output: &output,
                },
                &action,
            )
        });
        match outcome {
            Some(Ok(value)) => {
                plots.placements.errors.remove(&placement_id);
                let notif = super::notification::command_from_action(&value, plots);
                if Self::refresh_widget(plots, &placement_id, gpu) {
                    return Command::batch(vec![
                        Command::done(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Changed))),
                        notif,
                    ]);
                }
                return notif;
            }
            Some(Err(e)) => {
                let label = resolved
                    .map(|r| r.label())
                    .unwrap_or_else(|| placement_id.clone());
                Self::note_widget_error(plots, &label, e)
            }
            // Unknown placement: ignore (stale message after hot-reload).
            None => {}
        }
        Command::none()
    }

    pub(crate) fn handle_set_opacity(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, false, |local, _| {
            local.opacity = TopLocal::snap_opacity(value)
        })
    }

    pub(crate) fn handle_set_floating(
        plots: &mut Plots,
        id: window::Id,
        value: bool,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, true, |local, _| local.floating = value)
    }

    pub(crate) fn handle_set_margin_top(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, true, |local, _| local.margins.top = value.max(0))
    }

    pub(crate) fn handle_set_margin_right(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, true, |local, _| {
            local.margins.right = value.max(0)
        })
    }

    pub(crate) fn handle_set_margin_bottom(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, true, |local, _| {
            local.margins.bottom = value.max(0)
        })
    }

    pub(crate) fn handle_set_margin_left(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, true, |local, _| {
            local.margins.left = value.max(0)
        })
    }

    pub(crate) fn handle_set_radius_tl(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, false, |local, _| {
            local.radius.top_left = value.max(0.0)
        })
    }

    pub(crate) fn handle_set_radius_tr(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, false, |local, _| {
            local.radius.top_right = value.max(0.0)
        })
    }

    pub(crate) fn handle_set_radius_bl(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, false, |local, _| {
            local.radius.bottom_left = value.max(0.0)
        })
    }

    pub(crate) fn handle_set_radius_br(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        Self::change_bar(plots, id, false, |local, _| {
            local.radius.bottom_right = value.max(0.0)
        })
    }

    /// Connector name for a bar's output (`""` when unknown/sentinel).
    pub(crate) fn output_name(plots: &Plots, bar_id: window::Id) -> String {
        plots
            .windows
            .ids
            .get(&bar_id)
            .copied()
            .and_then(|info| match info {
                PlotInfo::Top(o) => plots.windows.output_infos.get(&o),
                _ => None,
            })
            .and_then(|info| info.name.clone())
            .unwrap_or_default()
    }

    /// Serialize one bar's slots with effective values materialized,
    /// so the file always shows exactly what the bar renders.
    /// Placements of unknown widgets persist as written.
    fn serialize_slots(
        plots: &Plots,
        widgets: &[Vec<crate::config::WidgetPlacement>],
    ) -> Vec<crate::config::SlotWidgets> {
        edit::serialize_slots(&plots.catalog.definitions, widgets)
    }

    /// Write the bar's current state back to its `[[bar]]` entry and
    /// arm a coalesced config save (same idle-write as `ConfigPatch`
    /// drags). Entries are matched by index; out-of-range indices push.
    pub(crate) fn persist_bar(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        if Self::edit_context(plots).persist(bar_id) {
            plots.arm_config_save()
        } else {
            Command::none()
        }
    }

    fn edit_context(plots: &mut Plots) -> edit::EditContext<'_> {
        edit::EditContext {
            bars: &mut plots.windows.tops,
            identities: &plots.windows.ids,
            outputs: &plots.windows.output_infos,
            catalog: &plots.catalog.definitions,
            saved: &mut plots.config.bar,
        }
    }

    fn change_bar(
        plots: &mut Plots,
        bar: window::Id,
        relayout: bool,
        change: impl FnOnce(&mut TopLocal, f32),
    ) -> Command<Plant> {
        let (layout, persist) = Self::edit_context(plots).change(bar, relayout, change);
        if persist {
            Command::batch([layout, plots.arm_config_save()])
        } else {
            layout
        }
    }

    /// Remove a bar (`TopEvent::Remove`): drop tracking + cursor state,
    /// close its window, and delete its `[[bar]]` entry (persisted
    /// immediately) so it stays gone after restart.
    pub(crate) fn handle_remove(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        let mut cmds = Vec::new();
        // A bar going away takes its popup with it.
        let popup_id = plots
            .windows
            .popups
            .iter()
            .find(|(_, p)| p.bar_id == bar_id)
            .map(|(id, _)| *id);
        if let Some(pid) = popup_id {
            cmds.push(super::Popup::handle_dismiss(plots, pid));
        }
        if let Some(top) = plots.windows.tops.remove(&bar_id) {
            if top.bar_index < plots.config.bar.len() {
                plots.config.bar.remove(top.bar_index);
                // Indices after the hole shift down by one.
                for other in plots.windows.tops.values_mut() {
                    if other.bar_index > top.bar_index {
                        other.bar_index -= 1;
                    }
                }
            }
            // flush_config_save only writes when dirty — mark it first
            // (same for persist_new below).
            plots.config_jobs.dirty = true;
            plots.flush_config_save();
        }
        plots.forget_window(bar_id);
        cmds.push(iced_runtime::task::effect(Action::Window(
            WindowAction::Close(bar_id),
        )));
        Command::batch(cmds)
    }

    /// Push the bar's current size/exclusive/margins to its live window.
    /// Same window id throughout — no close/reopen flicker. Skips sentinel
    /// windows (fixed fallback until outputs arrive and replace them).
    pub(crate) fn apply_layout(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        Self::edit_context(plots).layout(bar_id)
    }

    /// Re-apply every bar on `output` (resolution/scale changed geometry:
    /// `%` sizes now resolve to different px). Called on `OutputUpdated`.
    pub(crate) fn reapply_for_output(plots: &mut Plots, output: OutputId) -> Command<Plant> {
        let bars: Vec<window::Id> = plots
            .windows
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
        let target = plots.input.presses.remove(&id);
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
        placement_id: String,
    ) -> Command<Plant> {
        if !matches!(plots.id_info(bar_id), Some(PlotInfo::Top(_))) {
            return Command::none();
        }
        let target = plots.input.presses.remove(&bar_id);
        if Self::release_matches_press(
            target,
            pos,
            Some(&placement_id),
            Button::Left,
            Instant::now(),
        ) {
            return Self::handle_widget_click(plots, bar_id, pos, &placement_id);
        }
        Command::none()
    }

    /// Left-click on a bar under the hold threshold: resolve the slot
    /// under the cursor and either toggle its widget popup or run its
    /// `on_press()` action. A slot holding both prefers the popup.
    pub(crate) fn handle_slot_click(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        let (output, top) = match plots.windows.ids.get(&bar_id).copied() {
            Some(PlotInfo::Top(o)) => match plots.windows.tops.get(&bar_id).cloned() {
                Some(t) => (o, t),
                None => return Command::none(),
            },
            _ => return Command::none(),
        };
        let Some((_, _, sw, sh)) = Background::available_rect(output, &plots.windows.output_infos)
        else {
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
        let cursor = plots.input.cursors.get(&bar_id).copied();
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
                        .any(|p| lua_has_func(&plots.placements.instances, &p.id, "popup"))
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
            .windows
            .popups
            .iter()
            .find(|(_, p)| p.bar_id == bar_id)
            .map(|(id, _)| *id)
        {
            let same = plots
                .windows
                .popups
                .get(&pid)
                .is_some_and(|p| p.slot == pos);
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

    /// Click on one placement's own mouse area (renderer hit-tested,
    /// so every placement in a slot gets its own calls): toggle its
    /// popup, run its `on_press()`, or fall back to the slot when it
    /// defines neither. A popup open for another placement is replaced.
    pub(crate) fn handle_widget_click(
        plots: &mut Plots,
        bar_id: window::Id,
        pos: usize,
        placement_id: &str,
    ) -> Command<Plant> {
        let output = match plots.windows.ids.get(&bar_id).copied() {
            Some(PlotInfo::Top(o)) => o,
            _ => return Command::none(),
        };
        let cursor = plots
            .input
            .cursors
            .get(&bar_id)
            .copied()
            .map(|p| (p.x, p.y));
        let mut cmds = Vec::new();
        if let Some(pid) = plots
            .windows
            .popups
            .iter()
            .find(|(_, p)| p.bar_id == bar_id)
            .map(|(id, _)| *id)
        {
            let same_placement = plots
                .windows
                .popups
                .get(&pid)
                .is_some_and(|p| p.placement == placement_id);
            cmds.push(Popup::handle_dismiss(plots, pid));
            if same_placement {
                return Command::batch(cmds);
            }
        }
        if lua_has_func(&plots.placements.instances, placement_id, "popup") {
            if let Some(cmd) =
                Popup::open_for(plots, bar_id, output, pos, cursor, Some(placement_id))
            {
                cmds.push(cmd);
            }
        } else if lua_has_func(&plots.placements.instances, placement_id, "on_press") {
            cmds.push(Self::run_on_press(plots, bar_id, placement_id));
        } else {
            let placements = plots
                .windows
                .tops
                .get(&bar_id)
                .map(|t| t.local.widgets_at(pos).to_vec())
                .unwrap_or_default();
            cmds.push(Self::handle_slot_fallback(
                plots,
                bar_id,
                output,
                pos,
                &placements,
                cursor,
            ));
        }
        if cmds.is_empty() {
            return Command::none();
        }
        Command::batch(cmds)
    }

    /// Slot fallback for gap clicks and action-less placements: open the
    /// first popup in the slot, else run the first `on_press()`.
    fn handle_slot_fallback(
        plots: &mut Plots,
        bar_id: window::Id,
        output: OutputId,
        pos: usize,
        placements: &[crate::config::WidgetPlacement],
        cursor: Option<(f32, f32)>,
    ) -> Command<Plant> {
        if placements
            .iter()
            .any(|p| lua_has_func(&plots.placements.instances, &p.id, "popup"))
        {
            if let Some(cmd) = Popup::open_for(plots, bar_id, output, pos, cursor, None) {
                return cmd;
            }
            return Command::none();
        }
        // No menu: run the first `on_press()` action in the slot, then
        // re-render that placement (a toggle flips its next output).
        if let Some(placement) = placements
            .iter()
            .find(|p| lua_has_func(&plots.placements.instances, &p.id, "on_press"))
        {
            return Self::run_on_press(plots, bar_id, &placement.id.clone());
        }
        Command::none()
    }

    /// Run one widget's `on_press()` click action, then re-render it
    /// (a toggle flips its next output). Errors log once-per-message.
    /// `on_action` sees the clicking bar's `bar.output`.
    fn run_on_press(plots: &mut Plots, bar: window::Id, placement_id: &str) -> Command<Plant> {
        let gpu = Popup::gpu_usage_percent();
        let resolved = Self::resolve_placement_id(plots, placement_id);
        let outcome = resolved.as_ref().and_then(|resolved| {
            let output = Self::output_name(plots, bar);
            plots.placements.press(
                resolved,
                &crate::lua::widgets::EntryContext {
                    services: crate::services::ServiceCtx::from_plots(plots, gpu),
                    output: &output,
                },
            )
        });
        match outcome {
            Some(Ok(())) => {
                if Self::refresh_widget(plots, placement_id, gpu) {
                    return Command::done(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Changed)));
                }
            }
            Some(Err(e)) => {
                let label = resolved
                    .map(|r| r.label())
                    .unwrap_or_else(|| placement_id.to_string());
                Self::note_widget_error(plots, &label, e)
            }
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
            let widgets: Vec<crate::config::SlotWidgets> = Top::serialize_slots(plots, &l.widgets);
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
            plots.config_jobs.dirty = true;
            plots.flush_config_save();
        }

        let (target_output, target_anchor): (Option<OutputId>, Anchor) = {
            if let Some(output) = menu_output {
                if let Some(info) = plots.windows.output_infos.get(&output) {
                    let anchor = menu_pos.map_or(Anchor::Top, |mp| closest_anchor(mp, info));
                    (Some(output), anchor)
                } else {
                    (Some(output), Anchor::Top)
                }
            } else if let Some(mp) = menu_pos {
                let mut found = plots.windows.output_infos.iter().find(|(_, info)| {
                    let (sx, sy, sw, sh) = Geo::output_geometry(info);
                    mp.x >= sx && mp.x < sx + sw && mp.y >= sy && mp.y < sy + sh
                });
                if found.is_none() && !plots.windows.output_infos.is_empty() {
                    let mut best: Option<(&OutputId, &OutputInfo, f32)> = None;
                    for (oid, info) in &plots.windows.output_infos {
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
            } else if let Some(oid) = plots.windows.output_infos.keys().next().copied() {
                (Some(oid), Anchor::Top)
            } else {
                (None, Anchor::Top)
            }
        };

        if let Some(output_id) = target_output {
            let anchor = target_anchor;
            let duplicate = plots.windows.ids.iter().any(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == output_id => plots
                    .windows
                    .tops
                    .get(wid)
                    .is_some_and(|t| t.anchor() == anchor),
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
            let (sw, sh) = Self::output_size(&plots.windows.output_infos, output_id);
            let (w, h) = top.local.px_size(sw, sh, top.is_horizontal());
            let (win_id, settings) = top.open(output_id.0, w, h);
            let output = plots
                .windows
                .output_infos
                .get(&output_id)
                .and_then(|info| info.name.clone())
                .unwrap_or_default();
            persist_new(plots, &mut top, anchor, output);
            plots.windows.tops.insert(win_id, top);
            plots.windows.ids.insert(win_id, PlotInfo::Top(output_id));
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
                .windows
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
            plots.windows.tops.insert(win_id, top);
            plots.windows.ids.insert(win_id, PlotInfo::Top(sentinel));
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

    fn lua_widget_def(name: &str) -> crate::config::WidgetDefinition {
        crate::config::WidgetDefinition {
            name: name.to_string(),
            file: std::path::PathBuf::from(format!("{name}.lua")),
            defaults: crate::config::WidgetDefaults::default(),
            schema: HashMap::new(),
        }
    }

    fn place(id: &str, widget: &str) -> crate::config::WidgetPlacement {
        crate::config::WidgetPlacement {
            id: id.to_string(),
            name: widget.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn lua_cell_text_reads_cache_with_size() {
        use crate::config::WidgetPlacement;
        let bar = window::Id::unique();
        let defs = vec![lua_widget_def("w")];
        let place = |id: &str| WidgetPlacement {
            id: id.to_string(),
            name: "w".to_string(),
            ..Default::default()
        };
        let empty: HashMap<(window::Id, String), String> = HashMap::new();
        assert_eq!(lua_cell_text(bar, &place("w1"), &defs, &empty), None);
        let mut outputs = HashMap::new();
        outputs.insert((bar, "w1".to_string()), "hi".to_string());
        assert_eq!(
            lua_cell_text(bar, &place("w1"), &defs, &outputs),
            Some(("hi".to_string(), 13.0))
        );
        // Unknown placements never read the cache.
        assert_eq!(lua_cell_text(bar, &place("nope"), &defs, &outputs), None);
        // Another bar's cache never leaks across.
        assert_eq!(
            lua_cell_text(window::Id::unique(), &place("w1"), &defs, &outputs),
            None
        );
    }

    #[test]
    fn preview_drop_rejects_stale_layout_and_removed_pool_definitions() {
        let old = vec![vec![place("w1", "missing")], vec![]];
        let moved = vec![vec![], vec![place("w1", "missing")]];
        assert!(Top::valid_widget_layout(&old, &old, &moved, &[]));
        assert!(!Top::valid_widget_layout(&moved, &old, &moved, &[]));
        let added = vec![vec![place("w1", "missing"), place("w2", "clock")], vec![]];
        assert!(!Top::valid_widget_layout(&old, &old, &added, &[]));
        assert!(Top::valid_widget_layout(
            &old,
            &old,
            &added,
            &[lua_widget_def("clock")]
        ));
        let duplicates = vec![vec![place("w1", "missing"), place("w3", "missing")], vec![]];
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
        use crate::config::{SlotEntry, SlotWidgets, TopConfig};
        let cfg = TopConfig {
            slots: 3,
            widgets: vec![
                SlotWidgets::One("clock".to_string()),
                SlotWidgets::One("none".to_string()),
                SlotWidgets::Many(vec![
                    SlotEntry::Name("cpu".to_string()),
                    SlotEntry::Name("none".to_string()),
                    SlotEntry::Name("ram".to_string()),
                ]),
            ],
            ..Default::default()
        };
        let local = TopLocal::from(&cfg);
        let names: Vec<Vec<String>> = local
            .widgets
            .iter()
            .map(|slot| slot.iter().map(|p| p.name.clone()).collect())
            .collect();
        assert_eq!(
            names,
            vec![
                vec!["clock".to_string()],
                Vec::<String>::new(),
                vec!["cpu".to_string(), "ram".to_string()],
            ]
        );
        // Fresh placements get stable unique ids.
        let ids: Vec<&String> = local.widgets.iter().flatten().map(|p| &p.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|id| !id.is_empty()));
    }

    #[test]
    fn resync_bars_applies_edited_slot_widgets() {
        use crate::shell::Plots;
        use iced_wayland_subscriber::shell::channel;
        let (_tx, rx) = channel();
        let mut plots = Plots::new(rx);
        let bar = window::Id::unique();
        plots
            .windows
            .tops
            .insert(bar, Top::with_config(0, Anchor::Top, TopLocal::default()));
        // Edited config: clock with an override plus a bare stats entry.
        plots.config.bar = vec![crate::config::TopConfig {
            slots: 2,
            widgets: vec![
                crate::config::SlotWidgets::Many(vec![crate::config::SlotEntry::Full(
                    crate::config::WidgetPlacement {
                        name: "clock".to_string(),
                        size: Some(16.0),
                        ..Default::default()
                    },
                )]),
                crate::config::SlotWidgets::Many(vec![crate::config::SlotEntry::Name(
                    "stats".to_string(),
                )]),
            ],
            ..Default::default()
        }];
        assert_eq!(Top::resync_bars_from_config(&mut plots), vec![bar]);
        let top = plots.windows.tops.get(&bar).expect("bar kept");
        assert_eq!(top.local.widgets.len(), 2);
        let clock = &top.local.widgets[0][0];
        assert_eq!(clock.name, "clock");
        assert_eq!(clock.size, Some(16.0));
        assert!(!clock.id.is_empty(), "placements get stable ids");
        let stats = &top.local.widgets[1][0];
        assert_eq!(stats.name, "stats");
        assert!(!stats.id.is_empty());
        // Unknown bar index keeps its layout instead of clearing.
        plots
            .windows
            .tops
            .get_mut(&bar)
            .expect("bar kept")
            .bar_index = 99;
        Top::resync_bars_from_config(&mut plots);
        assert_eq!(
            plots
                .windows
                .tops
                .get(&bar)
                .expect("bar kept")
                .local
                .widgets
                .len(),
            2
        );
    }

    #[test]
    fn apply_prop_patch_sets_and_clears() {
        use crate::config::{PropValue, WidgetPlacement};
        use crate::shell::PlacementProp;
        let mut placement = WidgetPlacement {
            id: "w1".to_string(),
            name: "clock".to_string(),
            ..Default::default()
        };
        Top::apply_prop_patch(&mut placement, &PlacementProp::Interval(Some(5.0)));
        Top::apply_prop_patch(
            &mut placement,
            &PlacementProp::Prop {
                key: "format".to_string(),
                value: Some(PropValue::Text("%H".to_string())),
            },
        );
        assert_eq!(placement.interval, Some(5.0));
        assert_eq!(placement.props["format"], PropValue::Text("%H".to_string()));
        // Clearing removes the key; blank keys never land.
        Top::apply_prop_patch(
            &mut placement,
            &PlacementProp::Prop {
                key: "format".to_string(),
                value: None,
            },
        );
        assert!(!placement.props.contains_key("format"));
        Top::apply_prop_patch(
            &mut placement,
            &PlacementProp::Prop {
                key: "  ".to_string(),
                value: Some(PropValue::Bool(true)),
            },
        );
        assert!(placement.props.is_empty());
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
    fn build_anim_list_merges_live_and_ghosts() {
        use crate::ui::listview::{Axis, ListView};
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
                background: None,
                radius: None,
            },
            WidgetNode::Button {
                label: "[2]".to_string(),
                action: "go:2".to_string(),
                width: None,
                height: None,
                padding: None,
                color: None,
                background: None,
                radius: None,
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
                    background: None,
                    radius: None,
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
