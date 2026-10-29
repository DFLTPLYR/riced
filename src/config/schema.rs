//! Persisted schema and pure property/layout transformations.
#[cfg(test)]
use super::seed;
use super::{discovery, paths, util};
pub(crate) use discovery::{
    builtin_component_files, component_files, components_mtime, discover_widget_files,
    poll_components, widgets_dir_mtime,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
pub(crate) use util::decode_handle;

/// Top-level `config.lua` (migrated from TOML; Lua is the only source).
/// Unknown keys fail loudly so typos stay visible.
///
/// Shell appearance is supplied by Lua app modules under components/:
/// ```lua
/// return {
///     composable = { context_menu = "custom-menu.lua" },
/// }
/// ```
/// Default sources are omitted on save. Appearance lives in Lua `app.defaults`.
///
/// Theme selection mirrors reshell's `Global` (`general.theme` +
/// `general.darkmode`): `name` picks `~/.config/riced/theme/{name}.json`
/// (same schema as `reshell/core/data/themes/*.json`), `mode` picks
/// its `dark` vs `light` variant:
/// ```lua
/// return {
///     theme = { name = "gruvbox", mode = "dark" },
/// }
/// ```
///
/// Global animation speed for every animated transition (selection fade,
/// and any color/property change that interpolates):
/// ```lua
/// return {
///     animation = { speed = "medium" }, -- fast | medium | slow
/// }
/// ```
///
/// Named bars as a direct array. Each entry spawns one bar per
/// matching output (on the entry's `anchor`, skipped when that edge
/// already has a bar):
/// ```lua
/// return {
///     bar = {
///         {
///             anchor = "top", -- top | bottom | left | right
///             output = "", -- connector name (e.g. "DP-1"), "" = every output
///             length = 100.0, -- % of the output long axis (1-100)
///             thickness = 50.0, -- px (1-thin output axis)
///             slots = {
///                 { align = "start", widgets = { "cpu", "ram" } },
///                 { align = "center", widgets = { { name = "clock", size = 16 } } },
///                 { align = "end", widgets = {} },
///             },
///             slot_padding = 4.0, -- px inset inside every slot
///             slot_spacing = 4.0, -- px gap between slots
///             opacity = 1.0, -- bar backdrop opacity, snapped to steps
///             -- 0.0 0.25 0.5 0.75 1.0
///             floating = false,
///             margin = 8,
///             radius = { top_left = 12, top_right = 12, bottom_left = 4, bottom_right = 4 },
///         },
///     },
/// }
/// ```
/// Bars added via the context menu are appended here on creation and
/// panel edits update the entry by index (coalesced save), so bars survive
/// restarts. Same entry spawns on several outputs when `output = ""`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Disable the legacy native bar host. `bar` entries stay the single source
    /// of truth (Settings edits them), but a framework app renders them as Lua
    /// layers instead, so the two never draw the same edge twice.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub bar_host: bool,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::framework::deserialize_apps"
    )]
    pub(crate) apps: Vec<crate::framework::AppConfig>,
    /// Compiled-in Rust extensions enabled for this project. `riced build`
    /// generates a shell binary from this list.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::framework::deserialize_extensions"
    )]
    pub extensions: Vec<crate::framework::ExtensionConfig>,
    #[serde(skip_serializing_if = "crate::ui::settings::ComponentDefaults::is_empty")]
    pub component_defaults: crate::ui::settings::ComponentDefaults,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desktop: Vec<DesktopWidget>,
    #[serde(skip_serializing_if = "ComposableConfig::is_default")]
    pub composable: ComposableConfig,
    pub background: BackgroundConfig,
    pub theme: ThemeConfig,
    pub animation: AnimationConfig,
    pub interaction: InteractionConfig,
    pub notifications: NotificationConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<OverlayConfig>,
    /// Bar presets as a direct array (`bar`), no redundant key name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bar: Vec<TopConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            apps: Vec::new(),
            extensions: Vec::new(),
            component_defaults: Default::default(),
            desktop: Vec::new(),
            composable: Default::default(),
            background: Default::default(),
            theme: Default::default(),
            animation: Default::default(),
            interaction: Default::default(),
            notifications: Default::default(),
            overlays: Vec::new(),
            bar_host: true,
            bar: Vec::new(),
        }
    }
}

/// One output-local desktop app placement. Appearance defaults stay in Lua.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub struct DesktopWidget {
    #[serde(skip, default = "fresh_placement_id")]
    pub id: String,
    pub widget: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(deserialize_with = "desktop_coordinate")]
    pub x: f32,
    #[serde(deserialize_with = "desktop_coordinate")]
    pub y: f32,
    #[serde(skip_serializing_if = "desktop_zero")]
    #[serde(deserialize_with = "desktop_dimension")]
    pub width: f32,
    #[serde(skip_serializing_if = "desktop_zero")]
    #[serde(deserialize_with = "desktop_dimension")]
    pub height: f32,
    #[serde(skip_serializing_if = "desktop_zero_z")]
    pub z: i32,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub props: HashMap<String, PropValue>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HoldSpeed {
    Fast,
    #[default]
    Medium,
    Slow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum HoldDuration {
    Preset(HoldSpeed),
    Custom(u32),
}

impl Default for HoldDuration {
    fn default() -> Self {
        Self::Preset(HoldSpeed::Medium)
    }
}

impl HoldDuration {
    pub fn milliseconds(self) -> u32 {
        match self {
            Self::Preset(HoldSpeed::Fast) => 250,
            Self::Preset(HoldSpeed::Medium) => 500,
            Self::Preset(HoldSpeed::Slow) => 1000,
            Self::Custom(ms) => ms.clamp(1, 60_000),
        }
    }
    pub fn duration(self) -> Duration {
        Duration::from_millis(self.milliseconds().into())
    }
}

impl<'de> Deserialize<'de> for HoldDuration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            Preset(HoldSpeed),
            Custom(u32),
        }
        match Value::deserialize(deserializer)? {
            Value::Preset(speed) => Ok(Self::Preset(speed)),
            Value::Custom(ms @ 1..=60_000) => Ok(Self::Custom(ms)),
            Value::Custom(_) => Err(serde::de::Error::custom(
                "hold duration must be 1..=60000 milliseconds",
            )),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub struct InteractionConfig {
    pub hold: HoldDuration,
}
fn desktop_zero(value: &f32) -> bool {
    *value == 0.0
}
fn desktop_zero_z(value: &i32) -> bool {
    *value == 0
}
fn desktop_coordinate<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    let value = f32::deserialize(deserializer)?;
    if value.is_finite() && value.abs() <= 32768.0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "desktop coordinate must be finite and within -32768..=32768",
        ))
    }
}
fn desktop_dimension<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    let value = desktop_coordinate(deserializer)?;
    if value >= 0.0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "desktop dimension must be non-negative",
        ))
    }
}

/// Generates serde default fns from a single list, e.g.
/// `defs! { default_width: f32 = 180.0, ... }` expands to one
/// `fn default_width() -> f32 { 180.0 }` per entry. Referenced by name
/// from both `#[serde(default = "...")]` and the manual `Default` impls.
macro_rules! defs {
    ($($fn:ident : $ty:ty = $val:expr),* $(,)?) => {
        $(fn $fn() -> $ty { $val })*
    };
}

defs! {
    default_scale: f32 = 1.0,
    default_bar_length: f32 = 100.0,
    default_bar_thickness: f32 = 50.0,
    default_bar_slots: u32 = 1,
    default_bar_opacity: f32 = 1.0,
    default_widget_size: f32 = 13.0,
    default_widget_interval: f32 = 1.0,
    default_slot_padding: f32 = 0.0,
    default_slot_spacing: f32 = 4.0,
    default_notification_timeout_ms: u64 = 5000,
}

fn default_bar_anchor() -> String {
    "top".to_string()
}

fn default_theme_name() -> String {
    "gruvbox".to_string()
}

fn default_darkmode() -> bool {
    true
}

fn default_variant() -> String {
    "content".to_string()
}

fn default_notification_output() -> String {
    "mouse".to_string()
}

fn default_notification_source() -> String {
    "notifications.lua".to_string()
}

/// Notification service (`notifications`) and its Lua scene source.
/// Timeout hides the popup; history remains until explicitly cleared.
/// Presentation is ordinary iced content in the shared overlay host.
/// ```lua
/// return {
///     notifications = {
///         enabled = true,
///         output = "mouse", -- or an output name like "DP-1" to pin it
///         source = "notifications.lua", -- legacy scene; a Lua layer supersedes it
///         timeout_ms = 5000,
///     },
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub struct NotificationConfig {
    #[serde(skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default = "default_notification_output")]
    pub output: String,
    #[serde(default = "default_notification_source")]
    pub source: String,
    #[serde(default = "default_notification_timeout_ms")]
    pub timeout_ms: u64,
}
fn is_true(value: &bool) -> bool {
    *value
}
fn default_true() -> bool {
    true
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            output: default_notification_output(),
            source: default_notification_source(),
            timeout_ms: default_notification_timeout_ms(),
        }
    }
}

