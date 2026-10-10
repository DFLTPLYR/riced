//! Notification app entries and owned action decoding, independent of windows.
use crate::{
    services::ServiceCtx,
    ui::{decode::parse_node, listview::Transition, node::WidgetNode},
};
use mlua::{Lua, Table, Value};
use std::time::SystemTime;

pub(crate) struct NotificationData<'a> {
    pub id: u32,
    pub app: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub icon: &'a str,
    pub urgency: u8,
    pub has_image: bool,
    pub actions: &'a [(String, String)],
}

pub(crate) fn script_path() -> std::path::PathBuf {
    crate::config::widgets_dir().join("notifications.lua")
}

pub(crate) fn actions_table(lua: &Lua, actions: &[(String, String)]) -> Result<Table, String> {
    let table = lua.create_table().map_err(|error| error.to_string())?;
    for (index, (key, label)) in actions.iter().enumerate() {
        let action = lua.create_table().map_err(|error| error.to_string())?;
        action
            .set("key", key.as_str())
            .map_err(|error| error.to_string())?;
        action
            .set("label", label.as_str())
            .map_err(|error| error.to_string())?;
        table
            .set(index + 1, action)
            .map_err(|error| error.to_string())?;
    }
    Ok(table)
}

#[derive(Debug, Default)]
pub(crate) struct NotificationRenderer {
    pub lua: Option<Lua>,
    pub revision: Option<SystemTime>,
    pub error: Option<String>,
}

impl NotificationRenderer {
    pub(crate) fn sync_revision(&mut self) -> bool {
        let Ok(revision) =
            std::fs::metadata(script_path()).and_then(|metadata| metadata.modified())
        else {
            return false;
        };
        if self.revision == Some(revision) {
            return false;
        }
        self.lua = None;
        self.revision = Some(revision);
        true
    }

    pub(crate) fn note_error(&mut self, error: String) {
        super::error::report_once(&mut self.error, "riced: notifications.lua: ", error);
    }

    pub(crate) fn ensure(
        &mut self,
        defaults: &(Transition, Transition, Transition),
    ) -> Result<Option<super::transitions::TransitionSpec>, String> {
        if self.lua.is_some() {
            return Ok(None);
        }
        let source = std::fs::read_to_string(script_path())
            .map_err(|error| format!("cannot read {error}"))?;
        let lua = super::widgets::new_widget_lua().map_err(|error| error.to_string())?;
        super::widgets::load_widget_script(&lua, "notifications.lua", &source)
            .map_err(|error| error.to_string())?;
        let transitions = match super::transitions::parse_transitions(
            &lua,
            &defaults.0,
            &defaults.1,
            &defaults.2,
        ) {
            Ok(transitions) => Some(transitions),
            Err(error) => {
                self.note_error(error);
                None
            }
        };
        self.lua = Some(lua);
        Ok(transitions)
    }

    pub(crate) fn view(
        &self,
        services: &ServiceCtx<'_>,
        notification: NotificationData<'_>,
    ) -> Result<WidgetNode, String> {
        let lua = self.lua.as_ref().ok_or("runtime missing")?;
        crate::services::publish_all(services, lua).map_err(|error| error.to_string())?;
        crate::services::publish_bar(lua, "").map_err(|error| error.to_string())?;
        let table = lua.create_table().map_err(|error| error.to_string())?;
        table
            .set("id", notification.id)
            .map_err(|error| error.to_string())?;
        table
            .set("app", notification.app)
            .map_err(|error| error.to_string())?;
        table
            .set("title", notification.title)
            .map_err(|error| error.to_string())?;
        table
            .set("body", notification.body)
            .map_err(|error| error.to_string())?;
        table
            .set("icon", notification.icon)
            .map_err(|error| error.to_string())?;
        table
            .set("urgency", notification.urgency)
            .map_err(|error| error.to_string())?;
        table
            .set("has_image", notification.has_image)
            .map_err(|error| error.to_string())?;
        table
            .set("actions", actions_table(lua, notification.actions)?)
            .map_err(|error| error.to_string())?;
        let value = super::widgets::call_widget_method(
            lua,
            "view",
            mlua::MultiValue::from_vec(vec![Value::Table(table)]),
        )?;
        parse_node(&value)
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum NotificationAction {
    Dismiss(u32),
    Invoke(u32, String),
}

pub(crate) fn notification_action(value: &Value) -> Option<NotificationAction> {
    let Value::Table(table) = value else {
        return None;
    };
    if let Ok(id) = table.get::<u32>("dismiss") {
        return Some(NotificationAction::Dismiss(id));
    }
    let inner = table.get::<Table>("invoke").ok()?;
    Some(NotificationAction::Invoke(
        inner.get("id").ok()?,
        inner.get("key").ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_decoding_is_owned_and_malformed_actions_are_ignored() {
        let lua = Lua::new();
        let value: Value = lua.load("return {dismiss=42}").eval().unwrap();
        assert_eq!(
            notification_action(&value),
            Some(NotificationAction::Dismiss(42))
        );
        let value: Value = lua
            .load("return {invoke={id=7,key='open'}}")
            .eval()
            .unwrap();
        assert_eq!(
            notification_action(&value),
            Some(NotificationAction::Invoke(7, "open".into()))
        );
        for source in [
            "return nil",
            "return 42",
            "return {}",
            "return {invoke={id=7}}",
            "return {dismiss=-1}",
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            assert_eq!(notification_action(&value), None, "{source}");
        }
    }

    #[test]
    fn renderer_publishes_notification_metadata_services_and_bound_self() {
        let lua = super::super::widgets::new_widget_lua().unwrap();
        super::super::widgets::load_widget_script(&lua, "card", "return {calls=0,view=function(self,n) self.calls=self.calls+1; assert(bar.output==nil); return ui.text(n.title..':'..n.actions[1].key..':'..tostring(n.has_image)..':'..#notifications..':'..self.calls) end}").unwrap();
        let renderer = NotificationRenderer {
            lua: Some(lua),
            ..Default::default()
        };
        let system = sysinfo::System::new();
        let theme = crate::config::ThemeConfig::default();
        let outputs = Default::default();
        let notifications = Default::default();
        let toplevels = Default::default();
        let workspaces = Default::default();
        let services = ServiceCtx {
            sys: &system,
            gpu: None,
            theme: &theme,
            outputs: &outputs,
            notifications: &notifications,
            toplevels: &toplevels,
            workspaces: &workspaces,
        };
        let actions = [("open".into(), "Open".into())];
        for calls in [1, 2] {
            let node = renderer
                .view(
                    &services,
                    NotificationData {
                        id: 7,
                        app: "example",
                        title: "hello",
                        body: "body",
                        icon: "",
                        urgency: 1,
                        has_image: true,
                        actions: &actions,
                    },
                )
                .unwrap();
            assert!(
                matches!(node, WidgetNode::Text {content, ..} if content == format!("hello:open:true:0:{calls}"))
            );
        }
    }
}
