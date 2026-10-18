use super::*;
use crate::ui::{
    decode::parse_node,
    listview::{Axis, ListView},
    node::WidgetNode,
};
use std::{collections::VecDeque, time::Duration};

const VIEW: &str = r#"
    return iced.listview(items):id('test'):key('id')
        :delegate(function(item) return iced.text(item.id) end)
        :onEntered({duration=100, x={from=20,to=0}, callback=function(item)
            hits[#hits+1]='enter:'..item.id; return notification.dismiss(item.id) end})
        :onExit({duration=100, callback=function(item)
            hits[#hits+1]='exit:'..item.id; return notification.dismiss(item.id) end})
        :onDisplaced({duration=100, callback=function(item)
            hits[#hits+1]='move:'..item.id end})
"#;
fn lua() -> Lua {
    let lua = crate::lua::sandbox::new_lua(crate::lua::sandbox::Profile::App).unwrap();
    crate::ui::dsl::inject_ui_base(&lua).unwrap();
    lua.globals()
        .set("hits", lua.create_table().unwrap())
        .unwrap();
    lua
}
fn node(lua: &Lua, items: &str) -> WidgetNode {
    lua.globals()
        .set(
            "items",
            lua.load(format!("return {items}")).eval::<Value>().unwrap(),
        )
        .unwrap();
    parse_node(&lua.load(VIEW).eval().unwrap()).unwrap()
}
fn list(node: &WidgetNode) -> ListView<String, String> {
    let WidgetNode::ListView {
        callbacks,
        transitions,
        ..
    } = node
    else {
        panic!("list");
    };
    let mut list = ListView::new(32.0);
    list.set_transitions(
        transitions.0.clone(),
        transitions.1.clone(),
        transitions.2.clone(),
        true,
    );
    list.set_callbacks(callbacks, str::to_owned);
    list
}
fn invoke(callback: &Callback) -> mlua::Result<Option<Value>> {
    invoke_with_queue(callback, &VecDeque::new())
}
fn invoke_with_queue(
    callback: &Callback,
    queue: &VecDeque<crate::shell::screens::Notification>,
) -> mlua::Result<Option<Value>> {
    let system = sysinfo::System::new();
    let theme = crate::config::ThemeConfig::default();
    let outputs = Default::default();
    let toplevels = Default::default();
    let workspaces = Default::default();
    callback.invoke(
        &crate::services::ServiceCtx {
            sys: &system,
            gpu: None,
            sampled: 0.0,
            theme: &theme,
            outputs: &outputs,
            notifications: queue,
            toplevels: &toplevels,
            workspaces: &workspaces,
        },
        |item| crate::shell::screens::notification::callback_is_current(item, queue),
    )
}
fn hits(lua: &Lua) -> Vec<String> {
    lua.globals()
        .get::<Table>("hits")
        .unwrap()
        .sequence_values()
        .collect::<mlua::Result<_>>()
        .unwrap()
}

#[test]
fn completion_runs_once_after_motion_and_never_during_decode_or_sweep() {
    let lua = lua();
    let node = node(&lua, "{{id=7}}");
    let mut list = list(&node);
    let mut motion = crate::ui::anim::AnimRuntime::default();
    let key = "7".to_string();
    list.update(
        &mut motion,
        Duration::from_millis(100),
        Axis::Vertical,
        &[],
        std::slice::from_ref(&key),
        &[],
    );
    motion.tick(Duration::from_millis(99));
    list.sweep(&mut motion);
    assert!(list.take_completed().is_empty());
    motion.tick(Duration::from_millis(1));
    list.sweep(&mut motion);
    let ready = list.take_completed();
    assert_eq!(ready.len(), 1);
    assert!(hits(&lua).is_empty());
    let value = invoke(&ready[0]).unwrap().unwrap();
    assert_eq!(
        crate::lua::notifications::notification_action(&value),
        Some(crate::lua::notifications::NotificationAction::Dismiss(7))
    );
    assert_eq!(hits(&lua), ["enter:7"]);
    list.update(
        &mut motion,
        Duration::from_millis(100),
        Axis::Vertical,
        std::slice::from_ref(&key),
        &[],
        &[(0, key.clone(), "old".into())],
    );
    motion.tick(Duration::from_millis(100));
    list.sweep(&mut motion);
    let ready = list.take_completed();
    assert_eq!(ready.len(), 1);
    invoke(&ready[0]).unwrap();
    assert_eq!(hits(&lua), ["enter:7", "exit:7"]);
    list.sweep(&mut motion);
    assert!(list.take_completed().is_empty());
}

#[test]
fn readding_and_teardown_cancel_pending_exit_callbacks() {
    let lua = lua();
    let node = node(&lua, "{{id=7}}");
    let mut list = list(&node);
    let mut motion = crate::ui::anim::AnimRuntime::default();
    let key = "7".to_string();
    list.update(
        &mut motion,
        Duration::from_millis(100),
        Axis::Vertical,
        std::slice::from_ref(&key),
        &[],
        &[(0, key.clone(), "old".into())],
    );
    motion.tick(Duration::from_millis(40));
    list.update(
        &mut motion,
        Duration::from_millis(100),
        Axis::Vertical,
        &[],
        std::slice::from_ref(&key),
        &[],
    );
    motion.tick(Duration::from_millis(100));
    list.sweep(&mut motion);
    for callback in list.take_completed() {
        invoke(&callback).unwrap();
    }
    assert_eq!(hits(&lua), ["enter:7"]);
    list.update(
        &mut motion,
        Duration::from_millis(100),
        Axis::Vertical,
        std::slice::from_ref(&key),
        &[],
        &[(0, key.clone(), "old".into())],
    );
    list.clear_all(&mut motion);
    motion.tick(Duration::from_millis(200));
    list.sweep(&mut motion);
    assert!(list.take_completed().is_empty());
    assert!(!list.has_callbacks());
}

#[test]
fn displaced_callbacks_complete_for_the_moved_item() {
    let lua = lua();
    let node = node(&lua, "{{id=1},{id=2}}");
    let mut list = list(&node);
    let mut motion = crate::ui::anim::AnimRuntime::default();
    let keys = ["1".to_string(), "2".to_string()];
    list.update(
        &mut motion,
        Duration::from_millis(100),
        Axis::Vertical,
        &keys,
        &[keys[1].clone()],
        &[(0, keys[0].clone(), "old".into())],
    );
    motion.tick(Duration::from_millis(100));
    list.sweep(&mut motion);
    for callback in list.take_completed() {
        invoke(&callback).unwrap();
    }
    let mut hits = hits(&lua);
    hits.sort();
    assert_eq!(hits, ["exit:1", "move:2"]);
}

#[test]
fn reload_and_dropped_vm_invalidate_tokens_without_retaining_the_vm() {
    let lua = lua();
    let node = node(&lua, "{{id=7}}");
    let WidgetNode::ListView { callbacks, .. } = node else {
        panic!("list");
    };
    let callback = callbacks[0].1[1].clone().unwrap();
    new_epoch(&lua).unwrap();
    assert!(invoke(&callback).unwrap().is_none());
    drop(lua);
    assert!(callback.0.lua.try_upgrade().is_none());
    assert!(invoke(&callback).unwrap().is_none());
}

#[test]
fn invalid_callbacks_are_rejected_even_for_empty_lists() {
    let lua = lua();
    let value = lua.load("return iced.listview({}):id('x'):key('id'):delegate(function() return iced.space() end):onExit({callback=42})").eval().unwrap();
    assert!(
        parse_node(&value)
            .unwrap_err()
            .contains("callback must be a function")
    );
}

#[test]
fn callback_budget_errors_recover_and_tokens_cannot_cross_vms() {
    let lua = lua();
    let value = lua.load("return iced.listview({{id=7}}):id('x'):key('id'):delegate(function()return iced.space()end):onExit({callback=function()while true do end end})").eval().unwrap();
    let node = parse_node(&value).unwrap();
    let WidgetNode::ListView { callbacks, .. } = &node else {
        panic!("list");
    };
    let callback = callbacks[0].1[1].as_ref().unwrap();
    assert!(
        invoke(callback)
            .unwrap_err()
            .to_string()
            .contains("instruction budget")
    );
    let other = super::tests::lua();
    assert!(crate::ui::encode::to_table(&other, &node).is_err());
    let good = super::tests::node(&lua, "{{id=8}}");
    let WidgetNode::ListView { callbacks, .. } = good else {
        panic!("list");
    };
    invoke(callbacks[0].1[1].as_ref().unwrap()).unwrap();
    assert_eq!(hits(&lua), ["exit:8"]);
}

#[test]
fn measured_scene_callbacks_follow_actual_displacement_and_zero_duration_completes() {
    use crate::ui::{scene::Lists, widgets::measured::Measurement};
    let lua = lua();
    let old = node(&lua, "{{id=1},{id=2}}");
    let mut lists = Lists::default();
    let mut motion = crate::ui::anim::AnimRuntime::default();
    lists
        .synchronize(
            None,
            &old,
            &Measurement::default(),
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    motion.tick(Duration::from_millis(100));
    lists.sweep(&mut motion);
    assert_eq!(lists.take_completed().len(), 2);
    let mut before = Measurement::default();
    before.items.insert(
        "4:test2".into(),
        iced::Rectangle::new(iced::Point::new(0.0, 40.0), iced::Size::new(100.0, 30.0)),
    );
    let new = node(&lua, "{{id=2}}");
    lists
        .synchronize(
            Some(&old),
            &new,
            &before,
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    let mut after = Measurement::default();
    after.items.insert(
        "4:test2".into(),
        iced::Rectangle::new(iced::Point::ORIGIN, iced::Size::new(100.0, 30.0)),
    );
    assert!(lists.layout(&after, &mut motion, Duration::from_millis(100)));
    motion.tick(Duration::from_millis(100));
    lists.sweep(&mut motion);
    for callback in lists.take_completed() {
        invoke(&callback).unwrap();
    }
    let mut completed = hits(&lua);
    completed.sort();
    assert_eq!(completed, ["exit:1", "move:2"]);
    lists.clear(&mut motion);
    let zero = parse_node(
        &lua.load(VIEW.replace("duration=100", "duration=0"))
            .eval()
            .unwrap(),
    )
    .unwrap();
    lists
        .synchronize(
            None,
            &zero,
            &Measurement::default(),
            &mut motion,
            Duration::ZERO,
        )
        .unwrap();
    assert!(lists.has_callbacks());
    motion.tick(Duration::ZERO);
    lists.sweep(&mut motion);
    assert_eq!(lists.take_completed().len(), 1);
    assert!(!lists.has_callbacks());
}

#[test]
fn replacement_guard_prevents_old_callback_from_running_against_new_record() {
    let lua = lua();
    let node = node(&lua, "{{id=7,_notification_generation=1}}");
    let WidgetNode::ListView { callbacks, .. } = node else {
        panic!("list");
    };
    let mut replacement = crate::shell::screens::Notification::internal(
        &crate::config::NotificationConfig::default(),
        "new",
        "body".into(),
        1,
    );
    replacement.id = 7;
    replacement.generation = 2;
    let queue = VecDeque::from([replacement]);
    assert!(
        invoke_with_queue(callbacks[0].1[1].as_ref().unwrap(), &queue)
            .unwrap()
            .is_none()
    );
    assert!(hits(&lua).is_empty());
}

#[test]
fn completed_exit_releases_captured_item_userdata() {
    struct Owner {
        _owner: Arc<()>,
    }
    impl UserData for Owner {}
    let lua = lua();
    let owner = Arc::new(());
    let original = node(&lua, "{{id=7}}");
    let items: Table = lua.globals().get("items").unwrap();
    items
        .get::<Table>(1)
        .unwrap()
        .set(
            "image",
            lua.create_userdata(Owner {
                _owner: owner.clone(),
            })
            .unwrap(),
        )
        .unwrap();
    drop(items);
    drop(original);
    let node = parse_node(&lua.load(VIEW).eval().unwrap()).unwrap();
    let mut list = list(&node);
    let mut motion = crate::ui::anim::AnimRuntime::default();
    list.update(
        &mut motion,
        Duration::from_millis(100),
        Axis::Vertical,
        &["7".into()],
        &[],
        &[(0, "7".into(), "old".into())],
    );
    lua.globals().set("items", Value::Nil).unwrap();
    drop(node);
    lua.gc_collect().unwrap();
    lua.expire_registry_values();
    lua.gc_collect().unwrap();
    assert_eq!(Arc::strong_count(&owner), 2);
    motion.tick(Duration::from_millis(100));
    list.sweep(&mut motion);
    drop(list.take_completed());
    lua.expire_registry_values();
    lua.gc_collect().unwrap();
    assert_eq!(Arc::strong_count(&owner), 1);
}
