//! Bind self and invoke an app method through the common execution boundary.
use mlua::{Function, Lua, MultiValue, Table, Value};

// Only outer entries reset fuel; nested components share their caller's budget.
pub(crate) fn invoke(
    _lua: &Lua,
    app: Table,
    method: &str,
    mut args: MultiValue,
) -> mlua::Result<Value> {
    let callback: Function = app.get(method)?;
    args.push_front(Value::Table(app));
    callback.call(args)
}
