//! Notification service data/actions for Lua; scene rendering is host-generic.
use mlua::{Lua, Table, Value};

pub(crate) fn actions_table(lua: &Lua, actions: &[(String, String)]) -> Result<Table, String> {
    let table = lua.create_table().map_err(|error| error.to_string())?;
    for (index, (key, label)) in actions.iter().enumerate() {
        let action = lua.create_table().map_err(|error| error.to_string())?;
        action
            .set("key", key.as_str())
            .map_err(|error| error.to_string())?;
        action
            .set("label", label.as_str())
            .map_err(|error| error.to_string())?;
        table
            .set(index + 1, action)
            .map_err(|error| error.to_string())?;
    }
    Ok(table)
}
#[derive(Debug, PartialEq)]
pub(crate) enum NotificationAction {
    Dismiss(u32),
    DismissAll,
    Invoke(u32, String),
}
pub(crate) fn notification_action(value: &Value) -> Option<NotificationAction> {
    let Value::Table(table) = value else {
        return None;
    };
    if let Ok(id) = table.get::<u32>("dismiss") {
        return Some(NotificationAction::Dismiss(id));
    }
    if matches!(table.get::<Value>("dismiss_all"), Ok(Value::Boolean(true))) {
        return Some(NotificationAction::DismissAll);
    }
    let inner = table.get::<Table>("invoke").ok()?;
    Some(NotificationAction::Invoke(
        inner.get("id").ok()?,
        inner.get("key").ok()?,
    ))
}

/// Command constructors: callers return these from handlers or completion callbacks.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let api = lua.create_table()?;
    api.set(
        "dismiss",
        lua.create_function(|lua, id: u32| {
            let command = lua.create_table()?;
            command.set("dismiss", id)?;
            Ok(command)
        })?,
    )?;
    api.set(
        "dismissAll",
        lua.create_function(|lua, ()| {
            let command = lua.create_table()?;
            command.set("dismiss_all", true)?;
            Ok(command)
        })?,
    )?;
    lua.globals().set("notification", api)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actions_are_owned_and_malformed_commands_are_ignored() {
        let lua = Lua::new();
        let actions = actions_table(&lua, &[("open:details".into(), "Details".into())]).unwrap();
        assert_eq!(
            actions
                .get::<Table>(1)
                .unwrap()
                .get::<String>("key")
                .unwrap(),
            "open:details"
        );
        assert_eq!(
            notification_action(&lua.load("return {dismiss=42}").eval().unwrap()),
            Some(NotificationAction::Dismiss(42))
        );
        assert_eq!(
            notification_action(
                &lua.load("return {invoke={id=42,key='open:details'}}")
                    .eval()
                    .unwrap()
            ),
            Some(NotificationAction::Invoke(42, "open:details".into()))
        );
        for source in [
            "nil",
            "42",
            "{}",
            "{invoke={id='oops'}}",
            "{dismiss=-1}",
            "{dismiss_all='yes'}",
            "{dismiss_all=0}",
            "{dismiss_all=false}",
        ] {
            assert!(
                notification_action(&lua.load(format!("return {source}")).eval().unwrap())
                    .is_none()
            );
        }
    }
}
