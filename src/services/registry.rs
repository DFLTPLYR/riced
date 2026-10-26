//! Service registry: one `publish_all` call site for every Lua entry
//! point (widget render/popup/action, popup paths, notification cards).
//!
//! Adding a service means: new module implementing [`Service`], one
//! line in `SERVICES` below, tests for its table shape.

use super::toplevels::ToplevelCache;
use super::workspaces::WorkspaceCache;
use std::collections::{HashMap, VecDeque};

/// Read-only snapshot inputs every service publishes from. All borrows
/// are shared and short-lived; nothing outlives the publish call.
pub struct ServiceCtx<'a> {
    pub sys: &'a sysinfo::System,
    pub gpu: Option<f32>,
    /// Monotonic seconds at the most recent sample. `system.sampled` exposes
    /// this so time-series widgets advance on their own clock.
    pub sampled: f64,
    pub theme: &'a crate::config::ThemeConfig,
    pub component_defaults: &'a crate::ui::settings::ComponentDefaults,
    pub outputs:
        &'a HashMap<iced_wayland_subscriber::OutputId, iced_wayland_subscriber::OutputInfo>,
    pub notifications: &'a VecDeque<crate::shell::screens::notification::Notification>,
    pub toplevels: &'a ToplevelCache,
    pub workspaces: &'a WorkspaceCache,
    pub tray: &'a crate::tray::TrayCache,
}

/// One namespaced Lua table (`system`, `theme`, …).
pub trait Service {
    fn namespace(&self) -> &'static str;
    fn publish(&self, ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()>;
}

struct SystemService;
struct ThemeService;
struct NotificationsService;
struct WaylandService;
struct TrayService;

impl Service for TrayService {
    fn namespace(&self) -> &'static str {
        "systemtray"
    }
    fn publish(&self, ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
        super::tray::publish(ctx, lua)
    }
}

impl Service for SystemService {
    fn namespace(&self) -> &'static str {
        "system"
    }
    fn publish(&self, ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
        super::system::publish(ctx, lua)
    }
}

impl Service for ThemeService {
    fn namespace(&self) -> &'static str {
        "theme"
    }
    fn publish(&self, ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
        super::theme::publish(ctx, lua)
    }
}

impl Service for NotificationsService {
    fn namespace(&self) -> &'static str {
        "notifications"
    }
    fn publish(&self, ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
        super::notifications::publish(ctx, lua)
    }
}

impl Service for WaylandService {
    fn namespace(&self) -> &'static str {
        "wayland"
    }
    fn publish(&self, ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
        super::wayland::publish(ctx, lua)
    }
}

const SERVICES: &[&dyn Service] = &[
    &SystemService,
    &ThemeService,
    &NotificationsService,
    &WaylandService,
    &TrayService,
];

/// Framework topics share the existing publishers; no extra polling or IPC here.
pub(crate) fn valid_topic(name: &str) -> bool {
    SERVICES.iter().any(|service| service.namespace() == name)
        || crate::framework::extensions::registry().is_ok_and(|registry| registry.contains(name))
        || matches!(
            name,
            "wayland.workspaces" | "wayland.outputs" | "wayland.toplevels"
        )
}

pub(crate) fn snapshot(lua: &mlua::Lua, name: &str) -> mlua::Result<serde_json::Value> {
    use mlua::LuaSerdeExt;
    let registry = crate::framework::extensions::registry().map_err(mlua::Error::external)?;
    if registry.contains(name) {
        let data = registry.snapshot(name).map_err(mlua::Error::external)?;
        let available = registry.available(name).map_err(mlua::Error::external)?;
        return Ok(serde_json::json!({"name":name,"available":available,"data":data}));
    }
    if !valid_topic(name) {
        return Err(mlua::Error::RuntimeError(format!(
            "unknown service {name:?}"
        )));
    }
    let mut value: mlua::Value = mlua::Value::Table(lua.globals());
    for part in name.split('.') {
        let mlua::Value::Table(table) = value else {
            return Ok(serde_json::json!({"name":name,"available":false}));
        };
        value = table.raw_get(part)?;
    }
    let available = !value.is_nil();
    // Snapshot subscriptions carry data only, not executable functions or image handles.
    let data: serde_json::Value = lua.from_value_with(
        value,
        mlua::serde::de::Options::new().deny_unsupported_types(false),
    )?;
    Ok(serde_json::json!({"name":name,"available":available,"data":data}))
}

/// Publish every service table into `lua` globals.
pub fn publish_all(ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
    crate::ui::settings::publish(lua, ctx.component_defaults)?;
    for service in SERVICES {
        service
            .publish(ctx, lua)
            .map_err(|e| mlua::Error::RuntimeError(format!("{}: {e}", service.namespace())))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_all_namespaces_once() {
        let mut names: Vec<_> = SERVICES.iter().map(|s| s.namespace()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["notifications", "system", "systemtray", "theme", "wayland"]
        );
    }

    #[test]
    fn unified_snapshots_strip_opaque_values_and_validate_topics() {
        struct ImageToken;
        impl mlua::UserData for ImageToken {}
        let lua = mlua::Lua::new();
        let data = lua.create_table().unwrap();
        data.set("count", 1).unwrap();
        data.set("icon", lua.create_userdata(ImageToken).unwrap())
            .unwrap();
        lua.globals().set("systemtray", data).unwrap();
        let snapshot = snapshot(&lua, "systemtray").unwrap();
        assert_eq!(snapshot["data"]["count"], 1);
        assert!(snapshot["data"]["icon"].is_null());
        assert!(super::snapshot(&lua, "missing").is_err());
        assert!(valid_topic("wayland.workspaces"));
    }
}
