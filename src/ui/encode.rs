//! Export owned component structs as chainable iced Lua description tables.
use super::node::{NodeLength, WidgetNode};
use mlua::{Lua, Table, Value};

impl mlua::IntoLua for WidgetNode {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        to_table(lua, &self).map(Value::Table)
    }
}
impl mlua::FromLua for WidgetNode {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        super::decode::parse_node(&value).map_err(mlua::Error::RuntimeError)
    }
}

fn length(length: Option<&NodeLength>) -> Value {
    match length {
        None | Some(NodeLength::Native) => Value::Nil,
        Some(NodeLength::Fixed(px)) => Value::Number(f64::from(*px)),
        // Strings are allocated by the caller's Lua instance, below.
        _ => Value::Nil,
    }
}
fn set_length(lua: &Lua, table: &Table, key: &str, size: Option<&NodeLength>) -> mlua::Result<()> {
    table.set(
        key,
        match size {
            Some(NodeLength::Fill) => Value::String(lua.create_string("fill")?),
            Some(NodeLength::Shrink) => Value::String(lua.create_string("shrink")?),
            _ => length(size),
        },
    )
}
fn set_color(lua: &Lua, table: &Table, key: &str, color: &Option<iced::Color>) -> mlua::Result<()> {
    if let Some(color) = color {
        let rgb = lua.create_table()?;
        for (key, value) in [
            ("r", color.r),
            ("g", color.g),
            ("b", color.b),
            ("a", color.a),
        ] {
            rgb.set(key, value)?;
        }
        table.set(key, rgb)?;
    }
    Ok(())
}

