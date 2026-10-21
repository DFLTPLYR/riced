//! XDG-aware installation paths, independent of schema and persistence.
use std::path::{Path, PathBuf};
/// `~/.config/riced/config.lua`: the only shell config the runtime reads
/// (`$XDG_CONFIG_HOME` aware). Migrated once from `config.toml`.
pub fn config_path() -> PathBuf {
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

pub fn shaders_dir() -> PathBuf {
    config_path().with_file_name("shaders")
}
