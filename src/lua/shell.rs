//! Explicit host shell commands; Lua decides when, the host owns execution.
//!
//! A Lua action handler returns `shell.open_settings()` and the daemon performs
//! the corresponding shell action. Nothing runs during rendering: the value
//! travels back through the shared action boundary.
//!
//! Surfaces are deliberately *not* here. An app declares its layers in `view`
//! (see `riced.layer`) and owns any layer list as its own data, so the runtime
//! never has to invent a surface on request. Bars are Lua data too: adding or
//! removing one edits the app's own file, not a host command.
use mlua::{Lua, Table, Value};

/// A shell-level action a Lua host may request from the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellCommand {
    /// Toggle the Settings window, like the CLI `open-settings` command.
    OpenSettings,
}

fn command(lua: &Lua, action: &str) -> mlua::Result<Table> {
    let outer = lua.create_table()?;
    let inner = lua.create_table()?;
    inner.set("action", action)?;
    outer.set("shell", inner)?;
    Ok(outer)
}

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let api = lua.create_table()?;
    api.set(
        "open_settings",
        lua.create_function(|lua, ()| command(lua, "open-settings"))?,
    )?;
    lua.globals().set("shell", api)
}

/// Parse a Lua action result into a shell command, if it is one.
pub(crate) fn parse(value: &Value) -> mlua::Result<Option<ShellCommand>> {
    let Value::Table(outer) = value else {
        return Ok(None);
    };
    let value: Value = outer.raw_get("shell")?;
    if value == Value::Nil {
        return Ok(None);
    }
    let Value::Table(inner) = value else {
        return Err(mlua::Error::RuntimeError(
            "shell command must be a table".into(),
        ));
    };
    let action: String = inner.get("action")?;
    let allowed: &[&str] = match action.as_str() {
        "open-settings" => &[],
        _ => {
            return Err(mlua::Error::RuntimeError(format!(
                "unknown shell command {action}"
            )));
        }
    };
    for pair in inner.clone().pairs::<String, Value>() {
        let (key, _) = pair?;
        if key != "action" && !allowed.contains(&key.as_str()) {
            return Err(mlua::Error::RuntimeError(format!(
                "unknown shell command field {key}"
            )));
        }
    }
    match action.as_str() {
        "open-settings" => Ok(Some(ShellCommand::OpenSettings)),
        _ => Err(mlua::Error::RuntimeError(format!(
            "unknown shell command {action}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        install(&lua).unwrap();
        lua
    }

    #[test]
    fn commands_round_trip_and_reject_unknown_actions_or_fields() {
        let lua = lua();
        let value: Value = lua.load("return shell.open_settings()").eval().unwrap();
        assert_eq!(parse(&value).unwrap(), Some(ShellCommand::OpenSettings));
        // Values from other Lua hosts are not shell commands.
        for source in [
            "return {other=true}",
            "return {tray={id='1',action='open'}}",
            "return nil",
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            assert_eq!(parse(&value).unwrap(), None, "{source}");
        }
        for source in [
            "return {shell={action='nope'}}",
            "return {shell={action='open-settings',extra=1}}",
            "return {shell='open-settings'}",
            // Bars are app data now, not a shell action.
            "return {shell={action='add-bar',anchor='top'}}",
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            assert!(parse(&value).is_err(), "{source}");
        }
    }
}
