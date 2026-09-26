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

impl Default for Config {
    fn default() -> Self {
        Self {
            composable: ComposableConfig::default(),
        }
    }
}

/// A single runtime edit to the live config (e.g. from a Settings-panel
/// control). Applied to the single `Plots::config` source of truth, then
/// broadcast via full redraw — every view re-reads `plots.config`, so all
/// components pick the new value up on the next frame. No per-component
/// subscription registry needed; iced views are pure functions of state.
#[derive(Debug, Clone, Copy)]
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
