use super::Popup;
use super::background::Background;
use crate::app::app::{PlotInfo, Plots};
use crate::app::{Plant, TopEvent};
use crate::composables::panel_window::top_window;
use crate::config::WidgetDef;
use crate::theme;
use iced::mouse::Button;
use iced::widget::{Space, column, container, row, text};
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
/// `memory_stick` and `MemoryStick` all work. Unknown names render as
/// literal text so typos stay visible.
fn icon_bytes(name: &str) -> Option<&'static [u8]> {
    use lucide_iced::bytes::*;
    let key: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    match key.as_str() {
        "cpu" => Some(CPU),
        "gpu" => Some(GPU),
        "memory" | "memorystick" | "mem" | "ram" => Some(MEMORY_STICK),
        "thermometer" | "temp" => Some(THERMOMETER),
        "thermometersun" => Some(THERMOMETER_SUN),
        "harddrive" | "disk" => Some(HARD_DRIVE),
        "wifi" => Some(WIFI),
        "wifioff" => Some(WIFI_OFF),
        "signal" => Some(SIGNAL),
        "network" => Some(NETWORK),
        "battery" => Some(BATTERY),
        "batterycharging" => Some(BATTERY_CHARGING),
        "activity" => Some(ACTIVITY),
        "gauge" => Some(GAUGE),
        "chartline" | "chart" => Some(CHART_LINE),
        "zap" => Some(ZAP),
        "fan" => Some(FAN),
        "monitor" => Some(MONITOR),
        "volume2" | "volume" | "vol" => Some(VOLUME_2),
        "volumex" | "mute" => Some(VOLUME_X),
        "heart" => Some(HEART),
        "heartpulse" => Some(HEART_PULSE),
        "clock" => Some(CLOCK),
        "calendar" => Some(CALENDAR),
        "sun" => Some(SUN),
        "moon" => Some(MOON),
        "cloud" => Some(CLOUD),
        "download" => Some(DOWNLOAD),
        "upload" => Some(UPLOAD),
        "arrowup" | "up" => Some(ARROW_UP),
        "arrowdown" | "down" => Some(ARROW_DOWN),
        "power" => Some(POWER),
        "settings" => Some(SETTINGS),
        "bell" => Some(BELL),
        _ => None,
    }
}

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
fn render_slot_widgets(
    names: &[String],
    defs: &[WidgetDef],
    outputs: &HashMap<String, String>,
    gap: f32,
    horizontal: bool,
) -> Element<'static, Plant> {
    let mut items = Vec::new();
    for name in names {
        if TopLocal::is_empty_widget(name) {
            continue;
        }
        if let Some((output, size)) = lua_cell_text(name, defs, outputs) {
            items.push(rich_text(output, size, gap));
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

/// Sandboxed Lua state for one widget: string/table/math/os only, no
/// `io`, no `require`, no shell or file escapes from `os`. `print`
/// stays so scripts can log to the daemon output.
fn new_widget_lua() -> mlua::Result<Lua> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::OS,
        LuaOptions::default(),
    )?;
    let globals = lua.globals();
    for key in ["dofile", "loadfile", "require"] {
        globals.set(key, Value::Nil)?;
    }
    let os: Table = globals.get("os")?;
    for key in ["execute", "exit", "remove", "rename", "setlocale"] {
        os.set(key, Value::Nil)?;
    }
    Ok(lua)
}

/// Load a widget script into its state and verify it defines `render`.
fn load_widget_script(lua: &Lua, label: &str, source: &str) -> mlua::Result<()> {
    lua.load(source).set_name(format!("@{label}")).exec()?;
    let _: Function = lua.globals().get("render")?;
    Ok(())
}

fn lua_value_kind(value: &Value) -> &'static str {
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

