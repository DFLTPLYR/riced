//! Bundled widget contracts exercise canonical Lua/UI APIs, not the bar host.
use super::{
    props::publish_props,
    test_support::{load_seed_components, publish_test_services},
    widgets::{call_lua_named_action, call_lua_value, load_widget_script, new_widget_lua},
};
use crate::ui::{build::build_node, decode::parse_node, node::WidgetNode};
use mlua::Value;
use std::collections::HashMap;

#[test]
fn seed_components_define_working_builders() {
    let lua = new_widget_lua().unwrap();
    load_seed_components(&lua);
    for source in [
        r##"return ui.surface({body='x',border='#f00',border_width=0})"##,
        r##"return ui.styled_button({label='Go',action='go',background='#123'})"##,
        r##"return ui.styled_progress({value=0.5,color='#f00',background='#123'})"##,
        r##"return ui.styled_separator({height=2,color='#f00'})"##,
        r##"return ui.card({title='T',border='#f00',separator_color='#123'})"##,
        "return iced.use('spacer',{h=4})",
        "return iced.use('card',{title='T',body='b'})",
        "return iced.use('menu',{items={{label='Go',action='go'}}})",
    ] {
        let value: Value = lua.load(source).eval().unwrap();
        parse_node(&value).unwrap();
    }
    let value: Value = lua
        .load(r##"return ui.surface({body='x',border='#f00',border_width=0})"##)
        .eval()
        .unwrap();
    assert!(matches!(
        parse_node(&value).unwrap(),
        WidgetNode::Container {
            border: Some(_),
            border_width: 0.0,
            ..
        }
    ));
    let value: Value = lua
        .load("return iced.use('menu',{items={{label='Go',action='go'}}})")
        .eval()
        .unwrap();
    match parse_node(&value).unwrap() {
        WidgetNode::ListView { items, .. } => {
            assert!(matches!(&items[..], [(_,WidgetNode::Button {action, ..})] if action == "go"))
        }
        other => panic!("{other:?}"),
    }
    let value: Value = lua
        .load(r##"return iced.use('card',{title='T',color='#ff0000'})"##)
        .eval()
        .unwrap();
    match parse_node(&value).unwrap() {
        WidgetNode::Column { children, .. } => assert!(matches!(
            &children[..],
            [WidgetNode::Text { color: Some(_), .. }, _, _]
        )),
        other => panic!("{other:?}"),
    }
}

#[test]
fn notify_center_seed_reads_queue_and_dismisses() {
    let lua = new_widget_lua().unwrap();
    load_seed_components(&lua);
    load_widget_script(&lua, "notifycenter", crate::config::SEED_NOTIFY_CENTER_LUA).unwrap();
    lua.globals()
        .set("notifications", lua.create_table().unwrap())
        .unwrap();
    let _ = build_node(
        &parse_node(&call_lua_value(&lua, "view").unwrap()).unwrap(),
        13.0,
        None,
    )
    .unwrap();
    let _ =
        crate::shell::screens::Popup::parse_popup_content(call_lua_value(&lua, "popup").unwrap())
            .unwrap();
    lua.load("notifications={{id=42,app='mako',title='hi',body='b',urgency=1,has_image=false}}")
        .exec()
        .unwrap();
    assert!(matches!(
        parse_node(&call_lua_value(&lua, "view").unwrap()).unwrap(),
        WidgetNode::Row { .. }
    ));
    let content =
        crate::shell::screens::Popup::parse_popup_content(call_lua_value(&lua, "popup").unwrap())
            .unwrap();
    let mut actions = Vec::new();
    if let Some(tree) = &content.tree {
        button_actions(tree, &mut actions);
    }
    assert_eq!(actions, ["dismiss:42"]);
    match call_lua_named_action(&lua, "dismiss:42").unwrap() {
        Value::Table(table) => assert_eq!(table.get::<u32>("dismiss").unwrap(), 42),
        other => panic!("{other:?}"),
    }
}

fn node_has_icon(node: &WidgetNode) -> bool {
    match node {
        WidgetNode::Icon { .. } => true,
        WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => {
            children.iter().any(node_has_icon)
        }
        _ => false,
    }
}

fn button_actions(node: &WidgetNode, out: &mut Vec<String>) {
    match node {
        WidgetNode::ListView { items, .. } => {
            for (_, child) in items {
                button_actions(child, out);
            }
        }
        WidgetNode::Button { action, .. } => out.push(action.clone()),
        WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => {
            for child in children {
                button_actions(child, out);
            }
        }
        _ => {}
    }
}

#[test]
fn seed_usage_scripts_render_icon_free_text() {
    use crate::config::{SEED_CPU_LUA, SEED_GPU_LUA, SEED_RAM_LUA};
    let mut system = sysinfo::System::new();
    system.refresh_cpu_usage();
    system.refresh_memory();
    for source in [SEED_CPU_LUA, SEED_RAM_LUA, SEED_GPU_LUA] {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "seed", source).unwrap();
        publish_test_services(&lua, &system, None);
        assert!(!node_has_icon(
            &parse_node(&call_lua_value(&lua, "view").unwrap()).unwrap()
        ));
    }
}

#[test]
fn seed_scripts_parse_and_build() {
    use crate::config::{
        SEED_CLINEPASS_LUA, SEED_CLOCK_LUA, SEED_CPU_LUA, SEED_GPU_LUA, SEED_HELLO_LUA,
        SEED_RAM_LUA, SEED_STATS_LUA, SEED_SYSTEM_LUA, SEED_WORKSPACES_LUA,
    };
    let mut system = sysinfo::System::new();
    system.refresh_cpu_usage();
    system.refresh_memory();
    for source in [
        SEED_CLOCK_LUA,
        SEED_HELLO_LUA,
        SEED_STATS_LUA,
        SEED_CPU_LUA,
        SEED_RAM_LUA,
        SEED_GPU_LUA,
        SEED_WORKSPACES_LUA,
        SEED_CLINEPASS_LUA,
        SEED_SYSTEM_LUA,
    ] {
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua, "seed", source).unwrap();
        publish_test_services(&lua, &system, None);
        publish_props(&lua, &HashMap::new()).unwrap();
        let _ = build_node(
            &parse_node(&call_lua_value(&lua, "view").unwrap()).unwrap(),
            13.0,
            None,
        )
        .unwrap();
    }
    let lua = new_widget_lua().unwrap();
    load_seed_components(&lua);
    load_widget_script(&lua, "clinepass", SEED_CLINEPASS_LUA).unwrap();
    publish_test_services(&lua, &system, None);
    let content =
        crate::shell::screens::Popup::parse_popup_content(call_lua_value(&lua, "popup").unwrap())
            .unwrap();
    assert!(content.tree.is_some());
    let lua = new_widget_lua().unwrap();
    load_seed_components(&lua);
    load_widget_script(&lua, "system", SEED_SYSTEM_LUA).unwrap();
    let content =
        crate::shell::screens::Popup::parse_popup_content(call_lua_value(&lua, "popup").unwrap())
            .unwrap();
    assert!(content.items.is_empty());
    let mut actions = Vec::new();
    if let Some(tree) = &content.tree {
        button_actions(tree, &mut actions);
    }
    assert_eq!(actions, ["suspend", "hibernate", "reboot", "poweroff"]);
}

