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
    pub theme: &'a crate::config::ThemeConfig,
    pub outputs:
        &'a HashMap<iced_wayland_subscriber::OutputId, iced_wayland_subscriber::OutputInfo>,
    pub notifications: &'a VecDeque<crate::app::layers::notification::Notification>,
    pub toplevels: &'a ToplevelCache,
    pub workspaces: &'a WorkspaceCache,
}
impl<'a> ServiceCtx<'a> {
    /// Snapshot every service input from live app state. `gpu` is the
    /// already-computed usage reading (`None` degrades to nil).
    pub fn from_plots(plots: &'a crate::app::app::Plots, gpu: Option<f32>) -> Self {
        Self {
            sys: &plots.sysinfo,
            gpu,
            theme: &plots.config.theme,
            outputs: &plots.output_infos,
            notifications: &plots.notifications,
            toplevels: &plots.toplevel_cache,
            workspaces: &plots.workspace_cache,
        }
    }
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
];

/// Publish every service table into `lua` globals.
pub fn publish_all(ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
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
        assert_eq!(names, ["notifications", "system", "theme", "wayland"]);
    }
}
