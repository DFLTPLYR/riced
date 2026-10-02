use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Top-level `config.toml`. Unknown keys are ignored so old files
/// keep loading after new sections are added.
///
/// Layout is per-component tables, each key defaulting to `0.0`:
/// ```toml
/// [composable.menu]
/// width = 180.0
/// height = 92.0
/// # padding = 8.0         # when omitted, defaults to 0.0
///
/// [composable.context_menu]
/// width = 180.0
///
/// [composable.context_menu_item]
/// # padding = 4.0
/// ```
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
///                    # child alignment per slot (both axes)
/// widgets = ["clock", "none"]
///                    # widget names from widgets.toml by slot position
///                    # (`none` = placeholder; unknown names render as
///                    # placeholders too)
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
    default_width: f32 = 180.0,
    default_menu_height: f32 = 92.0,
    default_scale: f32 = 1.0,
    default_bar_length: f32 = 100.0,
    default_bar_thickness: f32 = 50.0,
    default_bar_slots: u32 = 1,
    default_bar_opacity: f32 = 1.0,
    default_widget_size: f32 = 13.0,
}

fn default_widget_type() -> String {
    "label".to_string()
}

fn default_clock_format() -> String {
    "%H:%M".to_string()
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

/// Global animation speed, applied to every animated transition
/// (selection fade and any interpolated color/property change).
/// ```toml
/// [animation]
/// speed = "medium"
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnimationSpeed {
    Fast,
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

impl Default for AnimationSpeed {
    fn default() -> Self {
        Self::Medium
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimationConfig {
    pub speed: AnimationSpeed,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            speed: AnimationSpeed::default(),
        }
    }
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ComposableConfig {
    pub menu: MenuConfig,
    pub context_menu: ContextMenuConfig,
    pub context_menu_item: ContextMenuItemConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMenuConfig {
    #[serde(default = "default_width")]
    pub width: f32,
    #[serde(default)]
    pub padding: f32,
    #[serde(default)]
    pub spacing: f32,
    #[serde(default)]
    pub rounding: f32,
}

impl Default for ContextMenuConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            padding: 0.0,
            spacing: 0.0,
            rounding: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuConfig {
    #[serde(default = "default_width")]
    pub width: f32,
    #[serde(default = "default_menu_height")]
    pub height: f32,
    #[serde(default)]
    pub padding: f32,
    #[serde(default)]
    pub spacing: f32,
    #[serde(default)]
    pub rounding: f32,
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_menu_height(),
            padding: 0.0,
            spacing: 0.0,
            rounding: 0.0,
        }
    }
}

/// Style keys for the buttons inside the context menu.
/// The gap *between* items is the parent column's spacing, so it stays on
/// `ContextMenuConfig`; buttons are `Fill`-width, so no width key lives here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMenuItemConfig {
    #[serde(default)]
    pub padding: f32,
    #[serde(default)]
    pub rounding: f32,
}

impl Default for ContextMenuItemConfig {
    fn default() -> Self {
        Self {
            padding: 0.0,
            rounding: 0.0,
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
    /// Child alignment per slot by position (`start`/`center`/`end`,
    /// both axes; unknown entries read as `center`). Shorter lists
    /// pad centered, longer ones truncate.
    #[serde(default)]
    pub aligns: Vec<String>,
    /// Widget name per slot by position, resolved against
    /// `widgets.toml` (`none`/unknown = numbered placeholder).
    /// Shorter lists pad empty, longer ones truncate.
    #[serde(default)]
    pub widgets: Vec<String>,
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

impl BackgroundImage {
    /// Files arrive as `file://` URIs (pickers, drag-drop) or plain paths
    /// (hand-written config). The image pipeline wants a plain path.
    ///
    /// Accepted forms, in resolution order: `file://` scheme stripped,
    /// `$VAR`/`${VAR}` expanded from the environment, then a leading `~`
    /// expanded to the home dir. Prefer `~/Pictures/…` in hand-written
    /// config: unlike `$CUSTOM_VAR` it doesn't depend on the daemon's
    /// environment, and unlike absolute paths it survives username changes.
    pub fn local_path(&self) -> PathBuf {
        let s = self.path.trim();
        let stripped = s.strip_prefix("file://").unwrap_or(s);
        let stripped = stripped.strip_prefix("localhost").unwrap_or(stripped);
        let expanded = expand_env(&percent_decode(stripped));
        expand_tilde(&expanded)
    }
}

/// Expand `$VAR` and `${VAR}` from the environment. Undefined vars and lone
/// `$` pass through untouched (the path then simply fails to open downstream).
fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&ch) = chars.peek() {
            let take = if braced {
                ch != '}'
            } else {
                ch.is_alphanumeric() || ch == '_'
            };
            if take {
                name.push(ch);
                chars.next();
            } else {
                break;
            }
        }
        if braced {
            if chars.peek() == Some(&'}') {
                chars.next();
            } else {
                // Unterminated `${…`: emit literally.
                out.push_str("${");
                out.push_str(&name);
                continue;
            }
        }
        if name.is_empty() {
            out.push('$');
            continue;
        }
        match std::env::var(&name) {
            Ok(v) => out.push_str(&v),
            Err(_) => {
                out.push('$');
                if braced {
                    out.push('{');
                    out.push_str(&name);
                    out.push('}');
                } else {
                    out.push_str(&name);
                }
            }
        }
    }
    out
}

/// Expand a leading `~` / `~/` to the home dir (`dirs` crate, already used
/// for the config dir). Anything else passes through; `~otheruser` is
/// intentionally unsupported.
fn expand_tilde(s: &str) -> PathBuf {
    if s == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    } else if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

/// Minimal `%XX` decoder for file URIs (`%20` spaces etc.). Malformed
/// sequences pass through untouched.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (
                hex_val(bytes.get(i + 1).copied().unwrap_or(0)),
                hex_val(bytes.get(i + 2).copied().unwrap_or(0)),
            )
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Synchronously decode a wallpaper file to RGBA bytes for a pre-warmed
/// [`Handle`](iced::widget::image::Handle). File-backed handles decode on a
/// worker thread whose completion redraw the shell drops, leaving first paint
/// blank — serving `from_rgba` instead loads synchronously ("very cheap" per
/// the renderer) so pixels exist on the very first frame. `None` for missing
/// or undecodable files (caller skips the entry, same as before).
pub(crate) fn decode_handle(
    path: &std::path::Path,
) -> Option<(u32, u32, iced::widget::image::Handle)> {
    use iced::widget::image::Handle;

    let img = image::open(path).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w == 0 || h == 0 {
        return None;
    }
    let handle = Handle::from_rgba(w, h, bytes::Bytes::from(rgba.into_raw()));
    Some((w, h, handle))
}

