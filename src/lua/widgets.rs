//! Owned per-placement VMs, revision tracking, and per-window render caches.
use crate::config::PropValue;
use crate::ui::node::WidgetNode;
use iced::window;
use mlua::{Function, Lua, Table, Value};

pub(crate) struct ResolvedWidget {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub interval: f32,
    pub size: f32,
    pub props: HashMap<String, PropValue>,
}

/// Immutable host inputs published before every placement entry.
pub(crate) struct EntryContext<'a> {
    pub services: crate::services::ServiceCtx<'a>,
    pub output: &'a str,
}

pub(crate) fn call_widget_method(
    lua: &Lua,
    method: &str,
    args: mlua::MultiValue,
) -> Result<Value, String> {
    super::sandbox::reset_budget(lua).map_err(|error| error.to_string())?;
    let app: Table = lua
        .named_registry_value("riced.widget.app")
        .map_err(|error| error.to_string())?;
    super::entry::invoke(lua, app, method, args).map_err(|error| error.to_string())
}

pub(crate) fn call_lua_value(lua: &Lua, method: &str) -> Result<Value, String> {
    call_widget_method(lua, method, mlua::MultiValue::new())
}

pub(crate) fn lua_has_func(states: &HashMap<String, Lua>, id: &str, method: &str) -> bool {
    states.get(id).is_some_and(|lua| {
        lua.named_registry_value::<Table>("riced.widget.app")
            .and_then(|app| app.get::<Function>(method))
            .is_ok()
    })
}

pub(crate) fn call_lua_named_action(lua: &Lua, action: &str) -> Result<Value, String> {
    let app: Table = lua
        .named_registry_value("riced.widget.app")
        .map_err(|error| error.to_string())?;
    if matches!(
        app.get::<Value>("on_action")
            .map_err(|error| error.to_string())?,
        Value::Nil
    ) {
        return Ok(Value::Nil);
    }
    let key = lua
        .create_string(action)
        .map_err(|error| error.to_string())?;
    call_widget_method(
        lua,
        "on_action",
        mlua::MultiValue::from_vec(vec![Value::String(key)]),
    )
}

impl ResolvedWidget {
    pub(crate) fn label(&self) -> String {
        format!("{} [{}]", self.name, self.id)
    }
}

pub(crate) fn new_widget_lua() -> mlua::Result<Lua> {
    let lua = super::sandbox::new_lua(super::sandbox::Profile::Widget)?;
    inject_ui(&lua)?;
    Ok(lua)
}

pub(crate) fn inject_ui(lua: &Lua) -> mlua::Result<()> {
    crate::ui::dsl::inject_ui_base(lua)?;
    super::library::install(
        lua,
        super::sandbox::Profile::Widget,
        crate::config::component_files(),
    )
}

pub(crate) fn load_widget_script(lua: &Lua, label: &str, source: &str) -> mlua::Result<()> {
    super::sandbox::reset_budget(lua)?;
    let returned: Value = lua.load(source).set_name(format!("@{label}")).eval()?;
    let Value::Table(app) = returned else {
        return Err(mlua::Error::RuntimeError(format!(
            "{label}: script must return an app table with a view method"
        )));
    };
    let _: Function = app.get("view").map_err(|_| {
        mlua::Error::RuntimeError(format!("{label}: returned app table needs a view method"))
    })?;
    for method in ["view", "popup", "on_action", "on_press", "transitions"] {
        if !matches!(app.get::<Value>(method)?, Value::Function(_) | Value::Nil) {
            return Err(mlua::Error::RuntimeError(format!(
                "{label}: app.{method} must be a function"
            )));
        }
    }
    lua.set_named_registry_value("riced.widget.app", app)
}
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Instant, SystemTime},
};

#[derive(Debug, Default)]
pub(crate) struct WidgetState {
    pub instances: HashMap<String, Lua>,
    pub outputs: HashMap<(window::Id, String), String>,
    pub trees: HashMap<(window::Id, String), WidgetNode>,
    pub last_run: HashMap<String, Instant>,
    pub errors: HashMap<String, String>,
    pub revisions: HashMap<PathBuf, SystemTime>,
}

