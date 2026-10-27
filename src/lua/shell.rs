//! Explicit host shell commands; Lua decides when, the host owns execution.
//!
//! A Lua action handler returns `shell.open_settings()` and the daemon performs
//! the corresponding shell action. Nothing runs during rendering: the value
//! travels back through the shared action boundary.
//!
//! Layer creation is deliberately *not* here. An app declares its layers in
//! `view` (see `riced.layer`) and owns any layer list as its own data, so the
//! runtime never has to invent a surface on request.
use mlua::{Lua, Table, Value};

/// A shell-level action a Lua host may request from the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellCommand {
    /// Toggle the Settings window, like the CLI `open-settings` command.
    OpenSettings,
    /// Append a bar placement to `config.bar`, the list Settings edits.
    AddBar {
        /// `top`, `bottom`, `left`, or `right`.
        anchor: String,
        /// Connector name; empty means every output.
        output: String,
        thickness: u32,
    },
    /// Remove a bar placement: the first entry matching this output and anchor.
    RemoveBar { anchor: String, output: String },
}

fn command(lua: &Lua, action: &str) -> mlua::Result<Table> {
    let outer = lua.create_table()?;
    let inner = lua.create_table()?;
    inner.set("action", action)?;
    outer.set("shell", inner)?;
    Ok(outer)
}

fn anchor_ok(anchor: &str) -> bool {
    matches!(anchor, "top" | "bottom" | "left" | "right")
}

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let api = lua.create_table()?;
    api.set(
        "open_settings",
        lua.create_function(|lua, ()| command(lua, "open-settings"))?,
    )?;
    api.set(
        "add_bar",
        lua.create_function(|lua, fields: Table| {
            let anchor: String = fields.get("anchor").unwrap_or_else(|_| "top".into());
            let output: String = fields.get("output").unwrap_or_default();
            let thickness: u32 = fields.get("thickness").unwrap_or(34);
            if !anchor_ok(&anchor) {
                return Err(mlua::Error::RuntimeError(
                    "shell.add_bar anchor must be top, bottom, left, or right".into(),
                ));
            }
            if output.len() > 64 || output.contains('\0') {
                return Err(mlua::Error::RuntimeError(
                    "shell.add_bar output must be at most 64 bytes".into(),
                ));
            }
            if !(8..=512).contains(&thickness) {
                return Err(mlua::Error::RuntimeError(
                    "shell.add_bar thickness must be in 8..=512".into(),
                ));
            }
            let inner = command(lua, "add-bar")?;
            let payload: Table = inner.raw_get("shell")?;
            payload.set("anchor", anchor)?;
            payload.set("output", output)?;
            payload.set("thickness", thickness)?;
            Ok(inner)
        })?,
    )?;
    api.set(
        "remove_bar",
        lua.create_function(|lua, fields: Table| {
            let anchor: String = fields.get("anchor").unwrap_or_else(|_| "top".into());
            let output: String = fields.get("output").unwrap_or_default();
            if !anchor_ok(&anchor) {
                return Err(mlua::Error::RuntimeError(
                    "shell.remove_bar anchor must be top, bottom, left, or right".into(),
                ));
            }
            let inner = command(lua, "remove-bar")?;
            let payload: Table = inner.raw_get("shell")?;
            payload.set("anchor", anchor)?;
            payload.set("output", output)?;
            Ok(inner)
        })?,
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
        "add-bar" => &["anchor", "output", "thickness"],
        "remove-bar" => &["anchor", "output"],
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
    let string = |name: &str| -> mlua::Result<String> {
        match inner.get::<Value>(name)? {
            Value::Nil => Ok(String::new()),
            Value::String(value) => Ok(value.to_str()?.to_owned()),
            _ => Err(mlua::Error::RuntimeError(format!(
                "shell command {name} must be a string"
            ))),
        }
    };
    match action.as_str() {
        "open-settings" => Ok(Some(ShellCommand::OpenSettings)),
        "add-bar" => {
            let anchor = string("anchor")?;
            if !anchor_ok(&anchor) {
                return Err(mlua::Error::RuntimeError(
                    "shell.add_bar anchor must be top, bottom, left, or right".into(),
                ));
            }
            let thickness = match inner.get::<Value>("thickness")? {
                Value::Integer(value) if (8..=512).contains(&value) => value as u32,
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "shell.add_bar thickness must be in 8..=512".into(),
                    ));
                }
            };
            Ok(Some(ShellCommand::AddBar {
                anchor,
                output: string("output")?,
                thickness,
            }))
        }
        "remove-bar" => {
            let anchor = string("anchor")?;
            if !anchor_ok(&anchor) {
                return Err(mlua::Error::RuntimeError(
                    "shell.remove_bar anchor must be top, bottom, left, or right".into(),
                ));
            }
            Ok(Some(ShellCommand::RemoveBar {
                anchor,
                output: string("output")?,
            }))
        }
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
            // Layer creation is not a shell action any more.
            "return {shell={action='add-layer',name='bar'}}",
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            assert!(parse(&value).is_err(), "{source}");
        }
    }
}