/// A single runtime edit to the live config (e.g. from a Settings-panel
/// control). Applied to the single `Plots::config` source of truth, then
/// broadcast via full redraw — every view re-reads `plots.config`, so all
/// components pick the new value up on the next frame. No per-component
/// subscription registry needed; iced views are pure functions of state.
#[derive(Debug, Clone)]
pub enum ConfigPatch {
    MenuWidth(f32),
    MenuHeight(f32),
    MenuPadding(f32),
    MenuSpacing(f32),
    MenuRounding(f32),
    ContextMenuWidth(f32),
    ContextMenuPadding(f32),
    ContextMenuSpacing(f32),
    ContextMenuRounding(f32),
    ContextMenuItemPadding(f32),
    ContextMenuItemRounding(f32),
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
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("riced").join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("riced.toml"))
}

/// Declarative bar widgets (`~/.config/riced/widgets.toml`,
/// `$XDG_CONFIG_HOME` aware). Slots reference entries by `name`
/// (see `[[bar]] widgets`); `type` picks the renderer, the rest are
/// per-type params:
/// ```toml
/// [[widget]]
/// name = "clock"   # slot reference
/// type = "clock"   # clock | label
/// format = "%H:%M" # clock only (%H %M %S)
/// size = 13.0
///
/// [[widget]]
/// name = "hello"
/// type = "label"
/// text = "hello"
/// size = 13.0
/// ```
/// Unknown `type` values load fine and render as placeholders, never
/// an error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetDef {
    /// Slot reference (`[[bar]] widgets = [...]`).
    #[serde(default)]
    pub name: String,
    /// Renderer: `clock` | `label` (case-insensitive).
    #[serde(rename = "type", default = "default_widget_type")]
    pub widget_type: String,
    /// Clock only: time format (`%H` `%M` `%S`; empty = `%H:%M`).
    #[serde(default = "default_clock_format")]
    pub format: String,
    /// Label only: static text.
    #[serde(default)]
    pub text: String,
    /// Text size for either renderer.
    #[serde(default = "default_widget_size")]
    pub size: f32,
}