pub(crate) fn to_table(lua: &Lua, node: &WidgetNode) -> mlua::Result<Table> {
    match node {
        WidgetNode::Anchored {
            child,
            placement,
            margin,
        } => {
            let table = to_table(lua, child)?;
            let properties: Table = table.raw_get("_properties")?;
            properties.set("anchor", placement.name())?;
            properties.set("margin", *margin)?;
            return Ok(table);
        }
        WidgetNode::Clickable { child, action } => {
            let table = to_table(lua, child)?;
            table
                .raw_get::<Table>("_properties")?
                .set("on_press", action.as_str())?;
            return Ok(table);
        }
        _ => {}
    }
    let mts: Table = lua.named_registry_value("riced.iced.metatables")?;
    let mt = mts.get(node.kind())?;
    super::dsl::node(lua, node.kind(), mt, |table| {
        macro_rules! fields { ($($key:literal => $value:expr),* $(,)?) => { $(table.set($key, $value)?;)* }; }
        macro_rules! sizes {
            ($width:expr, $height:expr) => {
                set_length(lua, table, "width", $width)?;
                set_length(lua, table, "height", $height)?;
            };
        }
        match node {
            WidgetNode::Text {
                content,
                size,
                width,
                height,
                wrapping,
                color,
                elide,
            } => {
                fields!("text" => content.as_str(), "size" => *size);
                sizes!(width.as_ref(), height.as_ref());
                if let Some(wrapping) = wrapping {
                    table.set(
                        "wrap",
                        match wrapping {
                            iced::widget::text::Wrapping::None => "none",
                            iced::widget::text::Wrapping::Word => "word",
                            iced::widget::text::Wrapping::Glyph => "glyph",
                            iced::widget::text::Wrapping::WordOrGlyph => "word_or_glyph",
                        },
                    )?;
                }
                set_color(lua, table, "color", color)?;
                if let Some(elide) = elide {
                    table.set("elide", elide.max_chars)?;
                    table.set("elide_mode", elide.mode_name())?;
                }
            }
            WidgetNode::Icon { name, color } => {
                table.set("name", name.as_str())?;
                set_color(lua, table, "color", color)?;
            }
            WidgetNode::Row {
                children,
                spacing,
                width,
                height,
            }
            | WidgetNode::Column {
                children,
                spacing,
                width,
                height,
            } => {
                let nodes = children
                    .iter()
                    .map(|node| to_table(lua, node))
                    .collect::<mlua::Result<Vec<_>>>()?;
                fields!("children" => lua.create_sequence_from(nodes)?, "spacing" => *spacing);
                sizes!(Some(width), Some(height));
            }
            WidgetNode::Container {
                child,
                width,
                height,
                padding,
                background,
                radius,
                border,
                border_width,
            } => {
                fields!("child" => to_table(lua, child)?, "padding" => *padding, "radius" => *radius, "border_width" => *border_width);
                sizes!(Some(width), Some(height));
                set_color(lua, table, "background", background)?;
                set_color(lua, table, "border", border)?;
            }
            WidgetNode::Scrollable {
                child,
                width,
                height,
            } => {
                table.set("child", to_table(lua, child)?)?;
                sizes!(Some(width), Some(height));
            }
            WidgetNode::Space { width, height } => {
                sizes!(Some(width), Some(height));
            }
            WidgetNode::Image {
                path,
                width,
                height,
            } => {
                table.set("path", path.as_str())?;
                sizes!(Some(width), Some(height));
            }
            WidgetNode::Button {
                label,
                action,
                width,
                height,
                padding,
                color,
                background,
                radius,
            } => {
                fields!("label" => label.as_str(), "action" => action.as_str(), "padding" => *padding, "radius" => *radius);
                sizes!(width.as_ref(), height.as_ref());
                set_color(lua, table, "color", color)?;
                set_color(lua, table, "background", background)?;
            }
            WidgetNode::Progress {
                value,
                width,
                height,
                color,
                background,
            } => {
                table.set("value", *value)?;
                sizes!(Some(width), height.as_ref());
                set_color(lua, table, "color", color)?;
                set_color(lua, table, "background", background)?;
            }
            WidgetNode::Separator { height, color } => {
                table.set("height", *height)?;
                set_color(lua, table, "color", color)?;
            }
            WidgetNode::Spinner => {}
            WidgetNode::Anchored { .. } | WidgetNode::Clickable { .. } => {
                unreachable!("handled above")
            }
            WidgetNode::Raster {
                handle,
                width,
                height,
            } => {
                table.set(
                    "image",
                    lua.create_userdata(super::image_value::ImageValue(handle.clone()))?,
                )?;
                sizes!(Some(width), Some(height));
            }
            WidgetNode::Checkbox {
                label,
                checked,
                on_toggle,
                width,
                size,
                spacing,
            } => {
                fields!("label" => label.as_str(), "checked" => *checked, "on_toggle" => on_toggle.as_deref(), "size" => *size, "spacing" => *spacing);
                set_length(lua, table, "width", width.as_ref())?;
            }
            WidgetNode::Toggler {
                label,
                enabled,
                on_toggle,
                width,
                size,
                spacing,
            } => {
                fields!("label" => label.as_str(), "enabled" => *enabled, "on_toggle" => on_toggle.as_deref(), "size" => *size, "spacing" => *spacing);
                set_length(lua, table, "width", width.as_ref())?;
            }
            WidgetNode::Slider {
                min,
                max,
                value,
                step,
                on_change,
                on_release,
                width,
                height,
            } => {
                fields!("min" => *min, "max" => *max, "value" => *value, "step" => *step, "on_change" => on_change.as_deref(), "on_release" => on_release.as_deref(), "height" => *height);
                set_length(lua, table, "width", width.as_ref())?;
            }
            WidgetNode::TextInput {
                placeholder,
                value,
                on_input,
                on_submit,
                width,
                size,
                padding,
                secure,
            } => {
                fields!("placeholder" => placeholder.as_str(), "value" => value.as_str(), "on_input" => on_input.as_deref(), "on_submit" => on_submit.as_deref(), "size" => *size, "padding" => *padding, "secure" => *secure);
                set_length(lua, table, "width", width.as_ref())?;
            }
            WidgetNode::PickList {
                options,
                selected,
                on_select,
                width,
                placeholder,
                padding,
                size,
            } => {
                fields!("options" => lua.create_sequence_from(options.iter().map(String::as_str))?, "selected" => selected.as_deref(), "on_select" => on_select.as_deref(), "placeholder" => placeholder.as_deref(), "padding" => *padding, "size" => *size);
                set_length(lua, table, "width", width.as_ref())?;
            }
            WidgetNode::Radio {
                label,
                value,
                selected,
                on_select,
                width,
                size,
                spacing,
            } => {
                fields!("label" => label.as_str(), "value" => value.as_str(), "selected" => selected.as_deref(), "on_select" => on_select.as_deref(), "size" => *size, "spacing" => *spacing);
                set_length(lua, table, "width", width.as_ref())?;
            }
            WidgetNode::ListView {
                id,
                items,
                horizontal,
                pitch,
                spacing,
                width,
                height,
                transitions,
            } => {
                let data = lua.create_table()?;
                for (index, (key, node)) in items.iter().enumerate() {
                    let item = lua.create_table()?;
                    item.set("key", key.as_str())?;
                    item.set("node", to_table(lua, node)?)?;
                    data.set(index + 1, item)?;
                }
                fields!("data" => data, "id" => id.as_str(), "key" => "key", "axis" => if *horizontal { "horizontal" } else { "vertical" }, "pitch" => *pitch, "spacing" => *spacing);
                // Export resolved items through a fresh pure delegate. Original
                // Lua functions/VM references were discarded during decoding.
                table.set(
                    "delegate",
                    lua.create_function(|_, item: Table| item.get::<Value>("node"))?,
                )?;
                sizes!(Some(width), Some(height));
                let specs = lua.create_table()?;
                for (slot, transition) in [
                    ("add", &transitions.0),
                    ("remove", &transitions.1),
                    ("displaced", &transitions.2),
                ] {
                    let spec = lua.create_table()?;
                    for (axis, from, to) in [
                        ("x", transition.from.x, transition.to.x),
                        ("y", transition.from.y, transition.to.y),
                        ("opacity", transition.from.opacity, transition.to.opacity),
                    ] {
                        let values = lua.create_table()?;
                        values.set("from", from)?;
                        values.set("to", to)?;
                        spec.set(axis, values)?;
                    }
                    if let Some(duration) = transition.duration {
                        spec.set("duration", duration.as_secs_f64() * 1000.0)?;
                    }
                    specs.set(slot, spec)?;
                }
                table.set("transitions", specs)?;
            }
        }
        Ok(())
    })
}
