//! One-time conversion of the old flat TOML shape into structured tables.
use toml::{Table, Value};

fn rename(table: &mut Table, old: &str, new: &str) -> Result<(), String> {
    if let Some(value) = table.remove(old)
        && table.insert(new.into(), value).is_some()
    {
        return Err(format!("both {old:?} and {new:?} are configured"));
    }
    Ok(())
}

fn geometry(table: &mut Table, prefix: &str, fields: &[(&str, &str)]) -> Result<(), String> {
    let mut grouped = Table::new();
    for (old, new) in fields {
        if let Some(value) = table.remove(*old) {
            grouped.insert((*new).into(), value);
        }
    }
    if !grouped.is_empty() && table.insert(prefix.into(), Value::Table(grouped)).is_some() {
        return Err(format!(
            "both grouped and flat {prefix} fields are configured"
        ));
    }
    Ok(())
}

fn bar(table: &mut Table) -> Result<(), String> {
    rename(table, "slot_padding", "slot-padding")?;
    rename(table, "slot_spacing", "slot-spacing")?;
    geometry(
        table,
        "margin",
        &[
            ("margin_top", "top"),
            ("margin_right", "right"),
            ("margin_bottom", "bottom"),
            ("margin_left", "left"),
        ],
    )?;
    geometry(
        table,
        "radius",
        &[
            ("radius_top_left", "top-left"),
            ("radius_top_right", "top-right"),
            ("radius_bottom_left", "bottom-left"),
            ("radius_bottom_right", "bottom-right"),
        ],
    )?;
    let legacy_slots = table.get("slots").is_some_and(Value::is_integer)
        || table.contains_key("aligns")
        || table.contains_key("widgets");
    if legacy_slots {
        let count = match table.remove("slots") {
            Some(Value::Integer(n)) if (0..=32).contains(&n) => n as usize,
            None => 1,
            _ => {
                return Err(
                    "legacy slots must be a count in 0..=32, without structured slots".into(),
                );
            }
        };
        let array = |value: Option<Value>, name: &str| match value {
            Some(Value::Array(values)) => Ok(values),
            None => Ok(Vec::new()),
            _ => Err(format!("legacy {name} must be an array")),
        };
        let aligns = array(table.remove("aligns"), "aligns")?;
        let widgets = array(table.remove("widgets"), "widgets")?;
        let slots = (0..count)
            .map(|index| {
                let widgets = match widgets.get(index) {
                    Some(Value::String(name)) => Value::Array(vec![Value::String(name.clone())]),
                    Some(value) => value.clone(),
                    None => Value::Array(Vec::new()),
                };
                Value::Table(Table::from_iter([
                    (
                        "align".into(),
                        aligns
                            .get(index)
                            .cloned()
                            .unwrap_or(Value::String("center".into())),
                    ),
                    ("widgets".into(), widgets),
                ]))
            })
            .collect();
        table.insert("slots".into(), Value::Array(slots));
    }
    Ok(())
}

pub(super) fn migrate(value: &mut Value) -> Result<(), String> {
    if let Some(table) = value.get_mut("composable").and_then(Value::as_table_mut) {
        for (old, new) in [
            ("selection_rect", "selection-rect"),
            ("context_menu", "context-menu"),
            ("context_menu_item", "context-menu-item"),
        ] {
            rename(table, old, new)?;
        }
    }
    if let Some(table) = value.get_mut("notification").and_then(Value::as_table_mut) {
        rename(table, "timeout_ms", "timeout-ms")?;
    }
    if let Some(table) = value.get_mut("theme").and_then(Value::as_table_mut) {
        rename(table, "templates_dir", "templates-dir")?;
        if let Some(darkmode) = table.remove("darkmode") {
            let Some(darkmode) = darkmode.as_bool() else {
                return Err("theme.darkmode must be a boolean".into());
            };
            if table
                .insert(
                    "mode".into(),
                    Value::String(if darkmode { "dark" } else { "light" }.into()),
                )
                .is_some()
            {
                return Err("both theme.darkmode and theme.mode are configured".into());
            }
        }
    }
    if let Some(table) = value.get_mut("background").and_then(Value::as_table_mut) {
        rename(table, "image", "images")?;
        if let Some(Value::Array(images)) = table.get_mut("images") {
            for image in images.iter_mut().filter_map(Value::as_table_mut) {
                if image.contains_key("x") || image.contains_key("y") {
                    let position = Value::Array(vec![
                        image.remove("x").unwrap_or(Value::Float(0.0)),
                        image.remove("y").unwrap_or(Value::Float(0.0)),
                    ]);
                    if image.insert("position".into(), position).is_some() {
                        return Err("both image.position and x/y are configured".into());
                    }
                }
            }
        }
        if let Some(monitors) = table.remove("monitor") {
            let Value::Array(monitors) = monitors else {
                return Err("background.monitor must be an array".into());
            };
            let mut keyed = Table::new();
            for monitor in monitors {
                let Value::Table(mut monitor) = monitor else {
                    return Err("monitor entry must be a table".into());
                };
                let Some(Value::String(output)) = monitor.remove("output") else {
                    return Err("monitor entry needs an output name".into());
                };
                if keyed
                    .insert(output.clone(), Value::Table(monitor))
                    .is_some()
                {
                    return Err(format!("duplicate wallpaper output {output:?}"));
                }
            }
            if table
                .insert("monitors".into(), Value::Table(keyed))
                .is_some()
            {
                return Err(
                    "both background.monitor and background.monitors are configured".into(),
                );
            }
        }
    }
    if let Some(Value::Array(bars)) = value.get_mut("bar") {
        for table in bars.iter_mut().filter_map(Value::as_table_mut) {
            bar(table)?;
        }
    }
    if let Some(table) = value.get_mut("top").and_then(Value::as_table_mut) {
        for table in table
            .iter_mut()
            .filter_map(|(_, value)| value.as_table_mut())
        {
            bar(table)?;
        }
    }
    Ok(())
}
