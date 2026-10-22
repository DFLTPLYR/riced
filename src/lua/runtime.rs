//! M0 host for the declarative Lua application runtime.
//!
//! One VM, one app environment, bounded synchronous entry points, owned
//! cached IR. Lua never receives an iced Element. The first milestone
//! reuses the existing owned WidgetNode decoder/realizer; M1 replaces
//! that adapter with ui::ir/RealizeCtx and designs borrowed widget state.
use super::{bridge, composable, sandbox};

use super::props::component_props;
use crate::ui::dsl::inject_ui_base;
use crate::ui::{
    decode::{parse_node, parse_node_hovered},
    node::WidgetNode as Node,
};
use mlua::{Function, IntoLua, Lua, LuaSerdeExt, RegistryKey, Table, Value};
use std::{collections::HashMap, path::Path, sync::Arc};

fn component_node(lua: &Lua, app: &Table, overrides: Value, host: Value) -> mlua::Result<Value> {
    let props = component_props(lua, app, overrides, host)?;
    app.set("props", props.clone())?;
    // Forward row hover into nested components so a delegate can render
    // `riced.component("card.lua", item)` while the card reads `self.hovered`.
    let hovered = props
        .get::<Value>("hovered")
        .is_ok_and(|value| value == Value::Boolean(true));
    app.set("hovered", hovered)?;
    let result = (|| {
        let node = super::entry::invoke(
            lua,
            app.clone(),
            "view",
            mlua::MultiValue::from_vec(vec![Value::Table(props)]),
        )?;
        let hover = lua
            .app_data_ref::<crate::ui::hover::HoverSet>()
            .map(|hover| hover.clone())
            .unwrap_or_default();
        let decoded = parse_node_hovered(&node, &hover).map_err(mlua::Error::RuntimeError)?;
        // Export resolved descriptions: user delegates and callback captures
        // must run once, not again when the parent validates its subtree.
        crate::ui::encode::to_table(lua, &decoded).map(Value::Table)
    })();
    app.raw_set("props", Value::Nil)?;
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

/// Owned boundary messages, including native control keys and typed values.
#[derive(Debug, Clone)]
pub enum Message {
    Action(crate::ui::action::Action),
    LuaEvent {
        handler: String,
        payload: serde_json::Value,
    },
    Reload,
    Tick,
}

pub use super::error::LuaError;

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
        Self::with_profile(sandbox::Profile::App, components)
    }

    pub(crate) fn with_profile(
        profile: sandbox::Profile,
        components: Vec<(std::path::PathBuf, String)>,
    ) -> mlua::Result<Self> {
        let lua = sandbox::new_lua(profile)?;
        inject_ui_base(&lua)?;
        // The application host shares the same bundled pure component
        // definitions as the shell's module widgets.
        super::library::install(&lua, profile, components)?;
        let api = lua.create_table()?;
        lua.set_named_registry_value("riced.component_modules", lua.create_table()?)?;
        lua.set_named_registry_value("riced.component_sources", lua.create_table()?)?;
        api.set(
            "component",
            lua.create_function(|lua, (src, overrides, host): (String, Value, Value)| {
                let path = composable::source_path(&src);
                let modules: Table = lua.named_registry_value("riced.component_modules")?;
                let cached: Option<Table> = modules.get(src.as_str())?;
                // Snapshot each source once per host entry. Repeated cards see
                // the same source, while the next entry still observes edits.
                let sources: Table = lua.named_registry_value("riced.component_sources")?;
                let source = (|| -> mlua::Result<String> {
                    if let Some(source) = sources.raw_get::<Option<String>>(src.as_str())? {
                        return Ok(source);
                    }
                    let source = std::fs::read_to_string(&path).map_err(mlua::Error::external)?;
                    sources.raw_set(src.as_str(), source.as_str())?;
                    Ok(source)
                })();
                let attempt = (|| -> mlua::Result<(Table, Value, String)> {
                    let source = source?;
                    let app: Table = if let Some(cached) = &cached
                        && cached.get::<String>("source")? == source
                    {
                        cached.get("app")?
                    } else {
                        let env = lua.create_table()?;
                        let meta = lua.create_table()?;
                        meta.set("__index", lua.globals())?;
                        env.set_metatable(Some(meta))?;
                        env.set("_G", env.clone())?;
                        lua.load(&source)
                            .set_name(format!("@{}", path.display()))
                            .set_environment(env)
                            .eval()?
                    };
                    let node = component_node(lua, &app, overrides.clone(), host.clone())?;
                    Ok((app, node, source))
                })();
                match attempt {
                    Ok((app, node, source)) => {
                        let entry = lua.create_table()?;
                        entry.set("source", source)?;
                        entry.set("app", app)?;
                        modules.set(src, entry)?;
                        Ok(node)
                    }
                    Err(error) => {
                        let Some(cached) = cached else {
                            return Err(error);
                        };
                        let message = error.to_string();
                        if cached.get::<Option<String>>("error")?.as_deref() != Some(&message) {
                            eprintln!("component {}: {message}", path.display());
                            cached.set("error", message)?;
                        }
                        component_node(lua, &cached.get::<Table>("app")?, overrides, host)
                    }
                }
            })?,
        )?;
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
            version: 0,
            views: HashMap::new(),
            last_error: None,
        })
    }

    fn begin(&self) -> mlua::Result<()> {
        self.lua.expire_registry_values();
        self.lua.remove_app_data::<crate::ui::hover::HoverSet>();
        self.lua
            .set_named_registry_value("riced.component_sources", self.lua.create_table()?)?;
        sandbox::reset_budget(&self.lua)?;
        self.lua
            .set_named_registry_value("riced.effects", self.lua.create_table()?)
    }

    fn finish<T>(&mut self, entry: &str, result: mlua::Result<T>) -> mlua::Result<T> {
        // Entry-local props must not keep opaque resources alive when either
        // the Lua view or Rust decoder fails. Preserve the original failure.
        let cleanup = self.clear_entry_props();
        let result = result.and_then(|value| cleanup.map(|()| value));
        match result {
            Ok(value) => {
                self.last_error = None;
                Ok(value)
            }
            Err(error) => {
                super::error::record_entry(&mut self.last_error, entry, error.to_string());
                Err(error)
            }
        }
    }

    fn clear_entry_props(&self) -> mlua::Result<()> {
        self.lua.remove_app_data::<crate::ui::hover::HoverSet>();
        if let Some(key) = &self.app {
            self.lua
                .registry_value::<Table>(key)?
                .raw_set("props", Value::Nil)?;
        }
        let modules: Table = self.lua.named_registry_value("riced.component_modules")?;
        for module in modules.pairs::<Value, Table>() {
            let (_, module) = module?;
            module.get::<Table>("app")?.raw_set("props", Value::Nil)?;
        }
        Ok(())
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
            super::widgets::validate_actions(&app)?;
            self.lua.create_registry_value(app)
        })();
        let key = self.finish("load", result)?;
        if let Some(previous) = self.app.replace(key) {
            self.lua.remove_registry_value(previous)?;
        }
        super::callbacks::new_epoch(&self.lua)?;
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
        let handler = match message {
            Message::LuaEvent { handler, .. } => handler.as_str(),
            Message::Action(_) if self.has_handler("actions") => "actions",
            _ => return Ok(Vec::new()),
        };
        let result = (|| {
            self.begin()?;
            let app: Table = self.lua.registry_value(
                self.app
                    .as_ref()
                    .ok_or_else(|| mlua::Error::RuntimeError("No Lua app loaded".into()))?,
            )?;
            // Serialization values exist only for this entry; they never enter IR.
            let arguments = match message {
                Message::Action(action) => action.arguments(&self.lua)?,
                Message::LuaEvent { payload, .. } => {
                    mlua::MultiValue::from_vec(vec![self.lua.to_value(payload)?])
                }
                _ => unreachable!("handler selected above"),
            };
            super::entry::invoke(&self.lua, app, handler, arguments)?;
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
            self.lua
                .set_named_registry_value("riced.animation_host", format!("window:{}", window.0))?;
            let app: Table = self.lua.registry_value(
                self.app
                    .as_ref()
                    .ok_or_else(|| mlua::Error::RuntimeError("No Lua app loaded".into()))?,
            )?;
            let returned = super::entry::invoke(
                &self.lua,
                app,
                "view",
                mlua::MultiValue::from_vec(vec![window.0.into_lua(&self.lua)?]),
            )?;
            parse_node(&returned).map_err(mlua::Error::RuntimeError)
        })();
        let node = self.finish("view", result)?;
        self.views.insert(window, (self.version, node.clone()));
        Ok(node)
    }

    pub fn cached_view(&self, window: WindowId) -> Option<&Node> {
        self.views.get(&window).map(|(_, node)| node)
    }

    /// Scene hosts publish fresh services and owned input tables at each entry.
    /// Layout/animation frames realize cached IR and never execute Lua.
    pub(crate) fn scene_view(
        &mut self,
        services: &crate::services::ServiceCtx<'_>,
        output: &str,
        props: impl FnOnce(&Lua) -> mlua::Result<Table>,
        hover: &crate::ui::hover::HoverSet,
    ) -> mlua::Result<Node> {
        let result = (|| {
            self.begin()?;
            self.lua.set_app_data(hover.clone());
            self.lua
                .set_named_registry_value("riced.animation_host", format!("output:{output}"))?;
            crate::services::publish_all(services, &self.lua)?;
            crate::services::publish_bar(&self.lua, output)?;
            let app: Table = self.lua.registry_value(
                self.app
                    .as_ref()
                    .ok_or_else(|| mlua::Error::RuntimeError("scene missing".into()))?,
            )?;
            let props = super::props::component_props(
                &self.lua,
                &app,
                Value::Table(props(&self.lua)?),
                Value::Nil,
            )?;
            app.set("props", props.clone())?;
            let value = super::entry::invoke(
                &self.lua,
                app,
                "view",
                mlua::MultiValue::from_vec(vec![Value::Table(props)]),
            )?;
            let node = parse_node_hovered(&value, hover).map_err(mlua::Error::RuntimeError)?;
            self.clear_entry_props()?;
            self.lua.gc_collect()?;
            Ok(node)
        })();
        self.finish("view", result)
    }

    pub(crate) fn scene_action(
        &mut self,
        services: &crate::services::ServiceCtx<'_>,
        output: &str,
        action: &crate::ui::action::Action,
    ) -> mlua::Result<Value> {
        if !self.has_handler("actions") {
            return Ok(Value::Nil);
        }
        let result = (|| {
            self.begin()?;
            crate::services::publish_all(services, &self.lua)?;
            crate::services::publish_bar(&self.lua, output)?;
            let app = self.lua.registry_value(
                self.app
                    .as_ref()
                    .ok_or_else(|| mlua::Error::RuntimeError("scene missing".into()))?,
            )?;
            super::entry::invoke(&self.lua, app, "actions", action.arguments(&self.lua)?)
        })();
        self.finish("actions", result)
    }

    pub(crate) fn scene_tick(
        &mut self,
        services: &crate::services::ServiceCtx<'_>,
        output: &str,
    ) -> mlua::Result<()> {
        let result = (|| {
            self.begin()?;
            crate::services::publish_all(services, &self.lua)?;
            crate::services::publish_bar(&self.lua, output)?;
            let app = self.lua.registry_value(
                self.app
                    .as_ref()
                    .ok_or_else(|| mlua::Error::RuntimeError("scene missing".into()))?,
            )?;
            super::entry::invoke(&self.lua, app, "on_tick", mlua::MultiValue::new())?;
            Ok(())
        })();
        self.finish("on_tick", result)
    }

    /// Composable entry: Lua defaults < config overrides < host-owned state.
    /// The same resolved table is available as both `props` and `self.props`.
    pub(crate) fn component_view(
        &mut self,
        overrides: &serde_json::Value,
        host: &serde_json::Value,
        theme: &crate::config::ThemeConfig,
        hovered: bool,
    ) -> mlua::Result<(Node, serde_json::Value)> {
        let result = (|| {
            self.begin()?;
            self.publish_theme(theme)?;
            let app: Table =
                self.lua.registry_value(self.app.as_ref().ok_or_else(|| {
                    mlua::Error::RuntimeError("No composable app loaded".into())
                })?)?;
            let props = component_props(
                &self.lua,
                &app,
                self.lua.to_value(overrides)?,
                self.lua.to_value(host)?,
            )?;
            let mut resolved = serde_json::Map::new();
            for pair in props.pairs::<String, Value>() {
                let (key, value) = pair?;
                resolved.insert(key, self.lua.from_value(value)?);
            }
            let resolved = serde_json::Value::Object(resolved);
            app.set("props", props.clone())?;
            app.set("hovered", hovered)?;
            let returned = super::entry::invoke(
                &self.lua,
                app,
                "view",
                mlua::MultiValue::from_vec(vec![Value::Table(props)]),
            )?;
            let node = parse_node(&returned).map_err(mlua::Error::RuntimeError)?;
            Ok((node, resolved))
        })();
        // The chrome host deduplicates errors across animation ticks.
        let cleanup = self.clear_entry_props();
        result.and_then(|value| cleanup.map(|()| value))
    }

    pub(crate) fn publish_theme(&self, theme: &crate::config::ThemeConfig) -> mlua::Result<()> {
        bridge::publish_theme(&self.lua, theme)
    }

    pub(crate) fn publish_component_defaults(
        &self,
        defaults: &crate::ui::settings::ComponentDefaults,
    ) -> mlua::Result<()> {
        crate::ui::settings::publish(&self.lua, defaults)
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
    #[test]
    fn nested_list_delegates_run_once_and_keep_host_hover() {
        use super::*;
        let root = std::env::temp_dir().join(format!("riced-once-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let child = root.join("list.lua");
        std::fs::write(&child, "return {calls=0, view=function(self) return iced.listview({{id='a'}}):id('inner'):key('id'):delegate(function(item) self.calls=self.calls+1; return iced.text((item.hovered and 'hot:' or 'cold:') .. self.calls) end) end}").unwrap();
        let mut runtime = LuaRuntime::new().unwrap();
        runtime
            .load(&format!(
                "return {{view=function() return riced.component({:?}) end}}",
                child.to_string_lossy()
            ))
            .unwrap();
        runtime.lua.set_app_data(crate::ui::hover::HoverSet::from([(
            "inner".into(),
            "a".into(),
        )]));
        // Match scene_view's entry setup without requiring live services.
        let app: Table = runtime
            .lua
            .registry_value(runtime.app.as_ref().unwrap())
            .unwrap();
        let value = super::super::entry::invoke(&runtime.lua, app, "view", mlua::MultiValue::new())
            .unwrap();
        let hover = crate::ui::hover::HoverSet::from([("inner".into(), "a".into())]);
        let node = parse_node_hovered(&value, &hover).unwrap();
        let Node::ListView { items, .. } = node else {
            panic!("list")
        };
        assert!(matches!(&items[0].1, Node::Text {content, ..} if content == "hot:1"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_views_clear_entry_props_and_preserve_original_errors() {
        use super::*;
        let mut runtime = LuaRuntime::new().unwrap();
        let retained = runtime.lua.create_table().unwrap();
        retained
            .set("props", runtime.lua.create_table().unwrap())
            .unwrap();
        let module = runtime.lua.create_table().unwrap();
        module.set("app", retained.clone()).unwrap();
        runtime
            .lua
            .named_registry_value::<Table>("riced.component_modules")
            .unwrap()
            .set("retained", module)
            .unwrap();
        runtime.load("return {view=function(self) self.props={resource='temporary'}; error('original failure') end}").unwrap();
        assert!(
            runtime
                .view(WindowId(1))
                .unwrap_err()
                .to_string()
                .contains("original failure")
        );
        let app: Table = runtime
            .lua
            .registry_value(runtime.app.as_ref().unwrap())
            .unwrap();
        assert_eq!(app.raw_get::<Value>("props").unwrap(), Value::Nil);
        assert_eq!(retained.raw_get::<Value>("props").unwrap(), Value::Nil);
    }

    #[test]
    fn component_sources_are_snapshotted_per_entry_and_reload_between_entries() {
        use super::*;
        let root =
            std::env::temp_dir().join(format!("riced-source-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let child = root.join("card.lua");
        let source =
            |label: &str| format!("return {{view=function() return iced.text('{label}') end}}");
        std::fs::write(&child, source("old")).unwrap();
        let mut runtime = LuaRuntime::new().unwrap();
        let rewrite_path = child.clone();
        runtime
            .lua
            .globals()
            .set(
                "rewrite",
                runtime
                    .lua
                    .create_function(move |_, ()| {
                        std::fs::write(
                            &rewrite_path,
                            "return {view=function() return iced.text('new') end}",
                        )
                        .map_err(mlua::Error::external)
                    })
                    .unwrap(),
            )
            .unwrap();
        runtime.load(&format!("return {{view=function() local first=riced.component({0:?}); rewrite(); return iced.row({{first,riced.component({0:?})}}) end}}", child.to_string_lossy())).unwrap();
        let Node::Row { children, .. } = runtime.view(WindowId(1)).unwrap() else {
            panic!("row")
        };
        assert!(
            children
                .iter()
                .all(|node| matches!(node, Node::Text {content, ..} if content == "old"))
        );
        let Node::Row { children, .. } = runtime.view(WindowId(2)).unwrap() else {
            panic!("row")
        };
        assert!(
            children
                .iter()
                .all(|node| matches!(node, Node::Text {content, ..} if content == "new"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn control_actions_update_owned_app_state_and_invalidate_cached_views() {
        use crate::ui::action::{Action, ActionValue};
        let mut runtime = super::LuaRuntime::new().unwrap();
        runtime.load("return {value=0, view=function(self) return iced.slider(0,100,self.value):on_change('volume') end, actions=function(self,events) assert(events.type=='action' and events.key=='volume'); self.value=events.value end}").unwrap();
        let window = super::WindowId(1);
        runtime.view(window).unwrap();
        runtime
            .update(&super::Message::Action(Action::change(
                "volume".into(),
                ActionValue::Number(42.0),
            )))
            .unwrap();
        assert!(matches!(
            runtime.view(window).unwrap(),
            crate::ui::node::WidgetNode::Slider { value: 42.0, .. }
        ));
    }
    use super::*;

    #[test]
    fn nested_components_observe_forwarded_hovered() {
        let root = std::env::temp_dir().join(format!("riced-nested-hover-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let child = root.join("card.lua");
        std::fs::write(
            &child,
            "return {view=function(self) return iced.text(self.hovered and 'hot' or 'cold') end}",
        )
        .unwrap();
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.load(&format!("local app={{}}; function app:view() return riced.component({:?}, {{hovered=true}}) end; return app", child.to_string_lossy())).unwrap();
        assert!(matches!(
            runtime.view(WindowId(1)).unwrap(),
            crate::ui::node::WidgetNode::Text { content, .. } if content == "hot"
        ));
        runtime.load(&format!("local app={{}}; function app:view() return riced.component({:?}, {{}}) end; return app", child.to_string_lossy())).unwrap();
        assert!(matches!(
            runtime.view(WindowId(2)).unwrap(),
            crate::ui::node::WidgetNode::Text { content, .. } if content == "cold"
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nested_components_cannot_reset_the_outer_entry_budget() {
        let root = std::env::temp_dir().join(format!("riced-nested-budget-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let child = root.join("child.lua");
        std::fs::write(
            &child,
            "return {view=function(self,props) return iced.space() end}",
        )
        .unwrap();
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.load(&format!("local app={{}}; function app:view(id) while true do riced.component({:?}, {{}}, {{}}) end end; return app", child.to_string_lossy())).unwrap();
        assert!(
            runtime
                .view(WindowId(1))
                .unwrap_err()
                .to_string()
                .contains("instruction budget exceeded")
        );
        runtime
            .load("return {view=function(self,id) return iced.text('recovered') end}")
            .unwrap();
        assert!(runtime.view(WindowId(1)).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn dirty_views_cache_owned_ir_and_updates_receive_serialized_payloads() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.load("local app = {calls=0, count=0}; function app:view(w) self.calls=self.calls+1; return iced.text(self.count .. ':' .. self.calls) end; function app:increment(n) self.count=self.count+n.delta; riced.invalidate() end; return app").unwrap();
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
            .load("return { view=function() return iced.text('recovered') end }")
            .unwrap();
        assert!(runtime.view(WindowId(1)).is_ok());
        assert!(runtime.last_error.is_none());
    }
    #[test]
    fn broken_reload_preserves_last_app_and_reports_traceback() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime
            .load("return { view=function() return iced.text('good') end }")
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
            .load(
                "private_value='old'; return {view=function() return iced.text(private_value) end}",
            )
            .unwrap();
        runtime.view(WindowId(1)).unwrap();
        runtime.load("return {view=function() return iced.text(private_value == nil and 'isolated' or 'leaked') end}").unwrap();
        assert!(
            matches!(runtime.view(WindowId(1)).unwrap(), Node::Text { content, .. } if content == "isolated")
        );
    }

    #[test]
    fn allocation_limit_becomes_a_runtime_error() {
        let mut runtime = LuaRuntime::new().unwrap();
        assert!(runtime.load("local huge = string.rep('x', 80 * 1024 * 1024); return {view=function() return iced.text(huge) end}").is_err());
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
        runtime.load("return {view=function() return iced.card({title='Shared components',body=iced.space():height(8)}) end}").unwrap();
        assert!(matches!(
            runtime.view(WindowId(1)).unwrap(),
            Node::Column { .. }
        ));
    }
}
