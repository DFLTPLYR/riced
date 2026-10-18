//! Owned per-placement VMs, revision tracking, and per-window render caches.
use crate::config::PropValue;
use crate::ui::node::WidgetNode;
use iced::window;
use mlua::{Function, Lua, LuaSerdeExt, Table, Value};

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
    lua.expire_registry_values();
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

#[cfg(test)]
pub(crate) fn call_lua_named_action(lua: &Lua, action: &str) -> Result<Value, String> {
    call_lua_action(lua, &action.into())
}
pub(crate) fn call_lua_action(
    lua: &Lua,
    action: &crate::ui::action::Action,
) -> Result<Value, String> {
    let app: Table = lua
        .named_registry_value("riced.widget.app")
        .map_err(|error| error.to_string())?;
    if matches!(
        app.get::<Value>("actions")
            .map_err(|error| error.to_string())?,
        Value::Nil
    ) {
        return Ok(Value::Nil);
    }
    lua.set_named_registry_value("riced.drag.requested", false)
        .map_err(|e| e.to_string())?;
    lua.set_named_registry_value("riced.drop.requested", false)
        .map_err(|e| e.to_string())?;
    let event = match &action.event {
        crate::ui::action::Event::Mouse { state, .. } => state.as_str(),
        _ => "action",
    };
    lua.set_named_registry_value("riced.gesture", event)
        .map_err(|e| e.to_string())?;
    let arguments = action.arguments(lua).map_err(|e| e.to_string())?;
    lua.set_named_registry_value("riced.in_actions", true)
        .map_err(|e| e.to_string())?;
    let result = call_widget_method(lua, "actions", arguments);
    lua.set_named_registry_value("riced.in_actions", false)
        .map_err(|e| e.to_string())?;
    let value = result?;
    let drag = lua
        .named_registry_value::<bool>("riced.drag.requested")
        .unwrap_or(false);
    let drop = lua
        .named_registry_value::<bool>("riced.drop.requested")
        .unwrap_or(false);
    if drag || drop {
        let command = match value {
            Value::Nil => lua.create_table().map_err(|e| e.to_string())?,
            Value::Table(table) => table,
            _ => return Err("actions host commands require a table or nil return".into()),
        };
        command
            .set(if drag { "drag" } else { "drop" }, true)
            .map_err(|e| e.to_string())?;
        Ok(Value::Table(command))
    } else {
        Ok(value)
    }
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
    validate_actions(&app)?;
    for method in ["view", "popup", "actions", "transitions"] {
        if !matches!(app.get::<Value>(method)?, Value::Function(_) | Value::Nil) {
            return Err(mlua::Error::RuntimeError(format!(
                "{label}: app.{method} must be a function"
            )));
        }
    }
    for (method, requested) in [
        ("drag", "riced.drag.requested"),
        ("drop", "riced.drop.requested"),
    ] {
        if app.raw_get::<Value>(method)? != Value::Nil {
            return Err(mlua::Error::RuntimeError(format!(
                "app.{method} is reserved for the host"
            )));
        }
        app.set(
            method,
            lua.create_function(move |lua, _: mlua::MultiValue| {
                if !lua
                    .named_registry_value::<bool>("riced.drag.available")
                    .unwrap_or(false)
                {
                    return Err(mlua::Error::RuntimeError(format!(
                        "self.{method}() requires a desktop placement"
                    )));
                }
                if !lua
                    .named_registry_value::<bool>("riced.in_actions")
                    .unwrap_or(false)
                {
                    return Err(mlua::Error::RuntimeError(format!(
                        "self.{method}() is only available inside actions"
                    )));
                }
                if lua.named_registry_value::<String>("riced.gesture")? != method {
                    return Err(mlua::Error::RuntimeError(format!(
                        "self.{method}() requires a {method} event"
                    )));
                }
                lua.set_named_registry_value(requested, true)
            })?,
        )?;
    }
    lua.set_named_registry_value("riced.widget.app", app)?;
    super::callbacks::new_epoch(lua)
}

pub(crate) fn validate_actions(app: &Table) -> mlua::Result<()> {
    for removed in ["on_action", "on_press", "handler", "reposition"] {
        if app.raw_get::<Value>(removed)? != Value::Nil {
            return Err(mlua::Error::RuntimeError(format!(
                "app.{removed} was removed; use app:actions(events)"
            )));
        }
    }
    if !matches!(
        app.raw_get::<Value>("actions")?,
        Value::Nil | Value::Function(_)
    ) {
        return Err(mlua::Error::RuntimeError(
            "app.actions must be a function".into(),
        ));
    }
    Ok(())
}
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Instant, SystemTime},
};

