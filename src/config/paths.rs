//! XDG-aware installation paths, independent of schema and persistence.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static PROJECT_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Select an existing project before starting any runtime workers.
pub fn set_project_root(root: PathBuf) -> Result<(), String> {
    PROJECT_ROOT
        .set(root)
        .map_err(|_| "project root already selected".into())
}
/// The selected project directory, if one was set before startup.
pub fn project_root() -> Option<PathBuf> {
    PROJECT_ROOT.get().cloned()
}
/// `~/.config/riced/config.lua`: the only shell config the runtime reads
/// (`$XDG_CONFIG_HOME` aware). Migrated once from `config.toml`.
pub fn config_path() -> PathBuf {
    if let Some(root) = PROJECT_ROOT.get() {
        return root.join("config.lua");
    }
    dirs::config_dir()
        .map(|d| d.join("riced").join("config.lua"))
        .unwrap_or_else(|| PathBuf::from("riced.lua"))
}
/// Legacy `config.toml`, read only by the one-time TOML → Lua migration.
/// `templates/config.toml` (theme templates) is unrelated and untouched.
pub fn legacy_config_path() -> PathBuf {
    config_path().with_file_name("config.toml")
}
/// Original `config.toml` after a successful migration.
pub fn config_migrated_path() -> PathBuf {
    config_path().with_file_name("config.toml.migrated")
}
pub fn widgets_path() -> PathBuf {
    if let Some(root) = PROJECT_ROOT.get() {
        return root.join("widgets.toml");
    }
    dirs::config_dir()
        .map(|d| d.join("riced").join("widgets.toml"))
        .unwrap_or_else(|| PathBuf::from("widgets.toml"))
}
pub fn widgets_dir() -> PathBuf {
    widgets_path()
        .parent()
        .map(|p| p.join("widgets"))
        .unwrap_or_else(|| PathBuf::from("widgets"))
}
pub fn widgets_migrated_path() -> PathBuf {
    widgets_path()
        .parent()
        .map(|p| p.join("widgets.toml.migrated"))
        .unwrap_or_else(|| PathBuf::from("widgets.toml.migrated"))
}
pub fn components_dir() -> PathBuf {
    widgets_dir()
        .parent()
        .unwrap_or(Path::new("."))
        .join("components")
}

/// Project-owned Settings page sources. Bundled copies are installed once;
/// existing directories are adopted as user-owned.
pub fn settings_dir() -> PathBuf {
    widgets_dir()
        .parent()
        .unwrap_or(Path::new("."))
        .join("settings")
}

pub fn shaders_dir() -> PathBuf {
    config_path().with_file_name("shaders")
}
