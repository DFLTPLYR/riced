use super::*;
use iced::futures::StreamExt;

#[test]
fn forced_service_refresh_preserves_scene_state_and_does_not_call_on_tick() {
    let dir = directory("wayland-delivery");
    let path = dir.join("scene.lua");
    std::fs::write(
        &path,
        r#"
        return { views=0, ticks=0,
            on_tick=function(self) self.ticks=self.ticks+1 end,
            view=function(self)
                self.views=self.views+1
                self.list=self.list or wayland.workspaces
                return iced.text(self.views .. ':' .. self.ticks .. ':' .. #self.list)
            end }
    "#,
    )
    .unwrap();
    let spec = Spec {
        id: "ws".into(),
        source: path.to_string_lossy().into(),
        input: input(0),
    };
    let mut scene = Scene::default();
    let mut motion = Default::default();
    assert!(render(&mut scene, &spec, &mut motion, false));
    let system = sysinfo::System::new();
    let workspaces = crate::services::WorkspaceCache::from_rows(vec![crate::services::Workspace {
        name: "code".into(),
        ..Default::default()
    }]);
    assert!(scene.render(
        &spec,
        &ServiceCtx {
            sys: &system,
            gpu: None,
            sampled: 0.0,
            theme: &Default::default(),
            component_defaults: &Default::default(),
            outputs: &Default::default(),
            notifications: &Default::default(),
            toplevels: &Default::default(),
            workspaces: &workspaces,
            tray: &Default::default(),
        },
        "DP-1",
        &mut motion,
        true
    ));
    assert!(matches!(scene.node, Some(WidgetNode::Text { ref content, .. }) if content == "2:0:1"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn relative_scene_sources_resolve_in_components_and_absolute_paths_pass_through() {
    for name in ["notifications.lua", "scenes/status.lua"] {
        assert_eq!(source_path(name), config::components_dir().join(name));
        assert_eq!(source_path(name), crate::lua::composable::source_path(name));
    }
    let absolute = std::env::current_dir().unwrap().join("absolute-scene.lua");
    assert_eq!(source_path(absolute.to_str().unwrap()), absolute);
    assert_eq!(
        source_path(&config::NotificationConfig::default().source),
        config::components_dir().join("notifications.lua")
    );
}
fn directory(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("riced-overlay-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
fn input(value: u32) -> Input {
    Arc::new(move |lua| {
        let table = lua.create_table()?;
        table.set("value", value)?;
        Ok(table)
    })
}
fn render(
    scene: &mut Scene,
    spec: &Spec,
    motion: &mut crate::ui::anim::AnimRuntime,
    force: bool,
) -> bool {
    let system = sysinfo::System::new();
    let theme = config::ThemeConfig::default();
    let outputs = HashMap::new();
    let notifications = std::collections::VecDeque::new();
    let toplevels = Default::default();
    let workspaces = Default::default();
    scene.render(
        spec,
        &ServiceCtx {
            sys: &system,
            gpu: None,
            sampled: 0.0,
            theme: &theme,
            component_defaults: &Default::default(),
            outputs: &outputs,
            notifications: &notifications,
            toplevels: &toplevels,
            workspaces: &workspaces,
            tray: &Default::default(),
        },
        "DP-1",
        motion,
        force,
    )
}
fn action(scene: &mut Scene, action: &Action) -> mlua::Value {
    let system = sysinfo::System::new();
    let theme = config::ThemeConfig::default();
    let outputs = HashMap::new();
    let notifications = std::collections::VecDeque::new();
    let toplevels = Default::default();
    let workspaces = Default::default();
    scene
        .runtime
        .as_mut()
        .unwrap()
        .scene_action(
            &ServiceCtx {
                sys: &system,
                gpu: None,
                sampled: 0.0,
                theme: &theme,
                component_defaults: &Default::default(),
                outputs: &outputs,
                notifications: &notifications,
                toplevels: &toplevels,
                workspaces: &workspaces,
                tray: &Default::default(),
            },
            "DP-1",
            action,
        )
        .unwrap()
}
fn messages(task: Task<Plant>) -> Vec<iced_runtime::Action<Plant>> {
    iced_runtime::task::into_stream(task)
        .map(|stream| iced::futures::executor::block_on(stream.collect()))
        .unwrap_or_default()
}

#[test]
fn shared_surfaces_are_full_output_passive_and_reused() {
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let output = OutputId(1);
    let created = messages(ensure(&mut plots, output));
    let [iced_runtime::Action::Output(Plant::NewLayerShell { id, settings })] = created.as_slice()
    else {
        panic!("create overlay")
    };
    assert_eq!(settings.anchor, Anchor::all());
    assert_eq!(settings.size, LayerSize::FILL);
    assert_eq!(settings.margin, Some((0, 0, 0, 0)));
    assert_eq!(settings.layer, Layer::Overlay);
    assert_eq!(settings.keyboard_interactivity, KeyboardInteractivity::None);
    assert!(settings.events_transparent);
    assert_eq!(plots.windows.overlays[&output], *id);
    assert!(messages(ensure(&mut plots, output)).is_empty());
    assert_eq!(plots.windows.overlays.len(), 1);
}

#[test]
fn measured_masks_merge_scenes_and_reject_stale_revisions_and_surfaces() {
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let output = OutputId(1);
    let _ = ensure(&mut plots, output);
    let surface = plots.windows.overlays[&output];
    for name in ["one", "two"] {
        plots.desktop.overlays.scenes.insert(
            (output, name.into()),
            Scene {
                revision: 7,
                ..Default::default()
            },
        );
    }
    let event = |scene: &str, revision, surface, x| OverlayEvent::Measured {
        surface,
        output,
        scene: scene.into(),
        revision,
        data: Measurement {
            regions: vec![iced::Rectangle::new(
                iced::Point::new(x, 20.0),
                iced::Size::new(30.0, 40.0),
            )],
            ..Default::default()
        },
    };
    assert!(messages(update(&mut plots, event("one", 6, surface, 10.0))).is_empty());
    assert!(
        messages(update(
            &mut plots,
            event("one", 7, window::Id::unique(), 10.0)
        ))
        .is_empty()
    );
    assert_eq!(
        messages(update(&mut plots, event("one", 7, surface, 10.0))).len(),
        1
    );
    assert!(messages(update(&mut plots, event("one", 7, surface, 10.0))).is_empty());
    let _ = update(&mut plots, event("two", 7, surface, 100.0));
    assert_eq!(
        plots.desktop.overlays.masks[&output],
        vec![(10, 20, 30, 40), (100, 20, 30, 40)]
    );
    let _ = update(
        &mut plots,
        OverlayEvent::Measured {
            surface,
            output,
            scene: "one".into(),
            revision: 7,
            data: Measurement::default(),
        },
    );
    assert_eq!(
        plots.desktop.overlays.masks[&output],
        vec![(100, 20, 30, 40)]
    );
}

#[test]
fn sources_reload_only_after_successful_decode_and_keep_last_good_state() {
    let dir = directory("reload");
    let path = dir.join("scene.lua");
    std::fs::write(
        &path,
        "return {view=function(self,p) return iced.text('valid:'..p.value) end}",
    )
    .unwrap();
    let spec = Spec {
        id: "test".into(),
        source: path.display().to_string(),
        input: input(1),
    };
    let mut scene = Scene::default();
    let mut motion = crate::ui::anim::AnimRuntime::default();
    assert!(render(&mut scene, &spec, &mut motion, false));
    let previous = scene.node.clone();
    assert!(!render(&mut scene, &spec, &mut motion, false));
    std::fs::write(
        &path,
        "return {view=function() return iced.slider(1,1,1) end}",
    )
    .unwrap();
    assert!(!render(&mut scene, &spec, &mut motion, false));
    assert_eq!(scene.node, previous);
    let updated = Spec {
        input: input(2),
        id: spec.id.clone(),
        source: spec.source.clone(),
    };
    assert!(
        render(&mut scene, &updated, &mut motion, true),
        "the last good runtime must still handle fresh input"
    );
    assert!(matches!(&scene.node,Some(WidgetNode::Text {content,..}) if content=="valid:2"));
    std::fs::write(
        &path,
        "return {view=function(self,p) return iced.text('repaired:'..p.value) end}",
    )
    .unwrap();
    assert!(render(&mut scene, &spec, &mut motion, false));
    assert_ne!(scene.node, previous);
    assert!(scene.error.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn hover_rerenders_only_on_flag_changes_and_ignores_stale_events() {
    let dir = directory("hover");
    let path = dir.join("scene.lua");
    std::fs::write(
        &path,
        "return {view=function(self, props) return iced.listview(props.notifications):id('rows'):key('id'):delegate(function(n) return iced.text(n.hovered and 'hot' or 'cold') end) end}",
    )
    .unwrap();
    let input: Input = Arc::new(move |lua| {
        let props = lua.create_table()?;
        let items = lua.create_table()?;
        for (index, id) in [7u32, 9].iter().enumerate() {
            let item = lua.create_table()?;
            item.set("id", *id)?;
            items.set(index + 1, item)?;
        }
        props.set("notifications", items)?;
        Ok(props)
    });
    let spec = Spec {
        id: "test".into(),
        source: path.display().to_string(),
        input,
    };
    let mut scene = Scene::default();
    let mut motion = crate::ui::anim::AnimRuntime::default();
    assert!(render(&mut scene, &spec, &mut motion, true));
    let hot = |scene: &Scene| {
        let WidgetNode::ListView { items, .. } = scene.node.as_ref().unwrap() else {
            panic!("list");
        };
        items
            .iter()
            .map(|(_, node)| match node {
                WidgetNode::Text { content, .. } => content.clone(),
                other => panic!("text, got {other:?}"),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(hot(&scene), ["cold", "cold"]);
    // Hover flags re-decode without touching the source or runtime.
    scene.hovered.insert(("rows".into(), "9".into()));
    assert!(render(&mut scene, &spec, &mut motion, false));
    assert_eq!(hot(&scene), ["cold", "hot"]);
    assert_eq!(scene.rendered_hovered, scene.hovered);
    assert!(!render(&mut scene, &spec, &mut motion, false));
    scene.hovered.clear();
    assert!(render(&mut scene, &spec, &mut motion, false));
    assert_eq!(hot(&scene), ["cold", "cold"]);
    // Event routing updates the set with per-scene exclusivity and
    // rejects unknown scenes and surfaces without touching Lua.
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let output = OutputId(9);
    let surface = window::Id::unique();
    plots.windows.overlays.insert(output, surface);
    plots
        .desktop
        .overlays
        .scenes
        .insert((output, "test".into()), Scene::default());
    let hover = |list: &str, key: &str, entered: bool| OverlayEvent::Hover {
        surface,
        output,
        scene: "test".into(),
        list: list.into(),
        key: key.into(),
        entered,
    };
    let set = |plots: &Plots| {
        plots.desktop.overlays.scenes[&(output, "test".to_string())]
            .hovered
            .clone()
    };
    let _ = update(&mut plots, hover("rows", "9", true));
    let _ = update(&mut plots, hover("rows", "7", true));
    assert_eq!(set(&plots), [("rows".to_string(), "7".to_string())].into());
    let _ = update(&mut plots, hover("rows", "7", true));
    let _ = update(&mut plots, hover("rows", "7", false));
    assert!(set(&plots).is_empty());
    // Wrong surfaces and unknown scenes are ignored without touching Lua.
    let _ = update(
        &mut plots,
        OverlayEvent::Hover {
            surface: window::Id::unique(),
            output,
            scene: "test".into(),
            list: "rows".into(),
            key: "9".into(),
            entered: true,
        },
    );
    assert!(set(&plots).is_empty());
    let _ = update(&mut plots, hover("rows", "9", true));
    assert_eq!(set(&plots), [("rows".to_string(), "9".to_string())].into());
    let _ = update(
        &mut plots,
        OverlayEvent::Hover {
            surface,
            output,
            scene: "missing".into(),
            list: "rows".into(),
            key: "7".into(),
            entered: true,
        },
    );
    assert_eq!(set(&plots), [("rows".to_string(), "9".to_string())].into());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn independent_scene_vms_and_opt_in_ticks_do_not_share_state() {
    let dir = directory("ticks");
    let path = dir.join("scene.lua");
    std::fs::write(&path,"return {count=0,on_tick=function(self)self.count=self.count+1 end,view=function(self,p)return iced.text(self.count..':'..p.value)end}").unwrap();
    let spec = Spec {
        id: "test".into(),
        source: path.display().to_string(),
        input: input(3),
    };
    let mut one = Scene::default();
    let mut two = Scene::default();
    let mut motion = crate::ui::anim::AnimRuntime::default();
    render(&mut one, &spec, &mut motion, false);
    render(&mut two, &spec, &mut motion, false);
    assert_eq!(one.node, two.node);
    assert!(render(&mut one, &spec, &mut motion, false));
    assert_ne!(one.node, two.node);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn default_notification_scene_composes_scoped_data_and_routes_structured_actions() {
    use crate::shell::screens::notification::Notification;
    let dir = directory("default");
    let source = dir.join("scene.lua");
    let card = dir.join("card.lua");
    std::fs::write(
        &card,
        include_str!("../../../scripts/components/notification_card.lua"),
    )
    .unwrap();
    std::fs::write(
        &source,
        include_str!("../../../scripts/components/notifications.lua")
            .replace("notification_card.lua", &card.display().to_string()),
    )
    .unwrap();
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    for (id, output) in [(1, OutputId(1)), (2, OutputId(2)), (3, OutputId(1))] {
        let mut item =
            Notification::internal(&plots.config.notifications, "title", "body".into(), 1);
        item.id = id;
        item.output = Some(output);
        item.actions = vec![("open:details".into(), "Open".into())];
        plots.notification.queue.push_back(item);
    }
    let mut spec = crate::shell::screens::notification::scene_spec(&plots, OutputId(1));
    spec.source = source.display().to_string();
    let mut scene = Scene::default();
    let mut motion = crate::ui::anim::AnimRuntime::default();
    assert!(
        render(&mut scene, &spec, &mut motion, true),
        "{:?}",
        scene.error
    );
    // Decode alone cannot detect a collapsed list. Exercise the complete
    // supplied scene after its real enter transition, with fill-width cards.
    use iced::advanced::{layout, renderer::Headless, widget::Tree};
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(13.0),
        Some("tiny-skia"),
    ))
    .unwrap();
    motion.tick(std::time::Duration::from_millis(300));
    scene.lists.sweep(&mut motion);
    let body = scene
        .lists
        .build(
            scene.node.as_ref().unwrap(),
            &motion,
            Arc::new(|_| Plant::Tend),
            None,
        )
        .unwrap();
    let mut body = measured::root(
        body,
        scene.frame.clone(),
        scene.revision,
        Arc::new(|_| Plant::Tend),
    );
    let mut tree = Tree::new(&body);
    let size = iced::Size::new(1000.0, 600.0);
    body.as_widget_mut()
        .layout(&mut tree, &renderer, &layout::Limits::new(size, size));
    let measured = scene.frame.lock().unwrap().last.clone().unwrap();
    assert_eq!(measured.items.len(), 2);
    for bounds in measured.items.values() {
        assert_eq!(
            bounds.width, 360.0,
            "notification card collapsed: {bounds:?}"
        );
        assert!(
            bounds.height > 0.0,
            "notification card has no visible height"
        );
    }
    assert!(
        !measured.regions.is_empty(),
        "visible cards need live hit regions"
    );
    let (node, layout) = crate::ui::layout_props::unwrap(scene.node.clone().unwrap());
    assert_eq!(layout.anchor.unwrap().name(), "top_right");
    let WidgetNode::ListView { items, .. } = node else {
        panic!("whole list scene")
    };
    assert_eq!(
        items
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec!["3", "1"]
    );
    let value = action(&mut scene, &Action::from("invoke:1:open:details"));
    assert_eq!(
        crate::lua::notifications::notification_action(&value),
        Some(crate::lua::notifications::NotificationAction::Invoke(
            1,
            "open:details".into()
        ))
    );
    let value = action(&mut scene, &Action::from("dismiss:3"));
    assert_eq!(
        crate::lua::notifications::notification_action(&value),
        Some(crate::lua::notifications::NotificationAction::Dismiss(3))
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn typed_control_actions_mutate_only_the_owning_scene() {
    let dir = directory("action");
    let path = dir.join("scene.lua");
    std::fs::write(&path,"return {value=0,view=function(self)return iced.slider(0,100,self.value):on_change('value')end,actions=function(self,events)if events.type=='action' then self.value=events.value end end}").unwrap();
    let spec = Spec {
        id: "test".into(),
        source: path.display().to_string(),
        input: input(0),
    };
    let mut one = Scene::default();
    let mut two = Scene::default();
    let mut motion = crate::ui::anim::AnimRuntime::default();
    render(&mut one, &spec, &mut motion, true);
    render(&mut two, &spec, &mut motion, true);
    action(
        &mut one,
        &Action::change("value".into(), crate::ui::action::ActionValue::Number(42.0)),
    );
    render(&mut one, &spec, &mut motion, true);
    assert!(matches!(
        one.node,
        Some(WidgetNode::Slider { value: 42.0, .. })
    ));
    assert!(matches!(
        two.node,
        Some(WidgetNode::Slider { value: 0.0, .. })
    ));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn opaque_images_drop_after_scene_props_and_ir_stop_referencing_them() {
    let dir = directory("images");
    let path = dir.join("scene.lua");
    std::fs::write(&path,"return {view=function(self,p) if p.image then return iced.image(p.image) else return iced.space() end end}").unwrap();
    let pixels: Arc<[u8]> = vec![127; 64 * 64 * 4].into();
    let weak = Arc::downgrade(&pixels);
    let image = iced::widget::image::Handle::from_rgba(64, 64, bytes::Bytes::from_owner(pixels));
    let spec = Spec {
        id: "test".into(),
        source: path.display().to_string(),
        input: Arc::new(move |lua| {
            let table = lua.create_table()?;
            table.set(
                "image",
                lua.create_userdata(crate::ui::image_value::ImageValue(image.clone()))?,
            )?;
            Ok(table)
        }),
    };
    let mut scene = Scene::default();
    let mut motion = crate::ui::anim::AnimRuntime::default();
    assert!(render(&mut scene, &spec, &mut motion, true));
    drop(spec);
    assert!(weak.upgrade().is_some());
    let spec = Spec {
        id: "test".into(),
        source: path.display().to_string(),
        input: input(0),
    };
    assert!(render(&mut scene, &spec, &mut motion, true));
    assert!(weak.upgrade().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn scene_sources_are_instruction_bounded() {
    let dir = directory("budget");
    let path = dir.join("scene.lua");
    std::fs::write(&path, "while true do end").unwrap();
    let spec = Spec {
        id: "test".into(),
        source: path.display().to_string(),
        input: input(0),
    };
    let mut scene = Scene::default();
    assert!(!render(
        &mut scene,
        &spec,
        &mut crate::ui::anim::AnimRuntime::default(),
        true
    ));
    assert!(scene.node.is_none());
    assert!(scene.error.is_some());
    std::fs::remove_dir_all(dir).unwrap();
}
