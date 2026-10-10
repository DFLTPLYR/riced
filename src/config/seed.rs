//! Bundled sources; installation policy preserves user-owned libraries.
use super::discovery::component_seed_aliases;
use super::migrate::migrate_components_in;
use super::paths::{components_dir, widgets_dir};
use std::path::{Path, PathBuf};

/// Initialization lives outside the library so deletion never triggers reseeding.
pub(super) fn seed_marker(dir: &Path) -> PathBuf {
    let name = dir
        .file_name()
        .expect("Lua library directory")
        .to_string_lossy();
    dir.parent()
        .unwrap_or(Path::new("."))
        .join(format!(".{name}-initialized"))
}
pub(super) fn initialize_scripts_in(dir: &Path, seed: fn(&Path)) {
    let marker = seed_marker(dir);
    if marker.exists() {
        return;
    }
    if !dir.exists() {
        seed(dir);
    }
    if dir.exists()
        && let Err(error) = std::fs::write(&marker, "")
    {
        eprintln!(
            "Lua library: cannot record initialization {}: {error}",
            marker.display()
        );
    }
}

pub(crate) const SEED_CLOCK_LUA: &str = include_str!("../../scripts/widgets/clock.lua");
pub(crate) const SEED_HELLO_LUA: &str = include_str!("../../scripts/widgets/hello.lua");
pub(crate) const SEED_STATS_LUA: &str = include_str!("../../scripts/widgets/stats.lua");
pub(crate) const SEED_CPU_LUA: &str = include_str!("../../scripts/widgets/cpu.lua");
pub(crate) const SEED_RAM_LUA: &str = include_str!("../../scripts/widgets/ram.lua");
pub(crate) const SEED_GPU_LUA: &str = include_str!("../../scripts/widgets/gpu.lua");
pub(crate) const SEED_SYSTEM_LUA: &str = include_str!("../../scripts/widgets/system.lua");
pub(crate) const SEED_COMPONENT_DEFINE: &str =
    include_str!("../../scripts/components/00-define.lua");
pub(crate) const SEED_COMPONENT_STYLED: &str =
    include_str!("../../scripts/components/05-styled.lua");
pub(crate) const SEED_SELECTION_RECT: &str =
    include_str!("../../scripts/components/selection_rect.lua");
pub(crate) const SEED_CONTEXT_MENU: &str =
    include_str!("../../scripts/components/context_menu.lua");
pub(crate) const SEED_CONTEXT_MENU_ITEM: &str =
    include_str!("../../scripts/components/context_menu_item.lua");
pub(crate) const SEED_COMPONENT_CARD: &str = include_str!("../../scripts/components/10-card.lua");
pub(crate) const SEED_COMPONENT_MENU: &str = include_str!("../../scripts/components/20-menu.lua");
pub(crate) const SEED_NOTIFICATIONS_LUA: &str =
    include_str!("../../scripts/widgets/notifications.lua");
pub(crate) const SEED_NOTIFY_CENTER_LUA: &str =
    include_str!("../../scripts/widgets/notifycenter.lua");
pub(crate) const SEED_WORKSPACES_LUA: &str = include_str!("../../scripts/widgets/workspaces.lua");
pub(crate) const SEED_CLINEPASS_LUA: &str = include_str!("../../scripts/widgets/clinepass.lua");

pub(super) fn seed_widgets_in(dir: &Path) {
    for (name, content) in [
        ("clock.lua", SEED_CLOCK_LUA),
        ("hello.lua", SEED_HELLO_LUA),
        ("stats.lua", SEED_STATS_LUA),
        ("cpu.lua", SEED_CPU_LUA),
        ("ram.lua", SEED_RAM_LUA),
        ("gpu.lua", SEED_GPU_LUA),
        ("workspaces.lua", SEED_WORKSPACES_LUA),
        ("clinepass.lua", SEED_CLINEPASS_LUA),
        ("system.lua", SEED_SYSTEM_LUA),
        ("notifications.lua", SEED_NOTIFICATIONS_LUA),
        ("notifycenter.lua", SEED_NOTIFY_CENTER_LUA),
    ] {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, content) {
            eprintln!("widgets: cannot write {}: {error}", path.display());
        }
    }
}

/// Seed one component file when missing (never overwrite).
fn seed_component(dir: &std::path::Path, name: &str, content: &str) {
    let path = dir.join(name);
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, content) {
        eprintln!("components: cannot write {}: {e}", path.display());
    }
}

/// Populate a fresh component library without overwriting existing files.
pub(super) fn seed_components_in(dir: &std::path::Path) {
    for (name, content) in [
        ("00-define.lua", SEED_COMPONENT_DEFINE),
        ("05-styled.lua", SEED_COMPONENT_STYLED),
        ("10-card.lua", SEED_COMPONENT_CARD),
        ("20-menu.lua", SEED_COMPONENT_MENU),
        ("selection_rect.lua", SEED_SELECTION_RECT),
        ("context_menu.lua", SEED_CONTEXT_MENU),
        ("context_menu_item.lua", SEED_CONTEXT_MENU_ITEM),
    ] {
        if component_seed_aliases()
            .iter()
            .any(|(seed, alias)| name == *seed && dir.join(alias).is_file())
        {
            continue;
        }
        seed_component(dir, name, content);
    }
}

