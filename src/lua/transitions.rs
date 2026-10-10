//! Decode motion descriptions independently of bar/window ownership.
use super::value::lua_value_kind;
use crate::ui::listview::Transition;
use mlua::{Lua, Table, Value};
use std::time::Duration;
pub(crate) type TransitionSpec = (Transition, Transition, Transition, bool);

pub(crate) fn parse_transitions(
    lua: &Lua,
    enter: &Transition,
    exit: &Transition,
    displaced: &Transition,
) -> Result<(Transition, Transition, Transition, bool), String> {
    let spec = match lua.named_registry_value::<Table>("riced.widget.app") {
        Ok(app) => match app
            .get::<Value>("transitions")
            .map_err(|error| error.to_string())?
        {
            Value::Nil => Value::Nil,
            Value::Function(_) => super::widgets::call_lua_value(lua, "transitions")?,
            _ => return Err("app.transitions must be a function".into()),
        },
        Err(_) => Value::Nil,
    };
    parse_transition_value(spec, enter, exit, displaced)
}

pub(crate) fn parse_transition_value(
    spec: Value,
    enter: &Transition,
    exit: &Transition,
    displaced: &Transition,
) -> Result<(Transition, Transition, Transition, bool), String> {
    fn num(value: &Value) -> Option<f32> {
        match value {
            Value::Integer(i) => Some(*i as f32),
            Value::Number(n) => Some(*n as f32),
            _ => None,
        }
        .filter(|v| v.is_finite())
    }
    fn axis_number(
        slot: &Table,
        slot_name: &str,
        axis: &str,
        field: &str,
        keep: f32,
    ) -> Result<f32, String> {
        let axis_value: Value = slot.get(axis).map_err(|e| e.to_string())?;
        let axis_table = match axis_value {
            Value::Nil => return Ok(keep),
            Value::Table(t) => t,
            other => {
                return Err(format!(
                    "transitions().{slot_name}.{axis} must be a table like {{ from = 0, to = 1 }}, got {}",
                    lua_value_kind(&other)
                ));
            }
        };
        match axis_table.get::<Value>(field).map_err(|e| e.to_string())? {
            Value::Nil => Ok(keep),
            v => num(&v).ok_or_else(|| {
                format!(
                    "transitions().{slot_name}.{axis}.{field} must be a number, got {}",
                    lua_value_kind(&v)
                )
            }),
        }
    }
    fn slot_duration(
        slot: &Table,
        slot_name: &str,
        keep: Option<Duration>,
    ) -> Result<Option<Duration>, String> {
        match slot.get::<Value>("duration").map_err(|e|e.to_string())? {Value::Nil=>Ok(keep),v=>num(&v).filter(|n|*n>=0.0).map(|n|Some(Duration::from_millis(n.clamp(0.0,5000.0) as u64))).ok_or_else(||format!("transitions().{slot_name}.duration must be a non-negative number of milliseconds, got {}",lua_value_kind(&v)))}
    }
    let spec_table = match spec {
        Value::Nil => return Ok((enter.clone(), exit.clone(), displaced.clone(), false)),
        Value::Table(t) => t,
        other => {
            return Err(format!(
                "transitions() must return a table, got {}",
                lua_value_kind(&other)
            ));
        }
    };
    let mut out_enter = enter.clone();
    let mut out_exit = exit.clone();
    let mut out_displaced = displaced.clone();
    let custom_enter = matches!(
        spec_table.get::<Value>("add").map_err(|e| e.to_string())?,
        Value::Table(_)
    );
    for (slot_name, is_exit, is_displaced) in [
        ("add", false, false),
        ("remove", true, false),
        ("displaced", false, true),
    ] {
        let slot_value: Value = spec_table.get(slot_name).map_err(|e| e.to_string())?;
        let Value::Table(slot) = slot_value else {
            if slot_value == Value::Nil {
                continue;
            }
            return Err(format!(
                "transitions().{slot_name} must be a table, got {}",
                lua_value_kind(&slot_value)
            ));
        };
        if is_displaced {
            out_displaced.duration = slot_duration(&slot, slot_name, displaced.duration)?;
            continue;
        }
        if is_exit {
            out_exit.to.x = axis_number(&slot, slot_name, "x", "to", exit.to.x)?;
            out_exit.to.y = axis_number(&slot, slot_name, "y", "to", exit.to.y)?;
            out_exit.to.opacity = axis_number(&slot, slot_name, "opacity", "to", exit.to.opacity)?;
            out_exit.duration = slot_duration(&slot, slot_name, exit.duration)?;
        } else {
            out_enter.from.x = axis_number(&slot, slot_name, "x", "from", enter.from.x)?;
            out_enter.from.y = axis_number(&slot, slot_name, "y", "from", enter.from.y)?;
            out_enter.from.opacity =
                axis_number(&slot, slot_name, "opacity", "from", enter.from.opacity)?;
            out_enter.to.x = axis_number(&slot, slot_name, "x", "to", enter.to.x)?;
            out_enter.to.y = axis_number(&slot, slot_name, "y", "to", enter.to.y)?;
            out_enter.to.opacity =
                axis_number(&slot, slot_name, "opacity", "to", enter.to.opacity)?;
            out_enter.duration = slot_duration(&slot, slot_name, enter.duration)?;
        }
    }
    Ok((out_enter, out_exit, out_displaced, custom_enter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::widgets::{load_widget_script, new_widget_lua};

    #[test]
    fn parse_transitions_reads_qml_subset_and_defaults() {
        let enter = Transition::slide_fade(16.0);
        let exit = Transition::slide_fade_out(-16.0);
        let displaced = Transition {
            from: crate::ui::anim::ItemMotion::settled(),
            to: crate::ui::anim::ItemMotion::settled(),
            duration: None,
        };
        let lua = new_widget_lua().unwrap();
        let (e, x, d, _) = parse_transitions(&lua, &enter, &exit, &displaced).unwrap();
        assert_eq!(e.from.x, 16.0);
        assert_eq!(x.to.x, -16.0);
        assert!(d.duration.is_none());
        load_widget_script(&lua, "transitions", r#"local app={view=function() return ui.text('x') end}; function app:transitions() return {
            add={x={from=200,to=0},opacity={from=0,to=1},duration=250}, remove={x={to=-200},duration=250}, displaced={duration=300}
        } end; return app"#).unwrap();
        let (e, x, d, custom) = parse_transitions(&lua, &enter, &exit, &displaced).unwrap();
        assert!(custom);
        assert_eq!((e.from.x, e.to.x, e.from.opacity), (200.0, 0.0, 0.0));
        assert_eq!(x.to.x, -200.0);
        assert_eq!(
            (e.duration, x.duration, d.duration),
            (
                Some(Duration::from_millis(250)),
                Some(Duration::from_millis(250)),
                Some(Duration::from_millis(300))
            )
        );
        load_widget_script(&lua, "transitions", "return {view=function() return '' end, transitions=function() return {add={x={from=50}}} end}").unwrap();
        let (e, x, d, _) = parse_transitions(&lua, &enter, &exit, &displaced).unwrap();
        assert_eq!(e.from.x, 50.0);
        assert_eq!(e.to.x, 0.0);
        assert_eq!(x.to.x, -16.0);
        assert!(d.duration.is_none());
        load_widget_script(&lua, "transitions", "return {view=function() return '' end, transitions=function() return {add={x='nope'}} end}").unwrap();
        let error = parse_transitions(&lua, &enter, &exit, &displaced).unwrap_err();
        assert!(error.contains("add"), "{error}");
        load_widget_script(
            &lua,
            "transitions",
            "return {view=function() return '' end, transitions=function() return nil end}",
        )
        .unwrap();
        assert!(parse_transitions(&lua, &enter, &exit, &displaced).is_ok());
    }
}