impl Default for WidgetDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            widget_type: default_widget_type(),
            format: default_clock_format(),
            text: String::new(),
            size: default_widget_size(),
        }
    }
}

/// Whole `widgets.toml`: one `[[widget]]` entry per declarative widget.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetsFile {
    #[serde(default)]
    pub widget: Vec<WidgetDef>,
}

/// Seed written when `widgets.toml` does not exist yet (same idea as
/// the theme/template seeds): a clock plus a commented label example.
const SEED_WIDGETS_TOML: &str = r#"# Riced widgets: declarative bar widgets referenced by [[bar]] `widgets`.
# `type` picks the renderer: "clock" (local time via `format`) or "label".

[[widget]]
name = "clock"
type = "clock"
format = "%H:%M"
size = 13.0

# [[widget]]
# name = "hello"
# type = "label"
# text = "hello"
# size = 13.0
"#;

/// `~/.config/riced/widgets.toml` (`$XDG_CONFIG_HOME` aware).
pub fn widgets_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("riced").join("widgets.toml"))
        .unwrap_or_else(|| PathBuf::from("widgets.toml"))
}

impl WidgetsFile {
    fn parse(content: &str) -> Vec<WidgetDef> {
        match toml::from_str::<WidgetsFile>(content) {
            Ok(file) => file.widget,
            Err(e) => {
                eprintln!("widgets: parse error, no widgets: {e}");
                Vec::new()
            }
        }
    }

    /// Load from [`widgets_path`]. Creates the file with a seeded clock
    /// (plus parent dirs) when it does not exist yet.
    pub fn load() -> (Vec<WidgetDef>, Option<SystemTime>) {
        let path = widgets_path();
        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&path, SEED_WIDGETS_TOML) {
                eprintln!("widgets: cannot write {}: {e}", path.display());
            }
            return (Self::parse(SEED_WIDGETS_TOML), read_mtime(&path));
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => (Self::parse(&content), read_mtime(&path)),
            Err(_) => (Vec::new(), None),
        }
    }

    /// Hot-reload check for the poll tick: fresh defs when the file
    /// changed since `known_mtime` (or appeared). Never fails.
    pub fn poll(known_mtime: &Option<SystemTime>) -> Option<(Vec<WidgetDef>, Option<SystemTime>)> {
        let path = widgets_path();
        let mtime = read_mtime(&path);
        if mtime != *known_mtime && path.exists() {
            let (defs, mtime) = Self::load();
            if mtime != *known_mtime {
                return Some((defs, mtime));
            }
        }
        None
    }
}

fn read_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn parse(content: &str) -> Config {
    match toml::from_str(content) {
        Ok(cfg) => migrate_legacy_top(cfg),
        Err(e) => {
            eprintln!("config: parse error, keeping defaults: {e}");
            Config::default()
        }
    }
}

/// Move legacy `[top.<name>]` entries into `[[bar]]` when no bars exist
/// yet (sorted by name for determinism). The legacy map is cleared so a
/// later save writes only the new format.
fn migrate_legacy_top(mut cfg: Config) -> Config {
    if cfg.bar.is_empty() && !cfg.top.is_empty() {
        let mut names: Vec<_> = cfg.top.keys().cloned().collect();
        names.sort();
        cfg.bar = names
            .into_iter()
            .filter_map(|k| cfg.top.remove(&k))
            .collect();
    } else {
        cfg.top.clear();
    }
    cfg
}

impl Config {
    /// Load from `path`, or defaults on any error (missing/unparseable).
    pub fn load_from(path: &std::path::Path) -> (Self, Option<SystemTime>) {
        match std::fs::read_to_string(path) {
            Ok(content) => (parse(&content), read_mtime(path)),
            Err(_) => (Config::default(), None),
        }
    }

