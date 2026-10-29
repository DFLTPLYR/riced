//! Host snapshot publication. Palette globals have the same shape in all VMs.
use crate::config::ThemeConfig;
use mlua::Lua;
pub(crate) fn publish_theme(lua: &Lua, theme: &ThemeConfig) -> mlua::Result<()> {
    let active = crate::theme::sync(theme);
    lua.set_app_data(active);
    let table = lua.create_table()?;
    for (key, color) in crate::theme::palette::snapshot(&active) {
        table.set(key, color)?;
    }
    lua.globals().set("theme", table)
}
