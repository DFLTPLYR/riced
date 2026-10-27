//! `riced.widget` host: widget files embedded inside Lua-declared layers.
//!
//! A layer declares `riced.widget("clock", { ... }, "bar-top-clock")`, which
//! renders as an `iced.native("widget", ...)` slot. The render path builds the
//! widget leaf with its own input wrapper and prefixed action sender, so clicks
//! and popups behave like native bar widgets. This module owns the shared
//! request registry, widget VM state, popup resolution, and action routing.
use crate::{
    config::PropValue,
    lua::widgets::{EntryContext, ResolvedWidget, WidgetState},
    services::ServiceCtx,
    ui::{action::Action, node::NodeLength},
};
use mlua::{Lua, Table, Value};
use std::collections::HashMap;

pub(crate) const REQUESTS: &str = "riced.widget_requests";

/// Resolve every `riced.widget_popup` marker in a layer tree.
pub(crate) fn resolve(
    lua: &Lua,
    value: Value,
    widgets: &mut WidgetState,
    active_popup: &mut Option<String>,
    active_bounds: &Option<iced::Rectangle>,
    services: &ServiceCtx<'_>,
    output: &str,
) -> mlua::Result<Value> {
    let Value::Table(table) = value else {
        return Ok(value);
    };
    if table.raw_get::<Option<String>>("type")?.as_deref() == Some("widget_popup") {
        return popup(
            lua,
            widgets,
            active_popup,
            active_bounds,
            services,
            output,
            &table,
        );
    }
    let mut entries = Vec::new();
    for pair in table.clone().pairs::<Value, Value>() {
        entries.push(pair?);
    }
    for (key, value) in entries {
        let resolved = resolve(
            lua,
            value,
            widgets,
            active_popup,
            active_bounds,
            services,
            output,
        )?;
        table.raw_set(key, resolved)?;
    }
    Ok(Value::Table(table))
}

fn popup(
    lua: &Lua,
    widgets: &WidgetState,
    active_popup: &Option<String>,
    active_bounds: &Option<iced::Rectangle>,
    services: &ServiceCtx<'_>,
    output: &str,
    marker: &Table,
) -> mlua::Result<Value> {
    let Some(id) = active_popup.as_deref() else {
        return Ok(Value::Nil);
    };
    let Some(resolved) = resolved(lua, id)? else {
        return Ok(Value::Nil);
    };
    let context = EntryContext {
        services: *services,
        output,
    };
    let Some(result) = widgets.popup(&resolved, &context) else {
        return Ok(Value::Nil);
    };
    let value = result.map_err(mlua::Error::RuntimeError)?;
    let node = crate::ui::decode::parse_node(&value).map_err(mlua::Error::RuntimeError)?;
    let crate::ui::node::WidgetNode::Popup {
        child,
        width,
        height,
        ..
    } = node
    else {
        return Ok(Value::Nil);
    };
    let popup_width = length_px(&width, 240.0).round().max(1.0) as u32;
    let popup_height = length_px(&height, 160.0).round().max(1.0) as u32;
    let (x, y) = position(
        marker,
        active_bounds,
        popup_width as f32,
        popup_height as f32,
    )?;
    let content = crate::ui::encode::to_table(lua, &child)?;
    let popup = lua.create_table()?;
    popup.set("id", "widget-popup")?;
    popup.set("x", x)?;
    popup.set("y", y)?;
    popup.set("width", popup_width)?;
    popup.set("height", popup_height)?;
    popup.set("content", Value::Table(content))?;
    Ok(Value::Table(popup))
}

/// Anchor the popup to the widget bounds reported by the click that opened it.
///
/// `bounds` are bar-surface-local; the marker supplies the bar edge, thickness,
/// and output size so the popup lands on the correct side of the widget.
fn position(
    marker: &Table,
    active_bounds: &Option<iced::Rectangle>,
    popup_width: f32,
    popup_height: f32,
) -> mlua::Result<(f64, f64)> {
    let fallback_x = marker.raw_get::<Option<f64>>("x")?.unwrap_or(0.0);
    let fallback_y = marker.raw_get::<Option<f64>>("y")?.unwrap_or(0.0);
    let Some(bounds) = active_bounds else {
        return Ok((fallback_x, fallback_y));
    };
    let edge = marker
        .raw_get::<Option<String>>("edge")?
        .unwrap_or_else(|| "top".into());
    let thickness = marker.raw_get::<Option<f64>>("thickness")?.unwrap_or(34.0);
    let output = marker.raw_get::<Option<Table>>("output")?;
    let width = output
        .as_ref()
        .and_then(|table| table.raw_get::<Option<f64>>("width").ok().flatten())
        .unwrap_or(0.0);
    let height = output
        .as_ref()
        .and_then(|table| table.raw_get::<Option<f64>>("height").ok().flatten())
        .unwrap_or(0.0);
    // Center the popup on the widget's cross axis, then slide it inside the
    // output when the widget is near an edge.
    let center_x = bounds.x as f64 + bounds.width as f64 / 2.0;
    let center_y = bounds.y as f64 + bounds.height as f64 / 2.0;
    let popup_width = popup_width as f64;
    let popup_height = popup_height as f64;
    let (x, y) = match edge.as_str() {
        "bottom" => (
            center_x - popup_width / 2.0,
            (height - thickness) - popup_height,
        ),
        "left" => (thickness, center_y - popup_height / 2.0),
        "right" => (
            (width - thickness) - popup_width,
            center_y - popup_height / 2.0,
        ),
        _ => (center_x - popup_width / 2.0, thickness),
    };
    Ok((
        x.clamp(0.0, (width - popup_width).max(0.0)),
        y.clamp(0.0, (height - popup_height).max(0.0)),
    ))
}