    /// Load from [`config_path`]. Creates the file with defaults
    /// (plus parent dirs) when it does not exist yet.
    pub fn load() -> (Self, Option<SystemTime>) {
        let path = config_path();
        if !path.exists() {
            let cfg = Config::default();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match toml::to_string_pretty(&cfg) {
                Ok(text) => {
                    if let Err(e) = std::fs::write(&path, text) {
                        eprintln!("config: cannot write {}: {e}", path.display());
                    }
                }
                Err(e) => eprintln!("config: cannot serialize defaults: {e}"),
            }
            return (cfg, read_mtime(&path));
        }
        Self::load_from(&path)
    }

    /// Hot-reload check for the poll tick: returns the fresh config
    /// when the file changed since `known_mtime` (or appeared).
    /// Never fails — parse errors keep serving the old config.
    pub fn poll(known_mtime: &Option<SystemTime>) -> Option<(Self, Option<SystemTime>)> {
        let path = config_path();
        let mtime = read_mtime(&path);
        if mtime != *known_mtime && path.exists() {
            let (cfg, mtime) = Self::load_from(&path);
            // Only report when the mtime actually advanced; a failed
            // read keeps the old stamp so we retry next tick.
            if mtime != *known_mtime {
                return Some((cfg, mtime));
            }
        }
        None
    }

