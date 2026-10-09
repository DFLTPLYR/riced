//! Decode motion descriptions independently of bar/window ownership.
use super::value::lua_value_kind;
use crate::ui::listview::Transition;
use mlua::{Table, Value};
use std::time::Duration;

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