#[derive(Debug, Default)]
pub(crate) struct WidgetState {
    /// Host geometry is presentation context, not a different widget contract.
    pub hosts: HashMap<String, serde_json::Value>,
    pub instances: HashMap<String, Lua>,
    pub outputs: HashMap<(window::Id, String), String>,
    pub trees: HashMap<(window::Id, String), WidgetNode>,
    pub last_run: HashMap<String, Instant>,
    pub errors: HashMap<String, String>,
    pub revisions: HashMap<PathBuf, SystemTime>,
    /// Placement-level hover flags by placement id. Placement ids are
    /// process-unique, so one map covers every bar.
    pub hovered: HashMap<String, bool>,
}

impl WidgetState {
    pub(crate) fn forget_window(&mut self, window: window::Id) {
        self.outputs.retain(|(owner, _), _| *owner != window);
        self.trees.retain(|(owner, _), _| *owner != window);
    }

    /// Drop hover flags for placements that no longer exist anywhere.
    /// Placement ids are never reused, so this only runs on hover edits.
    pub(crate) fn retain_hovered(&mut self, live: &HashSet<String>) {
        self.hovered.retain(|id, _| live.contains(id));
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
        hovered: bool,
        host: Option<&serde_json::Value>,
    ) -> Result<Table, String> {
        crate::services::publish_all(&context.services, lua).map_err(|error| error.to_string())?;
        crate::services::publish_bar(lua, context.output).map_err(|error| error.to_string())?;
        super::props::publish_props(lua, &resolved.props).map_err(|error| error.to_string())?;
        let app: Table = lua
            .named_registry_value("riced.widget.app")
            .map_err(|error| error.to_string())?;
        let result = (|| -> mlua::Result<Table> {
            lua.set_named_registry_value("riced.drag.available", host.is_some())?;
            let overrides: Table = app.get("props")?;
            if let Value::Table(defaults) = app.get::<Value>("defaults")?
                && let Value::Table(props) = defaults.get::<Value>("props")?
            {
                for pair in props.pairs::<String, Value>() {
                    let (key, value) = pair?;
                    if !overrides.contains_key(key.as_str())? {
                        overrides.set(key, value)?;
                    }
                }
            }
            let props = super::props::component_props(
                lua,
                &app,
                Value::Table(overrides),
                match host {
                    Some(host) => lua.to_value(host)?,
                    None => Value::Nil,
                },
            )?;
            app.set("props", props.clone())?;
            app.set("hovered", hovered)?;
            Ok(props)
        })();
        result.map_err(|error| error.to_string())
    }

    pub(crate) fn render(
        &mut self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
    ) -> Result<Value, String> {
        self.ensure_instance(resolved)?;
        let hovered = self.hovered.get(&resolved.id).copied().unwrap_or(false);
        let lua = &self.instances[&resolved.id];
        let props = Self::publish_entry(
            lua,
            resolved,
            context,
            hovered,
            self.hosts.get(&resolved.id),
        )?;
        call_widget_method(
            lua,
            "view",
            mlua::MultiValue::from_vec(vec![Value::Table(props)]),
        )
    }

    pub(crate) fn render_with_host(
        &mut self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
        host: serde_json::Value,
    ) -> Result<Value, String> {
        self.hosts.insert(resolved.id.clone(), host);
        self.render(resolved, context)
    }

    pub(crate) fn published_props(&self, id: &str) -> Result<serde_json::Value, String> {
        let lua = self.instances.get(id).ok_or("widget instance missing")?;
        let app: Table = lua
            .named_registry_value("riced.widget.app")
            .map_err(|error| error.to_string())?;
        lua.from_value(app.get("props").map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())
    }

    /// Missing instances are stale host messages and are ignored. Actions do
    /// not construct a new VM or reset the placement's existing self state.
    #[cfg(test)]
    pub(crate) fn action(
        &self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
        action: &str,
    ) -> Option<Result<Value, String>> {
        self.ui_action(resolved, context, &action.into())
    }
    pub(crate) fn ui_action(
        &self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
        action: &crate::ui::action::Action,
    ) -> Option<Result<Value, String>> {
        let hovered = self.hovered.get(&resolved.id).copied().unwrap_or(false);
        self.instances.get(&resolved.id).map(|lua| {
            Self::publish_entry(
                lua,
                resolved,
                context,
                hovered,
                self.hosts.get(&resolved.id),
            )?;
            call_lua_action(lua, action)
        })
    }

