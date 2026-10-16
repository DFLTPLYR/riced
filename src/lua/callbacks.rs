//! Opaque registry tokens for completion handlers. Tokens never keep a VM alive.
use mlua::{Function, Lua, RegistryKey, Table, UserData, Value, WeakLua};
use std::sync::Arc;

pub(crate) type ItemCallbacks = Vec<(String, [Option<Callback>; 3])>;

struct Entry {
    lua: WeakLua,
    key: RegistryKey,
    epoch: u64,
    output: String,
}
#[derive(Clone)]
pub(crate) struct Callback(Arc<Entry>);
impl std::fmt::Debug for Callback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("CompletionCallback")
            .field(&self.0.key)
            .finish()
    }
}
impl PartialEq for Callback {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl UserData for Callback {}

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.set_named_registry_value("riced.callback_epoch", 0_u64)?;
    lua.set_named_registry_value(
        "riced.capture_callback",
        lua.create_function(|lua, (function, item): (Function, Table)| {
            let snapshot = lua.create_table()?;
            for pair in item.pairs::<Value, Value>() {
                let (key, value) = pair?;
                snapshot.raw_set(key, value)?;
            }
            let entry = lua.create_table()?;
            entry.set("function", function)?;
            entry.set("item", snapshot)?;
            let output = lua
                .globals()
                .get::<Table>("bar")
                .ok()
                .and_then(|bar| bar.get::<String>("output").ok())
                .unwrap_or_default();
            lua.create_userdata(Callback(Arc::new(Entry {
                lua: lua.weak(),
                key: lua.create_registry_value(entry)?,
                epoch: lua.named_registry_value("riced.callback_epoch")?,
                output,
            })))
        })?,
    )
}

pub(crate) fn new_epoch(lua: &Lua) -> mlua::Result<()> {
    let epoch: u64 = lua.named_registry_value("riced.callback_epoch")?;
    lua.set_named_registry_value("riced.callback_epoch", epoch.wrapping_add(1))
}

impl Callback {
    pub(crate) fn belongs_to(&self, lua: &Lua) -> bool {
        lua.owns_registry_value(&self.0.key)
    }

    /// Called from the host update phase, never from widget layout/draw. A
    /// replacement notification or app invalidates the old completion.
    pub(crate) fn invoke(
        &self,
        services: &crate::services::ServiceCtx<'_>,
        validate: impl FnOnce(&Table) -> bool,
    ) -> mlua::Result<Option<Value>> {
        let Some(lua) = self.0.lua.try_upgrade() else {
            return Ok(None);
        };
        if lua.named_registry_value::<u64>("riced.callback_epoch")? != self.0.epoch {
            return Ok(None);
        }
        let entry: Table = lua.registry_value(&self.0.key)?;
        let item: Table = entry.get("item")?;
        if !validate(&item) {
            return Ok(None);
        }
        super::sandbox::reset_budget(&lua)?;
        crate::services::publish_all(services, &lua)?;
        crate::services::publish_bar(&lua, &self.0.output)?;
        let function: Function = entry.get("function")?;
        function.call(item).map(Some)
    }
}

#[cfg(test)]
#[path = "callback_tests.rs"]
mod tests;