/// Initialize shared components once, preserving intentional deletions.
fn seed_components() {
    if seed_marker(&components_dir()).exists() {
        return;
    }
    migrate_components_in(&widgets_dir().join("components"), &components_dir());
    initialize_scripts_in(&components_dir(), seed_components_in);
}

impl super::WidgetsFile {
    /// Populate a fresh widget library without overwriting existing files.
    #[cfg(test)]
    fn seed_all_in(dir: &std::path::Path) {
        seed_widgets_in(dir);
    }

    /// Initialize fresh widget/component libraries; existing libraries are
    /// user-owned, including files that were intentionally removed.
    fn seed_all() {
        initialize_scripts_in(&widgets_dir(), seed_widgets_in);
        seed_components();
    }

    /// Seed fresh installs once before discovery. Rescans and restarts do
    /// not restore removed scripts or touch `widgets.toml`.
    pub(crate) fn seed_widget_scripts() {
        Self::seed_all();
    }
}
#[cfg(test)]
mod tests {
    use super::super::WidgetsFile;
    use super::super::discovery::{component_files_in, shared_component_files_in};
    use super::*;

    #[test]
    fn lua_libraries_seed_once_and_respect_deleted_files_and_directories() {
        let root = std::env::temp_dir().join(format!("riced-seed-once-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (name, seed, removed) in [
            (
                "widgets",
                WidgetsFile::seed_all_in as fn(&std::path::Path),
                "hello.lua",
            ),
            (
                "components",
                seed_components_in as fn(&std::path::Path),
                "10-card.lua",
            ),
        ] {
            let dir = root.join(name);
            initialize_scripts_in(&dir, seed);
            assert!(dir.join(removed).exists());
            assert!(seed_marker(&dir).exists());
            std::fs::write(dir.join("custom.lua"), "-- user content").unwrap();
            std::fs::remove_file(dir.join(removed)).unwrap();
            initialize_scripts_in(&dir, seed);
            assert!(!dir.join(removed).exists());
            assert_eq!(
                std::fs::read_to_string(dir.join("custom.lua")).unwrap(),
                "-- user content"
            );
            std::fs::remove_dir_all(&dir).unwrap();
            initialize_scripts_in(&dir, seed);
            assert!(!dir.exists());
            if name == "components" {
                assert!(component_files_in(&dir).is_empty());
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_lua_libraries_are_adopted_without_restoring_missing_seeds() {
        let root = std::env::temp_dir().join(format!("riced-seed-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let widgets = root.join("widgets");
        std::fs::create_dir_all(&widgets).unwrap();
        std::fs::write(widgets.join("clock.lua"), "-- my clock").unwrap();
        initialize_scripts_in(&widgets, WidgetsFile::seed_all_in);
        assert!(!widgets.join("hello.lua").exists());
        assert_eq!(
            std::fs::read_to_string(widgets.join("clock.lua")).unwrap(),
            "-- my clock"
        );
        assert!(seed_marker(&widgets).exists());
        let components = root.join("components");
        std::fs::create_dir_all(&components).unwrap();
        initialize_scripts_in(&components, seed_components_in);
        assert!(shared_component_files_in(&components).is_empty());
        assert!(component_files_in(&components).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundled_widget_seeds_do_not_overwrite_custom_files() {
        // Hermetic temp dir (no env manipulation: parallel tests share
        // process-global env, so seeds take an explicit dir instead).
        let dir = std::env::temp_dir().join(format!("riced-widgets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        WidgetsFile::seed_all_in(&dir);
        assert!(dir.join("clock.lua").is_file());
        assert!(dir.join("cpu.lua").is_file());
        assert!(dir.join("gpu.lua").is_file());
        assert!(dir.join("hello.lua").is_file());
        assert!(dir.join("workspaces.lua").is_file());
        assert!(dir.join("clinepass.lua").is_file());
        assert!(dir.join("system.lua").is_file());
        assert!(dir.join("notifications.lua").is_file());
        assert!(dir.join("ram.lua").is_file());
        assert!(dir.join("stats.lua").is_file());
        // A user script is never overwritten by a re-seed.
        std::fs::write(dir.join("clock.lua"), "-- mine").unwrap();
        WidgetsFile::seed_all_in(&dir);
        assert_eq!(
            std::fs::read_to_string(dir.join("clock.lua")).unwrap(),
            "-- mine"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn components_seeds_restore_without_overwriting() {
        let dir =
            std::env::temp_dir().join(format!("riced-components-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        seed_components_in(&dir);
        assert!(dir.join("00-define.lua").is_file());
        assert!(dir.join("10-card.lua").is_file());
        assert!(dir.join("20-menu.lua").is_file());
        std::fs::write(dir.join("10-card.lua"), "-- mine").unwrap();
        seed_components_in(&dir);
        assert_eq!(
            std::fs::read_to_string(dir.join("10-card.lua")).unwrap(),
            "-- mine"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