/// Call a widget script function (`render`, `popup`, ...), tolerantly
/// coerced to text (numbers and booleans stringify, `nil` is empty).
/// Anything else is an error.
pub(crate) fn call_lua_text(lua: &Lua, func: &str) -> Result<String, String> {
    let render: Function = lua.globals().get(func).map_err(|e| e.to_string())?;
    match render.call::<Value>(()).map_err(|e| e.to_string())? {
        Value::String(s) => Ok(s.to_string_lossy()),
        Value::Integer(i) => Ok(i.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Boolean(b) => Ok(b.to_string()),
        Value::Nil => Ok(String::new()),
        other => Err(format!(
            "{func}() must return a string, got {}",
            lua_value_kind(&other)
        )),
    }
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

    pub fn view(
        &self,
        id: window::Id,
        widgets: &[WidgetDef],
        outputs: &HashMap<String, String>,
    ) -> Element<'_, Plant> {
        let n = self.local.slots.clamp(1, TopLocal::MAX_SLOTS) as usize;
        let gap = self.local.slot_spacing.clamp(0.0, TopLocal::MAX_SLOT_GAP);
        let pad = self.local.slot_padding.clamp(0.0, TopLocal::MAX_SLOT_GAP);
        let horizontal = self.is_horizontal();
        let cell = |pos: usize| -> Element<'_, Plant> {
            let body: Element<'_, Plant> = render_slot_widgets(
                self.local.widgets_at(pos),
                widgets,
                outputs,
                gap,
                horizontal,
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
        plots.press_starts.insert(id, Instant::now());
        println!("top press {button:?} on {id:?}");
        Command::none()
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
                let horizontal = plots.tops.get(&id).map_or(true, |t| t.is_horizontal());
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
    fn ensure_widget_lua(plots: &mut Plots, def: &crate::config::WidgetDef) -> Result<(), String> {
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

    /// Publish fresh system tables and call one widget's `render()`.
    /// Errors are returned for once-per-message logging by the caller.
    fn render_lua_widget(
        plots: &mut Plots,
        def: &crate::config::WidgetDef,
        gpu: Option<f32>,
    ) -> Result<String, String> {
        Self::sync_script_state(plots, def);
        Self::ensure_widget_lua(plots, def)?;
        let lua = plots
            .widget_lua
            .get(&def.name)
            .ok_or_else(|| "runtime missing".to_string())?;
        publish_system_tables(lua, &plots.sysinfo, gpu).map_err(|e| e.to_string())?;
        call_lua_text(lua, "render")
    }

    /// Drop a widget's runtime when its script file changed on disk, so
    /// the next render reloads it (live widget development without
    /// touching `widgets.toml`). Unreadable files keep the old state.
    fn sync_script_state(plots: &mut Plots, def: &crate::config::WidgetDef) {
        let path = Self::widget_script_path(def);
        let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            return;
        };
        match plots.widget_script_mtime.get(&def.name) {
            Some(known) if *known == mtime => {}
            _ => {
                plots.widget_lua.remove(&def.name);
                plots.widget_script_mtime.insert(def.name.clone(), mtime);
            }
        }
    }

    /// (Re)build runtimes for every def and render once, so bars
    /// populate immediately. Called at startup and after every
    /// `widgets.toml` hot-reload (which clears the old states).
    pub(crate) fn init_widget_lua(plots: &mut Plots) {
        plots.widget_lua.clear();
        plots.widget_outputs.clear();
        plots.widget_last_run.clear();
        plots.widget_last_error.clear();
        plots.widget_script_mtime.clear();
        plots.sysinfo.refresh_cpu_usage();
        plots.sysinfo.refresh_memory();
        let gpu = Popup::gpu_usage_percent();
        let defs = plots.widgets.clone();
        let now = Instant::now();
        for def in &defs {
            plots.widget_last_run.insert(def.name.clone(), now);
            match Self::render_lua_widget(plots, def, gpu) {
                Ok(text) => {
                    plots.widget_outputs.insert(def.name.clone(), text);
                }
                Err(e) => Self::note_widget_error(plots, &def.name, e),
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
            if !due {
                continue;
            }
            plots.widget_last_run.insert(def.name.clone(), now);
            match Self::render_lua_widget(plots, def, gpu) {
                Ok(text) => {
                    plots.widget_last_error.remove(&def.name);
                    if plots.widget_outputs.get(&def.name) != Some(&text) {
                        plots.widget_outputs.insert(def.name.clone(), text);
                        changed = true;
                    }
                }
                Err(e) => Self::note_widget_error(plots, &def.name, e),
            }
        }
        changed
    }

    /// Re-render due Lua widgets (`TopEvent::WidgetTick`): emits
    /// `WidgetsChanged` only when an output moved (which repaints).
    pub(crate) fn handle_widget_tick(plots: &mut Plots) -> Command<Plant> {
        if Self::run_due_widgets(plots) {
            return Command::done(Plant::TopPlot(TopEvent::WidgetsChanged));
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
        plots.press_starts.remove(&bar_id);
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
        let start = plots.press_starts.remove(&id);
        let clicked = button == Button::Left
            && matches!(&start, Some(t) if t.elapsed() < Self::HOLD_THRESHOLD);
        match start {
            Some(t) if t.elapsed() >= Self::HOLD_THRESHOLD => {
                println!("top hold {button:?} on {id:?} after {:?}", t.elapsed());
            }
            Some(t) => {
                println!("top click {button:?} on {id:?} after {:?}", t.elapsed());
            }
            // Silent: the global Graft release safety net in update can clear
            // the press first when release lands on another window, and a
            // same-window release fires both PanelWindow and Graft paths.
            None => {}
        }
        if clicked {
            return Self::handle_slot_click(plots, id);
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
        let full_length = top.local.length_pct >= 100.0;
        let (bx, by) = Popup::bar_origin(top.anchor(), bw as f32, bh as f32, sw, sh, full_length);
        // Content rect: floating margins inset the painted cells.
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
        let cursor = plots
            .last_cursor
            .get(&bar_id)
            .copied()
            .map(|p| Point::new(p.x + bx, p.y + by));
        let pos = cursor
            .and_then(|p| {
                Popup::slot_at_point(
                    (bx + pl, by + pt, bw as f32 - pl - pr, bh as f32 - pt - pb),
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
        // slot means it was just a close.
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
        // Anchor the menu at the click, falling back to the slot center.
        let anchor = cursor.unwrap_or_else(|| {
            Popup::slot_center(
                (bx, by, bw as f32, bh as f32),
                (pl, pt, pr, pb),
                n,
                gap,
                horizontal,
                pos,
            )
        });
        if names
            .iter()
            .any(|w| lua_has_func(&plots.widget_lua, w, "popup"))
        {
            if let Some(cmd) = Popup::open_for(plots, bar_id, output, pos, anchor) {
                cmds.push(cmd);
            }
            return if cmds.is_empty() {
                Command::none()
            } else {
                Command::batch(cmds)
            };
        }
        // No menu: run the first `on_press()` action in the slot, then
        // re-render that widget (a toggle flips its next output).
        if let Some(name) = names
            .iter()
            .find(|w| lua_has_func(&plots.widget_lua, w, "on_press"))
            .cloned()
        {
            let gpu = Popup::gpu_usage_percent();
            let outcome = match plots.widget_lua.get(&name) {
                Some(lua) => {
                    let acted = publish_system_tables(lua, &plots.sysinfo, gpu)
                        .map_err(|e| e.to_string())
                        .and_then(|()| call_lua_action(lua))
                        .and_then(|()| call_lua_text(lua, "render"));
                    Some(acted)
                }
                None => None,
            };
            match outcome {
                Some(Ok(text)) => {
                    plots.widget_last_error.remove(&name);
                    if plots.widget_outputs.get(&name) != Some(&text) {
                        plots.widget_outputs.insert(name, text);
                        cmds.push(Command::done(Plant::TopPlot(TopEvent::WidgetsChanged)));
                    }
                }
                Some(Err(e)) => Self::note_widget_error(plots, &name, e),
                None => {}
            }
        }
        if cmds.is_empty() {
            Command::none()
        } else {
            Command::batch(cmds)
        }
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
        let lua = new_widget_lua().expect("sandbox");
        let execute: Value = lua.load("return os.execute").eval().expect("eval");
        assert!(matches!(execute, Value::Nil));
        let hour: String = lua.load("return os.date('%H')").eval().expect("eval");
        assert_eq!(hour.len(), 2);
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
            let out = call_lua_text(&lua, "render").expect("render");
            assert!(!out.is_empty(), "seed must render text");
            assert!(
                !out.contains("{icon:"),
                "usage seeds stay icon-free, got {out:?}"
            );
        }
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
    fn rich_text_builds_without_a_renderer() {
        // Element construction is pure — smoke-test all three shapes.
        let _ = rich_text("12%".to_string(), 13.0, 4.0);
        let _ = rich_text("{icon:cpu} 12%".to_string(), 13.0, 4.0);
        let _ = rich_text("{icon:nope}".to_string(), 13.0, 4.0);
    }
}
