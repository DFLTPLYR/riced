//! Explicit bundled-library and service fixtures for Lua contract tests.
use mlua::Lua;

pub(crate) fn load_seed_components(lua: &Lua) {
    for (_, source) in crate::config::builtin_component_files() {
        lua.load(source).exec().expect("seed component");
    }
}

pub(crate) fn publish_test_services(lua: &Lua, system: &sysinfo::System, gpu: Option<f32>) {
    let theme = crate::config::ThemeConfig::default();
    let outputs = std::collections::HashMap::new();
    let notifications = std::collections::VecDeque::new();
    let toplevels = crate::services::ToplevelCache::default();
    let workspaces = crate::services::WorkspaceCache::default();
    crate::services::publish_all(
        &crate::services::ServiceCtx {
            sys: system,
            gpu,
            theme: &theme,
            outputs: &outputs,
            notifications: &notifications,
            toplevels: &toplevels,
            workspaces: &workspaces,
        },
        lua,
    )
    .expect("services");
    crate::services::publish_bar(lua, "").expect("bar");
}