impl WidgetState {
    pub(crate) fn forget_window(&mut self, window: window::Id) {
        self.outputs.retain(|(owner, _), _| *owner != window);
        self.trees.retain(|(owner, _), _| *owner != window);
    }

    /// Invalidate every instance using a changed path. Unreadable paths retain
    /// their previous revision and instances, allowing subsequent recovery.
    pub(crate) fn sync_revision<'a>(
        &mut self,
        path: &std::path::Path,
        placement_ids: impl IntoIterator<Item = &'a str>,
    ) -> bool {
        let Ok(revision) = std::fs::metadata(path).and_then(|metadata| metadata.modified()) else {
            return false;
        };
        if self.revisions.get(path) == Some(&revision) {
            return false;
        }
        for id in placement_ids {
            self.instances.remove(id);
        }
        self.revisions.insert(path.to_path_buf(), revision);
        true
    }

    fn publish_entry(
        lua: &Lua,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
    ) -> Result<(), String> {
        crate::services::publish_all(&context.services, lua).map_err(|error| error.to_string())?;
        crate::services::publish_bar(lua, context.output).map_err(|error| error.to_string())?;
        super::props::publish_props(lua, &resolved.props).map_err(|error| error.to_string())
    }

    pub(crate) fn render(
        &mut self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
    ) -> Result<Value, String> {
        self.ensure_instance(resolved)?;
        let lua = &self.instances[&resolved.id];
        Self::publish_entry(lua, resolved, context)?;
        call_lua_value(lua, "view")
    }

    /// Missing instances are stale host messages and are ignored. Actions do
    /// not construct a new VM or reset the placement's existing self state.
    pub(crate) fn action(
        &self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
        action: &str,
    ) -> Option<Result<Value, String>> {
        self.instances.get(&resolved.id).map(|lua| {
            Self::publish_entry(lua, resolved, context)?;
            call_lua_named_action(lua, action)
        })
    }

    pub(crate) fn popup(
        &self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
    ) -> Option<Result<Value, String>> {
        self.instances.get(&resolved.id).map(|lua| {
            Self::publish_entry(lua, resolved, context)?;
            call_lua_value(lua, "popup")
        })
    }

    pub(crate) fn press(
        &self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
    ) -> Option<Result<(), String>> {
        self.instances.get(&resolved.id).map(|lua| {
            Self::publish_entry(lua, resolved, context)?;
            call_lua_value(lua, "on_press").map(|_| ())
        })
    }

    pub(crate) fn ensure_instance(&mut self, resolved: &ResolvedWidget) -> Result<(), String> {
        if self.instances.contains_key(&resolved.id) {
            return Ok(());
        }
        let source = std::fs::read_to_string(&resolved.path)
            .map_err(|error| format!("cannot read {}: {error}", resolved.path.display()))?;
        let lua = new_widget_lua().map_err(|error| error.to_string())?;
        load_widget_script(&lua, &resolved.name, &source)
            .map_err(|error| format!("{}: {error}", resolved.path.display()))?;
        self.instances.insert(resolved.id.clone(), lua);
        Ok(())
    }
    pub(crate) fn reset(&mut self) {
        self.instances.clear();
        self.outputs.clear();
        self.trees.clear();
        self.last_run.clear();
        self.errors.clear();
        self.revisions.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Table;

    #[test]
    fn lua_popup_and_press_contract() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(
            &lua,
            "test",
            "return {view=function() return 'x' end, popup=function() return 'menu' end}",
        )
        .unwrap();
        assert_eq!(
            super::super::value::coerce_text(call_lua_value(&lua, "popup").unwrap(), "popup()")
                .unwrap(),
            "menu"
        );
        load_widget_script(&lua, "test", "return {flag=false, view=function(self) return self.flag and 1 or 0 end, on_press=function(self) self.flag=true end}").unwrap();
        call_lua_value(&lua, "on_press").unwrap();
        assert_eq!(
            super::super::value::coerce_text(call_lua_value(&lua, "view").unwrap(), "view()")
                .unwrap(),
            "1"
        );
        let mut states = HashMap::new();
        states.insert("w".into(), new_widget_lua().unwrap());
        load_widget_script(&states["w"], "w", "return {view=function() return 'x' end}").unwrap();
        assert!(!lua_has_func(&states, "w", "popup"));
        assert!(!lua_has_func(&states, "w", "on_press"));
        assert!(!lua_has_func(&states, "missing", "view"));
        assert!(call_lua_value(&states["w"], "on_press").is_err());
    }

    #[test]
    fn lua_on_action_receives_the_item_key() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "test", "seen={}; return {view=function() return '' end, on_action=function(self,name) seen[#seen+1]=name end}").unwrap();
        call_lua_named_action(&lua, "toggle").unwrap();
        assert_eq!(
            lua.load("return seen[1]").eval::<String>().unwrap(),
            "toggle"
        );
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "plain", "return {view=function() return 'x' end}").unwrap();
        call_lua_named_action(&lua, "ws:1").unwrap();
    }

    #[test]
    fn lua_sandbox_runs_app_view() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "test", "return {view=function() return 'hi' end}").unwrap();
        assert_eq!(
            super::super::value::coerce_text(call_lua_value(&lua, "view").unwrap(), "view()")
                .unwrap(),
            "hi"
        );
    }

    #[test]
    fn lua_return_values_coerce_to_text() {
        let lua = new_widget_lua().unwrap();
        for (source, expected) in [("42", "42"), ("true", "true"), ("nil", "")] {
            load_widget_script(
                &lua,
                "test",
                &format!("return {{view=function() return {source} end}}"),
            )
            .unwrap();
            assert_eq!(
                super::super::value::coerce_text(call_lua_value(&lua, "view").unwrap(), "view()")
                    .unwrap(),
                expected
            );
        }
        load_widget_script(&lua, "test", "return {view=function() return {} end}").unwrap();
        assert!(
            super::super::value::coerce_text(call_lua_value(&lua, "view").unwrap(), "view()")
                .is_err()
        );
    }

    #[test]
    fn lua_module_without_view_or_with_legacy_render_is_rejected() {
        let lua = new_widget_lua().unwrap();
        for source in ["x=1", "function render() return 'old' end", "return {}"] {
            assert!(load_widget_script(&lua, "test", source).is_err());
        }
    }

    #[test]
    fn module_methods_bind_self_without_exporting_globals() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "module", "return {count=0, view=function(self) return ui.text(tostring(self.count)) end, on_press=function(self) self.count=self.count+1 end, on_action=function(self,key) return {dismiss=self.count, key=key} end}").unwrap();
        for name in ["render", "view", "on_press", "on_action"] {
            assert_eq!(lua.globals().get::<Value>(name).unwrap(), Value::Nil);
        }
        call_lua_value(&lua, "on_press").unwrap();
        assert_eq!(
            crate::ui::decode::parse_node(&call_lua_value(&lua, "view").unwrap()).unwrap(),
            WidgetNode::Text {
                content: "1".into(),
                size: None,
                width: None,
                height: None,
                color: None,
            }
        );
        let Value::Table(action) = call_lua_named_action(&lua, "dismiss").unwrap() else {
            panic!("table");
        };
        assert_eq!(action.get::<u32>("dismiss").unwrap(), 1);
        assert_eq!(action.get::<String>("key").unwrap(), "dismiss");
        load_widget_script(&lua, "replacement", crate::config::SEED_HELLO_LUA).unwrap();
        assert_eq!(call_lua_named_action(&lua, "dismiss").unwrap(), Value::Nil);
    }

    #[test]
    fn revision_invalidates_shared_path_instances_and_missing_files_preserve_them() {
        let root =
            std::env::temp_dir().join(format!("riced-widget-revision-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("shared.lua");
        std::fs::write(&path, "return {}").unwrap();
        let mut state = WidgetState::default();
        for id in ["first", "second", "unrelated"] {
            state.instances.insert(id.into(), Lua::new());
        }
        assert!(state.sync_revision(&path, ["first", "second"]));
        assert_eq!(state.instances.len(), 1);
        assert!(state.instances.contains_key("unrelated"));
        state.instances.insert("first".into(), Lua::new());
        assert!(!state.sync_revision(&path, ["first", "second"]));
        assert!(state.instances.contains_key("first"));
        let revision = state.revisions[&path];
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(revision + std::time::Duration::from_secs(2)),
            )
            .unwrap();
        assert!(state.sync_revision(&path, ["first", "second"]));
        assert!(!state.instances.contains_key("first"));
        state.instances.insert("first".into(), Lua::new());
        let revision = state.revisions[&path];
        std::fs::remove_file(&path).unwrap();
        assert!(!state.sync_revision(&path, ["first", "second"]));
        assert!(state.instances.contains_key("first"));
        assert_eq!(state.revisions[&path], revision);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn entries_publish_fresh_output_and_props_and_ignore_stale_actions() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "entry", "return {view=function(self) return bar.output..self.props.label end, on_action=function(self,key) return bar.output..self.props.label..key end, on_press=function(self) self.last=bar.output..self.props.label end}").unwrap();
        let mut state = WidgetState::default();
        state.instances.insert("placement".into(), lua);
        let mut resolved = ResolvedWidget {
            id: "placement".into(),
            name: "entry".into(),
            path: "unused.lua".into(),
            interval: 1.0,
            size: 13.0,
            props: HashMap::from([("label".into(), PropValue::Text("/first/".into()))]),
        };
        let system = sysinfo::System::new();
        let theme = crate::config::ThemeConfig::default();
        let outputs = HashMap::new();
        let notifications = std::collections::VecDeque::new();
        let toplevels = Default::default();
        let workspaces = Default::default();
        let context = |output| EntryContext {
            services: crate::services::ServiceCtx {
                sys: &system,
                gpu: None,
                theme: &theme,
                outputs: &outputs,
                notifications: &notifications,
                toplevels: &toplevels,
                workspaces: &workspaces,
            },
            output,
        };
        assert_eq!(
            state
                .render(&resolved, &context("DP-1"))
                .unwrap()
                .as_string()
                .unwrap()
                .to_str()
                .unwrap(),
            "DP-1/first/"
        );
        resolved
            .props
            .insert("label".into(), PropValue::Text("/second/".into()));
        assert_eq!(
            state
                .action(&resolved, &context("DP-2"), "click")
                .unwrap()
                .unwrap()
                .as_string()
                .unwrap()
                .to_str()
                .unwrap(),
            "DP-2/second/click"
        );
        state.press(&resolved, &context("DP-3")).unwrap().unwrap();
        let app: Table = state.instances[&resolved.id]
            .named_registry_value("riced.widget.app")
            .unwrap();
        assert_eq!(app.get::<String>("last").unwrap(), "DP-3/second/");
        resolved.id = "removed".into();
        assert!(state.action(&resolved, &context("DP-1"), "click").is_none());
        assert!(state.press(&resolved, &context("DP-1")).is_none());
        assert!(!state.instances.contains_key("removed"));
    }

    #[test]
    fn placements_have_independent_self_state_and_failed_load_is_not_installed() {
        let root = std::env::temp_dir().join(format!("riced-widget-state-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("counter.lua");
        std::fs::write(&path, "local app={count=0}; function app:view() self.count=self.count+1; return self.count end; return app").unwrap();
        let mut state = WidgetState::default();
        for id in ["first", "second"] {
            state
                .ensure_instance(&ResolvedWidget {
                    id: id.into(),
                    name: "counter".into(),
                    path: path.clone(),
                    interval: 1.0,
                    size: 13.0,
                    props: HashMap::new(),
                })
                .unwrap();
        }
        let call = |lua: &Lua| {
            super::super::sandbox::reset_budget(lua).unwrap();
            let app: Table = lua.named_registry_value("riced.widget.app").unwrap();
            super::super::entry::invoke(lua, app, "view", mlua::MultiValue::new())
                .unwrap()
                .as_integer()
                .unwrap()
        };
        assert_eq!(call(&state.instances["first"]), 1);
        assert_eq!(call(&state.instances["first"]), 2);
        assert_eq!(call(&state.instances["second"]), 1);
        std::fs::write(&path, "return {}").unwrap();
        assert!(
            state
                .ensure_instance(&ResolvedWidget {
                    id: "broken".into(),
                    name: "counter".into(),
                    path,
                    interval: 1.0,
                    size: 13.0,
                    props: HashMap::new()
                })
                .is_err()
        );
        assert!(!state.instances.contains_key("broken"));
        state.reset();
        assert!(state.instances.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