/// Additional Lua scenes share each output's passive fullscreen overlay.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub struct OverlayConfig {
    pub id: String,
    /// Relative to components/; absolute paths are used as-is.
    pub source: String,
    /// Empty means every output; otherwise an exact connector name.
    pub output: String,
    pub props: HashMap<String, PropValue>,
}

/// Global animation speed, applied to every animated transition
/// (selection fade and any interpolated color/property change).
/// ```lua
/// return { animation = { speed = "medium" } }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AnimationSpeed {
    Fast,
    #[default]
    Medium,
    Slow,
}

impl AnimationSpeed {
    pub fn all() -> [Self; 3] {
        [Self::Fast, Self::Medium, Self::Slow]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Fast => "Fast",
            Self::Medium => "Medium",
            Self::Slow => "Slow",
        }
    }

    /// Transition duration for the speed (medium preserves the original
    /// 150ms QML Behavior feel).
    pub fn duration(self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(80),
            Self::Medium => Duration::from_millis(150),
            Self::Slow => Duration::from_millis(300),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
#[derive(Default)]
pub struct AnimationConfig {
    pub speed: AnimationSpeed,
    /// Snap declarative property animations to their targets.
    #[serde(skip_serializing_if = "is_false")]
    pub reduced_motion: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Theme selection under `theme`, mirroring reshell's `Global.general`
/// (`theme` + `darkmode`). `name` resolves to
/// `~/.config/riced/theme/{name}.json` with the exact reshell theme schema
/// (`dark`/`light` variants); unknown names fall back to the vendored copy.
/// `variant` is the Material You scheme variant used by
/// `riced generate-theme` (ported from `sys/src/colorgen.rs`), one of
/// `content tonalspot monochrome neutral vibrant expressive fidelity
/// rainbow fruitsalad`. `templates_dir` points at a `[templates]` dir
/// (same format as reshell `core/theme/`); empty uses the seeded
/// `~/.config/riced/templates/` copy, `"off"` disables template rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "super::format::Theme", into = "super::format::Theme")]
pub struct ThemeConfig {
    pub icon_theme: String,
    #[serde(default = "default_theme_name")]
    pub name: String,
    #[serde(default = "default_darkmode")]
    pub darkmode: bool,
    #[serde(default = "default_variant")]
    pub variant: String,
    #[serde(default)]
    pub templates_dir: String,
    /// Explicit color overrides over the selected named theme.
    pub palette: crate::theme::palette::PaletteOverrides,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            icon_theme: "hicolor".into(),
            name: default_theme_name(),
            darkmode: default_darkmode(),
            variant: default_variant(),
            templates_dir: String::new(),
            palette: Default::default(),
        }
    }
}

/// Source filenames in one `composable` table; styling belongs to Lua.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub struct ComposableConfig {
    pub selection_rect: String,
    pub context_menu: String,
    pub context_menu_item: String,
}

impl Default for ComposableConfig {
    fn default() -> Self {
        Self {
            selection_rect: "selection_rect.lua".into(),
            context_menu: "context_menu.lua".into(),
            context_menu_item: "context_menu_item.lua".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComposableKind {
    SelectionRect,
    ContextMenu,
    ContextMenuItem,
}

impl ComposableConfig {
    pub fn get(&self, kind: ComposableKind) -> &str {
        match kind {
            ComposableKind::SelectionRect => &self.selection_rect,
            ComposableKind::ContextMenu => &self.context_menu,
            ComposableKind::ContextMenuItem => &self.context_menu_item,
        }
    }
    fn get_mut(&mut self, kind: ComposableKind) -> &mut String {
        match kind {
            ComposableKind::SelectionRect => &mut self.selection_rect,
            ComposableKind::ContextMenu => &mut self.context_menu,
            ComposableKind::ContextMenuItem => &mut self.context_menu_item,
        }
    }
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

impl Serialize for ComposableConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let defaults = Self::default();
        let mut map = serializer.serialize_map(None)?;
        for (name, source, default) in [
            (
                "selection_rect",
                &self.selection_rect,
                &defaults.selection_rect,
            ),
            ("context_menu", &self.context_menu, &defaults.context_menu),
            (
                "context_menu_item",
                &self.context_menu_item,
                &defaults.context_menu_item,
            ),
        ] {
            if source != default {
                map.serialize_entry(name, source)?;
            }
        }
        map.end()
    }
}

/// One bar preset from `[[bar]]`: edge, size, floating look
/// (content inset) and per-corner rounding. Applied when the bar spawns;
/// panel edits write back by index (coalesced save), so bars survive restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "super::format::Bar", into = "super::format::Bar")]
pub struct TopConfig {
    #[serde(default = "default_bar_anchor")]
    pub anchor: String,
    /// Connector name to spawn on (`""` = every output).
    #[serde(default)]
    pub output: String,
    #[serde(default = "default_bar_length")]
    pub length: f32,
    #[serde(default = "default_bar_thickness")]
    pub thickness: f32,
    /// Grid cells along the bar's long axis (columns when horizontal,
    /// rows when vertical). Clamped to 1..=32 at spawn.
    #[serde(default = "default_bar_slots")]
    pub slots: u32,
    /// Child position per slot along the bar (`start`/`center`/`end`;
    /// the cross axis stays centered; unknown entries read as
    /// `center`). Shorter lists pad centered, longer ones truncate.
    #[serde(default)]
    pub aligns: Vec<String>,
    /// Widget placements per slot by position, resolved against
    /// discovered `widgets/*.lua` definitions. Bare names inherit all
    /// definition defaults; tables override per instance. Each slot
    /// renders its entries together; shorter lists pad empty, longer
    /// ones truncate.
    #[serde(default)]
    pub widgets: Vec<SlotWidgets>,
    /// Inset inside every slot cell, around the widget content (px,
    /// clamped to 0-64 at spawn).
    #[serde(default = "default_slot_padding")]
    pub slot_padding: f32,
    /// Gap between slot cells and icon/text segments (px, clamped to
    /// 0-64 at spawn).
    #[serde(default = "default_slot_spacing")]
    pub slot_spacing: f32,
    /// Backdrop opacity, snapped to 0.0/0.25/0.5/0.75/1.0 at spawn.
    #[serde(default = "default_bar_opacity")]
    pub opacity: f32,
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub margin_top: i32,
    #[serde(default)]
    pub margin_right: i32,
    #[serde(default)]
    pub margin_bottom: i32,
    #[serde(default)]
    pub margin_left: i32,
    #[serde(default)]
    pub radius_top_left: f32,
    #[serde(default)]
    pub radius_top_right: f32,
    #[serde(default)]
    pub radius_bottom_left: f32,
    #[serde(default)]
    pub radius_bottom_right: f32,
}

impl Default for TopConfig {
    fn default() -> Self {
        Self {
            anchor: default_bar_anchor(),
            output: String::new(),
            length: default_bar_length(),
            thickness: default_bar_thickness(),
            slots: default_bar_slots(),
            aligns: Vec::new(),
            widgets: Vec::new(),
            slot_padding: default_slot_padding(),
            slot_spacing: default_slot_spacing(),
            opacity: default_bar_opacity(),
            floating: false,
            margin_top: 0,
            margin_right: 0,
            margin_bottom: 0,
            margin_left: 0,
            radius_top_left: 0.0,
            radius_top_right: 0.0,
            radius_bottom_left: 0.0,
            radius_bottom_right: 0.0,
        }
    }
}

/// One slot's widgets: a single `"name"`, a `["first", "second"]`
/// list, or full placement tables with per-instance overrides, e.g.
/// `{ name = "clock", size = 16.0, props = { show_seconds = true } }`
/// (both read the same; saves write bare names for clean placements).
/// A `none` entry reads as an empty slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SlotWidgets {
    One(String),
    Many(Vec<SlotEntry>),
}

/// Wallpaper images under `background`, e.g.:
/// ```lua
/// return {
///     background = {
///         mode = "free_form",
///         images = {
///             { path = "/home/user/pic.png", position = { 0.0, 0.0 },
///               z = 0, scale = 1.0, width = 1920.0, height = 1080.0 },
///         },
///     },
/// }
/// ```
/// Global coords like the selection rect; `width`/`height` of `0` mean native
/// image size at load; `z` orders overlapping images on the map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "super::format::Background", into = "super::format::Background")]
pub struct BackgroundConfig {
    /// Disable the legacy native background when a framework app replaces it.
    pub disabled: bool,
    pub mode: WallpaperMode,
    /// Optional post-processing of the baked wallpaper, below desktop widgets.
    pub shader: Option<WallpaperShader>,
    pub monitor: Vec<MonitorWallpaper>,
    #[serde(default)]
    pub image: Vec<BackgroundImage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "super::format::Shader", into = "super::format::Shader")]
