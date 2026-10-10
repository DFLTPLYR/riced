//! Widget author defaults and property schema decoding; never invokes view.
use super::{props::prop_value_from_lua, value::lua_value_kind};
use crate::config::{PropSchema, WidgetDefaults};
use mlua::{Lua, Table, Value};
use std::collections::HashMap;

pub(crate) fn load_widget_meta(
    lua: &Lua,
    label: &str,
) -> (WidgetDefaults, HashMap<String, PropSchema>) {
    let app: Table = match lua.named_registry_value("riced.widget.app") {
        Ok(app) => app,
        Err(e) => {
            eprintln!("riced: widget {label:?}: cannot read app table: {e}");
            return (WidgetDefaults::default(), HashMap::new());
        }
    };
    let mut defaults = WidgetDefaults::default();
    let as_number = |value: Value| match value {
        Value::Number(n) => Some(n),
        Value::Integer(n) => Some(n as f64),
        _ => None,
    };
    if let Ok(Value::Table(table)) = app.get::<Value>("defaults") {
        match table.get::<Value>("interval") {
            Ok(Value::Nil) => {}
            Ok(value) => match as_number(value) {
                Some(interval) => defaults.interval = interval as f32,
                None => eprintln!("riced: widget {label:?}: defaults.interval must be a number"),
            },
            Err(e) => eprintln!("riced: widget {label:?}: bad defaults.interval: {e}"),
        }
        match table.get::<Value>("size") {
            Ok(Value::Nil) => {}
            Ok(value) => match as_number(value) {
                Some(size) => defaults.size = size as f32,
                None => eprintln!("riced: widget {label:?}: defaults.size must be a number"),
            },
            Err(e) => eprintln!("riced: widget {label:?}: bad defaults.size: {e}"),
        }
        if let Ok(Value::Table(props)) = table.get::<Value>("props") {
            for pair in props.pairs::<Value, Value>() {
                match pair {
                    Ok((Value::String(key), value)) => match prop_value_from_lua(&value) {
                        Some(prop) => {
                            defaults.props.insert(key.to_string_lossy(), prop);
                        }
                        None => eprintln!(
                            "riced: widget {label:?}: defaults.props.{} must be boolean/number/string",
                            key.to_string_lossy()
                        ),
                    },
                    Ok((key, _)) => eprintln!(
                        "riced: widget {label:?}: skipping non-string prop key ({})",
                        lua_value_kind(&key)
                    ),
                    Err(e) => eprintln!("riced: widget {label:?}: bad defaults.props: {e}"),
                }
            }
        } else if !matches!(table.get::<Value>("props"), Ok(Value::Nil)) {
            eprintln!("riced: widget {label:?}: defaults.props must be a table");
        }
    } else if !matches!(app.get::<Value>("defaults"), Ok(Value::Nil)) {
        eprintln!("riced: widget {label:?}: defaults must be a table");
    }
    let mut schema = HashMap::new();
    if let Ok(Value::Table(table)) = app.get::<Value>("property_schema") {
        for pair in table.pairs::<Value, Value>() {
            let (key, spec) = match pair {
                Ok((Value::String(key), Value::Table(spec))) => (key.to_string_lossy(), spec),
                Ok((key, _)) => {
                    let name = match &key {
                        Value::String(s) => s.to_string_lossy(),
                        _ => lua_value_kind(&key).to_string(),
                    };
                    eprintln!("riced: widget {label:?}: property_schema.{name} must be a table");
                    continue;
                }
                Err(e) => {
                    eprintln!("riced: widget {label:?}: bad property_schema: {e}");
                    continue;
                }
            };
            let get_str = |field: &str| match spec.get::<Value>(field) {
                Ok(Value::String(s)) => Some(s.to_string_lossy()),
                Ok(Value::Nil) => None,
                Ok(other) => {
                    eprintln!(
                        "riced: widget {label:?}: property_schema.{key}.{field} must be a string, got {}",
                        lua_value_kind(&other)
                    );
                    None
                }
                Err(_) => None,
            };
            let prop_type = match spec.get::<Value>("type") {
                Ok(Value::String(t)) => {
                    let t = t.to_string_lossy();
                    if ["boolean", "number", "string"].contains(&t.as_str()) {
                        Some(t)
                    } else {
                        eprintln!(
                            "riced: widget {label:?}: property_schema.{key}.type must be boolean/number/string"
                        );
                        None
                    }
                }
                Ok(Value::Nil) => None,
                Ok(other) => {
                    eprintln!(
                        "riced: widget {label:?}: property_schema.{key}.type must be a string, got {}",
                        lua_value_kind(&other)
                    );
                    None
                }
                Err(_) => None,
            };
            let number = |field: &str| match spec.get::<Value>(field) {
                Ok(Value::Number(n)) => Some(n),
                Ok(Value::Integer(n)) => Some(n as f64),
                Ok(Value::Nil) => None,
                Ok(other) => {
                    eprintln!(
                        "riced: widget {label:?}: property_schema.{key}.{field} must be a number, got {}",
                        lua_value_kind(&other)
                    );
                    None
                }
                Err(_) => None,
            };
            let choices = match spec.get::<Value>("choices") {
                Ok(Value::Table(list)) => {
                    let mut out = Vec::new();
                    for item in list.sequence_values::<Value>() {
                        match item {
                            Ok(Value::String(s)) => out.push(s.to_string_lossy()),
                            Ok(other) => eprintln!(
                                "riced: widget {label:?}: property_schema.{key}.choices must be strings, got {}",
                                lua_value_kind(&other)
                            ),
                            Err(e) => eprintln!(
                                "riced: widget {label:?}: bad property_schema.{key}.choices: {e}"
                            ),
                        }
                    }
                    out
                }
                Ok(Value::Nil) => Vec::new(),
                Ok(other) => {
                    eprintln!(
                        "riced: widget {label:?}: property_schema.{key}.choices must be a list, got {}",
                        lua_value_kind(&other)
                    );
                    Vec::new()
                }
                Err(_) => Vec::new(),
            };
            let label = get_str("label");
            let description = get_str("description");
            let min = number("min");
            let max = number("max");
            schema.insert(
                key,
                PropSchema {
                    label,
                    prop_type,
                    description,
                    min,
                    max,
                    choices,
                },
            );
        }
    } else if !matches!(app.get::<Value>("property_schema"), Ok(Value::Nil)) {
        eprintln!("riced: widget {label:?}: property_schema must be a table");
    }
    (defaults, schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::PropValue,
        lua::widgets::{load_widget_script, new_widget_lua},
    };

    #[test]
    fn widget_metadata_reads_defaults_and_schema() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "meta", r#"return {
            view = function() return "x" end,
            defaults = { interval = 5, size = 20, props = { format = "%H", n = 3, b = true, bad = {} } },
            property_schema = {
                format = { label = "Fmt", type = "string", description = "d", choices = { "%H", "%M" } },
                n = { type = "number", min = 1, max = 9 }, bogus = "nope",
            },
        }"#).unwrap();
        let (defaults, schema) = load_widget_meta(&lua, "meta");
        assert_eq!(defaults.interval, 5.0);
        assert_eq!(defaults.size, 20.0);
        assert_eq!(defaults.props["format"], PropValue::Text("%H".into()));
        assert_eq!(defaults.props["n"], PropValue::Number(3.0));
        assert_eq!(defaults.props["b"], PropValue::Bool(true));
        assert!(!defaults.props.contains_key("bad"));
        assert_eq!(schema["format"].label.as_deref(), Some("Fmt"));
        assert_eq!(schema["format"].choices, ["%H", "%M"]);
        assert_eq!(schema["n"].min, Some(1.0));
        assert_eq!(schema["n"].max, Some(9.0));
        assert!(!schema.contains_key("bogus"));
    }

    #[test]
    fn widget_metadata_missing_tables_fall_back() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "plain", "return {view=function() return 'x' end}").unwrap();
        let (defaults, schema) = load_widget_meta(&lua, "plain");
        assert_eq!(defaults.interval, 1.0);
        assert_eq!(defaults.size, 13.0);
        assert!(defaults.props.is_empty());
        assert!(schema.is_empty());
    }
}
