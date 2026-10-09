//! Explicit VM profiles. Native widget shell access is intentionally preserved.
use mlua::{Lua, LuaOptions, StdLib, Table, Value};
#[derive(Clone, Copy)]
pub(crate) enum Profile {
    App,
    Widget,
}
pub(crate) fn new_lua(profile: Profile) -> mlua::Result<Lua> {
    let libs = match profile {
        Profile::App => StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        Profile::Widget => StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::OS | StdLib::IO,
    };
    let lua = Lua::new_with(libs, LuaOptions::default())?;
    if matches!(profile, Profile::Widget) {
        let globals = lua.globals();
        for key in ["dofile", "loadfile", "require"] {
            globals.set(key, Value::Nil)?;
        }
        let os: Table = globals.get("os")?;
        for key in ["exit", "remove", "rename", "setlocale"] {
            os.set(key, Value::Nil)?;
        }
    }
    Ok(lua)
}
