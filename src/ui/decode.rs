//! Lua description decoding. Values are fully owned before realization.
use super::node::{NodeLength, WidgetNode};
use crate::lua::transitions::parse_transition_value;
use crate::lua::value::{coerce_text, lua_value_kind};
use mlua::{Table, Value};

pub(crate) fn node_property(t: &Table, field: &str) -> mlua::Result<Value> {
    if let Ok(properties) = t.raw_get::<Table>("_properties") {
        let value = properties.raw_get::<Value>(field)?;
        return if value == Value::Nil {
            t.raw_get(field)
        } else {
            Ok(value)
        };
    }
    t.get(field)
}

pub(crate) fn opt_number(t: &Table, field: &str, what: &str) -> Result<Option<f32>, String> {
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => Ok(None),
        Value::Integer(i) => Ok(Some(i as f32)),
        Value::Number(n) => Ok(Some(n as f32)),
        other => Err(format!(
            "{what} {field} must be a number, got {}",
            lua_value_kind(&other)
        )),
    }
}
pub(crate) fn opt_color(t: &Table, field: &str, what: &str) -> Result<Option<iced::Color>, String> {
    fn num(v: &Value) -> Option<f32> {
        match v {
            Value::Integer(i) => Some(*i as f32),
            Value::Number(n) => Some(*n as f32),
            _ => None,
        }
    }
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => Ok(None),
        Value::String(s) => {
            let raw = s.to_string_lossy();
            let expanded = if raw.len() == 4 && raw.starts_with('#') {
                let c: Vec<char> = raw.chars().collect();
                format!("#{}{}{}{}{}{}", c[1], c[1], c[2], c[2], c[3], c[3])
            } else {
                raw.to_string()
            };
            crate::theme::parse_hex(&expanded).ok_or_else(|| format!("{what} {field} must be a hex color like \"#rrggbb\" or an {{r, g, b}} table, got {raw:?}")).map(Some)
        }
        Value::Table(rgb) => {
            let chan = |k: &str, i: i32| {
                rgb.get::<Value>(k)
                    .ok()
                    .as_ref()
                    .and_then(num)
                    .or_else(|| rgb.get::<Value>(i).ok().as_ref().and_then(num))
            };
            match (chan("r", 1), chan("g", 2), chan("b", 3)) {
                (Some(r), Some(g), Some(b)) => Ok(Some(iced::Color::from_rgba(
                    r.clamp(0.0, 1.0),
                    g.clamp(0.0, 1.0),
                    b.clamp(0.0, 1.0),
                    chan("a", 4).unwrap_or(1.0).clamp(0.0, 1.0),
                ))),
                _ => Err(format!(
                    "{what} {field} must be a hex color like \"#rrggbb\" or an {{r, g, b}} table"
                )),
            }
        }
        other => Err(format!(
            "{what} {field} must be a hex color like \"#rrggbb\" or an {{r, g, b}} table, got {}",
            lua_value_kind(&other)
        )),
    }
}
/// Size-strategy names shared by the strict `size` table and error messages.
pub(crate) const LENGTH_NAMES: &[(&str, NodeLength)] = &[
    ("fill", NodeLength::Fill),
    ("shrink", NodeLength::Shrink),
    ("auto", NodeLength::Shrink),
];

pub(crate) fn opt_length(t: &Table, field: &str, what: &str) -> Result<Option<NodeLength>, String> {
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => Ok(None),
        Value::Integer(i) => Ok(Some(NodeLength::Fixed(i as f32))),
        Value::Number(n) => Ok(Some(NodeLength::Fixed(n as f32))),
        Value::String(s) => {
            let raw = s.to_string_lossy();
            crate::shared::enums::lookup(LENGTH_NAMES, &raw)
                .map(Some)
                .ok_or_else(|| {
                    format!(
                        "{what} {field} must be a number, {}, got {raw:?}",
                        crate::shared::enums::expected(LENGTH_NAMES)
                    )
                })
        }
        other => Err(format!(
            "{what} {field} must be a number, {}, got {}",
            crate::shared::enums::expected(LENGTH_NAMES),
            lua_value_kind(&other)
        )),
    }
}

