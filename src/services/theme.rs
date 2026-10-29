//! `theme` service: resolved host palette shared with `iced.palette()`.

use super::registry::ServiceCtx;

/// Publish `theme = { primary = "#rrggbb", on_primary = …, … }` via
/// [`crate::theme::lua_palette`].
pub fn publish(ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
    crate::lua::bridge::publish_theme(lua, ctx.theme)
}