    /// Apply a runtime [`ConfigPatch`] to the live config in place.
    pub fn apply(&mut self, patch: ConfigPatch) {
        let c = &mut self.composable;
        let images = &mut self.background.image;
        match patch {
            ConfigPatch::MenuWidth(v) => c.menu.width = v,
            ConfigPatch::MenuHeight(v) => c.menu.height = v,
            ConfigPatch::MenuPadding(v) => c.menu.padding = v,
            ConfigPatch::MenuSpacing(v) => c.menu.spacing = v,
            ConfigPatch::MenuRounding(v) => c.menu.rounding = v,
            ConfigPatch::ContextMenuWidth(v) => c.context_menu.width = v,
            ConfigPatch::ContextMenuPadding(v) => c.context_menu.padding = v,
            ConfigPatch::ContextMenuSpacing(v) => c.context_menu.spacing = v,
            ConfigPatch::ContextMenuRounding(v) => c.context_menu.rounding = v,
            ConfigPatch::ContextMenuItemPadding(v) => c.context_menu_item.padding = v,
            ConfigPatch::ContextMenuItemRounding(v) => c.context_menu_item.rounding = v,
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

    /// Persist the live config to [`config_path`], returning the new mtime
    /// (so the hot-reload poll doesn't immediately "reload" what we wrote).
    /// Best-effort: failures log and keep serving memory state.
    pub fn save(&self) -> Option<SystemTime> {
        let path = config_path();
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    eprintln!("config: cannot write {}: {e}", path.display());
                    return read_mtime(&path);
                }
                read_mtime(&path)
            }
            Err(e) => {
                eprintln!("config: cannot serialize: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_composable_menu_section() {
        let cfg: Config =
            toml::from_str("[composable.menu]\nwidth = 200.0\nheight = 100.0\n").unwrap();
        assert_eq!(cfg.composable.menu.width, 200.0);
        assert_eq!(cfg.composable.menu.height, 100.0);
    }

    #[test]
    fn composable_menu_falls_back_to_defaults() {
        let cfg: Config = toml::from_str("[composable.menu]\nwidth = 200.0\n").unwrap();
        assert_eq!(cfg.composable.menu.width, 200.0);
        assert_eq!(cfg.composable.menu.height, 92.0);
        assert_eq!(cfg.composable.menu.padding, 0.0);
        let empty: Config = toml::from_str("").unwrap();
        assert_eq!(empty.composable.menu.width, 180.0);
        assert_eq!(empty.composable.menu.padding, 0.0);
    }

    #[test]
    fn parses_composable_context_menu_section() {
        let cfg: Config = toml::from_str("[composable.context_menu]\nwidth = 250.0\n").unwrap();
        assert_eq!(cfg.composable.context_menu.width, 250.0);
        // unrelated sections keep their own values
        assert_eq!(cfg.composable.menu.width, 180.0);
        assert_eq!(cfg.composable.menu.height, 92.0);
    }

    #[test]
    fn composable_context_menu_falls_back_to_defaults() {
        let cfg: Config = toml::from_str("[composable.menu]\nwidth = 200.0\n").unwrap();
        assert_eq!(cfg.composable.context_menu.width, 180.0);
        let empty: Config = toml::from_str("").unwrap();
        assert_eq!(empty.composable.context_menu.width, 180.0);
    }

    #[test]
    fn partial_composable_style_keeps_provided_values() {
        // Regression: missing keys must default per-field, not reset the whole file.
        let cfg: Config = toml::from_str("[composable.context_menu]\npadding = 8.0\n").unwrap();
        assert_eq!(cfg.composable.context_menu.padding, 8.0);
        assert_eq!(cfg.composable.context_menu.spacing, 0.0);
        assert_eq!(cfg.composable.context_menu.rounding, 0.0);
        assert_eq!(cfg.composable.context_menu.width, 180.0);
    }

    #[test]
    fn parses_composable_context_menu_item_section() {
        let cfg: Config =
            toml::from_str("[composable.context_menu_item]\npadding = 4.0\nrounding = 2.0\n")
                .unwrap();
        assert_eq!(cfg.composable.context_menu_item.padding, 4.0);
        assert_eq!(cfg.composable.context_menu_item.rounding, 2.0);
    }

    #[test]
    fn context_menu_item_falls_back_to_defaults() {
        let empty: Config = toml::from_str("").unwrap();
        assert_eq!(empty.composable.context_menu_item.padding, 0.0);
        assert_eq!(empty.composable.context_menu_item.rounding, 0.0);
        // unrelated section present, item still defaults
        let cfg: Config = toml::from_str("[composable.menu]\nwidth = 200.0\n").unwrap();
        assert_eq!(cfg.composable.context_menu_item.padding, 0.0);
        assert_eq!(cfg.composable.context_menu_item.rounding, 0.0);
    }

    #[test]
    fn legacy_flat_sections_are_ignored() {
        // Restructure note: per-component tables moved under [composable.*].
        // Old flat keys are unknown fields, which serde ignores.
        let cfg: Config = toml::from_str("[menu]\nwidth = 200.0\n").unwrap();
        assert_eq!(cfg.composable.menu.width, 180.0);
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
    fn decode_handle_roundtrips_a_real_file() {
        use image::{ImageBuffer, Rgba};

        let dir = std::env::temp_dir().join(format!("riced-decode-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tiny.png");
        ImageBuffer::<Rgba<u8>, _>::from_pixel(3, 2, Rgba([9, 8, 7, 255]))
            .save(&path)
            .unwrap();

        let (w, h, handle) = decode_handle(&path).expect("decodable png");
        assert_eq!((w, h), (3, 2));
        // Handle carries the pre-decoded pixels (Rgba variant, sync load).
        assert!(matches!(handle, iced::widget::image::Handle::Rgba { .. }));

        assert!(decode_handle(&dir.join("missing.png")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_path_handles_uris_and_plain_paths() {
        let plain = BackgroundImage {
            path: "/home/u/pic.png".into(),
            ..Default::default()
        };
        assert_eq!(plain.local_path(), PathBuf::from("/home/u/pic.png"));

        let uri = BackgroundImage {
            path: "file:///home/dfltplyr/Pictures/Wallpaper/handcamera.png".into(),
            ..Default::default()
        };
        assert_eq!(
            uri.local_path(),
            PathBuf::from("/home/dfltplyr/Pictures/Wallpaper/handcamera.png")
        );

        let host = BackgroundImage {
            path: "file://localhost/home/u/my%20pic.png".into(),
            ..Default::default()
        };
        assert_eq!(host.local_path(), PathBuf::from("/home/u/my pic.png"));

        let empty = BackgroundImage::default();
        assert!(empty.local_path().as_os_str().is_empty());
    }

    #[test]
    fn local_path_expands_tilde_and_env() {
        if let Some(home) = dirs::home_dir() {
            let tilde = BackgroundImage {
                path: "~/Pictures/handcamera.png".into(),
                ..Default::default()
            };
            assert_eq!(tilde.local_path(), home.join("Pictures/handcamera.png"));
        }

        // Deterministic vars (unique names: tests run in parallel).
        // SAFETY: test-only, unique var name, no other thread touches it.
        unsafe { std::env::set_var("RICED_TEST_PICS", "/pics") };
        let dollar = BackgroundImage {
            path: "$RICED_TEST_PICS/w.png".into(),
            ..Default::default()
        };
        assert_eq!(dollar.local_path(), PathBuf::from("/pics/w.png"));
        let braced = BackgroundImage {
            path: "${RICED_TEST_PICS}/w.png".into(),
            ..Default::default()
        };
        assert_eq!(braced.local_path(), PathBuf::from("/pics/w.png"));
        let combo = BackgroundImage {
            path: "file://$RICED_TEST_PICS/my%20pic.png".into(),
            ..Default::default()
        };
        assert_eq!(combo.local_path(), PathBuf::from("/pics/my pic.png"));

        // Undefined vars and lone `$` pass through untouched.
        let undef = BackgroundImage {
            path: "$RICED_TEST_UNDEFINED_XYZ/w.png".into(),
            ..Default::default()
        };
        assert_eq!(
            undef.local_path(),
            PathBuf::from("$RICED_TEST_UNDEFINED_XYZ/w.png")
        );
    }

    #[test]
    fn patch_updates_only_the_targeted_leaf() {
        let mut cfg = Config::default();
        cfg.apply(ConfigPatch::ContextMenuRounding(6.0));
        assert_eq!(cfg.composable.context_menu.rounding, 6.0);
        // everything else untouched
        assert_eq!(cfg.composable.menu.width, 180.0);
        assert_eq!(cfg.composable.context_menu.padding, 0.0);
        cfg.apply(ConfigPatch::MenuWidth(250.0));
        assert_eq!(cfg.composable.menu.width, 250.0);
        assert_eq!(cfg.composable.context_menu.rounding, 6.0);
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
        let cfg: Config =
            toml::from_str("[[bar]]\nanchor = \"top\"\nwidgets = [\"clock\", \"none\"]\n").unwrap();
        assert_eq!(
            cfg.bar[0].widgets,
            vec!["clock".to_string(), "none".to_string()]
        );
    }

    #[test]
    fn widgets_file_parses_clock_and_label() {
        let file: WidgetsFile = toml::from_str(
            "[[widget]]\nname = \"clock\"\ntype = \"clock\"\nformat = \"%H:%M:%S\"\n\
             [[widget]]\nname = \"hello\"\ntype = \"label\"\ntext = \"hi\"\n",
        )
        .unwrap();
        assert_eq!(file.widget.len(), 2);
        assert_eq!(file.widget[0].widget_type, "clock");
        assert_eq!(file.widget[0].format, "%H:%M:%S");
        assert_eq!(file.widget[0].size, 13.0);
        assert_eq!(file.widget[1].text, "hi");
        // Sparse entry defaults the rest.
        let sparse: WidgetsFile = toml::from_str("[[widget]]\nname = \"x\"\n").unwrap();
        assert_eq!(sparse.widget[0].widget_type, "label");
        assert_eq!(sparse.widget[0].format, "%H:%M");
    }

    #[test]
    fn bar_opacity_defaults_to_opaque_and_parses_steps() {
        let sparse: Config = toml::from_str("[[bar]]\nanchor = \"top\"\n").unwrap();
        assert_eq!(sparse.bar[0].opacity, 1.0);
        let cfg: Config = toml::from_str("[[bar]]\nanchor = \"top\"\nopacity = 0.5\n").unwrap();
        assert_eq!(cfg.bar[0].opacity, 0.5);
    }

    #[test]
    fn legacy_top_tables_migrate_to_bar_array() {
        // NOTE: migration runs in `parse()` (file loads), not on raw
        // `toml::from_str` — the file path is what matters in prod.
        let cfg: Config =
            parse("[top.main]\nanchor = \"bottom\"\noutput = \"DP-1\"\nlength = 80.0\n");
        assert_eq!(cfg.bar.len(), 1);
        assert_eq!(cfg.bar[0].anchor, "bottom");
        assert_eq!(cfg.bar[0].output, "DP-1");
        // Legacy map is consumed, so a re-save writes only [[bar]].
        assert!(cfg.top.is_empty());
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("[[bar]]"));
        assert!(!text.contains("[top."));
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