/// Canonical wrap-strategy names used for decoding and error messages.
const WRAPPING_NAMES: &[(&str, iced::widget::text::Wrapping)] = &[
    ("none", iced::widget::text::Wrapping::None),
    ("word", iced::widget::text::Wrapping::Word),
    ("glyph", iced::widget::text::Wrapping::Glyph),
    ("word_or_glyph", iced::widget::text::Wrapping::WordOrGlyph),
];

pub(crate) fn opt_wrapping(
    t: &Table,
    field: &str,
    what: &str,
) -> Result<Option<iced::widget::text::Wrapping>, String> {
    let expected = crate::shared::enums::expected(WRAPPING_NAMES);
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => Ok(None),
        Value::String(s) => {
            let raw = s.to_string_lossy();
            crate::shared::enums::lookup(WRAPPING_NAMES, &raw)
                .map(Some)
                .ok_or_else(|| format!("{what} {field} must be {expected}, got {raw:?}"))
        }
        other => Err(format!(
            "{what} {field} must be {expected}, got {}",
            lua_value_kind(&other)
        )),
    }
}

const AXES: &[(&str, bool)] = &[("horizontal", true), ("vertical", false)];

/// Listview `axis` is case-insensitive and hyphen-tolerant through the shared
/// tables; non-strings and omissions keep the default vertical axis.
fn opt_axis(t: &Table) -> Result<bool, String> {
    match node_property(t, "axis").map_err(|e| e.to_string())? {
        Value::String(s) => {
            let raw = s.to_string_lossy();
            Ok(crate::shared::enums::lookup(AXES, &raw).unwrap_or(false))
        }
        _ => Ok(false),
    }
}

