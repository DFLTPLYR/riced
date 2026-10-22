//! Lua description decoding. Values are fully owned before realization.
use super::geometry::{Corners, opt_corners, opt_insets};
use super::node::{NodeLength, WidgetNode};
use crate::lua::transitions::parse_transition_value;
use crate::lua::value::{coerce_text, lua_value_kind};
use mlua::{Table, Value};

// Keep recursion below the stack budget of ordinary Rust worker threads,
// including debug builds where decoder frames are substantially larger.
const MAX_DEPTH: usize = 24;
const MAX_NODES: usize = 16_384;
const MAX_LIST_ITEMS: usize = 4_096;

#[derive(Default)]
struct DecodeBudget {
    nodes: usize,
    ancestors: Vec<*const std::ffi::c_void>,
    depth: usize,
}

pub(crate) fn node_property(t: &Table, field: &str) -> mlua::Result<Value> {
    if let Ok(properties) = t.raw_get::<Table>("_properties") {
        let value = properties.raw_get::<Value>(field)?;
        if value != Value::Nil {
            return Ok(value);
        }
    }
    match t.raw_get::<Value>(field)? {
        // Raw reads bypass setter methods in the node's metatable. An explicit
        // value, including an invalid function, must not inherit a default.
        Value::Nil => super::settings::inherited(t, field),
        value => Ok(value),
    }
}