    pub(crate) fn popup(
        &self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
    ) -> Option<Result<Value, String>> {
        let hovered = self.hovered.get(&resolved.id).copied().unwrap_or(false);
        self.instances.get(&resolved.id).map(|lua| {
            Self::publish_entry(
                lua,
                resolved,
                context,
                hovered,
                self.hosts.get(&resolved.id),
            )?;
            call_lua_value(lua, "popup")
        })
    }

    #[cfg(test)]
    pub(crate) fn press(
        &self,
        resolved: &ResolvedWidget,
        context: &EntryContext<'_>,
    ) -> Option<Result<(), String>> {
        let hovered = self.hovered.get(&resolved.id).copied().unwrap_or(false);
        self.instances.get(&resolved.id).map(|lua| {
            Self::publish_entry(
                lua,
                resolved,
                context,
                hovered,
                self.hosts.get(&resolved.id),
            )?;
            call_lua_action(lua, &crate::ui::action::Action::click()).map(|_| ())
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
        self.hosts.clear();
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
    fn actions_contract_rejects_legacy_callbacks_and_nonfunctions() {
        for source in [
            "return {view=function() end, on_action=function() end}",
            "return {view=function() end, on_press=function() end}",
            "return {view=function() end, handler=function() end}",
            "return {view=function() end, actions=true}",
            "return {view=function() end, reposition=function() end}",
            "return {view=function() end, drag=function() end}",
            "return {view=function() end, drop=function() end}",
        ] {
            assert!(load_widget_script(&new_widget_lua().unwrap(), "invalid", source).is_err());
        }
    }

    #[test]
    fn drag_and_drop_are_desktop_only_event_scoped_and_preserve_other_commands() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(
            &lua,
            "move",
            r#"return {
            view=function(self) self.drag() end,
            actions=function(self,events)
                if events.key=='fail' then self.drag(); error('failed') end
                if events.key=='invaliddrop' then self.drop() end
                if events.key=='move' then self:drag(); self.grabbed=true; return {dismiss=42} end
                if events.type=='drop' then assert(self.grabbed); self.drop() end
            end
        }"#,
        )
        .unwrap();
        assert!(
            call_lua_named_action(&lua, "move")
                .unwrap_err()
                .contains("desktop placement")
        );
        lua.set_named_registry_value("riced.drag.available", true)
            .unwrap();
        assert!(call_lua_named_action(&lua, "fail").is_err());
        assert!(
            call_lua_named_action(&lua, "invaliddrop")
                .unwrap_err()
                .contains("drop event")
        );
        assert!(
            call_lua_value(&lua, "view")
                .unwrap_err()
                .contains("inside actions")
        );
        assert_eq!(call_lua_named_action(&lua, "noop").unwrap(), Value::Nil);
        assert!(
            call_lua_named_action(&lua, "move")
                .unwrap_err()
                .contains("drag event")
        );
        let mut action = crate::ui::action::Action::click();
        action.key = "move".into();
        action.event = crate::ui::action::Event::Mouse {
            state: "drag".into(),
            button: Some("left".into()),
            position: None,
            origin: None,
        };
        action.key = "fail".into();
        assert!(
            call_lua_action(&lua, &action)
                .unwrap_err()
                .contains("failed")
        );
        assert_eq!(call_lua_named_action(&lua, "noop").unwrap(), Value::Nil);
        action.key = "move".into();
        let command = call_lua_action(&lua, &action).unwrap();
        let command = command.as_table().unwrap();
        assert!(command.get::<bool>("drag").unwrap());
        assert_eq!(command.get::<u32>("dismiss").unwrap(), 42);
        action.key.clear();
        if let crate::ui::action::Event::Mouse { state, .. } = &mut action.event {
            *state = "drop".into();
        }
        let command = call_lua_action(&lua, &action).unwrap();
        assert!(command.as_table().unwrap().get::<bool>("drop").unwrap());
    }

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
        load_widget_script(&lua, "test", "return {flag=false, view=function(self) return self.flag and 1 or 0 end, actions=function(self,events) assert(events.type=='click'); self.flag=true end}").unwrap();
        call_lua_action(&lua, &crate::ui::action::Action::click()).unwrap();
        assert_eq!(
            super::super::value::coerce_text(call_lua_value(&lua, "view").unwrap(), "view()")
                .unwrap(),
            "1"
        );
        let mut states = HashMap::new();
        states.insert("w".into(), new_widget_lua().unwrap());
        load_widget_script(&states["w"], "w", "return {view=function() return 'x' end}").unwrap();
        assert!(!lua_has_func(&states, "w", "popup"));
        assert!(!lua_has_func(&states, "w", "actions"));
        assert!(!lua_has_func(&states, "missing", "view"));
        assert!(call_lua_value(&states["w"], "actions").is_err());
    }

