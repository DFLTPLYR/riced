//! Lua description decoding. Values are fully owned before realization.
use super::node::{NodeLength, WidgetNode};
use crate::lua::transitions::parse_transition_value;
use crate::lua::value::{coerce_text, lua_value_kind};
use mlua::{Table, Value};

pub(crate) fn node_property(t: &Table, field: &str) -> mlua::Result<Value> {
    if let Ok(properties) = t.raw_get::<Table>("_properties") {
        return properties.raw_get(field);
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
pub(crate) fn opt_length(t: &Table, field: &str, what: &str) -> Result<Option<NodeLength>, String> {
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil | Value::Function(_) => Ok(None),
        Value::Integer(i) => Ok(Some(NodeLength::Fixed(i as f32))),
        Value::Number(n) => Ok(Some(NodeLength::Fixed(n as f32))),
        Value::String(s) => match s.to_string_lossy().to_lowercase().as_str() {
            "fill" => Ok(Some(NodeLength::Fill)),
            "shrink" | "auto" => Ok(Some(NodeLength::Shrink)),
            other => Err(format!(
                "{what} {field} must be a number, \"fill\", \"shrink\", or \"auto\", got {other:?}"
            )),
        },
        other => Err(format!(
            "{what} {field} must be a number, \"fill\", \"shrink\", or \"auto\", got {}",
            lua_value_kind(&other)
        )),
    }
}

pub(crate) fn parse_node(value: &Value) -> Result<WidgetNode, String> {
    let Value::Table(t) = value else {
        return Ok(WidgetNode::Text {
            content: coerce_text(value.clone(), "ui node")?,
            size: None,
            width: None,
            height: None,
            color: None,
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
                horizontal: matches!(node_property(t,"axis").map_err(|e|e.to_string())?,Value::String(s) if s.to_string_lossy()=="horizontal"),
                pitch: opt_number(t, "pitch", "iced.listview()")?
                    .unwrap_or(32.0)
                    .max(1.0),
                spacing: opt_number(t, "spacing", "iced.listview()")?
                    .unwrap_or(4.0)
                    .max(0.0),
                width: opt_length(t, "width", "iced.listview()")?.unwrap_or(NodeLength::Shrink),
                height: opt_length(t, "height", "iced.listview()")?.unwrap_or(NodeLength::Shrink),
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
                width: opt_length(t, "width", "iced.container()")?.unwrap_or(NodeLength::Shrink),
                height: opt_length(t, "height", "iced.container()")?.unwrap_or(NodeLength::Shrink),
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
            width: opt_length(t, "width", "iced.scrollable()")?.unwrap_or(NodeLength::Fill),
            height: opt_length(t, "height", "iced.scrollable()")?.unwrap_or(NodeLength::Fill),
        }),
        "space" => Ok(WidgetNode::Space {
            width: opt_length(t, "width", "iced.space()")?.unwrap_or(NodeLength::Shrink),
            height: opt_length(t, "height", "iced.space()")?.unwrap_or(NodeLength::Shrink),
        }),
        "image" => Ok(WidgetNode::Image {
            path: t.get("path").map_err(|e| e.to_string())?,
            width: opt_length(t, "width", "iced.image()")?.unwrap_or(NodeLength::Shrink),
            height: opt_length(t, "height", "iced.image()")?.unwrap_or(NodeLength::Shrink),
        }),
        "text" => Ok(WidgetNode::Text {
            content: coerce_text(
                t.get::<Value>("text").map_err(|e| e.to_string())?,
                "ui.text()",
            )?,
            size: opt_number(t, "size", "ui.text()")?,
            width: opt_length(t, "width", "ui.text()")?,
            height: opt_length(t, "height", "ui.text()")?,
            color: opt_color(t, "color", "ui.text()")?,
        }),
        "icon" => match t.get::<Value>("name").map_err(|e| e.to_string())? {
            Value::String(s) => Ok(WidgetNode::Icon {
                name: s.to_string_lossy(),
                color: opt_color(t, "color", "ui.icon()")?,
            }),
            Value::Nil => Err("ui.icon() needs a name".into()),
            other => Err(format!(
                "ui.icon() name must be a string, got {}",
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
                        "ui.{}() children must be an array, got {}",
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
                        "ui.{}() spacing must be a number, got {}",
                        kind,
                        lua_value_kind(&other)
                    ));
                }
            };
            let what = if kind == "row" {
                "ui.row()"
            } else {
                "ui.column()"
            };
            let width = opt_length(t, "width", what)?.unwrap_or(NodeLength::Shrink);
            let height = opt_length(t, "height", what)?.unwrap_or(NodeLength::Shrink);
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
                "ui.button() label",
            )?,
            action: coerce_text(
                t.get::<Value>("action").map_err(|e| e.to_string())?,
                "ui.button() action",
            )?,
            width: opt_length(t, "width", "ui.button()")?,
            height: opt_length(t, "height", "ui.button()")?,
            padding: opt_number(t, "padding", "ui.button()")?,
            color: opt_color(t, "color", "ui.button()")?,
            background: opt_color(t, "background", "ui.button()")?,
            radius: opt_number(t, "radius", "ui.button()")?.map(|r| r.max(0.0)),
        }),
        "progress" => {
            let value = match t.get::<Value>("value").map_err(|e| e.to_string())? {
                Value::Nil => 0.0,
                Value::Integer(i) => i as f32,
                Value::Number(n) => n as f32,
                other => {
                    return Err(format!(
                        "ui.progress() value must be a number, got {}",
                        lua_value_kind(&other)
                    ));
                }
            };
            Ok(WidgetNode::Progress {
                value,
                width: opt_length(t, "width", "ui.progress()")?.unwrap_or(NodeLength::Fixed(120.0)),
                height: opt_length(t, "height", "ui.progress()")?,
                color: opt_color(t, "color", "ui.progress()")?,
                background: opt_color(t, "background", "ui.progress()")?,
            })
        }
        "spinner" => Ok(WidgetNode::Spinner),
        "separator" => Ok(WidgetNode::Separator {
            height: opt_number(t, "height", "ui.separator()")?
                .unwrap_or(1.0)
                .max(1.0),
            color: opt_color(t, "color", "ui.separator()")?,
        }),
        other => Err(format!("unknown ui node type {other:?}")),
    }
}
