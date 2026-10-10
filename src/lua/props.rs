//! Property publication and table merging shared by Lua entry adapters.
use crate::config::PropValue;
use mlua::{Lua, Table, Value};
use std::collections::HashMap;
pub(crate) fn prop_value_from_lua(value: &Value) -> Option<PropValue> {
    match value {
        Value::Boolean(value) => Some(PropValue::Bool(*value)),
        Value::Integer(value) => Some(PropValue::Number(*value as f64)),
        Value::Number(value) => Some(PropValue::Number(*value)),
        Value::String(value) => Some(PropValue::Text(value.to_string_lossy())),
        _ => None,
    }
}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::widgets::{call_lua_value, load_widget_script, new_widget_lua};

    #[test]
    fn publish_props_lands_table_on_app() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "props", "return {view=function() return 'x' end}").unwrap();
        let props = HashMap::from([
            ("format".into(), PropValue::Text("%Y".into())),
            ("n".into(), PropValue::Number(2.0)),
            ("b".into(), PropValue::Bool(true)),
        ]);
        publish_props(&lua, &props).unwrap();
        let app: Table = lua.named_registry_value("riced.widget.app").unwrap();
        let published: Table = app.get("props").unwrap();
        let format: String = published.get("format").unwrap();
        let number: f64 = published.get("n").unwrap();
        let boolean: bool = published.get("b").unwrap();
        assert_eq!((format.as_str(), number, boolean), ("%Y", 2.0, true));
    }

    #[test]
    fn resolved_props_flow_into_view() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "fmt", "return {view=function(self) return self.props.format end, defaults={props={format='%H:%M'}}}").unwrap();
        publish_props(
            &lua,
            &HashMap::from([("format".into(), PropValue::Text("%Y".into()))]),
        )
        .unwrap();
        assert_eq!(
            call_lua_value(&lua, "view").unwrap(),
            Value::String(lua.create_string("%Y").unwrap())
        );
    }
}
