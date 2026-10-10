//! Persisted schema and pure property/layout transformations.
#[cfg(test)]
use super::seed;
use super::{discovery, paths, util};
pub(crate) use discovery::{
    builtin_component_files, component_files, components_mtime, discover_widget_files,
    poll_components, widgets_dir_mtime,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;
pub(crate) use util::decode_handle;

/// Top-level `config.toml`. Unknown keys are ignored so old files
/// keep loading after new sections are added.
///
/// Shell appearance is supplied by Lua app modules under components/:
/// ```toml
/// [composable.context_menu]
/// src = "context_menu.lua"
///
/// [composable.context_menu_item]
/// src = "context_menu_item.lua"
/// ```
/// Optional `[composable.context_menu.props]` overrides Lua defaults.
/// Legacy scalar tables migrate on load; `[composable.menu]` is retired.
///
/// Theme selection mirrors reshell's `Global` (`general.theme` +
/// `general.darkmode`): `name` picks `~/.config/riced/theme/{name}.json`
/// (same schema as `reshell/core/data/themes/*.json`), `darkmode` picks
/// its `dark` vs `light` variant:
/// ```toml
/// [theme]
/// name = "gruvbox"
/// darkmode = true
/// ```
///
/// Global animation speed for every animated transition (selection fade,
/// and any color/property change that interpolates):
/// ```toml
/// [animation]
/// speed = "medium"   # fast | medium | slow
/// ```
///
/// Named bars as a direct array. Each entry spawns one bar per
/// matching output (on the entry's `anchor`, skipped when that edge
/// already has a bar):
/// ```toml
/// [[bar]]
/// anchor = "top"     # top | bottom | left | right
/// output = ""        # connector name (e.g. "DP-1"), "" = every output
/// length = 100.0     # % of the output long axis (1-100)
/// thickness = 50.0   # px (1-thin output axis)
/// slots = 3          # grid cells along the long axis: columns when
///                    # horizontal (top/bottom), rows when vertical
///                    # (left/right); 1..=32
/// aligns = ["start", "center", "end"]
///                    # child position per slot along the bar
///                    # (cross axis stays centered)
/// widgets = [["cpu", "ram"], ["clock"]]
///                    # placements from discovered widgets by slot
///                    # position; each slot renders its entries together.
///                    # A bare "name" inherits all definition defaults;
///                    # tables override per instance, e.g.
///                    # `{ name = "clock", size = 16.0 }`.
///                    # (`none`/unknown render empty)
/// slot_padding = 4.0   # px inset inside every slot, around content
/// slot_spacing = 4.0   # px gap between slots (also icon/text runs)
/// opacity = 1.0      # bar backdrop opacity, snapped to steps
///                    # 0.0 0.25 0.5 0.75 1.0
/// floating = false
/// margin_top = 8
/// margin_right = 8
/// margin_bottom = 8
/// margin_left = 8
/// radius_top_left = 12.0
/// radius_top_right = 12.0
/// radius_bottom_left = 12.0
/// radius_bottom_right = 12.0
/// ```
/// Bars added via the context menu are appended here on creation and
/// panel edits update the entry by index (coalesced save), so bars survive
/// restarts. Same entry spawns on several outputs when `output = ""`.
/// Legacy `[top.<name>]` tables are still read (sorted by name) and merged
/// in when `[[bar]]` is empty, but never written back.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub composable: ComposableConfig,
    pub background: BackgroundConfig,
    pub theme: ThemeConfig,
    pub animation: AnimationConfig,
    pub notifications: NotificationConfig,
    /// Bar presets as a direct array (`[[bar]]`), no redundant key name.
    #[serde(default)]
    pub bar: Vec<TopConfig>,
    /// Legacy `[top.<name>]` tables: read for migration, never written.
    #[serde(default, skip_serializing)]
    pub top: HashMap<String, TopConfig>,
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
    default_notification_max_visible: u32 = 3,
    default_notification_width: f32 = 360.0,
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

fn default_notification_position() -> String {
    "top-right".to_string()
}

/// Notification layer (`[notifications]`): D-Bus freedesktop server +
/// internal events, rendered per output (mouse output by default).
/// The window spans the full output height and the stack scrolls —
/// `max_visible` is legacy and ignored.
/// ```toml
/// [notifications]
/// enabled = true
/// output = "mouse"  # or an output name like "DP-1" to pin it
/// position = "top-right"  # top-right | top-left | bottom-right | bottom-left
/// timeout_ms = 5000
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub enabled: bool,
    #[serde(default = "default_notification_output")]
    pub output: String,
    #[serde(default = "default_notification_position")]
    pub position: String,
    #[serde(default = "default_notification_timeout_ms")]
    pub timeout_ms: u64,
    /// Legacy cap, ignored: the window spans the output height and the
    /// whole stack scrolls. Kept so old configs still parse.
    #[serde(default = "default_notification_max_visible")]
    pub max_visible: u32,
    #[serde(default = "default_notification_width")]
    pub width: f32,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            output: default_notification_output(),
            position: default_notification_position(),
            timeout_ms: default_notification_timeout_ms(),
            max_visible: default_notification_max_visible(),
            width: default_notification_width(),
        }
    }
}