pub(crate) fn parse_node(value: &Value) -> Result<WidgetNode, String> {
    let Value::Table(t) = value else {
        return Ok(WidgetNode::Text {
            content: coerce_text(value.clone(), "ui node")?,
            size: None,
            width: None,
            height: None,
            wrapping: None,
            color: None,
            elide: None,
        });
    };
    let kind = match t.get::<Value>("type").map_err(|e| e.to_string())? {
        Value::String(s) => s.to_string_lossy(),
        Value::Nil => return Err("ui node table needs a type field".into()),
        other => {
            return Err(format!(
                "ui node type must be a string, got {}",
                lua_value_kind(&other)
            ));
        }
    };
    match kind.as_str() {
        "checkbox" | "toggler" | "slider" | "text_input" | "pick_list" | "radio" => {
            parse_control(t, &kind)
        }
        "listview" => {
            let id = match node_property(t, "id").map_err(|e| e.to_string())? {
                Value::String(s) => s.to_string_lossy(),
                _ => return Err("iced.listview needs :id('stable-name')".into()),
            };
            let key_field = match node_property(t, "key").map_err(|e| e.to_string())? {
                Value::String(s) => s.to_string_lossy(),
                _ => return Err("iced.listview needs :key('field')".into()),
            };
            let delegate = match node_property(t, "delegate").map_err(|e| e.to_string())? {
                Value::Function(f) => f,
                _ => return Err("iced.listview needs :delegate(function(item) ... end)".into()),
            };
            let data: Table = t.get("data").map_err(|e| e.to_string())?;
            let mut items = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for item in data.sequence_values::<Table>() {
                let item = item.map_err(|e| e.to_string())?;
                let key = match item
                    .get::<Value>(key_field.as_str())
                    .map_err(|e| e.to_string())?
                {
                    Value::String(s) => s.to_string_lossy(),
                    Value::Integer(n) => n.to_string(),
                    _ => return Err(format!("listview {id:?} key must be a string or integer")),
                };
                if !seen.insert(key.clone()) {
                    return Err(format!("listview {id:?} duplicate key {key:?}"));
                }
                let value: Value = delegate.call(item).map_err(|e| e.to_string())?;
                items.push((key, parse_node(&value)?));
            }
            let defaults: super::listview::ListView<String, WidgetNode> =
                super::listview::ListView::default();
            let (enter, exit, displaced, _) = parse_transition_value(
                t.get("transitions").map_err(|e| e.to_string())?,
                &defaults.enter_spec(),
                &defaults.exit_spec(),
                &defaults.displaced_spec(),
            )?;
            Ok(WidgetNode::ListView {
                id,
                items,
                horizontal: opt_axis(t)?,
                pitch: opt_number(t, "pitch", "iced.listview()")?
                    .unwrap_or(32.0)
                    .max(1.0),
                spacing: opt_number(t, "spacing", "iced.listview()")?
                    .unwrap_or(4.0)
                    .max(0.0),
                width: opt_length(t, "width", "iced.listview()")?.unwrap_or(NodeLength::Native),
                height: opt_length(t, "height", "iced.listview()")?.unwrap_or(NodeLength::Native),
                transitions: Box::new((enter, exit, displaced)),
            })
        }
        "container" => {
            let background = opt_color(t, "background", "iced.container()")?;
            let border = opt_color(t, "border", "iced.container()")?;
            let border_width = opt_number(t, "border_width", "iced.container()")?
                .map(|w| w.max(0.0))
                .unwrap_or(if border.is_some() { 1.0 } else { 0.0 });
            Ok(WidgetNode::Container {
                child: Box::new(parse_node(
                    &t.get::<Value>("child").map_err(|e| e.to_string())?,
                )?),
                width: opt_length(t, "width", "iced.container()")?.unwrap_or(NodeLength::Native),
                height: opt_length(t, "height", "iced.container()")?.unwrap_or(NodeLength::Native),
                padding: opt_number(t, "padding", "iced.container()")?.unwrap_or(0.0),
                radius: opt_number(t, "radius", "iced.container()")?.unwrap_or(0.0),
                background,
                border,
                border_width,
            })
        }
        "scrollable" => Ok(WidgetNode::Scrollable {
            child: Box::new(parse_node(
                &t.get::<Value>("child").map_err(|e| e.to_string())?,
            )?),
            width: opt_length(t, "width", "iced.scrollable()")?.unwrap_or(NodeLength::Native),
            height: opt_length(t, "height", "iced.scrollable()")?.unwrap_or(NodeLength::Native),
        }),
        "space" => Ok(WidgetNode::Space {
            width: opt_length(t, "width", "iced.space()")?.unwrap_or(NodeLength::Native),
            height: opt_length(t, "height", "iced.space()")?.unwrap_or(NodeLength::Native),
        }),
        "image" => Ok(WidgetNode::Image {
            path: t.get("path").map_err(|e| e.to_string())?,
            width: opt_length(t, "width", "iced.image()")?.unwrap_or(NodeLength::Native),
            height: opt_length(t, "height", "iced.image()")?.unwrap_or(NodeLength::Native),
        }),
        "text" => {
            let content = coerce_text(
                t.get::<Value>("text").map_err(|e| e.to_string())?,
                "iced.text()",
            )?;
            let elide = opt_elide(t)?;
            let content = elide.map(|spec| spec.apply(&content)).unwrap_or(content);
            Ok(WidgetNode::Text {
                content,
                size: opt_number(t, "size", "iced.text()")?,
                width: opt_length(t, "width", "iced.text()")?,
                height: opt_length(t, "height", "iced.text()")?,
                wrapping: opt_wrapping(t, "wrap", "iced.text()")?,
                color: opt_color(t, "color", "iced.text()")?,
                elide,
            })
        }
        "icon" => match t.get::<Value>("name").map_err(|e| e.to_string())? {
            Value::String(s) => Ok(WidgetNode::Icon {
                name: s.to_string_lossy(),
                color: opt_color(t, "color", "iced.icon()")?,
            }),
            Value::Nil => Err("iced.icon() needs a name".into()),
            other => Err(format!(
                "iced.icon() name must be a string, got {}",
                lua_value_kind(&other)
            )),
        },
        "row" | "column" => {
            let children = match t.get::<Value>("children").map_err(|e| e.to_string())? {
                Value::Table(list) => list
                    .sequence_values::<Value>()
                    .map(|child| {
                        child
                            .map_err(|e| e.to_string())
                            .and_then(|child| parse_node(&child))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Value::Nil => Vec::new(),
                other => {
                    return Err(format!(
                        "iced.{}() children must be an array, got {}",
                        kind,
                        lua_value_kind(&other)
                    ));
                }
            };
            let spacing = match node_property(t, "spacing").map_err(|e| e.to_string())? {
                Value::Nil | Value::Function(_) => 4.0,
                Value::Integer(i) => i as f32,
                Value::Number(n) => n as f32,
                other => {
                    return Err(format!(
                        "iced.{}() spacing must be a number, got {}",
                        kind,
                        lua_value_kind(&other)
                    ));
                }
            };
            let what = if kind == "row" {
                "iced.row()"
            } else {
                "iced.column()"
            };
            let width = opt_length(t, "width", what)?.unwrap_or(NodeLength::Native);
            let height = opt_length(t, "height", what)?.unwrap_or(NodeLength::Native);
            if kind == "row" {
                Ok(WidgetNode::Row {
                    children,
                    spacing,
                    width,
                    height,
                })
            } else {
                Ok(WidgetNode::Column {
                    children,
                    spacing,
                    width,
                    height,
                })
            }
        }
        "button" => Ok(WidgetNode::Button {
            label: coerce_text(
                t.get::<Value>("label").map_err(|e| e.to_string())?,
                "iced.button() label",
            )?,
            action: coerce_text(
                t.get::<Value>("action").map_err(|e| e.to_string())?,
                "iced.button() action",
            )?,
            width: opt_length(t, "width", "iced.button()")?,
            height: opt_length(t, "height", "iced.button()")?,
            padding: opt_number(t, "padding", "iced.button()")?,
            color: opt_color(t, "color", "iced.button()")?,
            background: opt_color(t, "background", "iced.button()")?,
            radius: opt_number(t, "radius", "iced.button()")?.map(|r| r.max(0.0)),
        }),
        "progress" => {
            let value = match t.get::<Value>("value").map_err(|e| e.to_string())? {
                Value::Nil => 0.0,
                Value::Integer(i) => i as f32,
                Value::Number(n) => n as f32,
                other => {
                    return Err(format!(
                        "iced.progress() value must be a number, got {}",
                        lua_value_kind(&other)
                    ));
                }
            };
            Ok(WidgetNode::Progress {
                value,
                width: opt_length(t, "width", "iced.progress()")?
                    .unwrap_or(NodeLength::Fixed(120.0)),
                height: opt_length(t, "height", "iced.progress()")?,
                color: opt_color(t, "color", "iced.progress()")?,
                background: opt_color(t, "background", "iced.progress()")?,
            })
        }
        "spinner" => Ok(WidgetNode::Spinner),
        "separator" => Ok(WidgetNode::Separator {
            height: opt_number(t, "height", "iced.separator()")?
                .unwrap_or(1.0)
                .max(1.0),
            color: opt_color(t, "color", "iced.separator()")?,
        }),
        other => Err(format!("unknown ui node type {other:?}")),
    }
}

fn control_string(t: &Table, field: &str, what: &str) -> Result<Option<String>, String> {
    match node_property(t, field).map_err(|error| error.to_string())? {
        Value::Nil => Ok(None),
        Value::String(value) => Ok(Some(value.to_string_lossy())),
        value => Err(format!(
            "{what} {field} must be a string, got {}",
            lua_value_kind(&value)
        )),
    }
}
fn control_bool(t: &Table, field: &str, what: &str) -> Result<bool, String> {
    match node_property(t, field).map_err(|error| error.to_string())? {
        Value::Nil => Ok(false),
        Value::Boolean(value) => Ok(value),
        value => Err(format!(
            "{what} {field} must be a boolean, got {}",
            lua_value_kind(&value)
        )),
    }
}
fn control_number(t: &Table, field: &str, what: &str) -> Result<Option<f32>, String> {
    let number = opt_number(t, field, what)?;
    if number.is_some_and(|number| !number.is_finite()) {
        return Err(format!("{what} {field} must be finite"));
    }
    Ok(number)
}

/// `:elide(n)` keeps at most `n` characters including the "…" mark;
/// `:elide_mode(...)` picks `"tail"` (default), `"head"`, or `"middle"`.
fn opt_elide(t: &Table) -> Result<Option<super::components::TextElide>, String> {
    const WHAT: &str = "iced.text()";
    let max_chars = match node_property(t, "elide").map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => None,
        Value::Integer(n) if n >= 1 => Some(n as usize),
        Value::Number(n) if n.is_finite() && n >= 1.0 && n.fract() == 0.0 => Some(n as usize),
        other => {
            return Err(format!(
                "{WHAT} elide must be a positive character count, got {}",
                lua_value_kind(&other)
            ));
        }
    };
    let mode = match node_property(t, "elide_mode").map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => super::components::ElideMode::Tail,
        Value::String(s) => {
            let raw = s.to_string_lossy();
            crate::shared::enums::lookup(super::components::ELIDE_MODES, &raw).ok_or_else(|| {
                format!(
                    "{WHAT} elide_mode must be {}, got {raw:?}",
                    crate::shared::enums::expected(super::components::ELIDE_MODES)
                )
            })?
        }
        other => {
            return Err(format!(
                "{WHAT} elide_mode must be {}, got {}",
                crate::shared::enums::expected(super::components::ELIDE_MODES),
                lua_value_kind(&other)
            ));
        }
    };
    Ok(max_chars.map(|max_chars| super::components::TextElide { max_chars, mode }))
}
fn parse_control(t: &Table, kind: &str) -> Result<WidgetNode, String> {
    use super::components::*;
    let what = format!("iced.{kind}()");
    let width = opt_length(t, "width", &what)?;
    if matches!(width, Some(NodeLength::Fixed(value)) if !value.is_finite()) {
        return Err(format!("{what} width must be finite"));
    }
    let size = control_number(t, "size", &what)?;
    if size.is_some_and(|value| value <= 0.0) {
        return Err(format!("{what} size must be positive"));
    }
    let spacing = control_number(t, "spacing", &what)?.unwrap_or(8.0).max(0.0);
    let label = || {
        coerce_text(
            t.get::<Value>("label").map_err(|error| error.to_string())?,
            &what,
        )
    };
    let string = |key| control_string(t, key, &what);
    match kind {
        "checkbox" => Ok(CheckboxNode {
            label: label()?,
            checked: control_bool(t, "checked", &what)?,
            on_toggle: string("on_toggle")?,
            width,
            size,
            spacing,
        }
        .into()),
        "toggler" => Ok(TogglerNode {
            label: label()?,
            enabled: control_bool(t, "enabled", &what)?,
            on_toggle: string("on_toggle")?,
            width,
            size,
            spacing,
        }
        .into()),
        "slider" => {
            let min = control_number(t, "min", &what)?
                .ok_or_else(|| format!("{what} needs a minimum"))?;
            let max = control_number(t, "max", &what)?
                .ok_or_else(|| format!("{what} needs a maximum"))?;
            let step = control_number(t, "step", &what)?.unwrap_or(1.0);
            if min >= max || !(max - min).is_finite() || step <= 0.0 {
                return Err(format!(
                    "{what} needs min < max and a positive step with finite range"
                ));
            }
            let value = control_number(t, "value", &what)?
                .unwrap_or(min)
                .clamp(min, max);
            let height = control_number(t, "height", &what)?;
            if height.is_some_and(|value| value <= 0.0) {
                return Err(format!("{what} height must be positive"));
            }
            Ok(SliderNode {
                min,
                max,
                value,
                step,
                on_change: string("on_change")?,
                on_release: string("on_release")?,
                width,
                height,
            }
            .into())
        }
        "text_input" => Ok(TextInputNode {
            placeholder: string("placeholder")?.unwrap_or_default(),
            value: string("value")?.unwrap_or_default(),
            on_input: string("on_input")?,
            on_submit: string("on_submit")?,
            width,
            size,
            padding: control_number(t, "padding", &what)?.map(|value| value.max(0.0)),
            secure: control_bool(t, "secure", &what)?,
        }
        .into()),
        "pick_list" => {
            let options: Table = t.get("options").map_err(|error| error.to_string())?;
            let options = options
                .sequence_values::<Value>()
                .map(|value| match value.map_err(|error| error.to_string())? {
                    Value::String(value) => Ok(value.to_string_lossy()),
                    _ => Err(format!("{what} options must contain strings")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let selected = string("selected")?;
            if selected
                .as_ref()
                .is_some_and(|value| !options.contains(value))
            {
                return Err(format!("{what} selected value must be in options"));
            }
            let mut seen = std::collections::HashSet::new();
            if options.iter().any(|value| !seen.insert(value)) {
                return Err(format!("{what} options must be unique"));
            }
            Ok(PickListNode {
                options,
                selected,
                on_select: string("on_select")?,
                width,
                placeholder: string("placeholder")?,
                padding: control_number(t, "padding", &what)?.map(|value| value.max(0.0)),
                size,
            }
            .into())
        }
        "radio" => Ok(RadioNode {
            label: label()?,
            value: string("value")?.ok_or_else(|| format!("{what} needs a string value"))?,
            selected: string("selected")?,
            on_select: string("on_select")?,
            width,
            size,
            spacing,
        }
        .into()),
        _ => Err(format!("unknown iced control {kind}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listview_delegate_accepts_mixed_nodes_and_components() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        lua.load(
            "iced.define('wsbtn',function(p) return iced.button(p.label,p.action):width(120) end)",
        )
        .exec()
        .unwrap();
        let value: Value = lua.load("return iced.listview({{key='a',label='x'},{key='b',label='y'}}):id('s'):key('key'):delegate(function(item) if item.key=='a' then return iced.text(item.label) else return iced.wsbtn({label=item.label,action='go'}) end end)").eval().unwrap();
        match parse_node(&value).unwrap() {
            WidgetNode::ListView { items, .. } => {
                assert_eq!(items.len(), 2);
                assert!(matches!(items[0].1, WidgetNode::Text { .. }));
                assert!(
                    matches!(items[1].1, WidgetNode::Button {ref label, ref action, ..} if label == "y" && action == "go")
                );
            }
            other => panic!("expected listview, got {other:?}"),
        }
    }

    #[test]
    fn parse_node_reads_full_trees_and_rejects_junk() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let value: Value = lua.load(r#"return iced.row({iced.text('hi'),iced.button('go','run'),7,{type='icon',name='cpu'}})"#).eval().unwrap();
        let text = |content: &str| WidgetNode::Text {
            content: content.into(),
            size: None,
            width: None,
            height: None,
            wrapping: None,
            color: None,
            elide: None,
        };
        assert_eq!(
            parse_node(&value).unwrap(),
            WidgetNode::Row {
                children: vec![
                    text("hi"),
                    WidgetNode::Button {
                        label: "go".into(),
                        action: "run".into(),
                        width: None,
                        height: None,
                        padding: None,
                        color: None,
                        background: None,
                        radius: None
                    },
                    text("7"),
                    WidgetNode::Icon {
                        name: "cpu".into(),
                        color: None
                    }
                ],
                width: NodeLength::Native,
                height: NodeLength::Native,
                spacing: 4.0,
            }
        );
        for bad in ["return {}", "return {type='nope'}"] {
            let value: Value = lua.load(bad).eval().unwrap();
            assert!(parse_node(&value).is_err(), "rejects {bad}");
        }
        assert!(lua.load("return iced.row('flat')").eval::<Value>().is_err());
    }

    #[test]
    fn text_elide_truncates_by_characters_with_head_middle_and_tail_modes() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let content = |source: &str| match parse_node(&lua.load(source).eval().unwrap()).unwrap() {
            WidgetNode::Text { content, .. } => content,
            other => panic!("{source}: unexpected {other:?}"),
        };
        assert_eq!(
            content("return iced.text('hello world'):elide(6)"),
            "hello…"
        );
        assert_eq!(
            content("return iced.text('hello world'):elide(6):elide_mode('head')"),
            "…world"
        );
        assert_eq!(
            content("return iced.text('hello world'):elide(7):elide_mode('middle')"),
            "hel…rld"
        );
        // Exact fits and short text pass through untouched.
        assert_eq!(content("return iced.text('hi'):elide(2)"), "hi");
        assert_eq!(content("return iced.text('hi')"), "hi");
        assert_eq!(content("return iced.text('hello'):elide(1)"), "…");
        // Multibyte characters count as one each.
        assert_eq!(
            content("return iced.text('héllo wörld'):elide(6)"),
            "héllo…"
        );
        // The budget and mode survive a table export round-trip.
        let value: Value = lua
            .load("return iced.text('hello world'):elide(6):elide_mode('head')")
            .eval()
            .unwrap();
        let node = parse_node(&value).unwrap();
        let exported = crate::ui::encode::to_table(&lua, &node).unwrap();
        assert_eq!(parse_node(&Value::Table(exported)).unwrap(), node);
        // Invalid budgets and modes fail before realization.
        for source in [
            "return iced.text('hi'):elide(0)",
            "return iced.text('hi'):elide(-3)",
            "return iced.text('hi'):elide(2.5)",
            "return iced.text('hi'):elide('many')",
            "return iced.text('hi'):elide_mode('both')",
            "return iced.text('hi'):elide_mode(42)",
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            assert!(parse_node(&value).is_err(), "{source}");
        }
        assert!(
            lua.load("return iced.button('go','go'):elide(4)")
                .eval::<Value>()
                .is_err(),
            "elide is a text-only setter"
        );
    }

    #[test]
    fn listview_axis_is_case_insensitive_and_unknown_values_default_to_vertical() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let horizontal = |axis: &str| {
            let value: Value = lua
                .load(format!(
                    "return iced.listview({{}}):id('a'):key('id'):delegate(function() return iced.text('x') end):axis('{axis}')"
                ))
                .eval()
                .unwrap();
            match parse_node(&value).unwrap() {
                WidgetNode::ListView { horizontal, .. } => horizontal,
                other => panic!("{axis}: unexpected {other:?}"),
            }
        };
        assert!(horizontal("horizontal"));
        assert!(horizontal("Horizontal"));
        assert!(horizontal("HORIZONTAL"));
        assert!(!horizontal("vertical"));
        assert!(!horizontal("Vertical"));
        // Omission keeps the default vertical axis.
        let value: Value = lua
            .load("return iced.listview({}):id('a'):key('id'):delegate(function() return iced.text('x') end)")
            .eval()
            .unwrap();
        assert!(matches!(
            parse_node(&value).unwrap(),
            WidgetNode::ListView {
                horizontal: false,
                ..
            }
        ));
        assert!(!horizontal("diagonal"));
        for axis in ["false", "42", "{}", "function() end"] {
            let value: Value = lua
                .load(format!("return iced.listview({{}}):id('a'):key('id'):delegate(function() return iced.text('x') end):axis({axis})"))
                .eval()
                .unwrap();
            assert!(
                matches!(
                    parse_node(&value).unwrap(),
                    WidgetNode::ListView {
                        horizontal: false,
                        ..
                    }
                ),
                "{axis}"
            );
        }
    }

    #[test]
    fn every_primitive_defaults_to_native_auto_sizing() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let native = |source: &str| {
            let value: Value = lua.load(source).eval().unwrap();
            parse_node(&value).unwrap()
        };
        for source in [
            "return iced.space()",
            "return iced.image('memory-only.png')",
            "return iced.container(iced.text('x'))",
            "return iced.scrollable(iced.text('x'))",
            "return iced.row({iced.text('x')})",
            "return iced.column({iced.text('x')})",
        ] {
            let node = native(source);
            let (width, height) = match &node {
                WidgetNode::Space { width, height }
                | WidgetNode::Image { width, height, .. }
                | WidgetNode::Container { width, height, .. }
                | WidgetNode::Scrollable { width, height, .. }
                | WidgetNode::Row { width, height, .. }
                | WidgetNode::Column { width, height, .. } => (width, height),
                other => panic!("{source}: unexpected {other:?}"),
            };
            assert_eq!(*width, NodeLength::Native, "{source} width");
            assert_eq!(*height, NodeLength::Native, "{source} height");
            // Native resolves through the live widget's own strategy, which
            // layout tests already prove matches hand-built iced widgets.
            assert_eq!(width.clone().iced(), iced::Length::Shrink);
        }
        assert!(matches!(
            native("return iced.text('x')"),
            WidgetNode::Text {
                width: None,
                height: None,
                ..
            }
        ));
        assert!(matches!(
            native("return iced.button('go','go')"),
            WidgetNode::Button {
                width: None,
                height: None,
                ..
            }
        ));
    }
}
