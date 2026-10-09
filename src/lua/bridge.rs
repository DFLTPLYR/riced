//! Host snapshot publication. Palette globals have the same shape in all VMs.
use crate::config::ThemeConfig;
use mlua::Lua;
pub(crate) fn publish_theme(lua: &Lua, theme: &ThemeConfig) -> mlua::Result<()> {
    let table = lua.create_table()?;
    for (key, color) in crate::theme::lua_palette(theme) {
        table.set(key, color)?;
    }
    lua.globals().set("theme", table)
}