/// Global animation speed, applied to every animated transition
/// (selection fade and any interpolated color/property change).
/// ```toml
/// [animation]
/// speed = "medium"
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
#[serde(default)]
#[derive(Default)]
pub struct AnimationConfig {
    pub speed: AnimationSpeed,
}

/// Theme selection under `[theme]`, mirroring reshell's `Global.general`
/// (`theme` + `darkmode`). `name` resolves to
/// `~/.config/riced/theme/{name}.json` with the exact reshell theme schema
/// (`dark`/`light` variants); unknown names fall back to the vendored copy.
/// `variant` is the Material You scheme variant used by
/// `riced generate-theme` (ported from `sys/src/colorgen.rs`), one of
/// `content tonalspot monochrome neutral vibrant expressive fidelity
/// rainbow fruitsalad`. `templates_dir` points at a `[templates]` dir
/// (same format as reshell `core/theme/`); empty uses the seeded
/// `~/.config/riced/templates/` copy, `"off"` disables template rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    #[serde(default = "default_theme_name")]
    pub name: String,
    #[serde(default = "default_darkmode")]
    pub darkmode: bool,
    #[serde(default = "default_variant")]
    pub variant: String,
    #[serde(default)]
    pub templates_dir: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: default_theme_name(),
            darkmode: default_darkmode(),
            variant: default_variant(),
            templates_dir: String::new(),
        }
    }
}

/// Per-component tables under `[composable.*]`.
#[derive(Debug, Clone, Serialize)]
pub struct ComposableConfig {
    pub selection_rect: SourceComposable,
    pub context_menu: SourceComposable,
    pub context_menu_item: SourceComposable,
}

