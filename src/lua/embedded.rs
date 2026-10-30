//! `riced.widget` host: widget files embedded inside Lua-declared layers.
//!
//! A layer declares `riced.widget("clock", { ... }, "bar-top-clock")`, which
//! renders as an `riced.plugin("widget", ...)` slot. The render path builds the
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
pub(crate) const PLUGIN_REQUESTS: &str = "riced.plugin_requests";

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
    let layer_output = table.raw_get::<Option<String>>("output").ok().flatten();
    let output = layer_output.as_deref().unwrap_or(output);
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
    if let Some(prefix) = marker.raw_get::<Option<String>>("owner")?
        && !id
            .strip_prefix("widget:")
            .unwrap_or(id)
            .starts_with(&prefix)
    {
        return Ok(Value::Nil);
    }
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
/// `bounds` and the returned position are parent-surface-local. The compositor
/// slides the popup inside the output when the widget is near an edge.
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
    // Center on the widget; negative offsets place bottom/right menus inward.
    let center_x = bounds.x as f64 + bounds.width as f64 / 2.0;
    let center_y = bounds.y as f64 + bounds.height as f64 / 2.0;
    let popup_width = popup_width as f64;
    let popup_height = popup_height as f64;
    Ok(match edge.as_str() {
        "bottom" => (center_x - popup_width / 2.0, -popup_height),
        "left" => (thickness, center_y - popup_height / 2.0),
        "right" => (-popup_width, center_y - popup_height / 2.0),
        _ => (center_x - popup_width / 2.0, thickness),
    })
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
    let (kind, rest) = action.key.split_once(':')?;
    if kind != "widget" && kind != "plugin" {
        return None;
    }
    let (id, key) = rest.split_once(':')?;
    let full_id = format!("{kind}:{id}");
    let resolved = resolved(lua, &full_id).ok()??;
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
            *active_popup = Some(full_id.clone());
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

/// Resolve a `widget:<id>` or `plugin:<id>` request to a widget VM entry.
fn resolved(lua: &Lua, full_id: &str) -> mlua::Result<Option<ResolvedWidget>> {
    let Some((kind, id)) = full_id.split_once(':') else {
        return Ok(None);
    };
    let registry = match kind {
        "plugin" => PLUGIN_REQUESTS,
        "widget" => REQUESTS,
        _ => return Ok(None),
    };
    let requests: Table = lua.named_registry_value(registry)?;
    let request: Value = requests.raw_get(id)?;
    let Value::Table(request) = request else {
        return Ok(None);
    };
    let name = request.get::<String>("name")?;
    let path = std::path::PathBuf::from(request.get::<String>("path")?);
    let props = props(request.raw_get::<Value>("props")?)?;
    Ok(Some(ResolvedWidget {
        id: full_id.to_owned(),
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

/// Convert a JSON props object from `riced.plugin("widget", ...)` args.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_positions_are_parent_relative_on_every_bar_edge() {
        let lua = Lua::new();
        let marker = lua.create_table().unwrap();
        marker.set("thickness", 34).unwrap();
        let bounds = Some(iced::Rectangle::new(
            iced::Point::new(10.0, 20.0),
            iced::Size::new(20.0, 20.0),
        ));
        for (edge, expected) in [
            ("top", (-80.0, 34.0)),
            ("bottom", (-80.0, -100.0)),
            ("left", (34.0, -20.0)),
            ("right", (-200.0, -20.0)),
        ] {
            marker.set("edge", edge).unwrap();
            assert_eq!(
                position(&marker, &bounds, 200.0, 100.0).unwrap(),
                expected,
                "{edge}"
            );
        }
        marker.set("x", -12).unwrap();
        marker.set("y", 48).unwrap();
        assert_eq!(
            position(&marker, &None, 200.0, 100.0).unwrap(),
            (-12.0, 48.0)
        );
    }
}
