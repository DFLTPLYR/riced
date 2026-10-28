use super::*;
use crate::ui::{
    decode::parse_node,
    widgets::measured::{Frame, SharedFrame},
};
use iced::advanced::{layout, renderer::Headless, widget::Tree};
use std::sync::{Arc, Mutex};

fn renderer() -> iced::Renderer {
    iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(13.0),
        Some("tiny-skia"),
    ))
    .unwrap()
}
fn node(anchor: &str, heights: &[u32]) -> WidgetNode {
    let lua = mlua::Lua::new();
    crate::ui::dsl::inject_ui_base(&lua).unwrap();
    let items = heights
        .iter()
        .enumerate()
        .map(|(index, height)| format!("{{id={},height={height}}}", index + 1))
        .collect::<Vec<_>>()
        .join(",");
    parse_node(&lua.load(format!(r##"return iced.listview({{{items}}}):id('cards'):key('id')
        :delegate(function(n) return iced.container(iced.space()):width(240):height(n.height):background('#123456'):on_press('hit:'..n.id) end)
        :onEntered({{x={{from=0,to=0}},opacity={{from=1,to=1}},duration=0}})
        :onExit({{x={{to=-100}},opacity={{to=1}},duration=100}})
        :spacing(7):width(240):anchor('{anchor}'):margin(16)"##)).eval().unwrap()).unwrap()
}
fn element(
    lists: &Lists,
    node: &WidgetNode,
    motion: &AnimRuntime,
    frame: SharedFrame,
) -> Element<'static, Plant> {
    let body = lists
        .build(
            node,
            motion,
            Arc::new(|action| {
                Plant::Overlay(crate::shell::overlay::OverlayEvent::Action {
                    surface: iced::window::Id::unique(),
                    output: iced_wayland_subscriber::OutputId(1),
                    scene: "test".into(),
                    action,
                })
            }),
            None,
            &mut crate::ui::build::no_native_slots::<Plant>(),
        )
        .unwrap();
    measured::root(body, frame, 1, Arc::new(|_| Plant::Tend))
}
fn layout(
    element: &mut Element<'static, Plant>,
    tree: &mut Tree,
    renderer: &iced::Renderer,
) -> layout::Node {
    element.as_widget_mut().layout(
        tree,
        renderer,
        &layout::Limits::new(iced::Size::new(800.0, 600.0), iced::Size::new(800.0, 600.0)),
    )
}
fn remeasure(
    element: &mut Element<'static, Plant>,
    tree: &mut Tree,
    node: &layout::Node,
    renderer: &iced::Renderer,
) -> Vec<Plant> {
    let mut messages = Vec::new();
    element.as_widget_mut().update(
        tree,
        &iced::Event::Window(iced::window::Event::RedrawRequested(
            std::time::Instant::now(),
        )),
        iced::advanced::Layout::new(node),
        iced::mouse::Cursor::Unavailable,
        renderer,
        &mut iced::advanced::clipboard::Null,
        &mut iced::advanced::Shell::new(&mut messages),
        &iced::Rectangle::with_size(iced::Size::new(800.0, 600.0)),
    );
    messages
}

#[test]
fn all_anchors_measure_actual_variable_height_rows() {
    let renderer = renderer();
    for (anchor, expected) in [
        ("top-left", (16.0, 16.0)),
        ("top-center", (280.0, 16.0)),
        ("top-right", (544.0, 16.0)),
        ("left", (16.0, 249.0)),
        ("center", (280.0, 249.0)),
        ("right", (544.0, 249.0)),
        ("bottom-left", (16.0, 482.0)),
        ("bottom-center", (280.0, 482.0)),
        ("bottom-right", (544.0, 482.0)),
    ] {
        let node = node(anchor, &[31, 64]);
        let mut lists = Lists::default();
        let mut motion = AnimRuntime::default();
        lists
            .synchronize(
                None,
                &node,
                &Measurement::default(),
                &mut motion,
                Duration::from_millis(100),
            )
            .unwrap();
        let frame = Arc::new(Mutex::new(Frame::default()));
        let mut element = element(&lists, &node, &motion, frame.clone());
        let mut tree = Tree::new(&element);
        let native = layout(&mut element, &mut tree, &renderer);
        assert_eq!(native.size(), iced::Size::new(800.0, 600.0));
        let data = frame.lock().unwrap().last.clone().unwrap();
        assert_eq!(data.regions.len(), 2, "{anchor}");
        assert_eq!(
            data.regions[0],
            Rectangle::new(
                iced::Point::new(expected.0, expected.1),
                iced::Size::new(240.0, 31.0)
            ),
            "{anchor}"
        );
        assert_eq!(data.regions[1].y, expected.1 + 38.0, "{anchor}");
        assert_eq!(data.regions[1].height, 64.0);
    }
}

#[test]
fn explicit_regions_contribute_hit_rectangles_without_an_action() {
    let lua = mlua::Lua::new();
    crate::ui::dsl::inject_ui_base(&lua).unwrap();
    let node = parse_node(
        &lua.load("return iced.region(iced.container(iced.space()):width(120):height(30))")
            .eval()
            .unwrap(),
    )
    .unwrap();
    let lists = Lists::default();
    let frame = Arc::new(Mutex::new(Frame::default()));
    let motion = AnimRuntime::default();
    let mut element = element(&lists, &node, &motion, frame.clone());
    let mut tree = Tree::new(&element);
    layout(&mut element, &mut tree, &renderer());
    assert_eq!(
        frame.lock().unwrap().last.as_ref().unwrap().regions,
        [Rectangle::new(
            iced::Point::new(0.0, 0.0),
            iced::Size::new(120.0, 30.0)
        )]
    );
}

#[test]
fn native_scroll_measurements_clip_and_translate_live_regions() {
    let renderer = renderer();
    let node = node("top-right", &[100; 20]);
    let mut lists = Lists::default();
    let mut motion = AnimRuntime::default();
    lists
        .synchronize(
            None,
            &node,
            &Measurement::default(),
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    let frame = Arc::new(Mutex::new(Frame::default()));
    let mut element = element(&lists, &node, &motion, frame.clone());
    let mut tree = Tree::new(&element);
    let native = layout(&mut element, &mut tree, &renderer);
    let mut scroll = iced::advanced::widget::operation::scrollable::scroll_to::<()>(
        lists.scroll_id("cards").into(),
        iced::advanced::widget::operation::scrollable::AbsoluteOffset {
            x: Some(0.0),
            y: Some(400.0),
        },
    );
    element.as_widget_mut().operate(
        &mut tree,
        iced::advanced::Layout::new(&native),
        &renderer,
        &mut scroll,
    );
    remeasure(&mut element, &mut tree, &native, &renderer);
    let data = frame.lock().unwrap().last.clone().unwrap();
    assert_eq!(data.items[&item_id("cards", "1")].y, -384.0);
    assert!(
        data.regions
            .iter()
            .all(|rect| rect.y >= 16.0 && rect.y + rect.height <= 584.0)
    );
    assert!(data.regions.len() < 20 && !data.regions.is_empty());
}

#[test]
fn keyed_lists_infer_fill_portion_width_from_children() {
    let renderer = renderer();
    let lua = mlua::Lua::new();
    crate::ui::dsl::inject_ui_base(&lua).unwrap();
    let node = parse_node(&lua.load("return iced.listview({{id=1},{id=2}}):id('portions'):key('id'):delegate(function() return iced.container(iced.space():height(20)):width('fill:3') end):height(100)").eval().unwrap()).unwrap();
    let lists = Lists::default();
    let motion = AnimRuntime::default();
    let frame = Arc::new(Mutex::new(Frame::default()));
    let mut element = element(&lists, &node, &motion, frame.clone());
    let mut tree = Tree::new(&element);
    layout(&mut element, &mut tree, &renderer);
    let measured = frame.lock().unwrap().last.clone().unwrap();
    assert_eq!(measured.items[&item_id("portions", "1")].width, 800.0);
    assert_eq!(measured.items[&item_id("portions", "2")].width, 800.0);
}

#[test]
fn exits_draw_on_the_full_canvas_and_never_contribute_hit_regions() {
    let mut renderer = renderer();
    let old = node("top-right", &[80]);
    let new = node("top-right", &[]);
    let mut lists = Lists::default();
    let mut motion = AnimRuntime::default();
    lists
        .synchronize(
            None,
            &old,
            &Measurement::default(),
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    let frame = Arc::new(Mutex::new(Frame::default()));
    let mut old_element = element(&lists, &old, &motion, frame.clone());
    let mut tree = Tree::new(&old_element);
    layout(&mut old_element, &mut tree, &renderer);
    let before = frame.lock().unwrap().last.clone().unwrap();
    drop(old_element);
    lists
        .synchronize(
            Some(&old),
            &new,
            &before,
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    motion.tick(Duration::from_millis(100));
    let mut element = element(&lists, &new, &motion, frame.clone());
    tree.diff(element.as_widget());
    let native = layout(&mut element, &mut tree, &renderer);
    element.as_widget().draw(
        &tree,
        &mut renderer,
        &iced::Theme::Dark,
        &iced::advanced::renderer::Style::default(),
        iced::advanced::Layout::new(&native),
        iced::mouse::Cursor::Unavailable,
        &Rectangle::with_size(iced::Size::new(800.0, 600.0)),
    );
    let pixels = renderer.screenshot(iced::Size::new(800, 600), 1.0, iced::Color::WHITE);
    let index = (24 * 800 + 452) * 4;
    assert_ne!(
        &pixels[index..index + 3],
        &[255, 255, 255],
        "ghost clipped to its old 544px column origin"
    );
    assert!(
        frame
            .lock()
            .unwrap()
            .last
            .as_ref()
            .unwrap()
            .regions
            .is_empty()
    );
    lists.sweep(&mut motion);
    assert!(
        lists
            .lists
            .values()
            .all(|list| list.ghosts_for(|_| true).is_empty())
    );
}

#[test]
fn duplicate_list_ids_fail_and_authored_transitions_are_retained() {
    let node = node("center", &[20]);
    let mut lists = Lists::default();
    let mut motion = AnimRuntime::default();
    let duplicate = WidgetNode::Row {
        children: vec![node.clone(), node.clone()],
        spacing: 0.0,
        width: super::super::node::NodeLength::Native,
        height: super::super::node::NodeLength::Native,
    };
    assert!(
        lists
            .synchronize(
                None,
                &duplicate,
                &Measurement::default(),
                &mut motion,
                Duration::from_millis(100)
            )
            .is_err()
    );
    lists
        .synchronize(
            None,
            &node,
            &Measurement::default(),
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    assert_eq!(lists.lists["cards"].exit_spec().to.x, -100.0);
    assert_eq!(
        lists.lists["cards"].exit_spec().duration,
        Some(Duration::from_millis(100))
    );
}

#[test]
fn lists_inside_stacks_are_synchronized_and_measured() {
    let node = WidgetNode::Stack {
        children: vec![node("center", &[20, 40])],
        width: super::super::node::NodeLength::Native,
        height: super::super::node::NodeLength::Native,
    };
    let mut lists = Lists::default();
    let mut motion = AnimRuntime::default();
    lists
        .synchronize(
            None,
            &node,
            &Measurement::default(),
            &mut motion,
            Duration::ZERO,
        )
        .unwrap();
    assert!(lists.lists.contains_key("cards"));
    let frame = Arc::new(Mutex::new(Frame::default()));
    let mut element = element(&lists, &node, &motion, frame.clone());
    let mut tree = Tree::new(&element);
    layout(&mut element, &mut tree, &renderer());
    assert_eq!(
        frame.lock().unwrap().last.as_ref().unwrap().regions.len(),
        2
    );
}

#[test]
fn natural_wrapped_text_controls_card_height_instead_of_a_host_constant() {
    let lua = mlua::Lua::new();
    crate::ui::dsl::inject_ui_base(&lua).unwrap();
    let node=parse_node(&lua.load("return iced.listview({{id=1,text='short'},{id=2,text=string.rep('word ',100)}}):id('natural'):key('id'):delegate(function(n)return iced.container(iced.text(n.text):width(size.fill)):padding(8):width(240):on_press('card')end):onEntered({x={from=0,to=0},opacity={from=1,to=1},duration=0}):width(240):anchor('right')").eval().unwrap()).unwrap();
    let mut lists = Lists::default();
    let mut motion = AnimRuntime::default();
    lists
        .synchronize(
            None,
            &node,
            &Measurement::default(),
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    let frame = Arc::new(Mutex::new(Frame::default()));
    let mut element = element(&lists, &node, &motion, frame.clone());
    let mut tree = Tree::new(&element);
    layout(&mut element, &mut tree, &renderer());
    let data = frame.lock().unwrap().last.clone().unwrap();
    assert_eq!(data.regions.len(), 2);
    assert!(data.regions[0].height < 60.0);
    assert!(data.regions[1].height > data.regions[0].height * 2.0);
}

#[test]
fn positioned_clicks_reach_the_row_action_and_empty_space_does_not() {
    let renderer = renderer();
    let node = node("right", &[40, 80]);
    let mut lists = Lists::default();
    let mut motion = AnimRuntime::default();
    lists
        .synchronize(
            None,
            &node,
            &Measurement::default(),
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    let frame = Arc::new(Mutex::new(Frame::default()));
    let mut element = element(&lists, &node, &motion, frame.clone());
    let mut tree = Tree::new(&element);
    let native = layout(&mut element, &mut tree, &renderer);
    let first = frame.lock().unwrap().last.as_ref().unwrap().regions[0].center();
    for (point, expected) in [(iced::Point::ORIGIN, None), (first, Some("hit:1"))] {
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            &mut tree,
            &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            iced::advanced::Layout::new(&native),
            iced::mouse::Cursor::Available(point),
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut iced::advanced::Shell::new(&mut messages),
            &Rectangle::with_size(iced::Size::new(800.0, 600.0)),
        );
        let keys: Vec<_> = messages
            .iter()
            .filter_map(|message| match message {
                Plant::Overlay(crate::shell::overlay::OverlayEvent::Action { action, .. }) => {
                    Some(action.key.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(keys, expected.into_iter().collect::<Vec<_>>());
    }
}

#[test]
fn removing_a_whole_list_keeps_its_exits_then_releases_list_state() {
    let renderer = renderer();
    let old = node("top-right", &[40, 80]);
    let new = WidgetNode::Space {
        width: super::super::node::NodeLength::Native,
        height: super::super::node::NodeLength::Native,
    };
    let mut lists = Lists::default();
    let mut motion = AnimRuntime::default();
    lists
        .synchronize(
            None,
            &old,
            &Measurement::default(),
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    let frame = Arc::new(Mutex::new(Frame::default()));
    let mut element = element(&lists, &old, &motion, frame.clone());
    let mut tree = Tree::new(&element);
    layout(&mut element, &mut tree, &renderer);
    let data = frame.lock().unwrap().last.clone().unwrap();
    lists
        .synchronize(
            Some(&old),
            &new,
            &data,
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    assert_eq!(lists.lists["cards"].ghosts_for(|_| true).len(), 2);
    motion.tick(Duration::from_millis(200));
    lists.sweep(&mut motion);
    assert!(lists.lists.is_empty());
    assert!(lists.origins.is_empty());
}

#[test]
fn image_pixels_survive_only_until_the_last_exit_owner_is_swept() {
    use super::super::node::NodeLength;
    let pixels: Arc<[u8]> = vec![127; 16 * 16 * 4].into();
    let weak = Arc::downgrade(&pixels);
    let image = WidgetNode::Raster {
        handle: iced::widget::image::Handle::from_rgba(16, 16, bytes::Bytes::from_owner(pixels)),
        width: NodeLength::Fixed(16.0),
        height: NodeLength::Fixed(16.0),
    };
    let defaults = ListView::<Key, WidgetNode>::default();
    let old = WidgetNode::ListView {
        id: "images".into(),
        callbacks: Vec::new(),
        items: vec![("one".into(), image)],
        horizontal: false,
        pitch: 0.0,
        spacing: 0.0,
        width: NodeLength::Native,
        height: NodeLength::Native,
        transitions: Box::new((
            defaults.enter_spec(),
            defaults.exit_spec(),
            defaults.displaced_spec(),
        )),
    };
    let new = WidgetNode::Space {
        width: NodeLength::Native,
        height: NodeLength::Native,
    };
    let mut lists = Lists::default();
    let mut motion = AnimRuntime::default();
    lists
        .synchronize(
            None,
            &old,
            &Measurement::default(),
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    motion.tick(Duration::from_millis(200));
    lists.sweep(&mut motion);
    let mut data = Measurement::default();
    data.items.insert(
        item_id("images", "one"),
        Rectangle::new(iced::Point::new(20.0, 20.0), iced::Size::new(16.0, 16.0)),
    );
    lists
        .synchronize(
            Some(&old),
            &new,
            &data,
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    drop(old);
    assert!(weak.upgrade().is_some());
    motion.tick(Duration::from_millis(200));
    lists.sweep(&mut motion);
    assert!(weak.upgrade().is_none());
}

#[test]
fn displacement_uses_measured_row_heights_not_a_fixed_pitch() {
    let renderer = renderer();
    let old = node("top-right", &[31, 64, 95]);
    let mut new = old.clone();
    let WidgetNode::Anchored { child, .. } = &mut new else {
        panic!("anchor")
    };
    let WidgetNode::ListView { items, .. } = child.as_mut() else {
        panic!("list")
    };
    items.remove(0);
    let mut lists = Lists::default();
    let mut motion = AnimRuntime::default();
    lists
        .synchronize(
            None,
            &old,
            &Measurement::default(),
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    let frame = Arc::new(Mutex::new(Frame::default()));
    let mut element = element(&lists, &old, &motion, frame.clone());
    let mut tree = Tree::new(&element);
    layout(&mut element, &mut tree, &renderer);
    let before = frame.lock().unwrap().last.clone().unwrap();
    drop(element);
    lists
        .synchronize(
            Some(&old),
            &new,
            &before,
            &mut motion,
            Duration::from_millis(100),
        )
        .unwrap();
    let mut element = self::element(&lists, &new, &motion, frame.clone());
    tree.diff(element.as_widget());
    layout(&mut element, &mut tree, &renderer);
    let after = frame.lock().unwrap().last.clone().unwrap();
    assert!(lists.layout(&after, &mut motion, Duration::from_millis(100)));
    let measured_delta =
        before.items[&item_id("cards", "2")].y - after.items[&item_id("cards", "2")].y;
    assert!(measured_delta > 30.0 && measured_delta < 40.0);
    assert_eq!(
        lists.moves[&("cards".into(), "2".into())]
            .value(&motion)
            .unwrap()
            .y,
        measured_delta
    );
}