impl Default for ComposableConfig {
    fn default() -> Self {
        Self {
            selection_rect: SourceComposable::default(),
            context_menu: SourceComposable::new("context_menu.lua"),
            context_menu_item: SourceComposable::new("context_menu_item.lua"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ComposableKind {
    SelectionRect,
    ContextMenu,
    ContextMenuItem,
}

impl ComposableConfig {
    pub fn get(&self, kind: ComposableKind) -> &SourceComposable {
        match kind {
            ComposableKind::SelectionRect => &self.selection_rect,
            ComposableKind::ContextMenu => &self.context_menu,
            ComposableKind::ContextMenuItem => &self.context_menu_item,
        }
    }
    fn get_mut(&mut self, kind: ComposableKind) -> &mut SourceComposable {
        match kind {
            ComposableKind::SelectionRect => &mut self.selection_rect,
            ComposableKind::ContextMenu => &mut self.context_menu,
            ComposableKind::ContextMenuItem => &mut self.context_menu_item,
        }
    }
}

impl<'de> Deserialize<'de> for ComposableConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Entry {
            src: Option<String>,
            props: BTreeMap<String, PropValue>,
            width: Option<f64>,
            height: Option<f64>,
            padding: Option<f64>,
            spacing: Option<f64>,
            rounding: Option<f64>,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Legacy {
            selection_rect: SourceComposable,
            context_menu: Entry,
            context_menu_item: Entry,
            menu: Entry,
        }
        fn convert(entry: Entry, default_src: &str) -> SourceComposable {
            let mut props = entry.props;
            for (key, value) in [
                ("width", entry.width),
                ("height", entry.height),
                ("padding", entry.padding),
                ("spacing", entry.spacing),
                ("rounding", entry.rounding),
            ] {
                if let Some(value) = value {
                    props.entry(key.into()).or_insert(PropValue::Number(value));
                }
            }
            SourceComposable {
                src: entry.src.unwrap_or_else(|| default_src.into()),
                props,
            }
        }
        let raw = Legacy::deserialize(deserializer)?;
        let mut context_menu = convert(raw.context_menu, "context_menu.lua");
        // The actual menu width wins over the old independent hit-test width.
        // Only width/height were read from the retired generic menu table.
        for (key, value) in [("width", raw.menu.width), ("height", raw.menu.height)] {
            if let Some(value) = value {
                context_menu
                    .props
                    .entry(key.into())
                    .or_insert(PropValue::Number(value));
            }
        }
        Ok(Self {
            selection_rect: raw.selection_rect,
            context_menu,
            context_menu_item: convert(raw.context_menu_item, "context_menu_item.lua"),
        })
    }
}

/// A Lua app module under components/, with sparse property overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceComposable {
    pub src: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub props: BTreeMap<String, PropValue>,
}

impl Default for SourceComposable {
    fn default() -> Self {
        Self {
            src: "selection_rect.lua".into(),
            props: BTreeMap::new(),
        }
    }
}

impl SourceComposable {
    pub fn new(src: &str) -> Self {
        Self {
            src: src.into(),
            props: BTreeMap::new(),
        }
    }
}

/// One bar preset from `[[bar]]`: edge, size, floating look
/// (content inset) and per-corner rounding. Applied when the bar spawns;
/// panel edits write back by index (coalesced save), so bars survive restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SlotWidgets {
    One(String),
    Many(Vec<SlotEntry>),
}

/// Wallpaper images under `[background.*]`, e.g.:
/// ```toml
/// [[background.image]]
/// path = "/home/user/pic.png"
/// x = 0.0
/// y = 0.0
/// z = 0
/// scale = 1.0
/// width = 1920.0
/// height = 1080.0
/// ```
/// Global coords like the selection rect; `width`/`height` of `0` mean native
/// image size at load; `z` orders overlapping images on the map.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundConfig {
    #[serde(default)]
    pub image: Vec<BackgroundImage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
    ComposableSource(ComposableKind, String),
    ComposableProp(ComposableKind, String, Option<PropValue>),
    ThemeName(String),
    ThemeDarkmode(bool),
    ThemeVariant(String),
    AnimationSpeed(AnimationSpeed),
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

/// `~/.config/riced/config.toml` (`$XDG_CONFIG_HOME` aware).
pub use paths::config_path;

/// Widget definitions: every `widgets/*.lua` file (`$XDG_CONFIG_HOME`
/// aware, non-recursive — `components/` never becomes widgets) is one
/// definition named by its file stem. Slots reference definitions by
/// name (see `[[bar]] widgets`); each file is a Lua script returning an
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
/// `app:on_action()`: opening a menu always uses the saved file, and
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
///   No focus: standard Wayland defines none.
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
///     return ui.row({ ui.icon("cpu"), ui.text("42%") })
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
///     return ui.text(os.date(self.props.format))
/// end
/// return app
/// ```
///
/// Each bar slot placement may override `interval`, `size`, and any
/// `props` key (a bare `"clock"` string inherits everything):
///
/// ```toml
/// widgets = [[{ name = "clock", size = 16.0, props = { show_seconds = true } }]]
/// ```
///
/// Before every `view()`/`popup()`/`on_action()`, the engine sets
/// `self.props` to the resolved table (definition defaults overlaid
/// with placement overrides) — read per-instance values from there,
/// never from `app.defaults` directly. `property_schema` is optional
/// metadata for nicer Settings controls (`label`, `type` =
/// boolean/number/string, `description`, `min`/`max`, `choices`);
/// without it, controls are inferred from default value types.
///
/// ## `ui.*` declarative constructors
///
/// - `ui.text(s)`: themed text (icon placeholders resolved).
/// - `ui.icon(name)`: full Lucide set by name (`"bot"`,
///   `"robot-vacuum"`, `"memory-stick"` — case/separators ignored)
///   plus short aliases (`mem`, `vol`, `up`...). Unknown names render
///   literal so typos stay visible.
/// - `ui.row({...} [, spacing])` / `ui.column({...} [, spacing])`:
///   nest freely.
/// - `ui.button(label, action)`: per-widget MouseArea — clicking calls
///   that widget's `on_action(action)` directly (missing `on_action`
///   is a silent no-op), never the slot popup/`on_press` fallback.
///   `:background()` repaints the resting surface; hover/press stay
///   themed so clicks still read.
/// - `ui.progress(0.0-1.0 [, width])`: bar, clamped, 120px default.
///   `:color()` repaints the fill, `:background()` the track.
/// - `ui.spinner()`: loading ring for slow fetches — return it first,
///   swap in cached data on later ticks (see clinepass seed).
/// - `ui.separator()`: horizontal hairline, theme border color
///   unless `:color()` overrides. Always full-width.
/// - `ui.container(child)`: the styling base — `:background()`,
///   `:border()` (implies 1px; `:border_width()` adjusts, bare width
///   without a color paints nothing), `:radius()`, `:padding()`.
///   Compose it (directly or via `iced.define`) for cards, chips and
///   panels: `ui.container(body):background(theme.surface)`
///   `:border(theme.outline):radius(8)`.
///
/// ## Chaining (iced-spelled setters)
///
/// Every node type carries its own setters, so Lua reads like iced
/// builders — each setter writes its field and returns the node:
///
/// ```lua
/// ui.progress(p / 100):width(200):height(12)
/// ui.progress(p / 100):width("fill")
/// ui.text("hi"):size(14):height(20)
/// ui.row({...}):spacing(8):width("fill")
/// ui.button("go", "run"):width(120):height(36):padding(4)
/// ```
///
/// Sizes are numbers (px, backward-compat), `"fill"`, or `"shrink"`
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
/// / `on_displaced` transitions. Direct `ui.button` children of a cell's
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
/// Override the motion per widget (or for cards, in
/// `notifications.lua`) with an optional `transitions()` — the QML
/// `Transition { NumberAnimation { ... } }` subset, declarative and
/// parsed once per diff:
///
/// ```lua
/// function app:transitions()
///     return {
///         add = { x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 },
///         remove = { x = { to = -200 }, opacity = { to = 0 }, duration = 250 },
///         displaced = { duration = 250 },
///     }
/// end
/// ```
///
/// Missing `transitions`, slots, or fields keep the defaults (`add`/
/// `remove` slide-fade ±16px, `displaced` glides the index delta at the
/// global speed), so partial specs compose. `displaced` takes only
/// `duration` (distance comes from the layout). Malformed specs log
/// once and keep the defaults — never half-applied.
///
/// ## Notification queue: the `notifications` global
///
/// Republished before every `render()`/`popup()`/`on_action` call
/// (newest-first): `notifications = { {id, app, title, body, urgency,
/// has_image}, ... }` — metadata only, no image handles or actions.
/// Build a notification center from this (see the `notifycenter`
/// seed) and dismiss with the `on_action` return convention:
///
/// ```lua
/// function app:on_action(key)
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
///     return ui.stat({ icon = "cpu", value = "42%" }):width("fill")
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
/// ## Clicks: `popup()` / `on_press()` / `on_action(action)`
///
/// Clicking a slot runs widget Lua: `popup()` (when defined) toggles a
/// menu with its return — either body text or a table with `text`,
/// `width`/`height`, clickable `items` (`{ label, action }` rows
/// calling `on_action(action)`), and a composed `ui` body (any `ui.*`
/// tree, rendered above the items). `ui.button`s inside the body work
/// exactly like `items` rows (same `on_action` key); `items` remains
/// the shorthand for uniform full-width menu rows. Otherwise
/// `on_press()` runs as a
/// bare click action and the cell re-renders after it. A popup with no
/// text, tree, or items never opens. Size without items is just
/// `{ text = os.date("%A"), width = 300, height = 200 }`. Clicks are
/// hit-tested per widget (each cell widget owns a mouse area), so
/// every widget in a slot gets its own popup/`on_press` calls.
/// Presses record their target and only the matching release acts,
/// which also keeps inner `ui.button` clicks from double-firing the
/// widget. Gap clicks fall back to the slot (first popup, else first
/// `on_press`).
///
/// ```lua
/// local app = { details = false }
/// function app:view() return ui.text("Details") end
/// function app:popup()
///     return { ui = ui.row({ ui.icon("clock"), ui.text(os.date("%H:%M")) }),
///              width = 300, height = 200 }
/// end
///
/// function app:on_action(name)  -- popup items + cell buttons land here
///     if name == "toggle" then self.details = not self.details end
/// end
/// return app
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
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
/// a runtime concern, regenerated on load. Saved placements write
/// effective `interval`/`size` plus the explicit `props` overrides, so
/// the file shows what the bar renders while absent props keep
/// inheriting widget defaults; a bare `"name"` string in hand-written
/// config still means "inherit everything".
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