    #[test]
    fn published_hovered_reaches_self_without_touching_props() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(
            &lua,
            "test",
            "return {view=function(self) return self.hovered and 'hot' or 'cold' end}",
        )
        .unwrap();
        let system = sysinfo::System::new();
        let outputs = Default::default();
        let queue = Default::default();
        let toplevels = Default::default();
        let workspaces = Default::default();
        let context = EntryContext {
            services: crate::services::ServiceCtx {
                sys: &system,
                gpu: None,
                theme: &crate::config::ThemeConfig::default(),
                outputs: &outputs,
                notifications: &queue,
                toplevels: &toplevels,
                workspaces: &workspaces,
            },
            output: "DP-1",
        };
        let resolved = ResolvedWidget {
            id: "w1".into(),
            name: "test".into(),
            path: "test.lua".into(),
            interval: 1.0,
            size: 13.0,
            props: HashMap::new(),
        };
        super::WidgetState::publish_entry(&lua, &resolved, &context, false, None).unwrap();
        assert_eq!(
            super::super::value::coerce_text(call_lua_value(&lua, "view").unwrap(), "view()")
                .unwrap(),
            "cold"
        );
        super::WidgetState::publish_entry(&lua, &resolved, &context, true, None).unwrap();
        assert_eq!(
            super::super::value::coerce_text(call_lua_value(&lua, "view").unwrap(), "view()")
                .unwrap(),
            "hot"
        );
    }

    #[test]
    fn lua_actions_receives_the_event_table() {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "test", "seen={}; return {view=function() return '' end, actions=function(self,events) assert(events.type=='action'); seen[#seen+1]=events.key end}").unwrap();
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
        load_widget_script(&lua, "module", "return {count=0, view=function(self) return iced.text(tostring(self.count)) end, actions=function(self,events) if events.type=='click' then self.count=self.count+1 else return {dismiss=self.count, key=events.key} end end}").unwrap();
        for name in ["render", "view", "actions"] {
            assert_eq!(lua.globals().get::<Value>(name).unwrap(), Value::Nil);
        }
        call_lua_action(&lua, &crate::ui::action::Action::click()).unwrap();
        assert_eq!(
            crate::ui::decode::parse_node(&call_lua_value(&lua, "view").unwrap()).unwrap(),
            WidgetNode::Text {
                content: "1".into(),
                size: None,
                width: None,
                height: None,
                wrapping: None,
                color: None,
                elide: None,
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
        load_widget_script(&lua, "entry", "return {view=function(self) return bar.output..self.props.label end, actions=function(self,events) if events.type=='click' then self.last=bar.output..self.props.label else self.value=events.value; return bar.output..self.props.label..events.key end end}").unwrap();
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
        for value in [
            crate::ui::action::ActionValue::Boolean(true),
            crate::ui::action::ActionValue::Number(12.5),
            crate::ui::action::ActionValue::Text("a:b|query".into()),
        ] {
            let action = crate::ui::action::Action::change("changed".into(), value.clone());
            assert_eq!(
                state
                    .ui_action(&resolved, &context("DP-4"), &action)
                    .unwrap()
                    .unwrap()
                    .as_string()
                    .unwrap()
                    .to_str()
                    .unwrap(),
                "DP-4/second/changed"
            );
            let stored: Value = app.get("value").unwrap();
            match value {
                crate::ui::action::ActionValue::Boolean(value) => {
                    assert_eq!(stored, Value::Boolean(value))
                }
                crate::ui::action::ActionValue::Number(value) => {
                    assert_eq!(stored, Value::Number(f64::from(value)))
                }
                crate::ui::action::ActionValue::Text(value) => {
                    assert_eq!(stored.as_string().unwrap().to_str().unwrap(), value)
                }
            }
        }
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
