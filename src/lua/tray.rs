//! Explicit tray commands; Lua owns rendering, the host owns D-Bus delivery.
use crate::tray::{TrayAction, TrayCache, TrayCommand};
use mlua::{Lua, Table, Value};

fn command(lua: &Lua, id: String, action: &str) -> mlua::Result<(Table, Table)> {
    if id.is_empty() {
        return Err(mlua::Error::RuntimeError(
            "tray id must not be empty".into(),
        ));
    }
    let outer = lua.create_table()?;
    let inner = lua.create_table()?;
    inner.set("id", id)?;
    inner.set("action", action)?;
    outer.set("tray", inner.clone())?;
    Ok((outer, inner))
}

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let api = lua.create_table()?;
    for action in ["activate", "secondary", "context"] {
        api.set(
            action,
            lua.create_function(move |lua, (id, x, y): (String, Option<i32>, Option<i32>)| {
                let (outer, inner) = command(lua, id, action)?;
                inner.set("x", x.unwrap_or(0))?;
                inner.set("y", y.unwrap_or(0))?;
                Ok(outer)
            })?,
        )?;
    }
    api.set(
        "scroll",
        lua.create_function(|lua, (id, delta, direction): (String, i32, String)| {
            if !matches!(direction.as_str(), "vertical" | "horizontal") {
                return Err(mlua::Error::RuntimeError(
                    "tray scroll orientation must be vertical or horizontal".into(),
                ));
            }
            let (outer, inner) = command(lua, id, "scroll")?;
            inner.set("delta", delta)?;
            inner.set("orientation", direction)?;
            Ok(outer)
        })?,
    )?;
    for (name, action, popup) in [
        ("open", "open", true),
        ("aboutToShow", "open", false),
        ("menuEvent", "menu", false),
    ] {
        api.set(
            name,
            lua.create_function(move |lua, (id, entry): (String, Option<i32>)| {
                let (outer, inner) = command(lua, id, action)?;
                inner.set("entry", entry.unwrap_or(0))?;
                if popup {
                    outer.set("popup", true)?;
                }
                Ok(outer)
            })?,
        )?;
    }
    lua.globals().set("tray", api)
}

fn parse(value: &Value) -> mlua::Result<Option<TrayCommand>> {
    let Value::Table(outer) = value else {
        return Ok(None);
    };
    let value: Value = outer.raw_get("tray")?;
    if value == Value::Nil {
        return Ok(None);
    }
    let Value::Table(inner) = value else {
        return Err(mlua::Error::RuntimeError(
            "tray command must be a table".into(),
        ));
    };
    let id: String = inner.get("id")?;
    if id.is_empty() {
        return Err(mlua::Error::RuntimeError(
            "tray id must not be empty".into(),
        ));
    }
    let action: String = inner.get("action")?;
    let fields: &[&str] = match action.as_str() {
        "activate" | "secondary" | "context" => &["x", "y"],
        "scroll" => &["delta", "orientation"],
        "open" | "menu" => &["entry"],
        _ => {
            return Err(mlua::Error::RuntimeError(format!(
                "unknown tray action {action}"
            )));
        }
    };
    for pair in inner.clone().pairs::<String, Value>() {
        let (key, _) = pair?;
        if key != "id" && key != "action" && !fields.contains(&key.as_str()) {
            return Err(mlua::Error::RuntimeError(format!(
                "unknown tray command field {key}"
            )));
        }
    }
    let action = match action.as_str() {
        "activate" => TrayAction::Activate(inner.get("x")?, inner.get("y")?),
        "secondary" => TrayAction::Secondary(inner.get("x")?, inner.get("y")?),
        "context" => TrayAction::Context(inner.get("x")?, inner.get("y")?),
        "scroll" => {
            let direction: String = inner.get("orientation")?;
            if !matches!(direction.as_str(), "vertical" | "horizontal") {
                return Err(mlua::Error::RuntimeError(
                    "tray scroll orientation must be vertical or horizontal".into(),
                ));
            }
            TrayAction::Scroll(inner.get("delta")?, direction)
        }
        "open" => TrayAction::Open(inner.get("entry")?),
        _ => TrayAction::MenuEvent(inner.get("entry")?),
    };
    Ok(Some(TrayCommand { id, action }))
}

pub(crate) fn dispatch(value: &Value, cache: &TrayCache) -> Result<(), String> {
    if let Some(command) = parse(value).map_err(|error| error.to_string())? {
        cache.command(command)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn open_requests_popup_and_routes_a_stable_item_key() {
        let lua = Lua::new();
        install(&lua).unwrap();
        let value: Value = lua.load("return tray.open(':1.4/Item')").eval().unwrap();
        assert!(crate::lua::widgets::command_requests_popup(&value));
        assert_eq!(
            parse(&value).unwrap(),
            Some(TrayCommand {
                id: ":1.4/Item".into(),
                action: TrayAction::Open(0)
            })
        );
    }
    #[test]
    fn commands_reject_unknown_keys_and_invalid_orientations() {
        let lua = Lua::new();
        install(&lua).unwrap();
        for source in [
            "return {tray={id='x',action='open',entry=0,typo=true}}",
            "return {tray={id='x',action='scroll',delta=1,orientation='diagonal'}}",
        ] {
            assert!(parse(&lua.load(source).eval().unwrap()).is_err());
        }
    }
}
