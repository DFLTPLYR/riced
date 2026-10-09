//! XDG-aware installation paths, independent of schema and persistence.
use std::path::{Path, PathBuf};
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("riced").join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("riced.toml"))
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