pub struct WallpaperShader {
    pub path: String,
    pub speed: f32,
    pub params: Vec<f32>,
    pub animated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallpaperMode {
    #[default]
    FreeForm,
    PerMonitor,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallpaperFit {
    Fit,
    #[default]
    Expand,
    Stretch,
    Center,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub struct MonitorWallpaper {
    pub output: String,
    pub path: String,
    pub fit: WallpaperFit,
}

impl BackgroundConfig {
    /// Source recipes for the active mode, deduplicated by the source loader.
    pub fn active_images(&self) -> Vec<BackgroundImage> {
        match self.mode {
            WallpaperMode::FreeForm => self.image.clone(),
            WallpaperMode::PerMonitor => self
                .monitor
                .iter()
                .filter(|m| !m.path.is_empty())
                .map(|m| BackgroundImage {
                    path: m.path.clone(),
                    ..Default::default()
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "super::format::Image", into = "super::format::Image")]
pub struct BackgroundImage {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: i32,
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default)]
    pub width: f32,
    #[serde(default)]
    pub height: f32,
}

impl Default for BackgroundImage {
    fn default() -> Self {
        Self {
            path: String::new(),
            x: 0.0,
            y: 0.0,
            z: 0,
            scale: default_scale(),
            width: 0.0,
            height: 0.0,
        }
    }
}

/// A single runtime edit to the live config (e.g. from a Settings-panel
/// control). Applied to the single `Plots::config` source of truth, then
/// broadcast via full redraw — every view re-reads `plots.config`, so all
/// components pick the new value up on the next frame. No per-component
/// subscription registry needed; iced views are pure functions of state.
#[derive(Debug, Clone)]
pub enum ConfigPatch {
    ComponentDefaults(crate::ui::settings::ComponentDefaults),
    AddDesktopWidget(DesktopWidget),
    SetDesktopWidgetProp {
        index: usize,
        key: String,
        value: Option<PropValue>,
    },
    RemoveDesktopWidget {
        index: usize,
    },
    MoveDesktopWidget {
        index: usize,
        x: f32,
        y: f32,
    },
    RepositionDesktopWidget {
        id: String,
        output: String,
        x: f32,
        y: f32,
    },
    SetDesktopWidgetSize {
        index: usize,
        width: f32,
        height: f32,
    },
    WallpaperMode(WallpaperMode),
    MonitorWallpaper(MonitorWallpaper),
    ComposableSource(ComposableKind, String),
    ThemeName(String),
    ThemePalette(crate::theme::palette::PaletteOverrides),
    IconTheme(String),
    ThemeDarkmode(bool),
    ThemeVariant(String),
    AnimationSpeed(AnimationSpeed),
    HoldDuration(HoldDuration),
    AddImage(BackgroundImage),
    MoveImage {
        index: usize,
        x: f32,
        y: f32,
    },
    SetImageScale {
        index: usize,
        scale: f32,
    },
    /// Native-size override (`0` = native file size, same as a sparse entry).
    SetImageSize {
        index: usize,
        width: f32,
        height: f32,
    },
    SetImageZ {
        index: usize,
        z: i32,
    },
    /// Cursor-anchored scale step (Ctrl+wheel on the map): placement and
    /// scale travel in one patch so the point under the cursor stays put.
    ScaleImage {
        index: usize,
        x: f32,
        y: f32,
        scale: f32,
    },
    RemoveImage {
        index: usize,
    },
}

/// `~/.config/riced/config.lua` (`$XDG_CONFIG_HOME` aware).
pub use paths::{config_migrated_path, config_path, legacy_config_path};

/// Widget definitions: every `widgets/*.lua` file (`$XDG_CONFIG_HOME`
/// aware, non-recursive — `components/` never becomes widgets) is one
/// definition named by its file stem. Slots reference definitions by
/// name (see `bar` slot `widgets`); each file is a Lua script returning an
/// app table (below).
///
/// ```text
/// widgets/
///   clock.lua      → widget "clock"
///   stats.lua      → widget "stats"
/// components/      → shared library, sibling of widgets/
/// ```
///
/// Discovery runs at startup and whenever the directory changes; new
/// files appear in the Settings pool automatically, deleted files
/// render blank (with a warning) until removed from their slots.
/// Complete Lua function reference: `docs/lua-api.md`.
///
/// ## Lifecycle
///
/// Each placement gets an independent Lua state (two clocks never
/// share `self`). Every effective `interval` seconds (clamped to
/// \>= 0.25) the engine republishes the service tables (see
/// `crate::services`: `system`, `theme`, `notifications`, `wayland`),
/// the bar's `bar.output`, and the placement's resolved `self.props`,
/// then calls `app:view()`; when the output (text or tree) differs
/// from the last tick, the bar repaints. Editing the `.lua` file
/// reloads it live (mtime watch) — including `app:popup()`/
///
/// `app:actions()`: opening a menu always uses the saved file, and
/// an open menu refreshes within a tick of saving (broken edits keep
/// the last good menu and log once). Errors log once per message,
/// never per tick.
///
/// ## Sandbox
///
/// Scripts see string/table/math/os/io — no `require`, and
/// `os.exit`/`os.remove`/`os.rename` are nil'd. `os.execute` and
/// `io.popen` are live native shell (owner-accepted risk: no
/// allowlist, `;` chains work, `rm -rf ~` needs no sudo).
/// `print()` stays for daemon-log debugging.
///
/// ## Globals
///
/// - `system`: `cpu_usage` (%), `cpu_count`, `mem_used`/`mem_total`
///   (bytes), `mem_usage` (%), `gpu_usage` (% or nil when the GPU
///   exposes nothing). Refreshed before every due `app:view()`.
/// - `theme`: live palette hex pairs (`theme.primary`, ...).
/// - `notifications`: queue snapshot, newest-first (`id`, `app`,
///   `title`, `body`, `urgency`, `has_image`).
/// - `wayland`: `outputs` (`name`, `x`, `y`, `w`, `h`), native
///   `workspaces` (`name`, `monitor`, `active`) from `ext-workspace`
///   plus native `toplevels` (`app_id`, `title`) from
///   `ext-foreign-toplevel-list`. `monitor` resolves by output
///   geometry (compositor names are unreliable); unresolvable
///   monitors stay empty and remain in the overview popup, not the
///   output-specific strip. Unsupported protocols yield empty lists.
///   Changes refresh Lua data and views immediately, independently of
///   widget intervals. Namespace/list references stay live across updates;
///   individual rows are snapshots. No focus: standard Wayland defines none.
/// - `bar`: where this render happens — `bar.output` is the bar's
///   connector name (nil while unknown). States are shared across
///   bars but trees render per bar, so filter per-bar content (e.g.
///   workspaces) on this, not on `wayland.outputs[1]`. Notification
///   cards have no bar: `bar.output` is nil there.
/// - `os.execute(cmd)` / `io.popen(cmd)`: native shell. Capture
///   stdout with `io.popen(cmd):read("*a")`; wrap slow calls in
///   `pcall`, cache on self, refresh hourly.
/// - `ui.*`: composable node constructors (below). Cell text may also
///   embed `{icon:name}` placeholders for theme-aware Lucide icons,
///   e.g. `"{icon:cpu} " .. string.format("%.0f", system.cpu_usage)`.
///
/// ## Returned app module
///
/// `app:view()` is required. It returns either plain text (numbers/booleans coerce,
/// `nil` is empty) or a `ui.*` tree. Trees refresh on the interval
/// like text; switching shapes clears the other cache.
///
/// ```lua
/// local app = {}
/// function app:view()
///     return iced.row({ iced.icon("cpu"), iced.text("42%") })
/// end
/// return app
/// ```
///
/// Scripts must return this table. Global render() scripts are rejected;
/// methods are stored in the Lua registry, not exported to globals.
///
/// ## Widget defaults and per-instance properties
///
/// A widget declares its own defaults in `app.defaults` (all optional;
/// built-ins are `interval = 1.0`, `size = 13.0`, no custom props):
///
/// ```lua
/// local app = {
///     defaults = {
///         interval = 1.0,
///         size = 13.0,
///         props = { format = "%H:%M", show_seconds = false },
///     },
///     property_schema = {
///         format = { label = "Time format", type = "string" },
///         show_seconds = { label = "Show seconds", type = "boolean" },
///     },
/// }
/// function app:view()
///     return iced.text(os.date(self.props.format))
/// end
/// return app
/// ```
///
/// Each bar slot placement may override `interval`, `size`, and any
/// `props` key (a bare `"clock"` string inherits everything):
///
/// ```lua
/// bar = { { slots = { { widgets = { { name = "clock", size = 16.0, props = { show_seconds = true } } } } } } }
/// ```
///
/// Before every `view()`/`popup()`/`actions()`, the engine sets
/// `self.props` to the resolved table (definition defaults overlaid
/// with placement overrides) — read per-instance values from there,
/// never from `app.defaults` directly. `property_schema` is optional
/// metadata for nicer Settings controls (`label`, `type` =
/// boolean/number/string, `description`, `min`/`max`, `choices`);
/// without it, controls are inferred from default value types.
///
/// ## `ui.*` declarative constructors
///
/// - `iced.text(s)`: themed text (icon placeholders resolved).
/// - `iced.icon(name)`: full Lucide set by name (`"bot"`,
///   `"robot-vacuum"`, `"memory-stick"` — case/separators ignored)
///   plus short aliases (`mem`, `vol`, `up`...). Unknown names render
///   literal so typos stay visible.
/// - `iced.row({...} [, spacing])` / `iced.column({...} [, spacing])`:
///   nest freely.
/// - `iced.button(label, action)` or `iced.button(label):action(key)`:
///   the last action setter wins over the constructor key. Clicking calls
///   that widget's `actions({type="action",key=key})` directly (missing
///   `actions` is a silent no-op), never the slot popup/click fallback.
///   `:background()` repaints the resting surface; hover/press stay
///   themed so clicks still read.
/// - `iced.progress(0.0-1.0 [, width])`: bar, clamped, 120px default.
///   `:color()` repaints the fill, `:background()` the track.
/// - `iced.spinner()`: loading ring for slow fetches — return it first,
///   swap in cached data on later ticks (see clinepass seed).
/// - `iced.separator()`: horizontal hairline, theme border color
///   unless `:color()` overrides. Always full-width.
/// - `iced.scrollable(child)`: constrain `:height()` to scroll overflowing
///   content; `:scrollbar("hidden")` hides the bar while keeping scrolling.
///   `:scrollbar("visible")` restores the default scrollbar.
/// - `iced.container(child)`: the styling base — `:background()`,
///   `:border()` (implies 1px; `:border_width()` adjusts, bare width
///   without a color paints nothing), `:radius()`, `:padding()`.
///   Compose it (directly or via `iced.define`) for cards, chips and
///   panels: `iced.container(body):background(theme.surface)`
///   `:border(theme.outline):radius(8)`.
///
/// ## Chaining (iced-spelled setters)
///
/// Every node type carries its own setters, so Lua reads like iced
/// builders — each setter writes its field and returns the node:
///
/// ```lua
/// iced.progress(p / 100):width(200):height(12)
/// iced.progress(p / 100):width("fill")
/// iced.text("hi"):size(14):height(20)
/// iced.row({...}):spacing(8):width("fill")
/// iced.button("go", "run"):width(120):height(36):padding(4)
/// ```
///
/// Sizes are numbers (px), `"fill"`, `"fill:n"` (positive integer weight), or `"shrink"`
/// (any case). Unset sizes mean `Shrink` — except progress width,
/// which stays `Fixed(120)` — so old scripts render identically.
/// Note: `Fill` only expands when the parent offers space; inside the
/// bar's shrink-wrapped cells it looks like a no-op and pays off in
/// popups and nested rows/columns.
///
/// Setters per type: `text` → `:size()`, `:width()`, `:height()`,
/// `:color()`; `row`/`column` → `:spacing()`, `:width()`, `:height()`;
/// `button` → `:width()`, `:height()`, `:padding()`, `:color()` (label
/// tint), `:background()` (resting surface), `:radius()`; `progress` → `:width()`
/// (same as the second constructor arg), `:height()` (bar thickness =
/// iced `girth`), `:color()` (fill), `:background()` (track);
/// `separator` → `:height()`, `:color()`; `container` → `:width()`,
/// `:height()`, `:padding()`, `:background()`, `:radius()`,
/// `:border()`, `:border_width()`; `icon` → `:color()` only (tint via
/// surrounding text color). `:color()`/`:background()`/`:border()`
/// take `"#rgb"` / `"#rrggbb"` / `"#rrggbbaa"` or `{r, g, b[, a]}` 0–1
/// tables; unset stays themed. Calling a setter the type doesn't own
/// (e.g. `:padding()` on progress) fails at eval — typos stay visible.
/// Wrong-typed values error at parse naming the field.
///
/// ## List transitions (QML-`ListView` add/remove)
///
/// One native list component (`layers::listview::ListView`) serves every
/// animated list in the shell, with QML-style `on_entered` / `on_exit`
/// / `on_displaced` transitions. Direct `iced.button` children of a cell's
/// top-level row/column animate on add/remove (keyed by `action`,
/// driven by aura-anim): entering items fade/slide in, removed items linger
/// as inert ghosts fading out, then leave. Notification cards use the
/// same component keyed `(output, id)`, sliding from the anchored edge.
/// Label-only edits on the same key swap instantly. First paint
/// settles with no animation; durations follow the global animation
/// speed. Slides draw through a GPU offset (layout never reflows
/// mid-transition) and fades ride style alpha; survivors glide toward
/// their new slot (`displaced`). Popup trees stay static for now.
///
/// Override the motion per listview with `onEntered`, `onExit`, and
/// `onDisplaced` — the QML `Transition { NumberAnimation { ... } }` subset,
/// declarative and parsed once per diff:
///
/// ```lua
/// iced.listview(items)
///     :onEntered({ x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 })
///     :onExit({ x = { to = -200 }, opacity = { to = 0 }, duration = 250 })
///     :onDisplaced({ duration = 250 })
/// ```
///
/// Missing transition fields keep the defaults (`add`/
/// `remove` slide-fade ±16px, `displaced` glides the index delta at the
/// global speed), so partial specs compose. `displaced` takes only
/// `duration` (distance comes from the layout). Malformed specs log
/// once and keep the defaults — never half-applied.
///
/// ## Notification queue: the `notifications` global
///
/// Republished before every `render()`/`popup()`/`actions` call
/// (newest-first): `notifications = { {id, app, title, body, icon,
/// urgency, has_image, actions, output, popup_visible, closed}, ... }`
/// — metadata only, no image handles. `actions` is `{key, label}[]`.
/// Build a notification center from this (see the `notifycenter`
/// seed) and dismiss with the `actions` return convention:
///
/// ```lua
/// function app:actions(events)
///     if events.type ~= "action" then return end
///     local key = events.key
///     local id = key:match("^dismiss:(%d+)$")
///     if id then return { dismiss = tonumber(id) } end
/// end
/// ```
///
/// `{ dismiss = id }` clicks a card away (D-Bus reason 2);
/// `{ invoke = { id = N, key = "k" } }` fires an action button.
/// Anything else (nil, text, unknown ids) just re-renders the widget —
/// the queue's own guards ignore bad targets.
///
/// ## Components: `iced.define` / `iced.use` + `components/`
///
/// Reusable Lua builders over the constructors above, for cells,
/// popups, and notification cards alike. `components/*.lua` (sorted)
/// runs in every widget state after `iced` is built:
///
/// ```lua
/// iced.define("stat", function(props)
///     return iced.row({ iced.icon(props.icon), iced.text(props.value) })
/// end)
///
/// local app = {}
/// function app:view()
///     return iced.stat({ icon = "cpu", value = "42%" }):width("fill")
/// end
/// return app
/// ```
///
/// Components must return `iced.*` constructor values (chaining and
/// parsing keep working); unknown names and non-node returns error
/// naming the component. Seeds ship `spacer`, `card`, and `menu`
/// (`00-define.lua`, `05-styled.lua`, `10-card.lua`, `20-menu.lua` — never overwritten).
/// Editing any component rebuilds every Lua state on the next tick,
/// like a widgets-dir change.
///
/// ## Theme colors: the `theme` table
///
/// Republished before every `render()` (widgets and notification
/// cards), so theme switches flow in live: `theme.primary`,
/// `theme.on_primary`, `theme.secondary`, `theme.surface`,
/// `theme.background`, `theme.success`, `theme.warning`, `theme.error`
/// (each `"#rrggbb"`, flattened from the live iced palette) plus their
/// `on_*` text colors. Pair with `:color()`:
///
/// ```lua
/// iced.text("! critical"):color(theme.error)
/// ```
///
/// ## Raw constructors vs components
///
/// Prefer `iced.use(...)` components over hand-rolled `iced.row`
/// trees: the seeds below are all component-built, and raw
/// constructor use is legacy (still fully supported — nothing was
/// removed, new widgets should just reach for components first).
///
/// ## Events: `popup()` / `actions(events)`
///
/// Clicking a slot runs widget Lua: `popup()` (when defined) toggles a
/// menu with its return — either body text or a table with `text`,
/// `width`/`height`, clickable `items` (`{ label, action }` rows
/// calling `actions({type="action",key=action})`), and a composed `ui` body (any `iced.*`
/// tree, rendered above the items). `iced.button`s inside the body work
/// exactly like `items` rows (same event key); `items` remains
/// the shorthand for uniform full-width menu rows. Otherwise
/// `actions(events)` receives a `click` event as a
/// bare click action and the cell re-renders after it. A popup with no
/// text, tree, or items never opens. Size without items is just
/// `{ text = os.date("%A"), width = 300, height = 200 }`. Clicks are
/// hit-tested per widget (each cell widget owns a mouse area), so
/// every widget in a slot gets its own popup/click calls.
/// Presses record their target and only the matching release acts,
/// which also keeps inner `iced.button` clicks from double-firing the
/// widget. Gap clicks fall back to the slot (first popup, else first
/// `actions` callback). Other unconsumed pointer events, configurable holds, and
/// delivered keys use the same callback. Desktop callbacks can call
/// `self.drag()`/`self.drop()`; see `docs/lua-api.md` for event fields and restrictions.
///
/// ```lua
/// local app = { details = false }
/// function app:view() return iced.text("Details") end
/// function app:popup()
///     return iced.popup(iced.row({ iced.icon("clock"), iced.text(os.date("%H:%M")) }))
///         :width(300):height(200)
/// end
///
/// function app:actions(events)  -- popup items + cell buttons land here
///     if events.type == "action" and events.key == "toggle" then self.details = not self.details end
/// end
/// return app
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetDef {
    /// Slot reference (`[[bar]] widgets = [...]`).
    #[serde(default)]
    pub name: String,
    /// Script file, relative to the widgets dir (absolute paths pass
    /// through). Empty defaults to `<name>.lua`.
    #[serde(default)]
    pub file: String,
    /// Seconds between `render()` calls (clamped to >= 0.25).
    #[serde(default = "default_widget_interval")]
    pub interval: f32,
    /// Text size.
    #[serde(default = "default_widget_size")]
    pub size: f32,
}

impl Default for WidgetDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            file: String::new(),
            interval: default_widget_interval(),
            size: default_widget_size(),
        }
    }
}

/// Scalar widget property: Lua booleans/numbers/strings round-trip.
/// Tables are rejected — props stay flat so Settings can introspect
/// and edit every value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropValue {
    Bool(bool),
    Number(f64),
    Text(String),
}

