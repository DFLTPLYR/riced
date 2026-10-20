//! Owned StatusNotifier snapshots; no D-Bus calls on Lua entry.
use super::ServiceCtx;
use mlua::{Lua, Table};

fn image(
    lua: &Lua,
    handle: Option<iced::widget::image::Handle>,
) -> mlua::Result<Option<mlua::AnyUserData>> {
    handle
        .map(|handle| lua.create_userdata(crate::ui::image_value::ImageValue(handle)))
        .transpose()
}

fn menu(lua: &Lua, node: crate::tray::MenuNode) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("id", node.id)?;
    table.set("label", node.label)?;
    table.set("type", node.kind)?;
    table.set("enabled", node.enabled)?;
    table.set("visible", node.visible)?;
    table.set("toggle_type", node.toggle_type)?;
    table.set("toggle_state", node.toggle_state)?;
    table.set("checked", node.toggle_state == 1)?;
    table.set("submenu", node.submenu)?;
    table.set("icon_name", node.icon_name)?;
    table.set("icon", image(lua, node.icon)?)?;
    let children = lua.create_table()?;
    for (index, child) in node.children.into_iter().enumerate() {
        children.set(index + 1, menu(lua, child)?)?;
    }
    table.set("children", children)?;
    Ok(table)
}

pub(super) fn publish(ctx: &ServiceCtx<'_>, lua: &Lua) -> mlua::Result<()> {
    let root = lua.create_table()?;
    let items = lua.create_table()?;
    for (index, item) in ctx.tray.snapshot().into_iter().enumerate() {
        let table = lua.create_table()?;
        table.set("id", item.id)?;
        table.set("app_id", item.app_id)?;
        table.set("title", item.title)?;
        table.set("status", item.status)?;
        table.set("icon_name", item.icon_name)?;
        table.set("icon_path", item.icon_path)?;
        table.set("icon_pixmap", image(lua, item.icon_pixmap)?)?;
        table.set("attention_icon", image(lua, item.attention_icon)?)?;
        table.set("overlay_icon", image(lua, item.overlay_icon)?)?;
        table.set("tooltip_title", item.tooltip_title)?;
        table.set("tooltip", item.tooltip)?;
        table.set("is_menu", item.is_menu)?;
        table.set("has_menu", item.menu.is_some())?;
        table.set("menu", item.menu.map(|node| menu(lua, node)).transpose()?)?;
        items.set(index + 1, table)?;
    }
    root.set("items", items)?;
    lua.globals().set("systemtray", root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_exposes_menu_tree_and_opaque_pixmap_without_clobbering_actions() {
        let lua = Lua::new();
        crate::lua::tray::install(&lua).unwrap();
        let cache = crate::tray::TrayCache::from_items(vec![crate::tray::TrayItem {
            id: ":1.4/Item".into(),
            title: "Example".into(),
            icon_pixmap: Some(iced::widget::image::Handle::from_rgba(
                1,
                1,
                vec![0, 0, 0, 255],
            )),
            menu: Some(crate::tray::MenuNode {
                id: 0,
                enabled: true,
                visible: true,
                children: vec![crate::tray::MenuNode {
                    id: 8,
                    label: "Check".into(),
                    enabled: false,
                    visible: true,
                    toggle_state: 1,
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }]);
        let ctx = ServiceCtx {
            sys: &sysinfo::System::new(),
            gpu: None,
            sampled: 0.0,
            theme: &Default::default(),
            component_defaults: &Default::default(),
            outputs: &Default::default(),
            notifications: &Default::default(),
            toplevels: &Default::default(),
            workspaces: &Default::default(),
            tray: &cache,
        };
        publish(&ctx, &lua).unwrap();
        publish(&ctx, &lua).unwrap();
        assert!(lua.load("local i=systemtray.items[1]; return i.id==':1.4/Item' and i.has_menu and type(i.icon_pixmap)=='userdata' and i.menu.children[1].checked and not i.menu.children[1].enabled and type(tray.activate)=='function'").eval::<bool>().unwrap());
    }
}
