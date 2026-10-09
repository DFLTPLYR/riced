//! Bundled sources; installation policy preserves user-owned libraries.
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
