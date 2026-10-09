//! Constructor/setter bindings. All hosts install exactly this UI namespace.
use crate::lua::value::lua_value_kind;
use mlua::{Function, Lua, Table, Value};

fn setter(lua: &Lua, field: &str, types: &'static [&'static str]) -> mlua::Result<Function> {
    let key = field.to_owned();
    lua.create_function(move |_, (node, value): (Table, Value)| {
        let kind: String = node.get("type")?;
        if !types.contains(&kind.as_str()) {
            return Err(mlua::Error::RuntimeError(format!(
                "ui {kind} has no :{key}() setter"
            )));
        }
        let properties: Table = node.raw_get("_properties")?;
        properties.set(key.clone(), value)?;
        Ok(node)
    })
}
fn mt_for(lua: &Lua, methods: &[(&str, Function)]) -> mlua::Result<Table> {
    let mt = lua.create_table()?;
    let index = lua.create_table()?;
    for (name, f) in methods {
        index.set(*name, f.clone())?;
    }
    mt.set("__index", index)?;
    Ok(mt)
}
fn node(
    lua: &Lua,
    kind: &str,
    mt: Table,
    build: impl FnOnce(&Table) -> mlua::Result<()>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("type", kind)?;
    build(&table)?;
    let properties = lua.create_table()?;
    for field in [
        "size",
        "width",
        "height",
        "spacing",
        "padding",
        "color",
        "background",
        "border",
        "border_width",
        "radius",
        "id",
        "key",
        "delegate",
        "axis",
        "pitch",
    ] {
        properties.set(field, table.raw_get::<Value>(field)?)?;
        table.raw_set(field, Value::Nil)?;
    }
    table.raw_set("_properties", properties)?;
    table.set_metatable(Some(mt))?;
    Ok(table)
}
pub(crate) fn inject_ui_base(lua: &Lua) -> mlua::Result<()> {
    let ui = lua.create_table()?;
    let shapes: [(&str, &[&str], &'static [&'static str]); 8] = [
        ("text", &["size", "width", "height", "color"], &["text"]),
        ("icon", &["color"], &["icon"]),
        (
            "rowcol",
            &["spacing", "width", "height"],
            &["row", "column"],
        ),
        (
            "button",
            &[
                "width",
                "height",
                "padding",
                "radius",
                "background",
                "color",
            ],
            &["button"],
        ),
        (
            "progress",
            &["width", "height", "color", "background"],
            &["progress"],
        ),
        ("bare", &[], &["spinner"]),
        ("separator", &["height", "color"], &["separator"]),
        (
            "listview",
            &[
                "id", "key", "delegate", "axis", "pitch", "spacing", "width", "height",
            ],
            &["listview"],
        ),
    ];
    let mts = lua.create_table()?;
    for (shape, fields, types) in shapes {
        let mut methods = Vec::new();
        for field in fields {
            let allowed = match (shape, *field) {
                ("listview", _) => &["listview"][..],
                ("separator", "height") => &["separator"][..],
                (_, "width" | "height") => &["text", "row", "column", "button", "progress"][..],
                (_, "color") => &["text", "icon", "button", "progress", "separator"][..],
                (_, "background") => &["container", "button", "progress"][..],
                ("button", "radius") => &["button", "container"][..],
                _ => types,
            };
            methods.push((*field, setter(lua, field, allowed)?));
        }
        if shape == "listview" {
            for (method, slot) in [
                ("onEntered", "add"),
                ("onExit", "remove"),
                ("onDisplaced", "displaced"),
            ] {
                methods.push((
                    method,
                    lua.create_function(move |_, (node, spec): (Table, Table)| {
                        let transitions: Table = node.get("transitions")?;
                        transitions.set(slot, spec)?;
                        Ok(node)
                    })?,
                ));
            }
        }
        let mt = mt_for(lua, &methods)?;
        mts.set(shape, mt.clone())?;
        if shape != "listview" {
            lua.globals().set(format!("_riced_ui_mt_{shape}"), mt)?;
        }
    }
    for kind in ["container", "scrollable", "space", "image"] {
        let mut methods = vec![
            (
                "width",
                setter(lua, "width", &["container", "scrollable", "space", "image"])?,
            ),
            (
                "height",
                setter(
                    lua,
                    "height",
                    &["container", "scrollable", "space", "image"],
                )?,
            ),
        ];
        if kind == "container" {
            for field in ["padding", "background", "radius", "border", "border_width"] {
                let allowed = if field == "background" {
                    &["container", "button", "progress"][..]
                } else {
                    &["container"][..]
                };
                methods.push((field, setter(lua, field, allowed)?));
            }
        }
        let mt = mt_for(lua, &methods)?;
        ui.set(
            kind,
            lua.create_function(move |lua, value: Value| {
                node(lua, kind, mt.clone(), |t| match kind {
                    "container" | "scrollable" => t.set("child", value),
                    "image" => t.set("path", value),
                    _ => Ok(()),
                })
            })?,
        )?;
    }
    for (kind, field) in [("text", "text"), ("icon", "name")] {
        let mt: Table = mts.get(kind)?;
        ui.set(
            kind,
            lua.create_function(move |lua, value: Value| {
                node(lua, kind, mt.clone(), |t| t.set(field, value))
            })?,
        )?;
    }
    for kind in ["row", "column"] {
        let mt: Table = mts.get("rowcol")?;
        ui.set(
            kind,
            lua.create_function(move |lua, (children, spacing): (Table, Value)| {
                node(lua, kind, mt.clone(), |t| {
                    t.set("children", children)?;
                    t.set("spacing", spacing)
                })
            })?,
        )?;
    }
    let mt: Table = mts.get("button")?;
    ui.set(
        "button",
        lua.create_function(move |lua, (label, action): (Value, Value)| {
            node(lua, "button", mt.clone(), |t| {
                t.set("label", label)?;
                t.set("action", action)
            })
        })?,
    )?;
    let mt: Table = mts.get("progress")?;
    ui.set(
        "progress",
        lua.create_function(move |lua, (value, width): (Value, Value)| {
            node(lua, "progress", mt.clone(), |t| {
                t.set("value", value)?;
                t.set("width", width)
            })
        })?,
    )?;
    for (kind, shape) in [("spinner", "bare"), ("separator", "separator")] {
        let mt: Table = mts.get(shape)?;
        ui.set(
            kind,
            lua.create_function(move |lua, _: Value| node(lua, kind, mt.clone(), |_| Ok(())))?,
        )?;
    }
    let mt: Table = mts.get("listview")?;
    ui.set(
        "listview",
        lua.create_function(move |lua, data: Table| {
            node(lua, "listview", mt.clone(), |t| {
                t.set("data", data)?;
                t.set("transitions", lua.create_table()?)
            })
        })?,
    )?;
    lua.globals()
        .set("_riced_components", lua.create_table()?)?;
    ui.set(
        "define",
        lua.create_function(|lua, (name, func): (String, Function)| {
            let namespace: Table = lua.globals().get("iced")?;
            if namespace.raw_get::<Value>(name.as_str())? != Value::Nil {
                return Err(mlua::Error::RuntimeError(format!(
                    "component {name:?} conflicts with an iced constructor"
                )));
            }
            let registry: Table = lua.globals().get("_riced_components")?;
            if registry.get::<Value>(name.clone())? != Value::Nil {
                eprintln!("iced: component {name:?} redefined (last wins)");
            }
            registry.set(name, func)?;
            Ok(())
        })?,
    )?;
    ui.set(
        "use",
        lua.create_function(|lua, (name, props): (String, Value)| {
            let registry: Table = lua.globals().get("_riced_components")?;
            let func = match registry.get::<Value>(name.clone())? {
                Value::Function(f) => f,
                _ => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "unknown component {name:?} — iced.define it first"
                    )));
                }
            };
            let out: Value = func.call(props)?;
            match &out {
                Value::Table(t) => match t.get::<Value>("type")? {
                    Value::String(_) => Ok(out),
                    other => Err(mlua::Error::RuntimeError(format!(
                        "component {name:?} must return a ui node table, got type {}",
                        lua_value_kind(&other)
                    ))),
                },
                _ => Err(mlua::Error::RuntimeError(format!(
                    "component {name:?} must return a ui node table, got {}",
                    lua_value_kind(&out)
                ))),
            }
        })?,
    )?;
    let methods = lua.create_table()?;
    methods.set(
        "__index",
        lua.create_function(|lua, (_table, name): (Table, String)| {
            let registry: Table = lua.globals().get("_riced_components")?;
            if registry.get::<Value>(name.as_str())? == Value::Nil {
                return Ok(Value::Nil);
            }
            Ok(Value::Function(lua.create_function(
                move |lua, props: Value| {
                    let namespace: Table = lua.globals().get("iced")?;
                    let use_component: Function = namespace.raw_get("use")?;
                    use_component.call::<Value>((name.clone(), props))
                },
            )?))
        })?,
    )?;
    ui.set_metatable(Some(methods))?;
    lua.globals().set("iced", ui.clone())?;
    lua.globals().set("ui", ui)?;
    Ok(())
}
