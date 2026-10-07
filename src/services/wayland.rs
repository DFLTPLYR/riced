//! `wayland` service: outputs plus Hyprland workspaces/toplevels.
//!
//! Outputs come from the existing `output_infos` (no new IPC).
//! Workspaces/toplevels come from the per-tick [`HyprCache`]; when the
//! compositor isn't Hyprland (or the socket is unreachable) those
//! tables are simply empty — same degrade stance as `gpu_usage` nil.

use super::registry::ServiceCtx;
use crate::app::layers::background::Background;

/// Publish `wayland = { outputs = …, workspaces = …, toplevels = …,
/// active_workspace = … }`:
/// - `outputs`: `{name, x, y, w, h}` sorted by name (deterministic).
/// - `workspaces`: `{id, name, monitor, windows}` in compositor order.
/// - `toplevels`: `{class, title, workspace}` in compositor order.
/// - `active_workspace`: focused workspace id, nil when unknown.
pub fn publish(ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
    let table = lua.create_table()?;

    let mut outputs: Vec<_> = ctx.outputs.iter().collect();
    outputs.sort_by(|a, b| {
        a.1.name
            .as_deref()
            .unwrap_or("")
            .cmp(b.1.name.as_deref().unwrap_or(""))
    });
    let out_list = lua.create_table()?;
    for (i, (_id, info)) in outputs.iter().enumerate() {
        let (x, y, w, h) = Background::output_geometry(info);
        let entry = lua.create_table()?;
        entry.set("name", info.name.clone().unwrap_or_default())?;
        entry.set("x", x)?;
        entry.set("y", y)?;
        entry.set("w", w)?;
        entry.set("h", h)?;
        out_list.set(i + 1, entry)?;
    }
    table.set("outputs", out_list)?;

    let ws_list = lua.create_table()?;
    for (i, ws) in ctx.hypr.workspaces.iter().enumerate() {
        let entry = lua.create_table()?;
        entry.set("id", ws.id)?;
        entry.set("name", ws.name.clone())?;
        entry.set("monitor", ws.monitor.clone())?;
        entry.set("windows", ws.windows)?;
        ws_list.set(i + 1, entry)?;
    }
    table.set("workspaces", ws_list)?;

    let tl_list = lua.create_table()?;
    for (i, c) in ctx.hypr.clients.iter().enumerate() {
        let entry = lua.create_table()?;
        entry.set("class", c.class.clone())?;
        entry.set("title", c.title.clone())?;
        entry.set("workspace", c.workspace)?;
        tl_list.set(i + 1, entry)?;
    }
    table.set("toplevels", tl_list)?;

    match ctx.hypr.active_workspace {
        Some(id) => table.set("active_workspace", id)?,
        None => table.set("active_workspace", mlua::Value::Nil)?,
    }

    lua.globals().set("wayland", table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::hypr::{HyprCache, HyprClient, HyprWorkspace};

    #[test]
    fn wayland_tables_cover_outputs_and_hypr_rows() {
        let lua = mlua::Lua::new();
        let sys = sysinfo::System::new();
        let theme = crate::config::ThemeConfig::default();
        let outputs = std::collections::HashMap::new();
        let queue = std::collections::VecDeque::new();
        let hypr = HyprCache {
            workspaces: vec![HyprWorkspace {
                id: 1,
                name: "1".to_string(),
                monitor: "DP-1".to_string(),
                windows: 2,
            }],
            clients: vec![HyprClient {
                class: "foot".to_string(),
                title: "t".to_string(),
                workspace: 1,
            }],
            active_workspace: Some(1),
        };
        let ctx = ServiceCtx {
            sys: &sys,
            gpu: None,
            theme: &theme,
            outputs: &outputs,
            notifications: &queue,
            hypr: &hypr,
        };
        publish(&ctx, &lua).expect("publish");
        let mon: String = lua
            .load("return wayland.workspaces[1].monitor")
            .eval()
            .expect("ws");
        assert_eq!(mon, "DP-1");
        let class: String = lua
            .load("return wayland.toplevels[1].class")
            .eval()
            .expect("tl");
        assert_eq!(class, "foot");
        let active: i32 = lua
            .load("return wayland.active_workspace")
            .eval()
            .expect("active");
        assert_eq!(active, 1);
    }
}