impl PropValue {
    /// Best-effort type name for control selection (`"boolean"`,
    /// `"number"`, `"string"`).
    pub fn kind(&self) -> &'static str {
        match self {
            PropValue::Bool(_) => "boolean",
            PropValue::Number(_) => "number",
            PropValue::Text(_) => "string",
        }
    }
}

/// One widget property's Settings metadata, from the optional
/// `app.property_schema` table. Every field is optional: absent
/// entries fall back to the default value's type with the key as
/// label.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PropSchema {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `"boolean"`, `"number"` or `"string"`. Unknown values are
    /// ignored (the default's type wins instead).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub prop_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// String choices render as preset buttons; ignored otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// Author-declared defaults from a widget's `app.defaults` table:
/// refresh interval, text size, and default custom properties.
/// Missing pieces fall back to the built-in interval/size and an
/// empty prop set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetDefaults {
    #[serde(default = "default_widget_interval")]
    pub interval: f32,
    #[serde(default = "default_widget_size")]
    pub size: f32,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub props: HashMap<String, PropValue>,
}

impl Default for WidgetDefaults {
    fn default() -> Self {
        Self {
            interval: default_widget_interval(),
            size: default_widget_size(),
            props: HashMap::new(),
        }
    }
}

/// One discovered widget definition: a `widgets/*.lua` file (name =
/// file stem) plus its author-declared defaults and property schema.
/// Discovered at startup and on directory change; never hand-written.
#[derive(Debug, Clone)]
pub struct WidgetDefinition {
    pub name: String,
    pub file: PathBuf,
    pub defaults: WidgetDefaults,
    pub schema: HashMap<String, PropSchema>,
}

