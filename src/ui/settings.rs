//! Persistent, per-component defaults. Explicit Lua properties always win.
use std::collections::BTreeMap;

use mlua::{Lua, LuaSerdeExt, Table, Value};
use serde::{Deserialize, Deserializer, Serialize};

type Properties = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ComponentDefaults(pub BTreeMap<String, Properties>);

impl<'de> Deserialize<'de> for ComponentDefaults {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let defaults = Self(BTreeMap::deserialize(deserializer)?);
        defaults.validate().map_err(serde::de::Error::custom)?;
        Ok(defaults)
    }
}

pub(crate) fn properties(kind: &str) -> Vec<&'static str> {
    let component = super::components::COMPONENTS
        .iter()
        .find(|c| c.name == kind);
    let mut fields = component.map(|c| c.setters.to_vec()).unwrap_or_default();
    fields.extend(super::layout_props::SETTERS);
    // A callback delegate is executable Lua, not a persistent value.
    fields.retain(|field| *field != "delegate");
    fields.sort_unstable();
    fields.dedup();
    fields
}

impl ComponentDefaults {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn set(
        &mut self,
        component: String,
        property: String,
        value: Option<serde_json::Value>,
    ) {
        if let Some(value) = value {
            self.0.entry(component).or_default().insert(property, value);
        } else if let Some(properties) = self.0.get_mut(&component) {
            properties.remove(&property);
            if properties.is_empty() {
                self.0.remove(&component);
            }
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.is_empty() {
            return Ok(());
        }
        let lua = Lua::new();
        super::dsl::inject_ui_base(&lua).map_err(|e| e.to_string())?;
        for (kind, fields) in &self.0 {
            if !super::components::COMPONENTS.iter().any(|c| c.name == kind) {
                return Err(format!("unknown component default {kind:?}"));
            }
            let allowed = properties(kind);
            for (field, value) in fields {
                if !allowed.contains(&field.as_str()) {
                    return Err(format!(
                        "iced.{kind} has no configurable {field:?} property"
                    ));
                }
                validate_value(&lua, kind, field, value)
                    .map_err(|error| format!("component_defaults.{kind}.{field}: {error}"))?;
            }
        }
        Ok(())
    }
}

fn validate_value(
    lua: &Lua,
    kind: &str,
    field: &str,
    value: &serde_json::Value,
) -> Result<(), String> {
    fn finite(value: &serde_json::Value) -> bool {
        use serde_json::Value as Json;
        match value {
            Json::Number(number) => number
                .as_f64()
                .is_some_and(|n| n.is_finite() && n.abs() <= f64::from(f32::MAX)),
            Json::Array(values) => values.iter().all(finite),
            Json::Object(values) => values.values().all(finite),
            _ => true,
        }
    }
    if !finite(value) {
        return Err("numbers must be finite and fit a component value".into());
    }
    if matches!(field, "width" | "height")
        && number(value)
            .is_some_and(|n| n < 0.0 || (matches!(kind, "shader" | "popup") && n == 0.0))
    {
        return Err(
            "dimensions must be non-negative (shader and popup dimensions must be positive)".into(),
        );
    }
    let table = lua.create_table().map_err(|e| e.to_string())?;
    table
        .set(field, lua.to_value(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let what = format!("iced.{kind}()");
    match field {
        "padding" | "margin" => {
            super::geometry::opt_insets(&table, field, &what)?;
        }
        "radius" => {
            super::geometry::opt_corners(&table, field, &what)?;
        }
        "width" => {
            let length = super::decode::opt_length(&table, field, &what)?;
            if kind == "popup" && matches!(length, Some(super::node::NodeLength::FillPortion(_))) {
                return Err("popup surface dimensions cannot use fill:n".into());
            }
        }
        "height" if !matches!(kind, "slider" | "separator") => {
            let length = super::decode::opt_length(&table, field, &what)?;
            if kind == "popup" && matches!(length, Some(super::node::NodeLength::FillPortion(_))) {
                return Err("popup surface dimensions cannot use fill:n".into());
            }
        }
        "color" | "background" | "border" => {
            super::decode::opt_color(&table, field, &what)?;
        }
        "align_x" | "align_y" => {
            super::decode::opt_alignment(&table, field, &what)?;
        }
        "anchor" => {
            super::placement::Placement::parse(value.as_str().ok_or("expected an anchor string")?)?;
        }
        "wrap" => {
            super::decode::opt_wrapping(&table, field, &what)?;
        }
        "scrollbar" | "axis" | "elide_mode" => {
            let choices = match field {
                "scrollbar" => &["hidden", "visible"][..],
                "axis" => &["horizontal", "vertical"][..],
                _ => &["head", "middle", "tail"][..],
            };
            if !value.as_str().is_some_and(|v| {
                choices
                    .iter()
                    .any(|choice| v.trim().eq_ignore_ascii_case(choice))
            }) {
                return Err(format!("expected {}", choices.join(" or ")));
            }
        }
        "checked" | "enabled" | "secure" | "animated" => {
            if !value.is_boolean() {
                return Err("expected a boolean".into());
            }
        }
        "params" => {
            let values = value.as_array().ok_or("expected an array of numbers")?;
            if values.len() > 16 || values.iter().any(|v| number(v).is_none()) {
                return Err("expected at most 16 finite numbers".into());
            }
        }
        "size" | "spacing" | "border_width" | "height" | "pitch" | "step" | "speed" | "elide" => {
            let n = number(value).ok_or("expected a finite number")?;
            if (field != "speed" && n < 0.0)
                || (matches!(field, "step" | "size") && n == 0.0)
                || (kind == "slider" && field == "height" && n == 0.0)
            {
                return Err(
                    "expected a non-negative number (size, step and slider height must be positive)"
                        .into(),
                );
            }
            if field == "elide" && (n.fract() != 0.0 || n < 1.0) {
                return Err("expected a positive character count".into());
            }
        }
        "value" if matches!(kind, "slider" | "progress") => {
            number(value).ok_or("expected a finite number")?;
        }
        "selected" => {
            if !value.is_string() {
                return Err("expected a string selection".into());
            }
        }
        _ => {
            if !value.is_string() {
                return Err("expected a string".into());
            }
        }
    }
    Ok(())
}

fn number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && n.abs() <= f64::from(f32::MAX))
}

const REGISTRY: &str = "riced.component-defaults";

pub(crate) fn table(lua: &Lua) -> mlua::Result<Table> {
    if let Ok(table) = lua.named_registry_value::<Table>(REGISTRY) {
        return Ok(table);
    }
    let table = lua.create_table()?;
    lua.set_named_registry_value(REGISTRY, table.clone())?;
    Ok(table)
}

pub(crate) fn publish(lua: &Lua, defaults: &ComponentDefaults) -> mlua::Result<()> {
    if lua
        .app_data_ref::<ComponentDefaults>()
        .is_some_and(|old| *old == *defaults)
    {
        return Ok(());
    }
    let root = table(lua)?;
    root.clear()?;
    for (kind, fields) in &defaults.0 {
        root.set(kind.as_str(), lua.to_value(fields)?)?;
    }
    lua.set_app_data(defaults.clone());
    Ok(())
}

pub(crate) fn inherited(table: &Table, field: &str) -> mlua::Result<Value> {
    let Ok(defaults) = table.raw_get::<Table>("_defaults") else {
        return Ok(Value::Nil);
    };
    let kind: String = table.raw_get("type")?;
    let Some(properties) = defaults.raw_get::<Option<Table>>(kind)? else {
        return Ok(Value::Nil);
    };
    properties.raw_get(field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{decode::parse_node, layout_props, node::WidgetNode};

    #[test]
    fn cached_lua_nodes_inherit_live_defaults_without_overriding_explicit_zero_or_false() {
        let lua = Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let mut defaults: ComponentDefaults = serde_json::from_str(
            r#"{"button":{"padding":9,"radius":3},"text_input":{"secure":true}}"#,
        )
        .unwrap();
        publish(&lua, &defaults).unwrap();
        let inherited: Value = lua
            .load("return iced.button('go','go'):anchor('center')")
            .eval()
            .unwrap();
        let explicit: Value = lua
            .load("return iced.button('go','go'):padding(0):radius(0)")
            .eval()
            .unwrap();
        let secure: Value = lua
            .load("return iced.text_input('', ''):secure(false)")
            .eval()
            .unwrap();
        let check = |value: &Value, padding: Option<f32>, radius: Option<f32>| {
            let (node, _) = layout_props::unwrap(parse_node(value).unwrap());
            assert!(
                matches!(node, WidgetNode::Button { padding: p, radius: r, .. } if p == padding.map(crate::ui::geometry::Insets::from) && r == radius.map(crate::ui::geometry::Corners::from))
            );
        };
        check(&inherited, Some(9.0), Some(3.0));
        check(&explicit, Some(0.0), Some(0.0));
        assert!(matches!(
            parse_node(&secure).unwrap(),
            WidgetNode::TextInput { secure: false, .. }
        ));
        defaults.set(
            "button".into(),
            "padding".into(),
            Some(serde_json::Value::from(15)),
        );
        publish(&lua, &defaults).unwrap();
        check(&inherited, Some(15.0), Some(3.0));
        check(&explicit, Some(0.0), Some(0.0));
        defaults.set("button".into(), "padding".into(), None);
        publish(&lua, &defaults).unwrap();
        check(&inherited, None, Some(3.0));
        check(&explicit, Some(0.0), Some(0.0));
        // Defaults remain local to a VM; neither user properties nor other apps leak.
        let other = Lua::new();
        crate::ui::dsl::inject_ui_base(&other).unwrap();
        let node =
            parse_node(&other.load("return iced.button('go','go')").eval().unwrap()).unwrap();
        assert!(matches!(
            node,
            WidgetNode::Button {
                padding: None,
                radius: None,
                ..
            }
        ));
    }

    #[test]
    fn persistent_defaults_roundtrip_and_reject_unknown_or_invalid_properties() {
        let source = r##"
return {
    component_defaults = {
        button = { padding = 8, background = "#123456" },
        column = { align_x = "end" },
        shader = { params = { 0.5, 1.0 }, animated = false },
        popup = { padding = 8, width = 150 },
    },
}
"##;
        let config: crate::config::Config = crate::config::test_parse_config(source).unwrap();
        let encoded = crate::config::test_format_config(&config).unwrap();
        let decoded: crate::config::Config = crate::config::test_parse_config(&encoded).unwrap();
        assert_eq!(decoded.component_defaults, config.component_defaults);
        for source in [
            "return { unknown = { padding = 1 } }",
            "return { component_defaults = { button = { paddding = 1 } } }",
            "return { component_defaults = { button = { padding = 'wide' } } }",
            "return { component_defaults = { button = { padding = -1 } } }",
            "return { component_defaults = { button = { width = 0/0 } } }",
            "return { component_defaults = { row = { align_x = 'center' } } }",
            "return { component_defaults = { text = { align_x = 'middle' } } }",
            "return { component_defaults = { text_input = { secure = 'yes' } } }",
            "return { component_defaults = { shader = { params = { true } } } }",
            "return { component_defaults = { listview = { delegate = 'a function' } } }",
            "return { component_defaults = { popup = { width = 'fill:2' } } }",
        ] {
            assert!(
                crate::config::test_parse_defaults(source).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn popup_defaults_are_inherited_and_explicit_zero_padding_wins() {
        let lua = Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let defaults: ComponentDefaults =
            serde_json::from_str(r#"{"popup":{"padding":12,"width":150,"height":40}}"#).unwrap();
        publish(&lua, &defaults).unwrap();
        for (suffix, expected_padding) in [("", 12.0), (":padding(0):width(90)", 0.0)] {
            let value = lua
                .load(format!("return iced.popup(iced.text('body')){suffix}"))
                .eval()
                .unwrap();
            let content =
                crate::shell::screens::Popup::parse_popup_content(value, &Default::default())
                    .unwrap();
            assert_eq!(
                content.width,
                Some(if suffix.is_empty() { 150.0 } else { 90.0 })
            );
            assert!(
                matches!(content.tree, Some(WidgetNode::Popup { padding, .. }) if padding == expected_padding.into())
            );
        }
    }

    #[test]
    fn geometry_defaults_are_atomic_and_sparse_lua_tables_zero_omitted_entries() {
        use crate::ui::geometry::{Corners, Insets};
        let lua = Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let config: crate::config::Config = crate::config::test_parse_config(
            "return { component_defaults = { popup = { padding = { top = 2, right = 3, bottom = 5, left = 7 }, radius = { top_left = 11, bottom_right = 13 } } } }",
        )
        .unwrap();
        let saved = crate::config::test_format_config(&config).unwrap();
        let loaded: crate::config::Config = crate::config::test_parse_config(&saved).unwrap();
        assert_eq!(loaded.component_defaults, config.component_defaults);
        publish(&lua, &loaded.component_defaults).unwrap();
        for (source, padding, radius) in [
            (
                "iced.popup(iced.space())",
                Insets {
                    top: 2.0,
                    right: 3.0,
                    bottom: 5.0,
                    left: 7.0,
                },
                Corners {
                    top_left: 11.0,
                    bottom_right: 13.0,
                    ..Corners::ZERO
                },
            ),
            (
                "iced.popup(iced.space()):padding({left=17}):radius({top_right=19})",
                Insets {
                    left: 17.0,
                    ..Insets::ZERO
                },
                Corners {
                    top_right: 19.0,
                    ..Corners::ZERO
                },
            ),
            (
                "iced.popup(iced.space()):padding(0):radius(0)",
                Insets::ZERO,
                Corners::ZERO,
            ),
        ] {
            let value = lua.load(format!("return {source}")).eval().unwrap();
            let node =
                crate::shell::screens::Popup::parse_popup_content(value, &Default::default())
                    .unwrap()
                    .tree
                    .unwrap();
            assert!(
                matches!(node, WidgetNode::Popup { padding: p, radius: r, .. } if p == padding && r == radius),
                "{source}"
            );
        }
        for source in [
            "return { popup = { padding = { lef = 1 } } }",
            "return { popup = { radius = { top = 1 } } }",
            "return { button = { padding = { 1, 2 } } }",
            "return { text_input = { padding = { left = 'bad' } } }",
        ] {
            assert!(
                crate::config::test_parse_defaults(source).is_err(),
                "{source}"
            );
        }
    }
}