#[test]
fn system_seed_whitelists_systemctl_actions() {
    let lua = new_widget_lua().unwrap();
    let os: mlua::Table = lua.globals().get("os").unwrap();
    os.set(
        "execute",
        lua.create_function(|lua, command: String| {
            let seen: mlua::Table = lua.globals().get("_seen")?;
            seen.set(seen.len()? + 1, command)?;
            Ok(true)
        })
        .unwrap(),
    )
    .unwrap();
    lua.globals()
        .set("_seen", lua.create_table().unwrap())
        .unwrap();
    load_widget_script(&lua, "system", crate::config::SEED_SYSTEM_LUA).unwrap();
    call_lua_named_action(&lua, "suspend").unwrap();
    call_lua_named_action(&lua, "x; rm -rf ~").unwrap();
    let seen: mlua::Table = lua.globals().get("_seen").unwrap();
    assert_eq!(seen.len().unwrap(), 1);
    assert_eq!(seen.get::<String>(1).unwrap(), "systemctl suspend");
}

#[test]
fn clinepass_popup_parses_real_usage_shape() {
    use crate::config::SEED_CLINEPASS_LUA;
    let body = r#"{"data":{"limits":[{"type":"five_hour","percentUsed":14,"resetsAt":"2026-10-03T20:48:05Z"},{"type":"weekly","percentUsed":5,"resetsAt":"2026-10-10T15:48:05Z"},{"type":"monthly","percentUsed":2,"resetsAt":"2026-11-02T15:48:05Z"}]},"success":true}"#;
    let lua = new_widget_lua().unwrap();
    load_seed_components(&lua);
    let theme = lua.create_table().unwrap();
    theme.set("primary", "#00ff00").unwrap();
    lua.globals().set("theme", theme).unwrap();
    let io: mlua::Table = lua.globals().get("io").unwrap();
    io.set(
        "popen",
        lua.create_function(move |lua, _: String| {
            let handle = lua.create_table()?;
            let body = body.to_string();
            handle.set(
                "read",
                lua.create_function(move |_, (_handle, _mode): (mlua::Value, mlua::Value)| {
                    Ok(body.clone())
                })?,
            )?;
            handle.set("close", lua.create_function(|_, _: mlua::Value| Ok(true))?)?;
            Ok(handle)
        })
        .unwrap(),
    )
    .unwrap();
    load_widget_script(
        &lua,
        "clinepass",
        &SEED_CLINEPASS_LUA.replace(r#"local API_KEY = """#, r#"local API_KEY = "x""#),
    )
    .unwrap();
    let content =
        crate::shell::screens::Popup::parse_popup_content(call_lua_value(&lua, "popup").unwrap())
            .unwrap();
    assert!(
        matches!(content.tree.unwrap(), WidgetNode::Column {children, ..} if children.len() == 3)
    );
    call_lua_value(&lua, "view").unwrap();
    let tree =
        crate::shell::screens::Popup::parse_popup_content(call_lua_value(&lua, "popup").unwrap())
            .unwrap()
            .tree
            .unwrap();
    let _ = build_node(&tree, 13.0, None).unwrap();
    match tree {
        WidgetNode::Column { children, .. } => {
            assert_eq!(children.len(), 3);
            let Some(WidgetNode::Column { children: rows, .. }) = children.get(2) else {
                panic!("body column");
            };
            assert_eq!(rows.len(), 3);
            for row in rows {
                assert!(matches!(row, WidgetNode::Row {children, ..} if children.len() == 3));
            }
        }
        other => panic!("expected column, got {other:?}"),
    }
}

#[test]
fn workspaces_seed_filters_bar_output_and_dispatches_actual_name() {
    use crate::services::{Toplevel, ToplevelCache, Workspace, WorkspaceCache};
    let lua = new_widget_lua().unwrap();
    let system = sysinfo::System::new();
    let theme = crate::config::ThemeConfig::default();
    let outputs = HashMap::new();
    let notifications = std::collections::VecDeque::new();
    let workspaces = WorkspaceCache::from_rows(vec![
        Workspace {
            name: "web".into(),
            monitor: "".into(),
            active: false,
            rects: vec![],
        },
        Workspace {
            name: "code".into(),
            monitor: "DP-1".into(),
            active: true,
            rects: vec![[0.0, 0.0, 2560.0, 1440.0]],
        },
        Workspace {
            name: "mail".into(),
            monitor: "HDMI-1".into(),
            active: false,
            rects: vec![[2560.0, 0.0, 1920.0, 1080.0]],
        },
    ]);
    let toplevels = ToplevelCache::from_rows(vec![
        Toplevel {
            app_id: "foot".into(),
            title: "shell".into(),
        },
        Toplevel {
            app_id: "firefox".into(),
            title: "".into(),
        },
    ]);
    crate::services::publish_all(
        &crate::services::ServiceCtx {
            sys: &system,
            gpu: None,
            theme: &theme,
            outputs: &outputs,
            notifications: &notifications,
            toplevels: &toplevels,
            workspaces: &workspaces,
        },
        &lua,
    )
    .unwrap();
    crate::services::publish_bar(&lua, "DP-1").unwrap();
    load_widget_script(&lua, "workspaces", crate::config::SEED_WORKSPACES_LUA).unwrap();
    match parse_node(&call_lua_value(&lua, "view").unwrap()).unwrap() {
        WidgetNode::ListView {
            id,
            horizontal,
            items,
            ..
        } => {
            assert_eq!(id, "wayland-strip");
            assert!(horizontal);
            let labels: Vec<_> = items
                .iter()
                .map(|(_, item)| match item {
                    WidgetNode::Button { label, action, .. } => {
                        assert_eq!(action, "code");
                        label.as_str()
                    }
                    other => panic!("button: {other:?}"),
                })
                .collect();
            assert_eq!(labels, ["1"]);
        }
        other => panic!("listview: {other:?}"),
    }
    let os: mlua::Table = lua.globals().get("os").unwrap();
    os.set(
        "execute",
        lua.create_function(|lua, command: String| lua.globals().set("dispatched", command))
            .unwrap(),
    )
    .unwrap();
    call_lua_named_action(&lua, "1001").unwrap();
    assert_eq!(
        lua.globals().get::<String>("dispatched").unwrap(),
        "hyprctl dispatch 'hl.dsp.focus({workspace = 1001})' >/dev/null 2>&1"
    );
}