impl WidgetDefinition {
    pub fn fallback(name: &str, file: PathBuf) -> Self {
        Self {
            name: name.to_string(),
            file,
            defaults: WidgetDefaults::default(),
            schema: HashMap::new(),
        }
    }
}

/// One placed widget instance: `id` is stable per placement (it
/// travels with drags and keys the Lua state); `name` selects the
/// discovered definition; `interval`/`size`/`file`/`props` override the
/// definition defaults when present. Serialized without `id` — ids are
/// a runtime concern, regenerated on load. Saved placements retain explicit
/// overrides only; absent values keep inheriting Lua defaults. A bare
/// `"name"` string means "inherit everything".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetPlacement {
    #[serde(skip)]
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub props: HashMap<String, PropValue>,
}

impl WidgetPlacement {
    /// Effective refresh interval: placement override, else definition
    /// default (clamped to >= 0.25 by the caller).
    pub fn effective_interval(&self, defaults: &WidgetDefaults) -> f32 {
        self.interval.unwrap_or(defaults.interval)
    }

    /// Effective text size: placement override, else definition default.
    pub fn effective_size(&self, defaults: &WidgetDefaults) -> f32 {
        self.size.unwrap_or(defaults.size)
    }

    /// Resolved custom properties: definition defaults overlaid with
    /// placement overrides. Unknown override keys (no default, e.g.
    /// after a widget update removed them) are kept — Settings shows
    /// them as free-form values rather than dropping user data.
    pub fn resolve_props(
        &self,
        defaults: &HashMap<String, PropValue>,
    ) -> HashMap<String, PropValue> {
        let mut out = defaults.clone();
        out.extend(self.props.clone());
        out
    }

    /// `true` when nothing is overridden (serializes as a bare name).
    pub fn is_inherited(&self) -> bool {
        self.file.is_none()
            && self.interval.is_none()
            && self.size.is_none()
            && self.props.is_empty()
    }
}

/// Fresh placement ids (`w1`, `w2`, …), process-unique. Assigned on
/// load and on pool drops; never serialized.
static PLACEMENT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(crate) fn fresh_placement_id() -> String {
    format!(
        "w{}",
        PLACEMENT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Fill empty placement ids in place (legacy strings, pool drops).
/// Existing ids are never reassigned.
pub(crate) fn ensure_placement_ids(slots: &mut [Vec<WidgetPlacement>]) {
    for slot in slots.iter_mut() {
        for placement in slot.iter_mut() {
            if placement.id.is_empty() {
                placement.id = fresh_placement_id();
            }
        }
    }
}

/// One slot entry in `[[bar]] widgets`: a bare `"name"` (inherit
/// everything) or a full placement table with overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SlotEntry {
    Name(String),
    Full(WidgetPlacement),
}

impl SlotEntry {
    /// Definition name this entry places (`""` when a table omits it;
    /// callers treat that as unknown, like a typo).
    pub fn widget_name(&self) -> &str {
        match self {
            SlotEntry::Name(name) => name,
            SlotEntry::Full(placement) => &placement.name,
        }
    }
}

impl SlotWidgets {
    /// Serialize one slot: placements write full tables with
    /// explicit `interval`/`size` and `props`
    /// overrides only — except placements already bare, which stay
    /// bare strings. Ids are runtime-only and never written.
    pub fn from_placements(slot: &[WidgetPlacement]) -> Self {
        SlotWidgets::Many(
            slot.iter()
                .map(|placement| {
                    if placement.is_inherited() {
                        SlotEntry::Name(placement.name.clone())
                    } else {
                        SlotEntry::Full(placement.clone())
                    }
                })
                .collect(),
        )
    }
}

/// Retired `widgets.toml` registry: one `[[widget]]` entry per
/// declarative widget. Read once for the placement migration, then the
/// file is renamed to `widgets.toml.migrated` and never read again.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetsFile {
    #[serde(default)]
    pub widget: Vec<WidgetDef>,
}

/// Seed Lua clock, written next to the other seed widgets.
/// Globals persist between calls; clicking the cell toggles the date menu.
#[cfg(test)]
pub(crate) use seed::SEED_CLOCK_LUA;

/// Seed Lua label example, written next to the other seed widgets.
#[cfg(test)]
pub(crate) use seed::SEED_HELLO_LUA;

/// Seed stats example: icon + CPU + memory via the live tables.
/// Place it in a bar slot (or the Settings pool) to use it.
#[cfg(test)]
pub(crate) use seed::SEED_STATS_LUA;

/// Seed CPU usage: plain percent, no icon. Uncomment its
/// bar slot entry to use it.
#[cfg(test)]
pub(crate) use seed::SEED_CPU_LUA;

/// Seed RAM usage: plain percent, no icon. Uncomment its
/// bar slot entry to use it.
#[cfg(test)]
pub(crate) use seed::SEED_RAM_LUA;

/// Seed GPU usage: plain percent, no icon. Reads "--" when the GPU
/// exposes nothing readable. Uncomment its `[[widget]]` entry in
/// a bar slot to use it.
#[cfg(test)]
pub(crate) use seed::SEED_GPU_LUA;

/// Seed session menu: power-icon cell, popup with suspend /
/// poweroff / hibernate / reboot rows dispatching `systemctl`.
/// `actions` whitelists the four keys (never interpolates a raw
/// key into shell). Place it in a bar slot to use it.
/// to use it.
#[cfg(test)]
pub(crate) use seed::SEED_SYSTEM_LUA;

#[cfg(test)]
pub(crate) use seed::{SEED_CONTEXT_MENU, SEED_CONTEXT_MENU_ITEM, SEED_SELECTION_RECT};

/// Seed notification-center widget: a bell cell counting the live
/// queue, with a popup listing the newest notifications and a dismiss
/// button per row. Reads the `notifications` global (republished
/// before every render) and dismisses via the `actions` return
/// convention (`{ dismiss = id }`). Uncomment its `[[widget]]` entry
/// in a bar slot to use it.
#[cfg(test)]
pub(crate) use seed::SEED_NOTIFY_CENTER_LUA;

/// Seed CPU/RAM/GPU line-graph widget: compact sparkline in a bar
/// slot and a detailed graph with legend in its popup. Its desktop
/// counterpart lives in `usage.desktop.lua`.
#[cfg(test)]
pub(crate) use seed::{SEED_USAGE_DESKTOP_LUA, SEED_USAGE_LUA};

/// Numbered horizontal workspace buttons filtered by `bar.output`.
/// Native ext-workspace supplies metadata; clicks dispatch
/// `hl.dsp.focus` through hyprctl using the actual workspace name.
/// Place it in a bar slot (or the Settings pool) to use it.
#[cfg(test)]
pub(crate) use seed::SEED_WORKSPACES_LUA;

