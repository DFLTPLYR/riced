//! Constructor/setter bindings. All hosts install exactly this UI namespace.
use crate::lua::value::lua_value_kind;
use mlua::{Function, Lua, Table, Value};

fn setter(lua: &Lua, field: &str, expected: &'static str) -> mlua::Result<Function> {
    let key = field.to_owned();
    lua.create_function(move |lua, (node, value): (Table, Value)| {
        let kind: String = node.get("type")?;
        if kind != expected {
            return Err(mlua::Error::RuntimeError(format!(
                "iced.{kind} has no :{key}() setter"
            )));
        }
        let properties: Table = node.raw_get("_properties")?;
        // Lua cannot store `nil`, but `:on_open()` / `:on_close()` with no
        // argument must still declare the phase (using the popup's root
        // `:animate` transition) rather than erasing it.
        let value = if matches!(key.as_str(), "on_open" | "on_close") && value.is_nil() {
            Value::Table(lua.create_table()?)
        } else {
            value
        };
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
pub(super) fn node(
    lua: &Lua,
    kind: &str,
    mt: Table,
    build: impl FnOnce(&Table) -> mlua::Result<()>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("type", kind)?;
    table.raw_set("_defaults", super::settings::table(lua)?)?;
    table.raw_set(
        "_animation_owner",
        lua.named_registry_value::<Function>("riced.animation_owner")?,
    )?;
    build(&table)?;
    let properties = lua.create_table()?;
    let component = super::components::COMPONENTS
        .iter()
        .find(|component| component.name == kind)
        .ok_or_else(|| mlua::Error::RuntimeError(format!("unknown iced component {kind}")))?;
    for &field in component.setters.iter().chain(super::layout_props::SETTERS) {
        properties.set(field, table.raw_get::<Value>(field)?)?;
        table.raw_set(field, Value::Nil)?;
    }
    table.raw_set("_properties", properties)?;
    if matches!(kind, "listview" | "popup") {
        table.raw_set(
            "_capture_callback",
            lua.named_registry_value::<Function>("riced.capture_callback")?,
        )?;
    }
    if kind == "popup" {
        table.raw_set(
            "_capture_lifecycle",
            lua.named_registry_value::<Function>("riced.capture_lifecycle")?,
        )?;
    }
    table.set_metatable(Some(mt))?;
    Ok(table)
}
pub(crate) fn inject_ui_base(lua: &Lua) -> mlua::Result<()> {
    crate::lua::notifications::install(lua)?;
    crate::lua::tray::install(lua)?;
    crate::lua::shell::install(lua)?;
    crate::lua::callbacks::install(lua)?;
    let iced = lua.create_table()?;
    super::animations::install(lua, &iced)?;
    crate::framework::install(lua, &iced)?;
    let mts = lua.create_table()?;
    let schemas = lua.create_table()?;
    for component in super::components::COMPONENTS {
        let kind = component.name;
        let mut methods = Vec::new();
        for &field in component.setters.iter().chain(super::layout_props::SETTERS) {
            methods.push((field, setter(lua, field, kind)?));
        }
        if kind == "listview" {
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
        mts.set(kind, mt.clone())?;
        let schema = lua.create_table()?;
        schema.set("rust_type", component.rust_type)?;
        schema.set(
            "arguments",
            lua.create_sequence_from(component.arguments.iter().copied())?,
        )?;
        schema.set(
            "setters",
            lua.create_sequence_from(methods.iter().map(|(name, _)| *name))?,
        )?;
        schemas.set(kind, schema)?;
        iced.set(
            kind,
            lua.create_function(move |lua, args: mlua::MultiValue| {
                if kind == "shader" && args.len() != 1 {
                    return Err(mlua::Error::RuntimeError(
                        "iced.shader needs exactly one WGSL path".into(),
                    ));
                }
                if matches!(
                    kind,
                    "row" | "column" | "stack" | "masonry" | "listview" | "pick_list"
                ) && !matches!(args.front(), Some(Value::Table(_)))
                {
                    return Err(mlua::Error::RuntimeError(format!(
                        "iced.{kind} needs an array table"
                    )));
                }
                node(lua, kind, mt.clone(), |t| {
                    for (index, &field) in component.arguments.iter().enumerate() {
                        t.set(field, args.get(index).cloned().unwrap_or(Value::Nil))?;
                    }
                    if kind == "listview" {
                        t.set("transitions", lua.create_table()?)?;
                    }
                    Ok(())
                })
            })?,
        )?;
    }
    lua.set_named_registry_value("riced.iced.metatables", mts)?;
    iced.set("components", schemas)?;
    iced.set(
        "to_table",
        lua.create_function(|lua, value: Value| {
            let node = super::decode::parse_node(&value).map_err(mlua::Error::RuntimeError)?;
            super::encode::to_table(lua, &node)
        })?,
    )?;
    lua.globals()
        .set("_riced_components", lua.create_table()?)?;
    iced.set(
        "define",
        lua.create_function(|lua, (name, definition): (String, Value)| {
            let namespace: Table = lua.globals().get("iced")?;
            if namespace.raw_get::<Value>(name.as_str())? != Value::Nil {
                return Err(mlua::Error::RuntimeError(format!(
                    "component {name:?} conflicts with an iced constructor"
                )));
            }
            let definition = match definition {
                Value::Function(function) => Value::Function(function),
                Value::Table(table) => super::animations::Easing::descriptor(&table)
                    .map_err(mlua::Error::RuntimeError)?
                    .encode(lua)?,
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "iced.define expects a component function or easing descriptor".into(),
                    ));
                }
            };
            let registry: Table = lua.globals().get("_riced_components")?;
            if registry.get::<Value>(name.clone())? != Value::Nil {
                eprintln!("iced: component {name:?} redefined (last wins)");
            }
            registry.set(name, definition)?;
            Ok(())
        })?,
    )?;
    iced.set(
        "use",
        lua.create_function(|lua, (name, props): (String, Value)| {
            let registry: Table = lua.globals().get("_riced_components")?;
            let func = match registry.get::<Value>(name.clone())? {
                Value::Function(f) => f,
                Value::Table(table) => {
                    return super::animations::Easing::descriptor(&table)
                        .map_err(mlua::Error::RuntimeError)?
                        .encode(lua);
                }
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
    iced.set_metatable(Some(methods))?;

    // Strict `size` table: the canonical size strategies as values, with an
    // __index that raises on unknown keys — `size.fil` errors at the call
    // site instead of silently becoming an omitted field. Values are the
    // exact strings the decode tables accept.
    let size = lua.create_table()?;
    for &(name, _) in super::decode::LENGTH_NAMES {
        size.raw_set(name, name)?;
    }
    let size_names: Vec<_> = super::decode::LENGTH_NAMES
        .iter()
        .map(|(name, _)| format!("size.{name}"))
        .collect();
    let size_options: Vec<_> = size_names.iter().map(|name| (name.as_str(), ())).collect();
    let size_expected = crate::shared::enums::expected(&size_options);
    let size_mt = lua.create_table()?;
    size_mt.set(
        "__index",
        lua.create_function(move |_, (_table, key): (mlua::Table, mlua::LuaString)| {
            Err::<(), _>(mlua::Error::RuntimeError(format!(
                "unknown size {} — expected {size_expected}",
                key.to_string_lossy()
            )))
        })?,
    )?;
    size.set_metatable(Some(size_mt))?;
    lua.globals().set("size", size)?;

    lua.globals().set("iced", iced)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_action_setter_overrides_constructor_and_roundtrips() {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        let decode = |source: &str| {
            super::super::decode::parse_node(&lua.load(source).eval().unwrap()).unwrap()
        };
        let expected = decode("return iced.button('Dismiss', 'dismiss:42'):width(40)");
        for source in [
            "return iced.button('Dismiss'):action('dismiss:42'):width(40)",
            "return iced.button('Dismiss', 'old'):width(40):action('dismiss:42')",
            "return iced.button('Dismiss'):action('old'):width(40):action('dismiss:42')",
        ] {
            let node = decode(source);
            assert_eq!(node, expected);
            let encoded = super::super::encode::to_table(&lua, &node).unwrap();
            assert_eq!(
                super::super::decode::parse_node(&Value::Table(encoded)).unwrap(),
                expected
            );
        }
        assert!(
            matches!(decode("return iced.button('No key')"), super::super::node::WidgetNode::Button { action, .. } if action.is_empty())
        );
        assert!(
            lua.load("return iced.text('label'):action('dismiss:42')")
                .eval::<Value>()
                .is_err()
        );
    }
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
            wrapping: None,
            color: None,
            elide: None,
        }
    }

    #[test]
    fn ui_with_chains_like_iced_builders() {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        let parse = |source: &str| parse_node(&lua.load(source).eval::<Value>().unwrap()).unwrap();
        assert_eq!(
            parse("return iced.progress(0.5):width(200)"),
            WidgetNode::Progress {
                value: 0.5,
                width: NodeLength::Fixed(200.0),
                height: None,
                color: None,
                background: None
            }
        );
        assert_eq!(
            parse("return iced.text('hi'):size(14)"),
            WidgetNode::Text {
                content: "hi".into(),
                size: Some(14.0),
                width: None,
                height: None,
                wrapping: None,
                color: None,
                elide: None
            }
        );
        assert_eq!(
            parse("return iced.button('go','run'):width(120):padding(4)"),
            WidgetNode::Button {
                label: "go".into(),
                action: "run".into(),
                width: Some(NodeLength::Fixed(120.0)),
                height: None,
                padding: Some(4.0.into()),
                color: None,
                background: None,
                radius: None
            }
        );
        assert_eq!(
            parse("return iced.row({iced.text('x')}):spacing(8)"),
            WidgetNode::Row {
                children: vec![text("x")],
                width: NodeLength::Native,
                height: NodeLength::Native,
                spacing: 8.0
            }
        );
        assert!(
            !lua.load(
                "local ok=pcall(function() return iced.progress(0.5):padding(4) end); return ok"
            )
            .eval::<bool>()
            .unwrap()
        );
        let value: Value = lua
            .load("return iced.progress(0.5):width('wide')")
            .eval()
            .unwrap();
        assert!(parse_node(&value).is_err());
        assert_eq!(
            parse("return iced.progress(0.5):width(200):height(12)"),
            WidgetNode::Progress {
                value: 0.5,
                width: NodeLength::Fixed(200.0),
                height: Some(NodeLength::Fixed(12.0)),
                color: None,
                background: None
            }
        );
        assert!(matches!(
            parse("return iced.button('go','run'):height(36)"),
            WidgetNode::Button {
                height: Some(_),
                ..
            }
        ));
        assert!(matches!(
            parse("return iced.text('hi'):height(20)"),
            WidgetNode::Text {
                height: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn ui_constructors_build_description_tables() {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        let node: Table = lua
            .load("return iced.row({iced.icon('cpu'),iced.text('42%')},8)")
            .eval()
            .unwrap();
        assert_eq!(node.get::<String>("type").unwrap(), "row");
        assert_eq!(
            crate::ui::decode::node_property(&node, "spacing").unwrap(),
            Value::Number(8.0)
        );
        let children: Vec<Table> = node
            .get::<Table>("children")
            .unwrap()
            .sequence_values()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(children.len(), 2);
        let value: Value = lua
            .load("return iced.column({iced.progress(0.5)})")
            .eval()
            .unwrap();
        assert_eq!(
            parse_node(&value).unwrap(),
            WidgetNode::Column {
                children: vec![WidgetNode::Progress {
                    value: 0.5,
                    width: NodeLength::Fixed(120.0),
                    height: None,
                    color: None,
                    background: None
                }],
                width: NodeLength::Native,
                height: NodeLength::Native,
                spacing: 4.0
            }
        );
        let value: Value = lua.load("return iced.progress(0.5,200)").eval().unwrap();
        assert_eq!(
            parse_node(&value).unwrap(),
            WidgetNode::Progress {
                value: 0.5,
                width: NodeLength::Fixed(200.0),
                height: None,
                color: None,
                background: None
            }
        );
        let value: Value = lua.load("return iced.progress(0.5,'wide')").eval().unwrap();
        assert!(parse_node(&value).is_err());
    }

    #[test]
    fn separator_parses_defaults_and_chains() {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        assert!(lua.load("return ui == nil").eval::<bool>().unwrap());
        for (source, height) in [
            ("return iced.separator()", 1.0),
            ("return iced.separator():height(3)", 3.0),
            ("return iced.separator()", 1.0),
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            assert_eq!(
                parse_node(&value).unwrap(),
                WidgetNode::Separator {
                    height,
                    color: None
                },
                "{source}"
            );
        }
    }

    #[test]
    fn ui_length_strings_map_to_fill_and_shrink() {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        let parse = |source: &str| parse_node(&lua.load(source).eval::<Value>().unwrap()).unwrap();
        assert_eq!(
            parse("return iced.progress(0.5):width('fill')"),
            WidgetNode::Progress {
                value: 0.5,
                width: NodeLength::Fill,
                height: None,
                color: None,
                background: None
            }
        );
        assert_eq!(
            parse("return iced.row({iced.text('x')}):width('FILL'):height('shrink')"),
            WidgetNode::Row {
                children: vec![text("x")],
                width: NodeLength::Fill,
                height: NodeLength::Shrink,
                spacing: 4.0
            }
        );
        assert!(matches!(
            parse("return iced.text('hi'):width('fill')"),
            WidgetNode::Text {
                width: Some(NodeLength::Fill),
                ..
            }
        ));
        let value: Value = lua
            .load("return iced.progress(0.5):width('huge')")
            .eval()
            .unwrap();
        assert!(parse_node(&value).is_err());
        assert_eq!(
            parse("return iced.row({iced.text('x')})"),
            WidgetNode::Row {
                children: vec![text("x")],
                width: NodeLength::Native,
                height: NodeLength::Native,
                spacing: 4.0
            }
        );
        let fill = WidgetNode::Progress {
            value: 0.5,
            width: NodeLength::Fill,
            height: Some(NodeLength::Fill),
            color: None,
            background: None,
        };
        let _ = crate::ui::build::build_node(&fill, 13.0, None).unwrap();
    }

    #[test]
    fn strict_size_table_decodes_identically_and_raises_on_unknown_keys() {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        let parse = |source: &str| parse_node(&lua.load(source).eval::<Value>().unwrap());
        // size.* decodes exactly like the equivalent string form.
        for (table, string, expected) in [
            ("size.fill", "'fill'", NodeLength::Fill),
            ("size.shrink", "'shrink'", NodeLength::Shrink),
            ("size.auto", "'auto'", NodeLength::Shrink),
        ] {
            let via_table = parse(&format!("return iced.text('x'):width({table})")).unwrap();
            let via_string = parse(&format!("return iced.text('x'):width({string})")).unwrap();
            assert_eq!(via_table, via_string, "{table} vs {string}");
            assert!(
                matches!(via_table, WidgetNode::Text { width: Some(w), .. } if w == expected),
                "{table}"
            );
        }
        // Unknown keys raise at the Lua call site, naming the options —
        // script errors already surface through the notification path.
        let error = lua
            .load("return size.fil")
            .eval::<Value>()
            .expect_err("unknown size key must raise");
        let message = error.to_string();
        assert!(message.contains("size.fil"), "{message}");
        assert!(
            message.contains("\"size.fill\", \"size.shrink\", or \"size.auto\""),
            "{message}"
        );
        // The table is global and shadows nothing shipped (scripts use
        // `size` only as a table field or local).
        assert!(
            lua.load("return size.fill == 'fill'")
                .eval::<bool>()
                .unwrap()
        );
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
    fn spin_box_decodes_chains_builds_and_roundtrips() {
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        let parse = |source: &str| parse_node(&lua.load(source).eval::<Value>().unwrap());
        let expected = WidgetNode::SpinBox {
            value: 8.0,
            min: 0.0,
            max: 256.0,
            step: 1.0,
            decimals: 0,
            on_change: Some("margin".into()),
            width: None,
            height: None,
        };
        assert_eq!(
            parse("return iced.spin_box(8, 0, 256):on_change('margin')").unwrap(),
            expected
        );
        // Later setters win, defaults fill the rest, and the value clamps.
        let node = parse(
            "return iced.spin_box(8, 0, 256):step(2):step(4):decimals(1):width(120):value(999)",
        )
        .unwrap();
        assert!(matches!(
            &node,
            WidgetNode::SpinBox {
                value,
                step: 4.0,
                decimals: 1,
                width: Some(NodeLength::Fixed(120.0)),
                ..
            } if (*value - 256.0).abs() < f32::EPSILON
        ));
        // Realizes in the native host and round-trips through encode.
        crate::ui::build::build_node(&node, 13.0, None).unwrap();
        let encoded = super::super::encode::to_table(&lua, &node).unwrap();
        assert_eq!(parse_node(&Value::Table(encoded)).unwrap(), node);
        // Invalid ranges, decimals, and heights are rejected at decode.
        for source in [
            "iced.spin_box(0, 10, 0)",
            "iced.spin_box(0, 5, 5)",
            "iced.spin_box(5, 0, 10):step(0)",
            "iced.spin_box(5, 0, 10):decimals(9)",
            "iced.spin_box(5, 0, 10):decimals(1.5)",
            "iced.spin_box(5, 0, 10):height(-1)",
            "iced.spin_box(5, 0, 10):min(20)",
        ] {
            let value: Value = lua.load(format!("return {source}")).eval().unwrap();
            assert!(parse_node(&value).is_err(), "{source}");
        }
        assert!(
            lua.load("return iced.spin_box(5, 0, 10):spacing(4)")
                .eval::<Value>()
                .is_err(),
            "spacing is not a spin_box setter"
        );
    }

    #[test]
    fn representative_builders_produce_expected_owned_descriptions() {
        use iced::widget::text::Wrapping;
        assert_eq!(
            description("return iced.text('x'):size(14):size(16):width('auto')"),
            WidgetNode::Text {
                content: "x".into(),
                size: Some(16.0),
                width: Some(NodeLength::Shrink),
                height: None,
                wrapping: None,
                color: None,
                elide: None,
            }
        );
        for (source, wrapping) in [
            ("return iced.text('x'):wrap('none')", Wrapping::None),
            ("return iced.text('x'):wrap('word')", Wrapping::Word),
            ("return iced.text('x'):wrap('glyph')", Wrapping::Glyph),
            (
                "return iced.text('x'):wrap('word-or-glyph')",
                Wrapping::WordOrGlyph,
            ),
            (
                "return iced.text('x'):wrap('WORD_OR_GLYPH'):wrap('word')",
                Wrapping::Word,
            ),
        ] {
            assert_eq!(
                description(source),
                WidgetNode::Text {
                    content: "x".into(),
                    size: None,
                    width: None,
                    height: None,
                    wrapping: Some(wrapping),
                    color: None,
                    elide: None,
                },
                "{source}"
            );
        }
        let lua = Lua::new();
        inject_ui_base(&lua).unwrap();
        assert!(
            lua.load("return iced.text('x'):wrap('wide')")
                .eval::<Value>()
                .is_ok_and(|value| parse_node(&value).is_err()),
            "unknown wrap modes are rejected at decode"
        );
        assert!(
            lua.load("return iced.row({iced.text('x')}):wrap('word')")
                .eval::<Value>()
                .is_err(),
            "wrap is a text-only setter"
        );
        assert_eq!(
            description("return iced.row({iced.icon('cpu'), iced.text('x')}):spacing(5)"),
            WidgetNode::Row {
                children: vec![
                    WidgetNode::Icon {
                        name: "cpu".into(),
                        color: None
                    },
                    text("x")
                ],
                spacing: 5.0,
                width: NodeLength::Native,
                height: NodeLength::Native,
            }
        );
        assert_eq!(
            description(
                "return iced.container(iced.column({iced.button('go','go'):radius(3),iced.progress(0.4,100)})):padding(4)"
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
                            radius: Some(3.0.into())
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
                    width: NodeLength::Native,
                    height: NodeLength::Native,
                }),
                width: NodeLength::Native,
                height: NodeLength::Native,
                padding: 4.0.into(),
                background: None,
                radius: 0.0.into(),
                border: None,
                border_width: 0.0,
            }
        );
        let defaults: ListView<String, WidgetNode> = ListView::default();
        assert_eq!(
            description(
                "return iced.listview({{id=1}}):id('test'):key('id'):delegate(function(item) return iced.text(item.id) end)"
            ),
            WidgetNode::ListView {
                id: "test".into(),
                callbacks: Vec::new(),
                items: vec![("1".into(), text("1"))],
                horizontal: false,
                pitch: 32.0,
                spacing: 4.0,
                width: NodeLength::Native,
                height: NodeLength::Native,
                transitions: Box::new((
                    defaults.enter_spec(),
                    defaults.exit_spec(),
                    defaults.displaced_spec()
                )),
            }
        );
        assert_eq!(
            description(
                "iced.define('custom', function(p) return iced.separator():height(p.h) end); return iced.custom({h=2})"
            ),
            WidgetNode::Separator {
                height: 2.0,
                color: None
            }
        );
    }
}
