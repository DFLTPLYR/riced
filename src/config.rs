use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

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
/// [composable.panel]
/// # padding = 4.0
///
/// [composable.context_menu]
/// width = 180.0
///
/// [composable.context_menu_item]
/// # padding = 4.0
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub composable: ComposableConfig,
    pub background: BackgroundConfig,
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
}

/// Per-component tables under `[composable.*]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ComposableConfig {
    pub menu: MenuConfig,
    pub panel: PanelConfig,
    pub context_menu: ContextMenuConfig,
    pub context_menu_item: ContextMenuItemConfig,
}

impl Default for ComposableConfig {
    fn default() -> Self {
        Self {
            menu: MenuConfig::default(),
            panel: PanelConfig::default(),
            context_menu: ContextMenuConfig::default(),
            context_menu_item: ContextMenuItemConfig::default(),
        }
    }
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

/// Style keys for the `Panel` composable (selection overlay, bars).
/// Rendered geometry there is positional, so only style keys live here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelConfig {
    #[serde(default)]
    pub padding: f32,
    #[serde(default)]
    pub spacing: f32,
    #[serde(default)]
    pub rounding: f32,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundConfig {
    #[serde(default)]
    pub image: Vec<BackgroundImage>,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self { image: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    } else if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
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
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                hex_val(bytes.get(i + 1).copied().unwrap_or(0)),
                hex_val(bytes.get(i + 2).copied().unwrap_or(0)),
            ) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
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

impl Default for Config {
    fn default() -> Self {
        Self {
            composable: ComposableConfig::default(),
            background: BackgroundConfig::default(),
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
    MenuWidth(f32),
    MenuHeight(f32),
    MenuPadding(f32),
    MenuSpacing(f32),
    MenuRounding(f32),
    PanelPadding(f32),
    PanelSpacing(f32),
    PanelRounding(f32),
    ContextMenuWidth(f32),
    ContextMenuPadding(f32),
    ContextMenuSpacing(f32),
    ContextMenuRounding(f32),
    ContextMenuItemPadding(f32),
    ContextMenuItemRounding(f32),
    AddImage(BackgroundImage),
    MoveImage { index: usize, x: f32, y: f32 },
    SetImageScale { index: usize, scale: f32 },
    SetImageZ { index: usize, z: i32 },
    RemoveImage { index: usize },
}

/// `~/.config/riced/config.toml` (`$XDG_CONFIG_HOME` aware).
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("riced").join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("riced.toml"))
}

fn read_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn parse(content: &str) -> Config {
    match toml::from_str(content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("config: parse error, keeping defaults: {e}");
            Config::default()
        }
    }
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
            ConfigPatch::PanelPadding(v) => c.panel.padding = v,
            ConfigPatch::PanelSpacing(v) => c.panel.spacing = v,
            ConfigPatch::PanelRounding(v) => c.panel.rounding = v,
            ConfigPatch::ContextMenuWidth(v) => c.context_menu.width = v,
            ConfigPatch::ContextMenuPadding(v) => c.context_menu.padding = v,
            ConfigPatch::ContextMenuSpacing(v) => c.context_menu.spacing = v,
            ConfigPatch::ContextMenuRounding(v) => c.context_menu.rounding = v,
            ConfigPatch::ContextMenuItemPadding(v) => c.context_menu_item.padding = v,
            ConfigPatch::ContextMenuItemRounding(v) => c.context_menu_item.rounding = v,
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
        cfg.apply(ConfigPatch::PanelRounding(6.0));
        assert_eq!(cfg.composable.panel.rounding, 6.0);
        // everything else untouched
        assert_eq!(cfg.composable.menu.width, 180.0);
        assert_eq!(cfg.composable.panel.padding, 0.0);
        cfg.apply(ConfigPatch::MenuWidth(250.0));
        assert_eq!(cfg.composable.menu.width, 250.0);
        assert_eq!(cfg.composable.panel.rounding, 6.0);
    }
}
