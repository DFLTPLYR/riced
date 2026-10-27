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
    services: &ServiceCtx<'_>,
    output: &str,
) -> mlua::Result<Value> {
    let Value::Table(table) = value else {
        return Ok(value);
    };
    if table.raw_get::<Option<String>>("type")?.as_deref() == Some("widget_popup") {
        return popup(lua, widgets, active_popup, services, output, &table);
    }
    let mut entries = Vec::new();
    for pair in table.clone().pairs::<Value, Value>() {
        entries.push(pair?);
    }
    for (key, value) in entries {
        let resolved = resolve(lua, value, widgets, active_popup, services, output)?;
        table.raw_set(key, resolved)?;
    }
    Ok(Value::Table(table))
}

fn popup(
    lua: &Lua,
    widgets: &WidgetState,
    active_popup: &Option<String>,
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
    let content = crate::ui::encode::to_table(lua, &child)?;
    let x = marker.raw_get::<Option<f64>>("x")?.unwrap_or(0.0);
    let y = marker.raw_get::<Option<f64>>("y")?.unwrap_or(0.0);
    let popup = lua.create_table()?;
    popup.set("id", "widget-popup")?;
    popup.set("x", x)?;
    popup.set("y", y)?;
    popup.set("width", length_px(&width, 240.0).round().max(1.0) as u32)?;
    popup.set("height", length_px(&height, 160.0).round().max(1.0) as u32)?;
    popup.set("content", Value::Table(content))?;
    Ok(Value::Table(popup))
}

/// Route an action whose key carries a `widget:<id>:` prefix to that widget.
///
/// Returns `None` when the action belongs to the parent app instead.
pub(crate) fn action(
    lua: &Lua,
    widgets: &WidgetState,
    active_popup: &mut Option<String>,
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
    let mut action = action.clone();
    action.key = key.to_owned();
    let result = widgets.ui_action(&resolved, &context, &action)?;
    Some(result.map_err(mlua::Error::RuntimeError).inspect(|value| {
        if matches!(value, Value::Table(table) if table.get::<bool>("popup").unwrap_or(false)) {
            *active_popup = Some(id.to_owned());
        }
    }))
}

/// Clear the active widget popup when its host closes.
pub(crate) fn popup_closed(active_popup: &mut Option<String>, id: &str) {
    if id == "widget-popup" {
        *active_popup = None;
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
