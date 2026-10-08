//! M0 host for the declarative Lua application runtime.
//!
//! One VM, one app environment, bounded synchronous entry points, owned
//! cached IR. Lua never receives an iced Element. The first milestone
//! reuses the existing owned WidgetNode decoder/realizer; M1 replaces
//! that adapter with ui::ir/RealizeCtx and designs borrowed widget state.
pub mod composable;
pub mod demo;

use crate::app::layers::top::{WidgetNode as Node, inject_ui_base, parse_node};
use mlua::{
    Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, RegistryKey, StdLib, Table, Value,
    VmState,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const INSTRUCTION_LIMIT: usize = 200_000;
const HOOK_INTERVAL: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

/// Owned boundary messages. Callback dispatch is introduced in M2.
#[derive(Debug, Clone)]
pub enum Message {
    LuaEvent {
        handler: String,
        payload: serde_json::Value,
    },
    Reload,
    Tick,
}

#[derive(Debug, Clone)]
pub struct LuaError {
    pub entry: String,
    pub message: String,
}

/// Effect/Subscription payloads are Rust-owned. M4 extends these into
/// executor tasks; M0 only needs explicit dirty invalidation.
#[derive(Debug, Clone)]
pub enum Effect {
    Invalidate,
}

/// M0 exposes the subscription boundary but has no user subscription
/// variants. M4 adds timer/event descriptors here.
#[derive(Debug, Clone)]
pub enum SubSpec {}

pub struct LuaRuntime {
    lua: Lua,
    app: Option<RegistryKey>,
    budget: Arc<AtomicUsize>,
    version: u64,
    views: HashMap<WindowId, (u64, Node)>,
    pub last_error: Option<Arc<LuaError>>,
}

impl LuaRuntime {
    pub fn new() -> mlua::Result<Self> {
        Self::with_components(crate::config::builtin_component_files())
    }

    pub(crate) fn with_components(
        components: Vec<(std::path::PathBuf, String)>,
    ) -> mlua::Result<Self> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
            LuaOptions::default(),
        )?;
        lua.set_memory_limit(MEMORY_LIMIT)?;
        let budget = Arc::new(AtomicUsize::new(INSTRUCTION_LIMIT));
        let remaining = budget.clone();
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(HOOK_INTERVAL as u32),
            move |_, _| {
                let before = remaining.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                    left.checked_sub(HOOK_INTERVAL)
                });
                if before.is_err() {
                    Err(mlua::Error::RuntimeError(
                        "Lua instruction budget exceeded".into(),
                    ))
                } else {
                    Ok(VmState::Continue)
                }
            },
        )?;
        inject_ui_base(&lua)?;
        // The application host shares the same bundled pure component
        // definitions as the shell's module widgets.
        for (path, source) in components {
            budget.store(INSTRUCTION_LIMIT, Ordering::Relaxed);
            lua.load(&source)
                .set_name(format!("@{}", path.display()))
                .exec()?;
        }
        let api = lua.create_table()?;
        api.set(
            "invalidate",
            lua.create_function(|lua, ()| {
                let effects: Table = lua.named_registry_value("riced.effects")?;
                effects.set(effects.raw_len() + 1, "invalidate")
            })?,
        )?;
        lua.globals().set("riced", api)?;
        lua.set_named_registry_value("riced.effects", lua.create_table()?)?;
        Ok(Self {
            lua,
            app: None,
            budget,
            version: 0,
            views: HashMap::new(),
            last_error: None,
        })
    }

    fn begin(&self) -> mlua::Result<()> {
        self.budget.store(INSTRUCTION_LIMIT, Ordering::Relaxed);
        self.lua
            .set_named_registry_value("riced.effects", self.lua.create_table()?)
    }

    fn finish<T>(&mut self, entry: &str, result: mlua::Result<T>) -> mlua::Result<T> {
        match result {
            Ok(value) => {
                self.last_error = None;
                Ok(value)
            }
            Err(error) => {
                let message = error.to_string();
                tracing::error!(entry, error = %message, "Lua entry failed");
                self.last_error = Some(Arc::new(LuaError {
                    entry: entry.into(),
                    message,
                }));
                Err(error)
            }
        }
    }

    /// Evaluate a candidate in its own environment before replacing the
    /// current app. Keep the last app and IR when a reload fails.
    pub fn load(&mut self, script: &str) -> mlua::Result<()> {
        self.load_named(script, "main.lua")
    }

    pub fn load_named(&mut self, script: &str, label: &str) -> mlua::Result<()> {
        let result = (|| {
            self.begin()?;
            let env = self.lua.create_table()?;
            let meta = self.lua.create_table()?;
            meta.set("__index", self.lua.globals())?;
            env.set_metatable(Some(meta))?;
            // _G is local to this app environment, not the shared API table.
            env.set("_G", env.clone())?;
            let app: Table = self
                .lua
                .load(script)
                .set_name(format!("@{label}"))
                .set_environment(env)
                .eval()?;
            let _: Function = app.get("view")?;
            self.lua.create_registry_value(app)
        })();
        let key = self.finish("load", result)?;
        if let Some(previous) = self.app.replace(key) {
            self.lua.remove_registry_value(previous)?;
        }
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    pub fn load_path(&mut self, path: &Path) -> mlua::Result<()> {
        let source = std::fs::read_to_string(path).map_err(mlua::Error::external);
        match source {
            Ok(source) => self.load_named(&source, &path.display().to_string()),
            Err(error) => self.finish("load", Err(error)),
        }
    }

    pub fn update(&mut self, message: &Message) -> mlua::Result<Vec<Effect>> {
        let Message::LuaEvent { handler, payload } = message else {
            return Ok(Vec::new());
        };
        let result = (|| {
            self.begin()?;
            let app: Table = self.lua.registry_value(
                self.app
                    .as_ref()
                    .ok_or_else(|| mlua::Error::RuntimeError("No Lua app loaded".into()))?,
            )?;
            let callback: Function = app.get(handler.as_str())?;
            // Serialization values exist only for this entry; they never enter IR.
            callback.call::<()>((app, self.lua.to_value(payload)?))?;
            let pending: Table = self.lua.named_registry_value("riced.effects")?;
            let mut effects = Vec::new();
            for effect in pending.sequence_values::<String>() {
                if effect? == "invalidate" {
                    effects.push(Effect::Invalidate);
                }
            }
            Ok(effects)
        })();
        // Conservatively dirty after every handler, including failures
        // that may have partially mutated app state.
        self.version = self.version.wrapping_add(1);
        self.finish(handler, result)
    }

    pub fn view(&mut self, window: WindowId) -> mlua::Result<Node> {
        if let Some((version, node)) = self.views.get(&window)
            && *version == self.version
        {
            return Ok(node.clone());
        }
        let result = (|| {
            self.begin()?;
            let app: Table = self.lua.registry_value(
                self.app
                    .as_ref()
                    .ok_or_else(|| mlua::Error::RuntimeError("No Lua app loaded".into()))?,
            )?;
            let view: Function = app.get("view")?;
            let returned: Value = view.call((app, window.0))?;
            parse_node(&returned).map_err(mlua::Error::RuntimeError)
        })();
        let node = self.finish("view", result)?;
        self.views.insert(window, (self.version, node.clone()));
        Ok(node)
    }

    pub fn cached_view(&self, window: WindowId) -> Option<&Node> {
        self.views.get(&window).map(|(_, node)| node)
    }

    /// Composable entry: Lua defaults < config overrides < host-owned state.
    /// The same resolved table is available as both `props` and `self.props`.
    pub(crate) fn component_view(
        &mut self,
        overrides: &serde_json::Value,
        host: &serde_json::Value,
        theme: &crate::config::ThemeConfig,
    ) -> mlua::Result<(Node, serde_json::Value)> {
        let result = (|| {
            self.begin()?;
            self.publish_theme(theme)?;
            let app: Table =
                self.lua.registry_value(self.app.as_ref().ok_or_else(|| {
                    mlua::Error::RuntimeError("No composable app loaded".into())
                })?)?;
            let defaults: Value = app.get("defaults")?;
            let mut props = if defaults == Value::Nil {
                serde_json::Map::new()
            } else {
                let Value::Table(defaults) = defaults else {
                    return Err(mlua::Error::RuntimeError(
                        "composable defaults must be a props table".into(),
                    ));
                };
                let mut props = serde_json::Map::new();
                for entry in defaults.pairs::<String, Value>() {
                    let (key, value) = entry?;
                    props.insert(key, self.lua.from_value(value)?);
                }
                props
            };
            for source in [overrides, host] {
                if let Some(values) = source.as_object() {
                    props.extend(values.clone());
                }
            }
            let resolved = serde_json::Value::Object(props);
            let props = self.lua.to_value(&resolved)?;
            app.set("props", props.clone())?;
            let view: Function = app.get("view")?;
            let returned: Value = view.call((app, props))?;
            let node = parse_node(&returned).map_err(mlua::Error::RuntimeError)?;
            Ok((node, resolved))
        })();
        // The chrome host deduplicates errors across animation ticks.
        result
    }

    pub(crate) fn publish_theme(&self, theme: &crate::config::ThemeConfig) -> mlua::Result<()> {
        let palette = self.lua.create_table()?;
        for (key, color) in crate::theme::lua_palette(theme) {
            palette.set(key, color)?;
        }
        self.lua.globals().set("theme", palette)
    }

    pub fn has_handler(&self, name: &str) -> bool {
        self.app
            .as_ref()
            .and_then(|key| self.lua.registry_value::<Table>(key).ok())
            .and_then(|app| app.raw_get::<Value>(name).ok())
            .is_some_and(|value| matches!(value, Value::Function(_)))
    }

    /// M0 has no user subscription bridge yet; tick/GC remains host-owned.
    pub fn subscriptions(&mut self) -> Vec<SubSpec> {
        Vec::new()
    }

    pub fn tick(&mut self) {
        if let Err(error) = self.lua.gc_step() {
            let _ = self.finish::<()>("gc", Err(error));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dirty_views_cache_owned_ir_and_updates_receive_serialized_payloads() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.load("local app = {calls=0, count=0}; function app:view(w) self.calls=self.calls+1; return ui.text(self.count .. ':' .. self.calls) end; function app:increment(n) self.count=self.count+n.delta; riced.invalidate() end; return app").unwrap();
        assert!(runtime.has_handler("increment"));
        assert!(!runtime.has_handler("missing"));
        let a = runtime.view(WindowId(1)).unwrap();
        assert_eq!(a, runtime.view(WindowId(1)).unwrap());
        let effects = runtime
            .update(&Message::LuaEvent {
                handler: "increment".into(),
                payload: serde_json::json!({"delta": 3}),
            })
            .unwrap();
        assert_eq!(effects.len(), 1);
        let node = runtime.view(WindowId(1)).unwrap();
        assert!(matches!(node, Node::Text { content, .. } if content == "3:2"));
    }
    #[test]
    fn runaway_entries_error_and_runtime_recovers() {
        let mut runtime = LuaRuntime::new().unwrap();
        assert!(runtime.load("while true do end").is_err());
        assert!(
            runtime
                .last_error
                .as_ref()
                .unwrap()
                .message
                .contains("budget exceeded")
        );
        runtime
            .load("return { view=function() return ui.text('recovered') end }")
            .unwrap();
        assert!(runtime.view(WindowId(1)).is_ok());
        assert!(runtime.last_error.is_none());
    }
    #[test]
    fn broken_reload_preserves_last_app_and_reports_traceback() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime
            .load("return { view=function() return ui.text('good') end }")
            .unwrap();
        let before = runtime.view(WindowId(1)).unwrap();
        assert!(runtime.load_named("error('broken')", "bad.lua").is_err());
        assert!(
            runtime
                .last_error
                .as_ref()
                .unwrap()
                .message
                .contains("bad.lua")
        );
        assert_eq!(before, runtime.view(WindowId(1)).unwrap());
        assert!(runtime.last_error.is_some());
    }

    #[test]
    fn app_environments_do_not_leak_globals_between_loads() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime
            .load("private_value='old'; return {view=function() return ui.text(private_value) end}")
            .unwrap();
        runtime.view(WindowId(1)).unwrap();
        runtime.load("return {view=function() return ui.text(private_value == nil and 'isolated' or 'leaked') end}").unwrap();
        assert!(
            matches!(runtime.view(WindowId(1)).unwrap(), Node::Text { content, .. } if content == "isolated")
        );
    }

    #[test]
    fn allocation_limit_becomes_a_runtime_error() {
        let mut runtime = LuaRuntime::new().unwrap();
        assert!(runtime.load("local huge = string.rep('x', 80 * 1024 * 1024); return {view=function() return ui.text(huge) end}").is_err());
        assert!(
            runtime
                .last_error
                .as_ref()
                .unwrap()
                .message
                .to_lowercase()
                .contains("memory")
        );
    }

    #[test]
    fn migrated_widgets_use_the_same_app_contract_as_main() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.load(crate::config::SEED_HELLO_LUA).unwrap();
        assert!(
            matches!(runtime.view(WindowId(1)).unwrap(), Node::Text { content, .. } if content == "hello")
        );
        // The application host and widget loader share the bundled builders.
        runtime.load("return {view=function() return ui.card({title='Shared components',body=ui.space():height(8)}) end}").unwrap();
        assert!(matches!(
            runtime.view(WindowId(1)).unwrap(),
            Node::Column { .. }
        ));
    }
}
