//! `wayland` service: outputs plus native toplevels.
//!
//! Outputs come from the existing `output_infos` (no new IPC).
//! Toplevels come from [`ToplevelCache`] (`ext-foreign-toplevel-list`);
//! compositors without the protocol yield an empty list — same degrade
//! stance as `gpu_usage` nil. No workspace or focus state exists here:
//! standard Wayland defines neither, so there is nothing to publish.

use super::Toplevel;
use super::registry::ServiceCtx;
use crate::app::layers::background::Background;

/// Publish `wayland = { outputs = …, toplevels = … }`:
/// - `outputs`: `{name, x, y, w, h}` sorted by name (deterministic).
/// - `toplevels`: `{app_id, title}`, sorted for stable order.
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

    let tl_list = lua.create_table()?;
    let rows: Vec<Toplevel> = ctx.toplevels.snapshot();
    for (i, tl) in rows.iter().enumerate() {
        let entry = lua.create_table()?;
        entry.set("app_id", tl.app_id.clone())?;
        entry.set("title", tl.title.clone())?;
        tl_list.set(i + 1, entry)?;
    }
    table.set("toplevels", tl_list)?;

    lua.globals().set("wayland", table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{Toplevel, ToplevelCache};

    #[test]
    fn wayland_tables_cover_outputs_and_toplevels() {
        let lua = mlua::Lua::new();
        let sys = sysinfo::System::new();
        let theme = crate::config::ThemeConfig::default();
        let outputs = std::collections::HashMap::new();
        let queue = std::collections::VecDeque::new();
        let toplevels = ToplevelCache::from_rows(vec![Toplevel {
            app_id: "foot".to_string(),
            title: "shell".to_string(),
        }]);
        let ctx = ServiceCtx {
            sys: &sys,
            gpu: None,
            theme: &theme,
            outputs: &outputs,
            notifications: &queue,
            toplevels: &toplevels,
        };
        publish(&ctx, &lua).expect("publish");
        let app_id: String = lua
            .load("return wayland.toplevels[1].app_id")
            .eval()
            .expect("tl");
        assert_eq!(app_id, "foot");
        let title: String = lua
            .load("return wayland.toplevels[1].title")
            .eval()
            .expect("title");
        assert_eq!(title, "shell");
        let count: i64 = lua.load("return #wayland.outputs").eval().expect("len");
        assert_eq!(count, 0);
    }
}