/// Route an action whose key carries a `widget:<id>:` prefix to that widget.
///
/// Returns `None` when the action belongs to the parent app instead.
#[allow(clippy::too_many_arguments)]
pub(crate) fn action(
    lua: &Lua,
    widgets: &WidgetState,
    active_popup: &mut Option<String>,
    active_bounds: &mut Option<iced::Rectangle>,
    fallback_bounds: Option<iced::Rectangle>,
    services: &ServiceCtx<'_>,
    output: &str,
    action: &Action,
) -> Option<mlua::Result<Value>> {
    let rest = action.key.strip_prefix("widget:")?;
    let (id, key) = rest.split_once(':')?;
    let resolved = resolved(lua, id).ok()??;
    let context = EntryContext {
        services: *services,
        output,
    };
    let bounds = match &action.event {
        crate::ui::action::Event::Mouse { bounds, .. } => bounds.or(fallback_bounds),
        _ => fallback_bounds,
    };
    let mut action = action.clone();
    action.key = key.to_owned();
    let result = widgets.ui_action(&resolved, &context, &action)?;
    Some(result.map_err(mlua::Error::RuntimeError).inspect(|value| {
        if matches!(value, Value::Table(table) if table.get::<bool>("popup").unwrap_or(false)) {
            *active_popup = Some(id.to_owned());
            *active_bounds = bounds;
        }
    }))
}

/// Clear the active widget popup when its host closes.
pub(crate) fn popup_closed(
    active_popup: &mut Option<String>,
    active_bounds: &mut Option<iced::Rectangle>,
    id: &str,
) {
    if id == "widget-popup" {
        *active_popup = None;
        *active_bounds = None;
    }
}

fn resolved(lua: &Lua, id: &str) -> mlua::Result<Option<ResolvedWidget>> {
    let requests: Table = lua.named_registry_value(REQUESTS)?;
    let request: Value = requests.raw_get(id)?;
    let Value::Table(request) = request else {
        return Ok(None);
    };
    let name = request.get::<String>("name")?;
    let path = std::path::PathBuf::from(request.get::<String>("path")?);
    let props = props(request.raw_get::<Value>("props")?)?;
    Ok(Some(ResolvedWidget {
        id: id.to_owned(),
        name,
        path,
        interval: request.raw_get::<Option<f32>>("interval")?.unwrap_or(1.0),
        size: request.raw_get::<Option<f32>>("size")?.unwrap_or(13.0),
        props,
    }))
}

fn length_px(length: &NodeLength, fallback: f32) -> f32 {
    match length {
        NodeLength::Fixed(px) => *px,
        _ => fallback,
    }
}

fn props(value: Value) -> mlua::Result<HashMap<String, PropValue>> {
    let mut out = HashMap::new();
    let Value::Table(table) = value else {
        return Ok(out);
    };
    for pair in table.pairs::<String, Value>() {
        let (key, value) = pair?;
        let value = match value {
            Value::Boolean(value) => PropValue::Bool(value),
            Value::Integer(value) => PropValue::Number(value as f64),
            Value::Number(value) => PropValue::Number(value),
            Value::String(value) => PropValue::Text(value.to_string_lossy().to_string()),
            _ => continue,
        };
        out.insert(key, value);
    }
    Ok(out)
}

/// Convert a JSON props object from `iced.native("widget", ...)` args.
pub(crate) fn json_props(value: &serde_json::Value) -> HashMap<String, PropValue> {
    let mut out = HashMap::new();
    let Some(map) = value.as_object() else {
        return out;
    };
    for (key, value) in map {
        let value = match value {
            serde_json::Value::Bool(value) => Some(PropValue::Bool(*value)),
            serde_json::Value::Number(value) => value.as_f64().map(PropValue::Number),
            serde_json::Value::String(value) => Some(PropValue::Text(value.clone())),
            _ => None,
        };
        if let Some(value) = value {
            out.insert(key.clone(), value);
        }
    }
    out
}