pub(crate) fn opt_number(t: &Table, field: &str, what: &str) -> Result<Option<f32>, String> {
    let number = match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil => return Ok(None),
        Value::Integer(i) => i as f32,
        Value::Number(n) => n as f32,
        other => {
            return Err(format!(
                "{what} {field} must be a number, got {}",
                lua_value_kind(&other)
            ));
        }
    };
    if !number.is_finite() {
        return Err(format!("{what} {field} must be finite"));
    }
    if matches!(field, "size" | "pitch") && number <= 0.0 {
        return Err(format!("{what} {field} must be positive"));
    }
    if matches!(field, "spacing" | "border_width" | "height") && number < 0.0 {
        return Err(format!("{what} {field} must be non-negative"));
    }
    Ok(Some(number))
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
        Value::Nil => Ok(None),
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
            let chan = |k: &str, i: i32| -> Result<Option<f32>, String> {
                let named = rgb.raw_get::<Value>(k).map_err(|error| error.to_string())?;
                let value = if named == Value::Nil {
                    rgb.raw_get::<Value>(i).map_err(|error| error.to_string())?
                } else {
                    named
                };
                if value == Value::Nil {
                    return Ok(None);
                }
                num(&value)
                    .filter(|channel| channel.is_finite())
                    .map(Some)
                    .ok_or_else(|| format!("{what} {field}.{k} must be a finite number"))
            };
            let alpha = chan("a", 4)?.unwrap_or(1.0);
            match (chan("r", 1)?, chan("g", 2)?, chan("b", 3)?) {
                (Some(r), Some(g), Some(b)) => Ok(Some(iced::Color::from_rgba(
                    r.clamp(0.0, 1.0),
                    g.clamp(0.0, 1.0),
                    b.clamp(0.0, 1.0),
                    alpha.clamp(0.0, 1.0),
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

const SCROLLBAR_NAMES: &[(&str, bool)] = &[("visible", false), ("hidden", true)];
fn scrollbar_hidden(t: &Table) -> Result<bool, String> {
    let value = node_property(t, "scrollbar").map_err(|error| error.to_string())?;
    if value == Value::Nil {
        return Ok(false);
    }
    if let Value::String(name) = &value
        && let Some(hidden) = crate::shared::enums::lookup(SCROLLBAR_NAMES, &name.to_string_lossy())
    {
        return Ok(hidden);
    }
    Err(format!(
        "iced.scrollable() scrollbar must be {}, got {}",
        crate::shared::enums::expected(SCROLLBAR_NAMES),
        match &value {
            Value::String(name) => format!("{:?}", name.to_string_lossy()),
            other => lua_value_kind(other).into(),
        }
    ))
}

pub(crate) fn opt_length(t: &Table, field: &str, what: &str) -> Result<Option<NodeLength>, String> {
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil => Ok(None),
        Value::Integer(_) | Value::Number(_) => {
            let number = opt_number(t, field, what)?;
            if number.is_some_and(|n| n < 0.0) {
                return Err(format!("{what} {field} must be non-negative"));
            }
            Ok(number.map(NodeLength::Fixed))
        }
        Value::String(s) => {
            let raw = s.to_string_lossy();
            if let Some(length) = crate::shared::enums::lookup(LENGTH_NAMES, &raw) {
                return Ok(Some(length));
            }
            if let Some((kind, portion)) = raw.trim().split_once(':')
                && kind.trim().eq_ignore_ascii_case("fill")
            {
                let portion = portion.trim();
                if !portion.is_empty() && portion.bytes().all(|digit| digit.is_ascii_digit()) {
                    let portion = portion.bytes().fold(0_u16, |n, digit| {
                        n.saturating_mul(10).saturating_add(u16::from(digit - b'0'))
                    });
                    if portion > 0 {
                        return Ok(Some(NodeLength::FillPortion(portion)));
                    }
                }
                return Err(format!(
                    "{what} {field} fill:n requires a positive integer portion, got {raw:?}"
                ));
            }
            Err(format!(
                "{what} {field} must be a number, {}, or \"fill:n\" (positive integer n), got {raw:?}",
                crate::shared::enums::expected(LENGTH_NAMES)
            ))
        }
        other => Err(format!(
            "{what} {field} must be a number, {}, or \"fill:n\" (positive integer n), got {}",
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
        Value::Nil => Ok(None),
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

const ALIGNMENTS: &[(&str, iced::Alignment)] = &[
    ("start", iced::Alignment::Start),
    ("center", iced::Alignment::Center),
    ("end", iced::Alignment::End),
];

pub(crate) fn opt_alignment(
    t: &Table,
    field: &str,
    what: &str,
) -> Result<Option<iced::Alignment>, String> {
    let expected = crate::shared::enums::expected(ALIGNMENTS);
    match node_property(t, field).map_err(|e| e.to_string())? {
        Value::Nil => Ok(None),
        Value::String(s) => {
            let raw = s.to_string_lossy();
            crate::shared::enums::lookup(ALIGNMENTS, &raw)
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

/// Listview `axis` uses the shared enum normalization; omission is vertical.
fn opt_axis(t: &Table) -> Result<bool, String> {
    match node_property(t, "axis").map_err(|e| e.to_string())? {
        Value::Nil => Ok(false),
        Value::String(s) => {
            let raw = s.to_string_lossy();
            crate::shared::enums::lookup(AXES, &raw).ok_or_else(|| {
                format!("iced.listview() axis must be horizontal or vertical, got {raw:?}")
            })
        }
        other => Err(format!(
            "iced.listview() axis must be horizontal or vertical, got {}",
            lua_value_kind(&other)
        )),
    }
}

pub(crate) fn parse_node(value: &Value) -> Result<WidgetNode, String> {
    parse_node_hovered(value, &super::hover::HoverSet::new())
}

/// Decode with host-owned hover state: every listview delegate observes
/// `item.hovered` for its own `(list id, key)`. Absence means unhovered.
pub(crate) fn parse_node_hovered(
    value: &Value,
    hover: &super::hover::HoverSet,
) -> Result<WidgetNode, String> {
    decode_node(value, hover, &mut DecodeBudget::default())
}

fn decode_node(
    value: &Value,
    hover: &super::hover::HoverSet,
    budget: &mut DecodeBudget,
) -> Result<WidgetNode, String> {
    if budget.depth >= MAX_DEPTH {
        return Err(format!("ui tree exceeds maximum depth {MAX_DEPTH}"));
    }
    if budget.nodes >= MAX_NODES {
        return Err(format!("ui tree exceeds maximum node count {MAX_NODES}"));
    }
    let pointer = match value {
        Value::Table(table) => Some(table.to_pointer()),
        _ => None,
    };
    if pointer.is_some_and(|pointer| budget.ancestors.contains(&pointer)) {
        return Err("ui tree contains a cyclic component reference".into());
    }
    budget.nodes += 1;
    budget.depth += 1;
    if let Some(pointer) = pointer {
        budget.ancestors.push(pointer);
    }
    let result = decode_node_inner(value, hover, budget);
    if pointer.is_some() {
        budget.ancestors.pop();
    }
    budget.depth -= 1;
    result
}

fn decode_node_inner(
    value: &Value,
    hover: &super::hover::HoverSet,
    budget: &mut DecodeBudget,
) -> Result<WidgetNode, String> {
    let node = parse_component(value, hover, budget)?;
    let Value::Table(table) = value else {
        return Ok(node);
    };
    let anchor = control_string(table, "anchor", "iced component")?
        .map(|name| super::placement::Placement::parse(&name))
        .transpose()?;
    let margin = opt_insets(table, "margin", "iced component")?.unwrap_or_default();
    let on_press = control_string(table, "on_press", "iced component")?;
    let align = match node.kind() {
        kind @ ("text" | "container" | "popup") => {
            let what = format!("iced.{kind}()");
            super::layout_props::Align {
                x: opt_alignment(table, "align_x", &what)?,
                y: opt_alignment(table, "align_y", &what)?,
            }
        }
        "row" => super::layout_props::Align {
            x: None,
            y: opt_alignment(table, "align_y", "iced.row()")?,
        },
        "column" => super::layout_props::Align {
            x: opt_alignment(table, "align_x", "iced.column()")?,
            y: None,
        },
        _ => super::layout_props::Align::default(),
    };
    Ok(super::layout_props::Common {
        anchor,
        margin,
        on_press,
        align,
    }
    .wrap(node))
}

fn parse_child(
    value: &Value,
    hover: &super::hover::HoverSet,
    budget: &mut DecodeBudget,
) -> Result<WidgetNode, String> {
    let child = decode_node(value, hover, budget)?;
    if super::node::contains_popup(&child) {
        return Err(
            "iced.popup() is a surface root and cannot be nested inside another component".into(),
        );
    }
    Ok(child)
}

fn parse_component(
    value: &Value,
    hover: &super::hover::HoverSet,
    budget: &mut DecodeBudget,
) -> Result<WidgetNode, String> {
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
            let mut callbacks = Vec::new();
            let specs: Option<Table> = t.get("transitions").map_err(|e| e.to_string())?;
            let phase_spec = |phase: &str| -> Result<Value, String> {
                specs
                    .as_ref()
                    .map(|specs| specs.get::<Value>(phase))
                    .transpose()
                    .map(|value| value.unwrap_or(Value::Nil))
                    .map_err(|error| error.to_string())
            };
            for phase in ["add", "remove", "displaced"] {
                if let Value::Table(spec) = phase_spec(phase)?
                    && !matches!(
                        spec.get::<Value>("callback").map_err(|e| e.to_string())?,
                        Value::Nil | Value::Function(_)
                    )
                {
                    return Err(format!(
                        "listview {id:?} {phase}.callback must be a function"
                    ));
                }
            }
            let mut seen = std::collections::HashSet::new();
            for (index, item) in data.sequence_values::<Table>().enumerate() {
                if index >= MAX_LIST_ITEMS {
                    return Err(format!(
                        "listview {id:?} exceeds maximum item count {MAX_LIST_ITEMS}"
                    ));
                }
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
                let mut item_callbacks = [None, None, None];
                for (index, phase) in ["add", "remove", "displaced"].into_iter().enumerate() {
                    if let Value::Table(spec) = phase_spec(phase)? {
                        match spec.get::<Value>("callback").map_err(|e| e.to_string())? {
                            Value::Nil => {}
                            Value::Function(function) => {
                                let capture: mlua::Function =
                                    t.get("_capture_callback").map_err(|e| e.to_string())?;
                                let token: mlua::AnyUserData = capture
                                    .call((function, item.clone()))
                                    .map_err(|e| e.to_string())?;
                                item_callbacks[index] = Some(
                                    token
                                        .borrow::<crate::lua::callbacks::Callback>()
                                        .map_err(|e| e.to_string())?
                                        .clone(),
                                );
                            }
                            _ => {
                                return Err(format!(
                                    "listview {id:?} {phase}.callback must be a function"
                                ));
                            }
                        }
                    }
                }
                if let Value::Table(encoded) =
                    item.get::<Value>("_callbacks").map_err(|e| e.to_string())?
                {
                    for (index, phase) in ["add", "remove", "displaced"].into_iter().enumerate() {
                        if let Value::UserData(token) =
                            encoded.get::<Value>(phase).map_err(|e| e.to_string())?
                        {
                            item_callbacks[index] = Some(
                                token
                                    .borrow::<crate::lua::callbacks::Callback>()
                                    .map_err(|e| e.to_string())?
                                    .clone(),
                            );
                        }
                    }
                }
                if item_callbacks.iter().any(Option::is_some) {
                    callbacks.push((key.clone(), item_callbacks));
                }
                item.set("hovered", hover.contains(&(id.clone(), key.clone())))
                    .map_err(|e| e.to_string())?;
                let value: Value = delegate.call(item).map_err(|e| e.to_string())?;
                let child = parse_child(&value, hover, budget)
                    .map_err(|error| format!("listview {id:?}.items[{key:?}]: {error}"))?;
                items.push((key, child));
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
                callbacks,
            })
        }
        "container" | "popup" => {
            let what = format!("iced.{kind}()");
            if kind == "popup" {
                let component = super::components::COMPONENTS
                    .iter()
                    .find(|c| c.name == "popup")
                    .ok_or("popup schema missing")?;
                let allowed = |field: &str| {
                    component.setters.contains(&field)
                        || super::layout_props::SETTERS.contains(&field)
                };
                for pair in t.clone().pairs::<String, Value>() {
                    let (key, _) = pair.map_err(|e| e.to_string())?;
                    if !allowed(&key)
                        && !matches!(key.as_str(), "type" | "child" | "_properties" | "_defaults")
                    {
                        return Err(format!("{what} unknown field {key:?}"));
                    }
                }
                let properties = t
                    .raw_get::<Value>("_properties")
                    .map_err(|e| e.to_string())?;
                if !matches!(properties, Value::Nil | Value::Table(_)) {
                    return Err(format!("{what} _properties must be a table"));
                }
                if let Value::Table(props) = properties {
                    for pair in props.pairs::<String, Value>() {
                        let (key, _) = pair.map_err(|e| e.to_string())?;
                        if !allowed(&key) {
                            return Err(format!("{what} unknown property {key:?}"));
                        }
                    }
                }
                for field in component.setters.iter().chain(super::layout_props::SETTERS) {
                    if matches!(
                        node_property(t, field).map_err(|e| e.to_string())?,
                        Value::Function(_)
                    ) || matches!(
                        t.raw_get::<Value>(*field).map_err(|e| e.to_string())?,
                        Value::Function(_)
                    ) {
                        return Err(format!("{what} {field} cannot be a function"));
                    }
                }
            }
            let background = opt_color(t, "background", &what)?;
            let border = opt_color(t, "border", &what)?;
            let border_width = opt_number(t, "border_width", &what)?
                .map(|w| w.max(0.0))
                .unwrap_or(if border.is_some() { 1.0 } else { 0.0 });
            let child = Box::new(
                parse_child(
                    &t.get::<Value>("child").map_err(|e| e.to_string())?,
                    hover,
                    budget,
                )
                .map_err(|error| format!("{kind}.child: {error}"))?,
            );
            let width = opt_length(t, "width", &what)?.unwrap_or(NodeLength::Native);
            let height = opt_length(t, "height", &what)?.unwrap_or(NodeLength::Native);
            let padding = opt_insets(t, "padding", &what)?.unwrap_or_default();
            let radius = opt_corners(t, "radius", &what)?.unwrap_or(Corners::ZERO);
            if kind == "popup" {
                for (field, length) in [("width", &width), ("height", &height)] {
                    match length {
                        NodeLength::FillPortion(_) => {
                            return Err(format!(
                                "{what} {field} cannot use fill:n on a surface root"
                            ));
                        }
                        NodeLength::Fixed(n) if !n.is_finite() || *n <= 0.0 => {
                            return Err(format!("{what} {field} must be positive and finite"));
                        }
                        _ => {}
                    }
                }
                if !border_width.is_finite() || border_width < 0.0 {
                    return Err(format!(
                        "{what} border width must be finite and non-negative"
                    ));
                }
                Ok(WidgetNode::Popup {
                    child,
                    width,
                    height,
                    padding,
                    background,
                    radius,
                    border,
                    border_width,
                })
            } else {
                Ok(WidgetNode::Container {
                    child,
                    width,
                    height,
                    padding,
                    background,
                    radius,
                    border,
                    border_width,
                })
            }
        }
        "scrollable" => Ok(WidgetNode::Scrollable {
            scrollbar_hidden: scrollbar_hidden(t)?,
            child: Box::new(parse_child(
                &t.get::<Value>("child").map_err(|e| e.to_string())?,
                hover,
                budget,
            )?),
            width: opt_length(t, "width", "iced.scrollable()")?.unwrap_or(NodeLength::Native),
            height: opt_length(t, "height", "iced.scrollable()")?.unwrap_or(NodeLength::Native),
        }),
        "space" => Ok(WidgetNode::Space {
            width: opt_length(t, "width", "iced.space()")?.unwrap_or(NodeLength::Native),
            height: opt_length(t, "height", "iced.space()")?.unwrap_or(NodeLength::Native),
        }),
        "image" | "image_handle" => {
            let image: Value = t
                .get(if kind == "image" { "path" } else { "image" })
                .map_err(|error| error.to_string())?;
            let width = opt_length(t, "width", "iced.image()")?.unwrap_or(NodeLength::Native);
            let height = opt_length(t, "height", "iced.image()")?.unwrap_or(NodeLength::Native);
            if let Value::String(path) = image {
                Ok(WidgetNode::Image {
                    path: path.to_string_lossy(),
                    width,
                    height,
                })
            } else {
                Ok(WidgetNode::Raster {
                    handle: super::image_value::handle(image).map_err(|error| error.to_string())?,
                    width,
                    height,
                })
            }
        }
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
        "row" | "column" | "stack" => {
            let children = match t.get::<Value>("children").map_err(|e| e.to_string())? {
                Value::Table(list) => list
                    .sequence_values::<Value>()
                    .enumerate()
                    .map(|(index, child)| {
                        child
                            .map_err(|e| e.to_string())
                            .and_then(|child| parse_child(&child, hover, budget))
                            .map_err(|error| format!("{kind}.children[{}]: {error}", index + 1))
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
            if kind == "stack" {
                return Ok(WidgetNode::Stack {
                    children,
                    width: opt_length(t, "width", "iced.stack()")?.unwrap_or(NodeLength::Native),
                    height: opt_length(t, "height", "iced.stack()")?.unwrap_or(NodeLength::Native),
                });
            }
            let spacing = opt_number(t, "spacing", &format!("iced.{kind}()"))?.unwrap_or(4.0);
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
                node_property(t, "action").map_err(|e| e.to_string())?,
                "iced.button() action",
            )?,
            width: opt_length(t, "width", "iced.button()")?,
            height: opt_length(t, "height", "iced.button()")?,
            padding: opt_insets(t, "padding", "iced.button()")?,
            color: opt_color(t, "color", "iced.button()")?,
            background: opt_color(t, "background", "iced.button()")?,
            radius: opt_corners(t, "radius", "iced.button()")?,
        }),
        "progress" => {
            let value = opt_number(t, "value", "iced.progress()")?.unwrap_or(0.0);
            Ok(WidgetNode::Progress {
                value,
                width: opt_length(t, "width", "iced.progress()")?
                    .unwrap_or(NodeLength::Fixed(120.0)),
                height: opt_length(t, "height", "iced.progress()")?,
                color: opt_color(t, "color", "iced.progress()")?,
                background: opt_color(t, "background", "iced.progress()")?,
            })
        }
        "canvas" => parse_canvas(t),
        "shader" => super::shader::parse(t),
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
        Value::Nil => None,
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
        Value::Nil => super::components::ElideMode::Tail,
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
            layout: Default::default(),
            label: label()?,
            checked: control_bool(t, "checked", &what)?,
            on_toggle: string("on_toggle")?,
            width,
            size,
            spacing,
        }
        .into()),
        "toggler" => Ok(TogglerNode {
            layout: Default::default(),
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
                layout: Default::default(),
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
            layout: Default::default(),
            placeholder: string("placeholder")?.unwrap_or_default(),
            value: string("value")?.unwrap_or_default(),
            on_input: string("on_input")?,
            on_submit: string("on_submit")?,
            width,
            size,
            padding: opt_insets(t, "padding", &what)?,
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
                layout: Default::default(),
                options,
                selected,
                on_select: string("on_select")?,
                width,
                placeholder: string("placeholder")?,
                padding: opt_insets(t, "padding", &what)?,
                size,
            }
            .into())
        }
        "radio" => Ok(RadioNode {
            layout: Default::default(),
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

fn parse_canvas(t: &Table) -> Result<WidgetNode, String> {
    const WHAT: &str = "iced.canvas()";
    let drawing = match node_property(t, "drawing").map_err(|error| error.to_string())? {
        Value::Nil => super::canvas::CanvasDrawing::default(),
        Value::Table(spec) => parse_canvas_drawing(&spec)?,
        other => {
            return Err(format!(
                "{WHAT} drawing must be a table, got {}",
                lua_value_kind(&other)
            ));
        }
    };
    Ok(WidgetNode::Canvas {
        drawing,
        width: opt_length(t, "width", WHAT)?.unwrap_or(NodeLength::Native),
        height: opt_length(t, "height", WHAT)?.unwrap_or(NodeLength::Native),
    })
}

fn parse_canvas_drawing(spec: &Table) -> Result<super::canvas::CanvasDrawing, String> {
    const WHAT: &str = "iced.canvas()";
    let mut drawing = super::canvas::CanvasDrawing::default();
    match spec.raw_get::<Value>("rects").map_err(|e| e.to_string())? {
        Value::Nil => {}
        Value::Table(list) => {
            for item in list.sequence_values::<Table>() {
                let item = item.map_err(|e| e.to_string())?;
                let rect = "iced.canvas() rect";
                let width = canvas_number(&item, "width", rect)?;
                let height = canvas_number(&item, "height", rect)?;
                if width < 0.0 || height < 0.0 {
                    return Err(format!("{rect} width/height must be non-negative"));
                }
                drawing.rects.push(super::canvas::CanvasRect {
                    x: canvas_number(&item, "x", rect)?,
                    y: canvas_number(&item, "y", rect)?,
                    width,
                    height,
                    color: canvas_color(&item, rect)?,
                });
            }
        }
        other => {
            return Err(format!(
                "{WHAT} drawing.rects must be an array, got {}",
                lua_value_kind(&other)
            ));
        }
    }
    match spec.raw_get::<Value>("lines").map_err(|e| e.to_string())? {
        Value::Nil => {}
        Value::Table(list) => {
            for item in list.sequence_values::<Table>() {
                let item = item.map_err(|e| e.to_string())?;
                let line = "iced.canvas() line";
                let points = match item.raw_get::<Value>("points").map_err(|e| e.to_string())? {
                    Value::Table(points) => points
                        .sequence_values::<Value>()
                        .map(|point| canvas_point(&point.map_err(|e| e.to_string())?))
                        .collect::<Result<Vec<_>, _>>()?,
                    Value::Nil => return Err(format!("{line} needs a points array")),
                    other => {
                        return Err(format!(
                            "{line} points must be an array, got {}",
                            lua_value_kind(&other)
                        ));
                    }
                };
                let width = match opt_number(&item, "width", line)? {
                    Some(width) if width.is_finite() && width >= 0.0 => width,
                    Some(_) => {
                        return Err(format!("{line} width must be a non-negative number"));
                    }
                    None => 1.0,
                };
                drawing.lines.push(super::canvas::CanvasLine {
                    points,
                    color: canvas_color(&item, line)?,
                    width,
                });
            }
        }
        other => {
            return Err(format!(
                "{WHAT} drawing.lines must be an array, got {}",
                lua_value_kind(&other)
            ));
        }
    }
    Ok(drawing)
}

fn canvas_number(t: &Table, field: &str, what: &str) -> Result<f32, String> {
    opt_number(t, field, what)?
        .filter(|number| number.is_finite())
        .ok_or_else(|| format!("{what} {field} must be a finite number"))
}

fn canvas_color(t: &Table, what: &str) -> Result<iced::Color, String> {
    opt_color(t, "color", what)?.ok_or_else(|| format!("{what} needs a color"))
}

fn canvas_point(value: &Value) -> Result<(f32, f32), String> {
    let Value::Table(t) = value else {
        return Err(format!(
            "iced.canvas() line point must be a table with x and y, got {}",
            lua_value_kind(value)
        ));
    };
    let num = |value: Value| match value {
        Value::Integer(n) => Some(n as f32),
        Value::Number(n) => Some(n as f32),
        _ => None,
    };
    let coordinate = |key: &str, index: i32| {
        t.raw_get::<Value>(key)
            .ok()
            .and_then(num)
            .or_else(|| t.raw_get::<Value>(index).ok().and_then(num))
    };
    match (coordinate("x", 1), coordinate("y", 2)) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok((x, y)),
        _ => Err("iced.canvas() line point needs finite x and y".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_rejects_cycles_and_recovers_for_shared_siblings() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let cyclic = lua
            .load("local t={type='container'}; t.child=t; return t")
            .eval()
            .unwrap();
        assert!(parse_node(&cyclic).unwrap_err().contains("cyclic"));
        let shared = lua
            .load("local t=iced.text('shared'); return iced.row({t,t})")
            .eval()
            .unwrap();
        assert!(parse_node(&shared).is_ok());
    }

    #[test]
    fn decoding_bounds_depth_total_nodes_and_list_items() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        for (source, expected) in [
            (
                format!(
                    "local t=iced.text('leaf'); for i=1,{MAX_DEPTH} do t=iced.container(t) end; return t"
                ),
                "depth",
            ),
            (
                format!(
                    "local t=iced.text('leaf'); local children={{}}; for i=1,{MAX_NODES} do children[i]=t end; return iced.row(children)"
                ),
                "node count",
            ),
            (
                format!(
                    "local data={{}}; for i=1,{} do data[i]={{id=i}} end; return iced.listview(data):id('bounded'):key('id'):delegate(function() return iced.space() end)",
                    MAX_LIST_ITEMS + 1
                ),
                "item count",
            ),
        ] {
            let value = lua.load(source).eval().unwrap();
            assert!(parse_node(&value).unwrap_err().contains(expected));
        }
    }

    #[test]
    fn invalid_primitive_properties_fail_with_child_context() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        for source in [
            "iced.text('x'):size(0/0)",
            "iced.text('x'):size(0)",
            "iced.space():width(-1)",
            "iced.space():height(1/0)",
            "iced.row({}):spacing(-1)",
            "iced.text('x'):wrap(function() end)",
            "iced.text('x'):color({r=0/0,g=0,b=0})",
            "iced.progress(1/0)",
        ] {
            let value = lua
                .load(format!("return iced.column({{{source}}})"))
                .eval()
                .unwrap();
            let error = parse_node(&value).unwrap_err();
            assert!(error.contains("column.children[1]"), "{source}: {error}");
        }
    }

    #[test]
    fn fill_portions_are_strict_clamped_and_roundtrip_without_changing_plain_fill() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        for (value, expected) in [
            ("fill", NodeLength::Fill),
            ("fill:1", NodeLength::FillPortion(1)),
            (" FILL : 3 ", NodeLength::FillPortion(3)),
            ("fill:65535", NodeLength::FillPortion(u16::MAX)),
            (
                "fill:999999999999999999999999999999",
                NodeLength::FillPortion(u16::MAX),
            ),
        ] {
            let node = parse_node(
                &lua.load(format!(
                    "return iced.space():width('{value}'):height('{value}')"
                ))
                .eval()
                .unwrap(),
            )
            .unwrap();
            assert!(
                matches!(&node, WidgetNode::Space { width, height } if *width == expected && *height == expected)
            );
            let encoded = super::super::encode::to_table(&lua, &node).unwrap();
            if expected == NodeLength::Fill {
                assert_eq!(
                    node_property(&encoded, "width")
                        .unwrap()
                        .as_string()
                        .unwrap()
                        .to_str()
                        .unwrap(),
                    "fill"
                );
            }
            assert_eq!(parse_node(&Value::Table(encoded)).unwrap(), node);
        }
        let node = parse_node(
            &lua.load("return iced.space():width('fill:2'):width('fill:3')")
                .eval()
                .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            node,
            WidgetNode::Space {
                width: NodeLength::FillPortion(3),
                ..
            }
        ));
        for axis in ["width", "height"] {
            for bad in [
                "fill:0", "fill:000", "fill:-1", "fill:1.5", "fill:x", "fill:", "fill:+2",
                "fill:2:3", "shrink:2",
            ] {
                let value = lua
                    .load(format!("return iced.space():{axis}('{bad}')"))
                    .eval()
                    .unwrap();
                let error = parse_node(&value).unwrap_err();
                assert!(error.contains(axis), "{error}");
            }
        }
    }

    #[test]
    fn scrollbar_visibility_is_strict_repeatable_and_roundtrips() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        for (suffix, hidden) in [
            ("", false),
            (":scrollbar('hidden')", true),
            (":scrollbar(' HIDDEN ')", true),
            (":scrollbar('hidden'):scrollbar('visible')", false),
        ] {
            let value = lua
                .load(format!("return iced.scrollable(iced.text('body')){suffix}"))
                .eval()
                .unwrap();
            let node = parse_node(&value).unwrap();
            assert!(
                matches!(&node, WidgetNode::Scrollable { scrollbar_hidden, .. } if *scrollbar_hidden == hidden)
            );
            let encoded = super::super::encode::to_table(&lua, &node).unwrap();
            assert_eq!(parse_node(&Value::Table(encoded)).unwrap(), node);
        }
        for invalid in ["'invisible'", "true", "42", "{}", "function() end"] {
            let value = lua
                .load(format!(
                    "return iced.scrollable(iced.text('body')):scrollbar({invalid})"
                ))
                .eval()
                .unwrap();
            let error = parse_node(&value).unwrap_err();
            assert!(
                error.contains("scrollbar must be")
                    && error.contains("visible")
                    && error.contains("hidden"),
                "{error}"
            );
        }
        assert!(
            lua.load("return iced.text('body'):scrollbar('hidden')")
                .eval::<Value>()
                .is_err()
        );
    }

    #[test]
    fn alignment_is_case_insensitive_axis_specific_and_roundtrips() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        for (source, x, y) in [
            (
                "return iced.text('x'):align_x('center'):align_y('END')",
                Some(iced::Alignment::Center),
                Some(iced::Alignment::End),
            ),
            ("return iced.text('x')", None, None),
            (
                "return iced.row({}):align_y('start')",
                None,
                Some(iced::Alignment::Start),
            ),
            (
                "return iced.column({}):align_x('center')",
                Some(iced::Alignment::Center),
                None,
            ),
            (
                "return iced.container(iced.space()):align_x('end'):align_y('start')",
                Some(iced::Alignment::End),
                Some(iced::Alignment::Start),
            ),
        ] {
            let node = parse_node(&lua.load(source).eval::<Value>().unwrap()).unwrap();
            match &node {
                WidgetNode::Aligned {
                    child,
                    align_x,
                    align_y,
                } => {
                    assert_eq!((*align_x, *align_y), (x, y), "{source}");
                    assert!(!matches!(child.as_ref(), WidgetNode::Aligned { .. }));
                }
                other => assert_eq!((None, None), (x, y), "{source}: {other:?}"),
            }
            let encoded = super::super::encode::to_table(&lua, &node).unwrap();
            assert_eq!(
                parse_node(&Value::Table(encoded)).unwrap(),
                node,
                "{source}"
            );
        }
        let error = parse_node(
            &lua.load("return iced.text('x'):align_x('middle')")
                .eval::<Value>()
                .unwrap(),
        )
        .unwrap_err();
        assert!(
            error.contains("align_x")
                && error.contains("start")
                && error.contains("center")
                && error.contains("end"),
            "{error}"
        );
        for source in [
            "return iced.row({}):align_x('center')",
            "return iced.column({}):align_y('center')",
            "return iced.button('go','go'):align_y('center')",
        ] {
            assert!(
                lua.load(source).eval::<Value>().is_err(),
                "{source} must not expose that axis"
            );
        }
    }

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
    fn delegate_items_observe_hovered_from_decode_context() {
        use std::collections::HashSet;
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let source = "return iced.listview({{id=1},{id=2}}):id('rows'):key('id'):delegate(function(item) return iced.text(item.hovered and 'hot' or 'cold') end)";
        // Absent context means unhovered: delegates stay total without hosts.
        let plain = parse_node(&lua.load(source).eval().unwrap()).unwrap();
        let WidgetNode::ListView { items, .. } = &plain else {
            panic!("listview");
        };
        assert!(items.iter().all(
            |(_, node)| matches!(node, WidgetNode::Text { content, .. } if content == "cold")
        ));
        // Host hover sets surface per (list id, item key); other lists and
        // keys are unaffected, and nested trees inherit the context.
        let hovered = HashSet::from([("rows".to_string(), "2".to_string())]);
        let node = parse_node_hovered(&lua.load(source).eval().unwrap(), &hovered).unwrap();
        let WidgetNode::ListView { items, .. } = &node else {
            panic!("listview");
        };
        assert!(matches!(&items[0].1, WidgetNode::Text { content, .. } if content == "cold"));
        assert!(matches!(&items[1].1, WidgetNode::Text { content, .. } if content == "hot"));
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
    fn listview_axis_is_case_insensitive_and_rejects_unknown_values() {
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
        for axis in ["'diagonal'", "false", "42", "{}", "function() end"] {
            let value: Value = lua
                .load(format!("return iced.listview({{}}):id('a'):key('id'):delegate(function() return iced.text('x') end):axis({axis})"))
                .eval()
                .unwrap();
            assert!(parse_node(&value).unwrap_err().contains("axis"), "{axis}");
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
            "return iced.stack({iced.text('x')})",
        ] {
            let node = native(source);
            let (width, height) = match &node {
                WidgetNode::Space { width, height }
                | WidgetNode::Image { width, height, .. }
                | WidgetNode::Container { width, height, .. }
                | WidgetNode::Scrollable { width, height, .. }
                | WidgetNode::Row { width, height, .. }
                | WidgetNode::Column { width, height, .. }
                | WidgetNode::Stack { width, height, .. } => (width, height),
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
