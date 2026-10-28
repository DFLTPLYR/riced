//! `notifications` service: retained history, including hidden/expired popups.

use super::registry::ServiceCtx;

/// Publish `notifications = { {id, app, title, body, icon, urgency,
/// has_image, actions, output, popup_visible, closed}, … }`, newest-first.
/// Metadata only — no image handles cross into Lua. `actions` is a plain
/// `{key, label}` array so a Lua-only notification layer can render buttons.
pub fn publish(ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
    let list = lua.create_table()?;
    let mut i = 0;
    for n in ctx.notifications.iter().rev() {
        let entry = lua.create_table()?;
        entry.set("id", n.id)?;
        entry.set("_notification_generation", n.generation)?;
        entry.set("popup_visible", n.popup_visible)?;
        entry.set("closed", n.closed)?;
        entry.set("app", n.app.clone())?;
        entry.set("title", n.title.clone())?;
        entry.set("body", n.body.clone())?;
        entry.set("icon", n.icon.clone())?;
        entry.set("urgency", n.urgency)?;
        entry.set("has_image", n.image.is_some())?;
        entry.set(
            "output",
            n.output
                .and_then(|id| ctx.outputs.get(&id))
                .and_then(|info| info.name.clone()),
        )?;
        entry.set(
            "actions",
            crate::lua::notifications::actions_table(lua, &n.actions)
                .map_err(mlua::Error::RuntimeError)?,
        )?;
        i += 1;
        list.set(i, entry)?;
    }
    lua.globals().set("notifications", list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::screens::Notification;

    #[test]
    fn notification_list_is_newest_first_metadata_only() {
        let lua = mlua::Lua::new();
        let theme = crate::config::ThemeConfig::default();
        let sys = sysinfo::System::new();
        let outputs = std::collections::HashMap::new();
        let workspaces = crate::services::WorkspaceCache::default();
        let toplevels = super::super::ToplevelCache::default();
        let mk = || {
            Notification::internal(
                &crate::config::NotificationConfig::default(),
                "t",
                "b".to_string(),
                1,
            )
        };
        let mut queue = std::collections::VecDeque::new();
        let mut a = mk();
        a.id = 1;
        a.icon = "dialog-information".into();
        a.actions = vec![("open".into(), "Open".into())];
        let mut b = mk();
        b.id = 2;
        queue.push_back(a);
        queue.push_back(b);
        let ctx = ServiceCtx {
            sys: &sys,
            gpu: None,
            sampled: 0.0,
            theme: &theme,
            component_defaults: &Default::default(),
            outputs: &outputs,
            notifications: &queue,
            toplevels: &toplevels,
            workspaces: &workspaces,
            tray: &Default::default(),
        };
        publish(&ctx, &lua).expect("publish");
        let first: u32 = lua.load("return notifications[1].id").eval().expect("id");
        assert_eq!(first, 2);
        let count: i64 = lua.load("return #notifications").eval().expect("len");
        assert_eq!(count, 2);
        let icon: String = lua
            .load("return notifications[2].icon")
            .eval()
            .expect("icon");
        assert_eq!(icon, "dialog-information");
        let key: String = lua
            .load("return notifications[2].actions[1].key")
            .eval()
            .expect("action key");
        assert_eq!(key, "open");
        let visible: bool = lua
            .load("return notifications[2].popup_visible")
            .eval()
            .expect("visible");
        assert!(visible);
    }
}