/// Seed Cline Pass usage: robot icon cell, popup with quota rows.
/// Paste the API key into `API_KEY` below (no input widget exists —
/// the file hot-reloads on save). Blank key renders a connect hint.
#[cfg(test)]
pub(crate) use seed::SEED_CLINEPASS_LUA;

/// `~/.config/riced/widgets.toml` (`$XDG_CONFIG_HOME` aware).
pub use paths::widgets_path;

/// Adopted `widgets.toml` after retirement: definitions came from it,
/// values folded into placements or Lua defaults.
pub use paths::widgets_migrated_path;

pub use paths::shaders_dir;
/// Shared component library dir (`~/.config/riced/components/`):
/// every `*.lua` file (sorted) is concatenated and executed in each
/// widget state after `iced` is built, so `iced.define` components are
/// available to widgets and overlay scenes. No
/// `require` needed (and none available — the sandbox nils it).
pub use paths::{components_dir, settings_dir};
#[cfg(test)]
pub(crate) use seed::SHADER_SEEDS;

impl Config {
    /// Apply a runtime [`ConfigPatch`] to the live config in place.
    pub fn apply(&mut self, patch: ConfigPatch) {
        let c = &mut self.composable;
        let images = &mut self.background.image;
        match patch {
            ConfigPatch::ComponentDefaults(defaults) => self.component_defaults = defaults,
            ConfigPatch::AddDesktopWidget(mut widget) => {
                if widget.id.is_empty() || self.desktop.iter().any(|entry| entry.id == widget.id) {
                    widget.id = fresh_placement_id();
                }
                self.desktop.push(widget);
            }
            ConfigPatch::SetDesktopWidgetProp { index, key, value } => {
                if let Some(widget) = self.desktop.get_mut(index) {
                    if let Some(value) = value {
                        widget.props.insert(key, value);
                    } else {
                        widget.props.remove(&key);
                    }
                }
            }
            ConfigPatch::RemoveDesktopWidget { index } => {
                if index < self.desktop.len() {
                    self.desktop.remove(index);
                }
            }
            ConfigPatch::MoveDesktopWidget { index, x, y } => {
                if x.is_finite()
                    && y.is_finite()
                    && let Some(widget) = self.desktop.get_mut(index)
                {
                    widget.x = x;
                    widget.y = y;
                }
            }
            ConfigPatch::RepositionDesktopWidget { id, output, x, y } => {
                if !output.is_empty()
                    && x.is_finite()
                    && y.is_finite()
                    && x.abs() <= 32768.0
                    && y.abs() <= 32768.0
                    && let Some(widget) = self.desktop.iter_mut().find(|widget| widget.id == id)
                {
                    widget.output = output;
                    widget.x = x;
                    widget.y = y;
                }
            }
            ConfigPatch::SetDesktopWidgetSize {
                index,
                width,
                height,
            } => {
                if width.is_finite()
                    && height.is_finite()
                    && let Some(widget) = self.desktop.get_mut(index)
                {
                    widget.width = width.max(0.0);
                    widget.height = height.max(0.0);
                }
            }
            ConfigPatch::WallpaperMode(mode) => self.background.mode = mode,
            ConfigPatch::MonitorWallpaper(wallpaper) => {
                if let Some(existing) = self
                    .background
                    .monitor
                    .iter_mut()
                    .find(|m| m.output == wallpaper.output)
                {
                    *existing = wallpaper;
                } else {
                    self.background.monitor.push(wallpaper);
                }
            }
            ConfigPatch::ComposableSource(kind, src) => *c.get_mut(kind) = src,
            ConfigPatch::ThemeName(name) => {
                self.theme.name = name;
                self.theme.palette = Default::default();
            }
            ConfigPatch::ThemePalette(patch) => self.theme.palette.merge(patch),
            ConfigPatch::IconTheme(name) => self.theme.icon_theme = name,
            ConfigPatch::ThemeDarkmode(dark) => self.theme.darkmode = dark,
            ConfigPatch::ThemeVariant(variant) => self.theme.variant = variant,
            ConfigPatch::AnimationSpeed(speed) => self.animation.speed = speed,
            ConfigPatch::HoldDuration(duration) => self.interaction.hold = duration,
            ConfigPatch::AddImage(img) => images.push(img),
            ConfigPatch::MoveImage { index, x, y } => {
                if let Some(img) = images.get_mut(index) {
                    img.x = x;
                    img.y = y;
                }
            }
            ConfigPatch::SetImageScale { index, scale } => {
                if let Some(img) = images.get_mut(index) {
                    img.scale = scale.max(0.01);
                }
            }
            ConfigPatch::SetImageSize {
                index,
                width,
                height,
            } => {
                if let Some(img) = images.get_mut(index) {
                    img.width = width.max(0.0);
                    img.height = height.max(0.0);
                }
            }
            ConfigPatch::ScaleImage { index, x, y, scale } => {
                if let Some(img) = images.get_mut(index) {
                    img.x = x;
                    img.y = y;
                    img.scale = scale.max(0.01);
                }
            }
            ConfigPatch::SetImageZ { index, z } => {
                if let Some(img) = images.get_mut(index) {
                    img.z = z;
                }
            }
            ConfigPatch::RemoveImage { index } => {
                if index < images.len() {
                    images.remove(index);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_format_config as save;
    use crate::config::test_parse_config as parse;
    use crate::config::test_parse_defaults as parse_defaults;

    #[test]
    fn desktop_placements_are_sparse_strict_and_get_independent_runtime_ids() {
        let mut config: Config = parse("return { desktop = { { widget = 'rectangle', output = 'DP-1', x = 40, y = 80, props = { color = '#123456' } }, { widget = 'rectangle', x = 0, y = 0 } } }").unwrap();
        assert_ne!(config.desktop[0].id, config.desktop[1].id);
        assert!(!config.desktop[0].id.is_empty());
        let id = config.desktop[0].id.clone();
        config.apply(ConfigPatch::MoveDesktopWidget {
            index: 0,
            x: 100.0,
            y: 200.0,
        });
        config.apply(ConfigPatch::SetDesktopWidgetSize {
            index: 0,
            width: 300.0,
            height: 150.0,
        });
        config.apply(ConfigPatch::SetDesktopWidgetProp {
            index: 0,
            key: "radius".into(),
            value: Some(PropValue::Number(8.0)),
        });
        let text = save(&config).unwrap();
        assert!(!text.contains("id ="));
        let restored: Config = parse(&text).unwrap();
        assert_eq!(restored.desktop[0].x, 100.0);
        assert_eq!(restored.desktop[0].width, 300.0);
        assert_eq!(restored.desktop[0].props, config.desktop[0].props);
        assert_eq!(config.desktop[0].id, id);
        assert_ne!(restored.desktop[0].id, id);
        config.apply(ConfigPatch::RemoveDesktopWidget { index: 0 });
        assert_eq!(config.desktop.len(), 1);
        for extra in [
            "return { desktop = { { widget = 'rectangle', widht = 5 } } }",
            "return { desktop = { { widget = 'rectangle', width = -1 } } }",
            "return { desktop = { { widget = 'rectangle', x = 0/0 } } }",
        ] {
            assert!(parse(extra).is_err(), "{extra}");
        }
        assert!(!save(&Config::default()).unwrap().contains("desktop"));
    }

    #[test]
    fn slot_entries_read_names_and_tables() {
        // Bare names inherit everything; tables carry overrides.
        let cfg: Config = parse(
            "return { bar = { { slots = { { widgets = { 'clock', { name = 'clock', size = 16.0 } } } } } } }",
        )
        .unwrap();
        assert!(matches!(cfg.bar[0].widgets[0], SlotWidgets::Many(_)));
        let SlotWidgets::Many(entries) = &cfg.bar[0].widgets[0] else {
            panic!("expected many");
        };
        assert!(matches!(entries[0], SlotEntry::Name(_)));
        assert_eq!(entries[0].widget_name(), "clock");
        match &entries[1] {
            SlotEntry::Full(p) => {
                assert_eq!(p.name, "clock");
                assert_eq!(p.size, Some(16.0));
                assert_eq!(p.interval, None);
                // Integer Lua values coerce to float props.
                assert!(p.props.is_empty());
            }
            SlotEntry::Name(_) => panic!("expected table"),
        }
        // Integer prop values still parse.
        let cfg: Config = parse(
            "return { bar = { { slots = { { widgets = { { name = 'x', props = { n = 2 } } } }, { widgets = { 'clock' } } } } } }",
        )
        .unwrap();
        let SlotWidgets::Many(entries) = &cfg.bar[0].widgets[0] else {
            panic!("expected many");
        };
        match &entries[0] {
            SlotEntry::Full(p) => {
                assert_eq!(
                    p.props.get("n"),
                    Some(&PropValue::Number(2.0)),
                    "integer props coerce to float"
                );
            }
            _ => panic!("expected table"),
        }
    }

    #[test]
    fn placement_inheritance_prefers_overrides() {
        let mut defaults = WidgetDefaults::default();
        defaults
            .props
            .insert("format".to_string(), PropValue::Text("%H:%M".to_string()));
        let bare = WidgetPlacement {
            name: "clock".to_string(),
            ..Default::default()
        };
        assert!(bare.is_inherited());
        assert_eq!(bare.effective_interval(&defaults), 1.0);
        assert_eq!(bare.effective_size(&defaults), 13.0);
        assert_eq!(
            bare.resolve_props(&defaults.props)["format"],
            PropValue::Text("%H:%M".to_string())
        );
        let custom = WidgetPlacement {
            interval: Some(5.0),
            props: [("format".to_string(), PropValue::Text("%H".to_string()))]
                .into_iter()
                .collect(),
            ..bare.clone()
        };
        assert!(!custom.is_inherited());
        assert_eq!(custom.effective_interval(&defaults), 5.0);
        assert_eq!(custom.effective_size(&defaults), 13.0);
        // Overrides win; unknown keys survive (no data loss on downgrade).
        let mut resolved = custom.resolve_props(&defaults.props);
        assert_eq!(resolved["format"], PropValue::Text("%H".to_string()));
        resolved.insert("extra".to_string(), PropValue::Bool(true));
        let extra = WidgetPlacement {
            props: [("extra".to_string(), PropValue::Bool(true))]
                .into_iter()
                .collect(),
            ..bare
        };
        assert_eq!(
            extra.resolve_props(&defaults.props)["extra"],
            PropValue::Bool(true)
        );
        let _ = resolved;
    }

    #[test]
    fn per_monitor_config_defaults_and_patches_roundtrip_without_losing_freeform() {
        let mut config: Config = parse(
            "return { background = { mode = 'per_monitor', monitors = { ['DP-1'] = { path = 'one.png' }, ['DP-2'] = { path = 'two.png', fit = 'center' } }, images = { { path = 'freeform.png', position = { 17.0, 0.0 } } } } }",
        )
        .unwrap();
        assert_eq!(config.background.mode, WallpaperMode::PerMonitor);
        assert_eq!(config.background.monitor[0].fit, WallpaperFit::Expand);
        assert_eq!(config.background.monitor[1].fit, WallpaperFit::Center);
        config.apply(ConfigPatch::MonitorWallpaper(MonitorWallpaper {
            output: "DP-1".into(),
            path: "changed.png".into(),
            fit: WallpaperFit::Stretch,
        }));
        assert_eq!(config.background.monitor.len(), 2);
        assert_eq!(config.background.monitor[1].path, "two.png");
        config.apply(ConfigPatch::WallpaperMode(WallpaperMode::FreeForm));
        assert_eq!(config.background.image[0].x, 17.0);
        let saved = save(&config).unwrap();
        assert_eq!(parse(&saved).unwrap().background, config.background);
        let legacy: Config =
            parse("return { background = { images = { { path = 'legacy.png' } } } }").unwrap();
        assert_eq!(legacy.background.mode, WallpaperMode::FreeForm);
        assert!(legacy.background.monitor.is_empty());
    }

    #[test]
    fn slots_serialize_clean_placements_as_names() {
        let clean = WidgetPlacement {
            id: "w9".to_string(),
            name: "clock".to_string(),
            ..Default::default()
        };
        // Ids are runtime-only: clean placements write as bare names.
        assert!(matches!(
            SlotWidgets::from_placements(std::slice::from_ref(&clean)),
            SlotWidgets::Many(ref entries)
                if entries == &[SlotEntry::Name("clock".to_string())]
        ));
        let dirty = WidgetPlacement {
            size: Some(16.0),
            ..clean
        };
        match SlotWidgets::from_placements(&[dirty]) {
            SlotWidgets::Many(entries) => match &entries[..] {
                [SlotEntry::Full(p)] => {
                    assert_eq!(p.size, Some(16.0));
                    // Ids are runtime-only: skipped on serialize even
                    // when the in-memory placement carries one.
                    let value = serde_json::to_value(p).unwrap();
                    assert!(value.get("id").is_none(), "{value}");
                    assert_eq!(value.get("size").and_then(|v| v.as_f64()), Some(16.0));
                }
                _ => panic!("expected one full entry"),
            },
            _ => panic!("expected many"),
        }
    }

    #[test]
    fn composable_sources_roundtrip_in_one_sparse_table() {
        let cfg: Config = parse(
            "return { composable = { context_menu = 'custom-frame.lua', context_menu_item = 'custom-item.lua' } }",
        )
        .unwrap();
        assert_eq!(cfg.composable.context_menu, "custom-frame.lua");
        assert_eq!(cfg.composable.context_menu_item, "custom-item.lua");
        assert_eq!(cfg.composable.selection_rect, "selection_rect.lua");
        let saved = save(&cfg).unwrap();
        assert!(saved.contains("composable"));
        assert!(!saved.contains("selection_rect ="));
        let reloaded: Config = parse(&saved).unwrap();
        assert_eq!(reloaded.composable, cfg.composable);
    }

    #[test]
    fn default_composable_sources_are_omitted_on_save() {
        let empty: Config = parse("return {}").unwrap();
        assert_eq!(empty.composable, ComposableConfig::default());
        let saved = save(&empty).unwrap();
        assert!(!saved.contains("composable"));
        let reloaded: Config = parse(&saved).unwrap();
        assert_eq!(reloaded.composable, empty.composable);
    }

    #[test]
    fn schema_rejects_unknown_composable_keys() {
        assert!(parse("return { composable = { bogus = 'menu.lua' } }").is_err());
    }

    #[test]
    fn unknown_top_level_keys_are_rejected() {
        // Strict configs: unknown keys are load-bearing errors.
        assert!(parse("return { menu = { width = 200.0 } }").is_err());
    }

    #[test]
    fn parses_background_image_entries() {
        let cfg: Config = parse(
            "return { background = { images = { { path = '/a.png', position = { 10.0, 20.0 }, z = 2 }, { path = '/b.png' } } } }",
        )
        .unwrap();
        assert_eq!(cfg.background.image.len(), 2);
        let a = &cfg.background.image[0];
        assert_eq!(a.path, "/a.png");
        assert_eq!((a.x, a.y, a.z), (10.0, 20.0, 2));
        assert_eq!(a.scale, 1.0);
        // sparse entry defaults everything else
        let b = &cfg.background.image[1];
        assert_eq!(b.path, "/b.png");
        assert_eq!((b.x, b.y, b.z, b.scale), (0.0, 0.0, 0, 1.0));
    }

    #[test]
    fn image_patches_roundtrip() {
        let mut cfg = Config::default();
        assert!(cfg.background.image.is_empty());
        cfg.apply(ConfigPatch::AddImage(BackgroundImage {
            path: "/a.png".into(),
            x: 5.0,
            ..Default::default()
        }));
        assert_eq!(cfg.background.image.len(), 1);
        cfg.apply(ConfigPatch::MoveImage {
            index: 0,
            x: 7.0,
            y: 9.0,
        });
        assert_eq!(
            (cfg.background.image[0].x, cfg.background.image[0].y),
            (7.0, 9.0)
        );
        cfg.apply(ConfigPatch::SetImageScale {
            index: 0,
            scale: 2.0,
        });
        assert_eq!(cfg.background.image[0].scale, 2.0);
        cfg.apply(ConfigPatch::SetImageZ { index: 0, z: 3 });
        assert_eq!(cfg.background.image[0].z, 3);
        cfg.apply(ConfigPatch::SetImageSize {
            index: 0,
            width: 800.0,
            height: 600.0,
        });
        assert_eq!(
            (
                cfg.background.image[0].width,
                cfg.background.image[0].height
            ),
            (800.0, 600.0)
        );
        // Negative sizes clamp to 0 (= native); OOB index is a no-op.
        cfg.apply(ConfigPatch::SetImageSize {
            index: 0,
            width: -5.0,
            height: -5.0,
        });
        assert_eq!(
            (
                cfg.background.image[0].width,
                cfg.background.image[0].height
            ),
            (0.0, 0.0)
        );
        cfg.apply(ConfigPatch::SetImageSize {
            index: 9,
            width: 1.0,
            height: 1.0,
        });
        cfg.apply(ConfigPatch::ScaleImage {
            index: 0,
            x: 11.0,
            y: 12.0,
            scale: 1.5,
        });
        assert_eq!(
            (
                cfg.background.image[0].x,
                cfg.background.image[0].y,
                cfg.background.image[0].scale
            ),
            (11.0, 12.0, 1.5)
        );
        // out-of-range patches are no-ops, never panics
        cfg.apply(ConfigPatch::MoveImage {
            index: 9,
            x: 0.0,
            y: 0.0,
        });
        cfg.apply(ConfigPatch::RemoveImage { index: 9 });
        cfg.apply(ConfigPatch::RemoveImage { index: 0 });
        assert!(cfg.background.image.is_empty());
    }

    #[test]
    fn patch_updates_only_the_targeted_leaf() {
        let mut cfg = Config::default();
        cfg.apply(ConfigPatch::ComposableSource(
            ComposableKind::ContextMenuItem,
            "custom-item.lua".into(),
        ));
        assert_eq!(cfg.composable.context_menu_item, "custom-item.lua");
        assert_eq!(cfg.composable.context_menu, "context_menu.lua");
        assert_eq!(cfg.composable.selection_rect, "selection_rect.lua");
    }

    #[test]
    fn theme_section_defaults_to_gruvbox_dark() {
        let empty: Config = parse("return {}").unwrap();
        assert_eq!(empty.theme.name, "gruvbox");
        assert!(empty.theme.darkmode);
        assert_eq!(empty.theme.variant, "content");
        assert!(empty.theme.templates_dir.is_empty());
    }

    #[test]
    fn theme_section_parses_name_and_mode() {
        let cfg: Config = parse("return { theme = { name = 'dracula', mode = 'light' } }").unwrap();
        assert_eq!(cfg.theme.name, "dracula");
        assert!(!cfg.theme.darkmode);
    }

    #[test]
    fn bar_section_parses_direct_array_with_defaults() {
        let cfg: Config = parse(
            "return { bar = { { anchor = 'bottom', output = 'DP-1', length = 80.0, floating = true, margin = { top = 8 } } } }",
        )
        .unwrap();
        assert_eq!(cfg.bar.len(), 1);
        let main = &cfg.bar[0];
        assert_eq!(main.anchor, "bottom");
        assert_eq!(main.output, "DP-1");
        assert_eq!(main.length, 80.0);
        assert!(main.floating);
        assert_eq!(main.margin_top, 8);
        // Omitted keys fall back to bar defaults.
        assert_eq!(main.thickness, 50.0);
        assert_eq!(main.slots, 1);
        assert_eq!(main.opacity, 1.0);
        assert_eq!(main.margin_right, 0);
        assert_eq!(main.radius_top_left, 0.0);
        // No section at all means no bars.
        let empty: Config = parse("return {}").unwrap();
        assert!(empty.bar.is_empty());
    }

    #[test]
    fn bar_slots_infer_count() {
        let cfg: Config = parse(
            "return { bar = { { anchor = 'left', output = 'DP-1', slots = { {}, {}, {}, {} } } } }",
        )
        .unwrap();
        assert_eq!(cfg.bar.len(), 1);
        assert_eq!(cfg.bar[0].slots, 4);
    }

    #[test]
    fn bar_aligns_default_empty_and_parse_names() {
        let sparse: Config = parse("return { bar = { { anchor = 'top' } } }").unwrap();
        assert_eq!(sparse.bar[0].aligns, ["center"]);
        let cfg: Config =
            parse("return { bar = { { slots = { { align = 'start' }, { align = 'end' } } } } }")
                .unwrap();
        assert_eq!(
            cfg.bar[0].aligns,
            vec!["start".to_string(), "end".to_string()]
        );
    }

    #[test]
    fn bar_widgets_default_empty_and_parse_names() {
        let sparse: Config = parse("return { bar = { { anchor = 'top' } } }").unwrap();
        assert!(
            matches!(&sparse.bar[0].widgets[0], SlotWidgets::Many(entries) if entries.is_empty())
        );
        // Flat names read the same shape.
        let cfg: Config = parse(
            "return { bar = { { slots = { { widgets = { 'cpu', 'ram' } }, { widgets = { 'clock' } } } } } }",
        )
        .unwrap();
        assert!(matches!(
            cfg.bar[0].widgets[0],
            SlotWidgets::Many(ref entries)
                if entries.iter().map(SlotEntry::widget_name).collect::<Vec<_>>()
                    == ["cpu", "ram"]
        ));
        assert!(matches!(
            cfg.bar[0].widgets[1],
            SlotWidgets::Many(ref entries) if entries == &[SlotEntry::Name("clock".into())]
        ));
    }

    #[test]
    fn widgets_file_still_parses_lua_entries() {
        let file: WidgetsFile = toml::from_str(
            "[[widget]]\nname = \"clock\"\nfile = \"clock.lua\"\ninterval = 5.0\nsize = 14.0\n\
             [[widget]]\nname = \"hello\"\nfile = \"hello.lua\"\n",
        )
        .unwrap();
        assert_eq!(file.widget.len(), 2);
        assert_eq!(file.widget[0].file, "clock.lua");
        assert_eq!(file.widget[0].interval, 5.0);
        assert_eq!(file.widget[0].size, 14.0);
        // Sparse entry defaults the rest.
        assert_eq!(file.widget[1].interval, 1.0);
        assert_eq!(file.widget[1].size, 13.0);
    }

    #[test]
    fn selection_source_resolves_from_components() {
        let config: Config =
            parse("return { composable = { selection_rect = 'custom-selection.lua' } }").unwrap();
        assert_eq!(
            crate::lua::composable::source_path(&config.composable.selection_rect),
            components_dir().join("custom-selection.lua")
        );
    }

    #[test]
    fn bar_slot_gaps_default_to_bare_cells() {
        let sparse: Config = parse("return { bar = { { anchor = 'top' } } }").unwrap();
        assert_eq!(sparse.bar[0].slot_padding, 0.0);
        assert_eq!(sparse.bar[0].slot_spacing, 4.0);
        let cfg: Config = parse(
            "return { bar = { { anchor = 'top', slot_padding = 6.0, slot_spacing = 2.0 } } }",
        )
        .unwrap();
        assert_eq!(cfg.bar[0].slot_padding, 6.0);
        assert_eq!(cfg.bar[0].slot_spacing, 2.0);
    }

    #[test]
    fn bar_opacity_defaults_to_opaque_and_parses_steps() {
        let sparse: Config = parse("return { bar = { { anchor = 'top' } } }").unwrap();
        assert_eq!(sparse.bar[0].opacity, 1.0);
        let cfg: Config = parse("return { bar = { { anchor = 'top', opacity = 0.5 } } }").unwrap();
        assert_eq!(cfg.bar[0].opacity, 0.5);
    }

    #[test]
    fn theme_patches_roundtrip() {
        let mut cfg = Config::default();
        cfg.apply(ConfigPatch::ThemeName("tokyo-night".into()));
        assert_eq!(cfg.theme.name, "tokyo-night");
        cfg.apply(ConfigPatch::ThemeDarkmode(false));
        assert!(!cfg.theme.darkmode);
        cfg.apply(ConfigPatch::ThemeVariant("vibrant".into()));
        assert_eq!(cfg.theme.variant, "vibrant");
    }

    #[test]
    fn animation_section_defaults_to_medium() {
        let empty: Config = parse("return {}").unwrap();
        assert_eq!(empty.animation.speed, AnimationSpeed::Medium);
        assert_eq!(
            AnimationSpeed::Medium.duration(),
            Duration::from_millis(150)
        );
    }

    #[test]
    fn hold_timing_presets_custom_values_and_strict_validation_roundtrip() {
        assert_eq!(
            Config::default().interaction.hold.duration(),
            Duration::from_millis(500)
        );
        for (value, milliseconds) in [
            ("'fast'", 250),
            ("'medium'", 500),
            ("'slow'", 1000),
            ("725", 725),
        ] {
            let config: Config =
                parse(&format!("return {{ interaction = {{ hold = {value} }} }}")).unwrap();
            assert_eq!(config.interaction.hold.milliseconds(), milliseconds);
            let serialized = save(&config).unwrap();
            let restored: Config = parse(&serialized).unwrap();
            assert_eq!(restored.interaction.hold, config.interaction.hold);
        }
        for value in ["0", "-1", "60001", "1.5", "'custom'", "'fas t'"] {
            assert!(parse(&format!("return {{ interaction = {{ hold = {value} }} }}")).is_err());
        }
        assert!(parse("return { interaction = { unknown = 1 } }").is_err());
        let mut config = Config::default();
        config.apply(ConfigPatch::HoldDuration(HoldDuration::Custom(850)));
        assert_eq!(
            config.interaction.hold.duration(),
            Duration::from_millis(850)
        );
    }

    #[test]
    fn notifications_section_defaults_and_parses() {
        let empty: Config = parse("return {}").unwrap();
        assert!(empty.notifications.enabled);
        assert_eq!(empty.notifications.output, "mouse");
        assert_eq!(empty.notifications.source, "notifications.lua");
        assert_eq!(empty.notifications.timeout_ms, 5000);
        let cfg: Config = parse(
            "return { notifications = { enabled = false, output = 'DP-1', source = 'custom.lua', timeout_ms = 8000 } }",
        )
        .unwrap();
        assert!(!cfg.notifications.enabled);
        assert_eq!(cfg.notifications.output, "DP-1");
        assert_eq!(cfg.notifications.source, "custom.lua");
        assert_eq!(cfg.notifications.timeout_ms, 8000);
    }

    #[test]
    fn animation_speed_parses_and_patches() {
        let cfg: Config = parse("return { animation = { speed = 'fast' } }").unwrap();
        assert_eq!(cfg.animation.speed, AnimationSpeed::Fast);
        assert_eq!(AnimationSpeed::Fast.duration(), Duration::from_millis(80));
        assert_eq!(AnimationSpeed::Slow.duration(), Duration::from_millis(300));
        let mut cfg = Config::default();
        cfg.apply(ConfigPatch::AnimationSpeed(AnimationSpeed::Slow));
        assert_eq!(cfg.animation.speed, AnimationSpeed::Slow);
    }

    #[test]
    fn saved_lua_defaults_parses_back() {
        // Seed-parity checks deserialize component defaults the same way the
        // daemon does for legacy dictionary fixtures.
        let defaults: crate::ui::settings::ComponentDefaults =
            parse_defaults("return { button = { padding = 9 } }").unwrap();
        assert!(defaults.0.contains_key("button"));
    }
}
