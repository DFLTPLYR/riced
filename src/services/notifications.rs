//! `notifications` service: the live queue snapshot. Moved verbatim
//! from `publish_notification_list` (same global name, same shape).

use super::registry::ServiceCtx;

/// Publish `notifications = { {id, app, title, body, urgency,
/// has_image}, … }`, newest-first. Metadata only — no image handles
/// or action arrays cross into Lua.
pub fn publish(ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
    let list = lua.create_table()?;
    let mut i = 0;
    for n in ctx.notifications.iter().rev() {
        let entry = lua.create_table()?;
        entry.set("id", n.id)?;
        entry.set("app", n.app.clone())?;
        entry.set("title", n.title.clone())?;
        entry.set("body", n.body.clone())?;
        entry.set("urgency", n.urgency)?;
        entry.set("has_image", n.image.is_some())?;
        i += 1;
        list.set(i, entry)?;
    }
    lua.globals().set("notifications", list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::layers::Notification;

    #[test]
    fn notification_list_is_newest_first_metadata_only() {
        let lua = mlua::Lua::new();
        let theme = crate::config::ThemeConfig::default();
        let sys = sysinfo::System::new();
        let outputs = std::collections::HashMap::new();
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
        let mut b = mk();
        b.id = 2;
        queue.push_back(a);
        queue.push_back(b);
        let ctx = ServiceCtx {
            sys: &sys,
            gpu: None,
            theme: &theme,
            outputs: &outputs,
            notifications: &queue,
            toplevels: &toplevels,
        };
        publish(&ctx, &lua).expect("publish");
        let first: u32 = lua.load("return notifications[1].id").eval().expect("id");
        assert_eq!(first, 2);
        let count: i64 = lua.load("return #notifications").eval().expect("len");
        assert_eq!(count, 2);
    }
}
