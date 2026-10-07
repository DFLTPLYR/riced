//! `bar` table: where the currently rendering bar lives.
//!
//! Unlike the snapshot services, this is per render call, not per
//! tick: each bar publishes its own output before `view()`/`popup()`/
//! `on_action()`, so one shared widget state still renders per-bar
//! output. Notification cards have no bar — `bar` stays nil there.

/// Publish `bar = { output = "<connector>" }`, nil when the output is
/// unknown (startup sentinel bars).
pub fn publish(lua: &mlua::Lua, output: &str) -> mlua::Result<()> {
    let table = lua.create_table()?;
    if output.is_empty() {
        table.set("output", mlua::Value::Nil)?;
    } else {
        table.set("output", output)?;
    }
    lua.globals().set("bar", table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_table_carries_output_or_nil() {
        let lua = mlua::Lua::new();
        publish(&lua, "DP-1").expect("publish");
        let output: String = lua.load("return bar.output").eval().expect("output");
        assert_eq!(output, "DP-1");
        publish(&lua, "").expect("publish");
        let nil: mlua::Value = lua.load("return bar.output").eval().expect("nil");
        assert!(matches!(nil, mlua::Value::Nil));
    }
}
