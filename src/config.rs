use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// Top-level `config.toml`. Unknown keys are ignored so old files
/// keep loading after new sections are added.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub menu: MenuConfig,
}

/// `[menu]` section — replaces the old `MENU_W` / `MENU_H` consts.
///
/// ```toml
/// [menu]
/// width = 180.0
/// height = 92.0
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuConfig {
    #[serde(default = "default_menu_width")]
    pub width: f32,
    #[serde(default = "default_menu_height")]
    pub height: f32,
}

fn default_menu_width() -> f32 {
    180.0
}

fn default_menu_height() -> f32 {
    92.0
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            width: default_menu_width(),
            height: default_menu_height(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            menu: MenuConfig::default(),
        }
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_menu_section() {
        let cfg: Config = toml::from_str("[menu]\nwidth = 200.0\nheight = 100.0\n").unwrap();
        assert_eq!(cfg.menu.width, 200.0);
        assert_eq!(cfg.menu.height, 100.0);
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let cfg: Config = toml::from_str("[menu]\nwidth = 200.0\n").unwrap();
        assert_eq!(cfg.menu.width, 200.0);
        assert_eq!(cfg.menu.height, 92.0);
        let empty: Config = toml::from_str("").unwrap();
        assert_eq!(empty.menu.width, 180.0);
    }
}