    /// Materialize effective scalar values: overrides win, missing
    /// interval/size fill from the definition defaults. Props stay
    /// sparse (overrides only) — absent keys inherit widget defaults at
    /// render. Saved placements persist in this form so the file shows
    /// exactly what the bar renders.
    pub fn materialized(&self, defaults: &WidgetDefaults) -> WidgetPlacement {
        WidgetPlacement {
            id: self.id.clone(),
            name: self.name.clone(),
            file: self.file.clone(),
            interval: Some(self.effective_interval(defaults)),
            size: Some(self.effective_size(defaults)),
            props: self.props.clone(),
        }
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
    /// effective `interval`/`size` materialized (see
    /// [`WidgetPlacement::materialized`]) and explicit `props`
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
/// `on_action` whitelists the four keys (never interpolates a raw
/// key into shell). Place it in a bar slot to use it.
/// to use it.
#[cfg(test)]
pub(crate) use seed::SEED_SYSTEM_LUA;

#[cfg(test)]
pub(crate) use seed::{SEED_CONTEXT_MENU, SEED_CONTEXT_MENU_ITEM, SEED_SELECTION_RECT};

/// Seed notification renderer: `render(n)` layouts one notification
/// card (`n` = `{ id, app, title, body, icon, urgency }`). Edit live —
/// visible cards re-render on save; delete the file to restore the
/// built-in layout.
#[cfg(test)]
pub(crate) use seed::SEED_NOTIFICATIONS_LUA;

/// Seed notification-center widget: a bell cell counting the live
/// queue, with a popup listing the newest notifications and a dismiss
/// button per row. Reads the `notifications` global (republished
/// before every render) and dismisses via the `on_action` return
/// convention (`{ dismiss = id }`). Uncomment its `[[widget]]` entry
/// in a bar slot to use it.
#[cfg(test)]
pub(crate) use seed::SEED_NOTIFY_CENTER_LUA;

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

/// Directory Lua `file` entries resolve against
/// (`~/.config/riced/widgets/`).
pub use paths::widgets_dir;

/// Adopted `widgets.toml` after retirement: definitions came from it,
/// values folded into placements or Lua defaults.
pub use paths::widgets_migrated_path;

/// Shared component library dir (`~/.config/riced/components/`):
/// every `*.lua` file (sorted) is concatenated and executed in each
/// widget state after `iced` is built, so `iced.define` components are
/// available to all widgets and the notification renderer. No
/// `require` needed (and none available — the sandbox nils it).
pub use paths::components_dir;

impl Config {
    /// Apply a runtime [`ConfigPatch`] to the live config in place.
    pub fn apply(&mut self, patch: ConfigPatch) {
        let c = &mut self.composable;
        let images = &mut self.background.image;
        match patch {
            ConfigPatch::ComposableSource(kind, src) => c.get_mut(kind).src = src,
            ConfigPatch::ComposableProp(kind, key, value) => {
                let props = &mut c.get_mut(kind).props;
                if let Some(value) = value {
                    props.insert(key, value);
                } else {
                    props.remove(&key);
                }
            }
            ConfigPatch::ThemeName(name) => self.theme.name = name,
            ConfigPatch::ThemeDarkmode(dark) => self.theme.darkmode = dark,
            ConfigPatch::ThemeVariant(variant) => self.theme.variant = variant,
            ConfigPatch::AnimationSpeed(speed) => self.animation.speed = speed,
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

    #[test]
    fn slot_entries_read_names_and_tables() {
        // Bare names inherit everything; tables carry overrides.
        let cfg: Config = toml::from_str(
            "[[bar]]\nanchor = \"top\"\nwidgets = [[\"clock\", { name = \"clock\", size = 16.0 }]]\n",
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
                // Integer TOML values coerce to float props.
                assert!(p.props.is_empty());
            }
            SlotEntry::Name(_) => panic!("expected table"),
        }
        // Integer prop values and legacy string slots still parse.
        let cfg: Config = toml::from_str(
            "[[bar]]\nanchor = \"top\"\nwidgets = [[{ name = \"x\", props = { n = 2 } }], \"clock\"]\n",
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
    fn materialized_fills_effective_values() {
        let mut defaults = WidgetDefaults::default();
        defaults
            .props
            .insert("format".to_string(), PropValue::Text("%H:%M".to_string()));
        // Bare placement materializes scalars from defaults; props
        // stay sparse (overrides only, inheriting the rest).
        let bare = WidgetPlacement {
            id: "w1".to_string(),
            name: "clock".to_string(),
            ..Default::default()
        };
        let full = bare.materialized(&defaults);
        assert_eq!(full.interval, Some(1.0));
        assert_eq!(full.size, Some(13.0));
        assert!(full.props.is_empty(), "absent props inherit");
        // Overrides win; unknown keys survive.
        let custom = WidgetPlacement {
            interval: Some(5.0),
            props: [("extra".to_string(), PropValue::Bool(true))]
                .into_iter()
                .collect(),
            ..bare
        };
        let full = custom.materialized(&defaults);
        assert_eq!(full.interval, Some(5.0));
        assert_eq!(full.size, Some(13.0));
        assert_eq!(full.props.len(), 1);
        assert_eq!(full.props["extra"], PropValue::Bool(true));
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
                    let value = toml::Value::try_from(p).unwrap();
                    assert!(value.get("id").is_none(), "{value}");
                    assert_eq!(value.get("size").and_then(|v| v.as_float()), Some(16.0));
                }
                _ => panic!("expected one full entry"),
            },
            _ => panic!("expected many"),
        }
    }

    #[test]
    fn explicit_source_and_props_win_over_legacy_scalars() {
        let cfg: Config = toml::from_str(
            r#"
            [composable.menu]
            width = 180.0
            height = 92.0
            [composable.context_menu]
            src = "custom-frame.lua"
            width = 178.0
            [composable.context_menu.props]
            width = 220.0
            height = 120.0
            [composable.context_menu_item]
            src = "custom-item.lua"
        "#,
        )
        .unwrap();
        assert_eq!(cfg.composable.context_menu.src, "custom-frame.lua");
        assert_eq!(
            cfg.composable.context_menu.props["width"],
            PropValue::Number(220.0)
        );
        assert_eq!(
            cfg.composable.context_menu.props["height"],
            PropValue::Number(120.0)
        );
        assert_eq!(cfg.composable.context_menu_item.src, "custom-item.lua");
    }

    #[test]
    fn parses_composable_context_menu_section() {
        let cfg: Config = toml::from_str("[composable.context_menu]\nwidth = 250.0\n").unwrap();
        assert_eq!(
            cfg.composable.context_menu.props["width"],
            PropValue::Number(250.0)
        );
        assert_eq!(cfg.composable.context_menu.src, "context_menu.lua");
    }

    #[test]
    fn composable_context_menu_falls_back_to_defaults() {
        let empty: Config = toml::from_str("").unwrap();
        assert_eq!(empty.composable.context_menu.src, "context_menu.lua");
        assert!(empty.composable.context_menu.props.is_empty());
    }

    #[test]
    fn partial_composable_style_keeps_provided_values() {
        // Regression: missing keys must default per-field, not reset the whole file.
        let cfg: Config = toml::from_str("[composable.context_menu]\npadding = 8.0\n").unwrap();
        assert_eq!(
            cfg.composable.context_menu.props["padding"],
            PropValue::Number(8.0)
        );
        assert_eq!(cfg.composable.context_menu.props.len(), 1);
    }

    #[test]
    fn parses_composable_context_menu_item_section() {
        let cfg: Config =
            toml::from_str("[composable.context_menu_item]\npadding = 4.0\nrounding = 2.0\n")
                .unwrap();
        assert_eq!(
            cfg.composable.context_menu_item.props["padding"],
            PropValue::Number(4.0)
        );
        assert_eq!(
            cfg.composable.context_menu_item.props["rounding"],
            PropValue::Number(2.0)
        );
    }

    #[test]
    fn context_menu_item_falls_back_to_defaults() {
        let empty: Config = toml::from_str("").unwrap();
        assert_eq!(
            empty.composable.context_menu_item.src,
            "context_menu_item.lua"
        );
        assert!(empty.composable.context_menu_item.props.is_empty());
        // unrelated section present, item still defaults
        let cfg: Config = toml::from_str("[composable.menu]\nwidth = 200.0\n").unwrap();
        assert!(cfg.composable.context_menu_item.props.is_empty());
    }

    #[test]
    fn legacy_flat_sections_are_ignored() {
        // Restructure note: per-component tables moved under [composable.*].
        // Old flat keys are unknown fields, which serde ignores.
        let cfg: Config = toml::from_str("[menu]\nwidth = 200.0\n").unwrap();
        assert!(cfg.composable.context_menu.props.is_empty());
    }

    #[test]
    fn parses_background_image_entries() {
        let cfg: Config = toml::from_str(
            "[[background.image]]\npath = \"/a.png\"\nx = 10.0\ny = 20.0\nz = 2\n\
             [[background.image]]\npath = \"/b.png\"\n",
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
        cfg.apply(ConfigPatch::ComposableProp(
            ComposableKind::ContextMenu,
            "rounding".into(),
            Some(PropValue::Number(6.0)),
        ));
        assert_eq!(
            cfg.composable.context_menu.props["rounding"],
            PropValue::Number(6.0)
        );
        // everything else untouched
        assert!(cfg.composable.context_menu_item.props.is_empty());
        assert!(!cfg.composable.context_menu.props.contains_key("padding"));
        cfg.apply(ConfigPatch::ComposableSource(
            ComposableKind::ContextMenuItem,
            "custom-item.lua".into(),
        ));
        assert_eq!(cfg.composable.context_menu_item.src, "custom-item.lua");
        cfg.apply(ConfigPatch::ComposableProp(
            ComposableKind::ContextMenu,
            "rounding".into(),
            None,
        ));
        assert!(cfg.composable.context_menu.props.is_empty());
    }

    #[test]
    fn theme_section_defaults_to_gruvbox_dark() {
        let empty: Config = toml::from_str("").unwrap();
        assert_eq!(empty.theme.name, "gruvbox");
        assert!(empty.theme.darkmode);
        assert_eq!(empty.theme.variant, "content");
        assert!(empty.theme.templates_dir.is_empty());
    }

    #[test]
    fn theme_section_parses_name_and_darkmode() {
        let cfg: Config =
            toml::from_str("[theme]\nname = \"dracula\"\ndarkmode = false\n").unwrap();
        assert_eq!(cfg.theme.name, "dracula");
        assert!(!cfg.theme.darkmode);
    }

    #[test]
    fn bar_section_parses_direct_array_with_defaults() {
        let cfg: Config = toml::from_str(
            "[[bar]]\nanchor = \"bottom\"\noutput = \"DP-1\"\nlength = 80.0\nfloating = true\nmargin_top = 8\n",
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
        let empty: Config = toml::from_str("").unwrap();
        assert!(empty.bar.is_empty());
    }

    #[test]
    fn bar_slots_parses_explicit_count() {
        let cfg: Config =
            toml::from_str("[[bar]]\nanchor = \"left\"\noutput = \"DP-1\"\nslots = 4\n").unwrap();
        assert_eq!(cfg.bar.len(), 1);
        assert_eq!(cfg.bar[0].slots, 4);
    }

    #[test]
    fn bar_aligns_default_empty_and_parse_names() {
        let sparse: Config = toml::from_str("[[bar]]\nanchor = \"top\"\n").unwrap();
        assert!(sparse.bar[0].aligns.is_empty());
        let cfg: Config =
            toml::from_str("[[bar]]\nanchor = \"top\"\naligns = [\"start\", \"end\"]\n").unwrap();
        assert_eq!(
            cfg.bar[0].aligns,
            vec!["start".to_string(), "end".to_string()]
        );
    }

    #[test]
    fn bar_widgets_default_empty_and_parse_names() {
        let sparse: Config = toml::from_str("[[bar]]\nanchor = \"top\"\n").unwrap();
        assert!(sparse.bar[0].widgets.is_empty());
        // Flat names and nested lists read the same shape.
        let cfg: Config = toml::from_str(
            "[[bar]]\nanchor = \"top\"\nwidgets = [[\"cpu\", \"ram\"], \"clock\"]\n",
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
            SlotWidgets::One(ref name) if name == "clock"
        ));
    }

    #[test]
    fn widgets_file_parses_lua_entries() {
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
    fn selection_composable_uses_src_and_sparse_props() {
        let config: Config = toml::from_str(
            r##"
            [composable.selection_rect]
            src = "custom-selection.lua"
            [composable.selection_rect.props]
            radius = 4.0
            color = "#ff0000"
        "##,
        )
        .unwrap();
        assert_eq!(config.composable.selection_rect.src, "custom-selection.lua");
        assert_eq!(
            config.composable.selection_rect.props["radius"],
            PropValue::Number(4.0)
        );
        let saved = toml::to_string(&config.composable.selection_rect).unwrap();
        let reloaded: SourceComposable = toml::from_str(&saved).unwrap();
        assert_eq!(reloaded.props, config.composable.selection_rect.props);
        assert_eq!(SourceComposable::default().src, "selection_rect.lua");
        assert!(SourceComposable::default().props.is_empty());
    }

    #[test]
    fn bar_slot_gaps_default_to_bare_cells() {
        let sparse: Config = toml::from_str("[[bar]]\nanchor = \"top\"\n").unwrap();
        assert_eq!(sparse.bar[0].slot_padding, 0.0);
        assert_eq!(sparse.bar[0].slot_spacing, 4.0);
        let cfg: Config =
            toml::from_str("[[bar]]\nanchor = \"top\"\nslot_padding = 6.0\nslot_spacing = 2.0\n")
                .unwrap();
        assert_eq!(cfg.bar[0].slot_padding, 6.0);
        assert_eq!(cfg.bar[0].slot_spacing, 2.0);
    }

    #[test]
    fn bar_opacity_defaults_to_opaque_and_parses_steps() {
        let sparse: Config = toml::from_str("[[bar]]\nanchor = \"top\"\n").unwrap();
        assert_eq!(sparse.bar[0].opacity, 1.0);
        let cfg: Config = toml::from_str("[[bar]]\nanchor = \"top\"\nopacity = 0.5\n").unwrap();
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
        let empty: Config = toml::from_str("").unwrap();
        assert_eq!(empty.animation.speed, AnimationSpeed::Medium);
        assert_eq!(
            AnimationSpeed::Medium.duration(),
            Duration::from_millis(150)
        );
    }

    #[test]
    fn notifications_section_defaults_and_parses() {
        let empty: Config = toml::from_str("").unwrap();
        assert!(empty.notifications.enabled);
        assert_eq!(empty.notifications.output, "mouse");
        assert_eq!(empty.notifications.position, "top-right");
        assert_eq!(empty.notifications.timeout_ms, 5000);
        assert_eq!(empty.notifications.max_visible, 3);
        let cfg: Config = toml::from_str(
            "[notifications]\nenabled = false\noutput = \"DP-1\"\nposition = \"bottom-left\"\ntimeout_ms = 8000\nmax_visible = 5\n",
        )
        .unwrap();
        assert!(!cfg.notifications.enabled);
        assert_eq!(cfg.notifications.output, "DP-1");
        assert_eq!(cfg.notifications.position, "bottom-left");
        assert_eq!(cfg.notifications.timeout_ms, 8000);
        assert_eq!(cfg.notifications.max_visible, 5);
    }

    #[test]
    fn animation_speed_parses_and_patches() {
        let cfg: Config = toml::from_str("[animation]\nspeed = \"fast\"\n").unwrap();
        assert_eq!(cfg.animation.speed, AnimationSpeed::Fast);
        assert_eq!(AnimationSpeed::Fast.duration(), Duration::from_millis(80));
        assert_eq!(AnimationSpeed::Slow.duration(), Duration::from_millis(300));
        let mut cfg = Config::default();
        cfg.apply(ConfigPatch::AnimationSpeed(AnimationSpeed::Slow));
        assert_eq!(cfg.animation.speed, AnimationSpeed::Slow);
    }
}
