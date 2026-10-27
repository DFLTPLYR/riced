//! `riced.widget` host: widget files embedded inside Lua-declared layers.
//!
//! A layer declares `riced.widget("clock", { ... }, "bar-top-clock")` and the
//! host renders the widget with its own VM, services, popup contract, and
//! action routing. The declaration is a placeholder; [`resolve`] replaces it
//! with the rendered tree before the parent layer is decoded, so widget files
//! stay ordinary `widgets/<name>.lua` modules.
use crate::{
    config::PropValue,
    lua::widgets::{EntryContext, ResolvedWidget, WidgetState},
    services::ServiceCtx,
    ui::{action::Action, node::NodeLength},
};
use mlua::{Lua, Table, Value};
use std::collections::HashMap;

/// Widget action keys that must carry the owning instance prefix so the host
/// can route them back to the widget's own `actions` handler.
const ACTION_KEYS: &[&str] = &[
    "action",
    "on_press",
    "on_toggle",
    "on_change",
    "on_release",
    "on_input",
    "on_submit",
    "on_select",
];

pub(crate) const REQUESTS: &str = "riced.widget_requests";

/// Resolve every `riced.widget`/`riced.widget_popup` marker in a layer tree.
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
    match table.raw_get::<Option<String>>("type")?.as_deref() {
        Some("embedded") => {
            let id = table.get::<String>("id")?;
            let Some(value) = render(lua, widgets, services, output, &id)? else {
                return Ok(Value::Nil);
            };
            let value = prefix_actions(lua, value, &id)?;
            Ok(value)
        }
        Some("widget_popup") => popup(lua, widgets, active_popup, services, output, &table),
        _ => {
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
    }
}

fn render(
    lua: &Lua,
    widgets: &mut WidgetState,
    services: &ServiceCtx<'_>,
    output: &str,
    id: &str,
) -> mlua::Result<Option<Value>> {
    let Some(resolved) = resolved(lua, id)? else {
        return Ok(None);
    };
    widgets.sync_revision(&resolved.path, std::iter::once(id));
    let context = EntryContext {
        services: *services,
        output,
    };
    let value = widgets
        .render(&resolved, &context)
        .map_err(mlua::Error::RuntimeError)?;
    let node = crate::ui::decode::parse_node(&value).map_err(mlua::Error::RuntimeError)?;
    crate::ui::encode::to_table(lua, &node)
        .map(Value::Table)
        .map(Some)
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

/// Prefix every action key in a rendered widget tree with its instance id.
fn prefix_actions(lua: &Lua, value: Value, id: &str) -> mlua::Result<Value> {
    let Value::Table(table) = value else {
        return Ok(value);
    };
    let mut entries = Vec::new();
    for pair in table.clone().pairs::<Value, Value>() {
        entries.push(pair?);
    }
    for (key, value) in entries {
        if let Value::String(key) = &key
            && ACTION_KEYS.contains(&key.to_string_lossy().as_ref())
            && let Value::String(action) = &value
        {
            let prefixed =
                lua.create_string(format!("widget:{id}:{}", action.to_string_lossy()))?;
            table.raw_set(key, prefixed)?;
            continue;
        }
        let value = prefix_actions(lua, value, id)?;
        table.raw_set(key, value)?;
    }
    Ok(Value::Table(table))
}
