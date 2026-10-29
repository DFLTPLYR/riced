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
/// The project directory (`~/.config/riced` by default).
pub fn project_dir() -> PathBuf {
    if let Some(root) = PROJECT_ROOT.get() {
        return root.clone();
    }
    dirs::config_dir()
        .map(|d| d.join("riced"))
        .unwrap_or_else(|| PathBuf::from("."))
}
/// `data/config.lua`: the only shell config the runtime reads. Settings owns
/// the file; the entry point is `shell.lua`.
pub fn config_path() -> PathBuf {
    project_dir().join("data").join("config.lua")
}
/// Pre-`data/` config, migrated into `data/config.lua` on the next load. The
/// original file is kept, never deleted.
pub fn legacy_lua_config_path() -> PathBuf {
    project_dir().join("config.lua")
}
/// The config file a CLI command reads for an explicit project directory.
pub fn config_path_in(project: &Path) -> PathBuf {
    let current = project.join("data").join("config.lua");
    if current.is_file() {
        current
    } else {
        project.join("config.lua")
    }
}
/// Legacy `config.toml`, read only by the one-time TOML → Lua migration.
/// `templates/config.toml` (theme templates) is unrelated and untouched.
pub fn legacy_config_path() -> PathBuf {
    project_dir().join("config.toml")
}
/// Original `config.toml` after a successful migration.
pub fn config_migrated_path() -> PathBuf {
    project_dir().join("config.toml.migrated")
}
pub fn theme_dir() -> PathBuf {
    project_dir().join("theme")
}
pub fn templates_dir() -> PathBuf {
    project_dir().join("templates")
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
    project_dir().join("shaders")
}
