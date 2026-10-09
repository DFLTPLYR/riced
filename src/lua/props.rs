//! Property publication and table merging shared by Lua entry adapters.
use crate::config::PropValue;
use mlua::{Lua, Table, Value};
use std::collections::HashMap;
pub(crate) fn prop_value_to_lua(lua: &Lua, value: &PropValue) -> mlua::Result<Value> {
    match value {
        PropValue::Bool(b) => Ok(Value::Boolean(*b)),
        PropValue::Number(n) => Ok(Value::Number(*n)),
        PropValue::Text(s) => Ok(Value::String(lua.create_string(s)?)),
    }
}
pub(crate) fn publish_props(lua: &Lua, props: &HashMap<String, PropValue>) -> mlua::Result<()> {
    let app: Table = lua.named_registry_value("riced.widget.app")?;
    let table = lua.create_table()?;
    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort();
    for key in keys {
        table.set(key.clone(), prop_value_to_lua(lua, &props[key])?)?;
    }
    app.set("props", table)
}
pub(crate) fn component_props(
    lua: &Lua,
    app: &Table,
    overrides: Value,
    host: Value,
) -> mlua::Result<Table> {
    let props = lua.create_table()?;
    for value in [app.get::<Value>("defaults")?, overrides, host] {
        match value {
            Value::Nil => {}
            Value::Table(table) => {
                for pair in table.pairs::<String, Value>() {
                    let (key, value) = pair?;
                    props.set(key, value)?;
                }
            }
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "composable props and defaults must be tables".into(),
                ));
            }
        }
    }
    Ok(props)
}
