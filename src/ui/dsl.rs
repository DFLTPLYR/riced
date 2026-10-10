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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{
        decode::parse_node,
        listview::ListView,
        node::{NodeLength, WidgetNode},
    };

    fn description(source: &str) -> WidgetNode {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        let value: Value = lua.load(source).eval().unwrap();
        parse_node(&value).unwrap()
    }

    fn text(content: &str) -> WidgetNode {
        WidgetNode::Text {
            content: content.into(),
            size: None,
            width: None,
            height: None,
            color: None,
        }
    }

    #[test]
    fn lua_setters_are_repeatable_and_components_are_directly_callable() {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        lua.load(r#"iced.define('label', function(p) return iced.text(p.text) end)"#)
            .exec()
            .unwrap();
        let value: Value = lua.load(r##"return iced.label({text='hello'}):width(10):width(30):color('#f00'):color('#0f0')"##).eval().unwrap();
        assert!(
            matches!(parse_node(&value).unwrap(), WidgetNode::Text { width: Some(NodeLength::Fixed(30.0)), color: Some(color), .. } if color.g == 1.0 && color.r == 0.0)
        );
        let value: Value = lua
            .load("return iced.row({}, 4):spacing(8):spacing(12)")
            .eval()
            .unwrap();
        assert!(matches!(
            parse_node(&value).unwrap(),
            WidgetNode::Row { spacing: 12.0, .. }
        ));
        assert!(
            lua.load("iced.define('text', function() return iced.text('x') end)")
                .exec()
                .is_err()
        );
    }

    #[test]
    fn iced_define_and_use_round_trip_with_chaining() {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        lua.load(r#"iced.define('__t_stat', function(props) return iced.row({iced.icon(props.icon), iced.text(props.value)}) end)"#).exec().unwrap();
        let value: Value = lua
            .load(r#"return iced.use('__t_stat', {icon='cpu',value='42%'}):width('fill')"#)
            .eval()
            .unwrap();
        match parse_node(&value).unwrap() {
            WidgetNode::Row {
                width: NodeLength::Fill,
                children,
                ..
            } => assert_eq!(children.len(), 2),
            other => panic!("unexpected {other:?}"),
        }
        let error = lua
            .load("return iced.use('__t_missing', {})")
            .eval::<Value>()
            .unwrap_err();
        assert!(error.to_string().contains("__t_missing"), "{error}");
        lua.load("iced.define('__t_bad', function() return 42 end)")
            .exec()
            .unwrap();
        let error = lua
            .load("return iced.use('__t_bad', {})")
            .eval::<Value>()
            .unwrap_err();
        assert!(error.to_string().contains("__t_bad"), "{error}");
    }

    #[test]
    fn representative_builders_produce_expected_owned_descriptions() {
        assert_eq!(
            description("return ui.text('x'):size(14):size(16):width('auto')"),
            WidgetNode::Text {
                content: "x".into(),
                size: Some(16.0),
                width: Some(NodeLength::Shrink),
                height: None,
                color: None,
            }
        );
        assert_eq!(
            description("return ui.row({ui.icon('cpu'), ui.text('x')}):spacing(5)"),
            WidgetNode::Row {
                children: vec![
                    WidgetNode::Icon {
                        name: "cpu".into(),
                        color: None
                    },
                    text("x")
                ],
                spacing: 5.0,
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
            }
        );
        assert_eq!(
            description(
                "return ui.container(ui.column({ui.button('go','go'):radius(3),ui.progress(0.4,100)})):padding(4)"
            ),
            WidgetNode::Container {
                child: Box::new(WidgetNode::Column {
                    children: vec![
                        WidgetNode::Button {
                            label: "go".into(),
                            action: "go".into(),
                            width: None,
                            height: None,
                            padding: None,
                            color: None,
                            background: None,
                            radius: Some(3.0)
                        },
                        WidgetNode::Progress {
                            value: 0.4,
                            width: NodeLength::Fixed(100.0),
                            height: None,
                            color: None,
                            background: None
                        },
                    ],
                    spacing: 4.0,
                    width: NodeLength::Shrink,
                    height: NodeLength::Shrink,
                }),
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                padding: 4.0,
                background: None,
                radius: 0.0,
                border: None,
                border_width: 0.0,
            }
        );
        let defaults: ListView<String, WidgetNode> = ListView::default();
        assert_eq!(
            description(
                "return ui.listview({{id=1}}):id('test'):key('id'):delegate(function(item) return ui.text(item.id) end)"
            ),
            WidgetNode::ListView {
                id: "test".into(),
                items: vec![("1".into(), text("1"))],
                horizontal: false,
                pitch: 32.0,
                spacing: 4.0,
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                transitions: Box::new((
                    defaults.enter_spec(),
                    defaults.exit_spec(),
                    defaults.displaced_spec()
                )),
            }
        );
        assert_eq!(
            description(
                "ui.define('custom', function(p) return ui.separator():height(p.h) end); return ui.custom({h=2})"
            ),
            WidgetNode::Separator {
                height: 2.0,
                color: None
            }
        );
    }
}
