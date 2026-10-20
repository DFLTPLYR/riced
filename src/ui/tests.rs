//! Integration checks for the component schema, Lua boundary, and native input.
use super::{
    action::{Action, ActionValue},
    build::realize,
    decode::parse_node,
};
use iced::advanced::{Layout, Shell, clipboard, layout, renderer::Headless, widget::Tree};
use iced::{Element, Event, Point, Rectangle, Size, mouse};
use mlua::{FromLua, IntoLua, Lua, Value};

const EXAMPLES: &[&str] = &[
    "iced.image(_owned_image):width(10):height(10)",
    "iced.text('hello'):size(16):wrap('word_or_glyph'):color({r=0.3,g=0.5,b=0.7,a=0.4})",
    "iced.icon('cpu'):color('#aabbcc')",
    "iced.space():width(size.fill):height(8)",
    "iced.image('memory-only.png'):width(20)",
    "iced.container(iced.text('nested')):padding(4):border('#112233')",
    "iced.popup(iced.text('popup')):width(120):padding(6):align_x('center')",
    "iced.scrollable(iced.text('scroll'))",
    "iced.row({iced.text('x'), iced.button('go','go')}, 3)",
    "iced.column({iced.space(), iced.spinner()}):width(size.fill)",
    "iced.stack({iced.space():width(100):height(40), iced.text('on top')}):width(120):height(60)",
    "iced.button('go','go'):padding(8):radius(3)",
    "iced.progress(0.4):height(8)",
    "iced.separator():height(2)",
    "iced.spinner()",
    "iced.listview({{id='a',text='a'}}):id('list'):key('id'):delegate(function(item) return iced.text(item.text) end):onEntered({duration=200,x={from=-10,to=0}})",
    "iced.checkbox('Enable',false):checked(true):on_toggle('check')",
    "iced.toggler('Enable',true):on_toggle('toggle'):size(24)",
    "iced.slider(0,100,40):step(0.5):on_change('volume'):on_release('released'):height(24)",
    "iced.text_input('Search','hello'):on_input('query'):on_submit('search'):secure(false)",
    "iced.pick_list({'one','two'},'two'):on_select('pick'):placeholder('Choose')",
    "iced.radio('Two','two','one'):on_select('radio'):spacing(12)",
    "iced.canvas({lines={{points={{x=0,y=0},{x=8,y=8}},color='#aabbcc',width=1.5}},rects={{x=0,y=0,width=4,height=4,color='#112233'}}}):width(16):height(16)",
    "iced.shader(_shader_path):width(96):height(48):speed(0.5):params({0.2,0.4,0.6}):animated(false)",
];

fn lua() -> Lua {
    let lua = Lua::new();
    super::dsl::inject_ui_base(&lua).unwrap();
    lua.globals()
        .set(
            "_shader_path",
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/shaders/gradient.wgsl"),
        )
        .unwrap();
    lua.globals()
        .set(
            "_owned_image",
            lua.create_userdata(super::image_value::ImageValue(
                iced::widget::image::Handle::from_rgba(1, 1, vec![255; 4]),
            ))
            .unwrap(),
        )
        .unwrap();
    lua
}
fn renderer() -> iced::Renderer {
    iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(13.0),
        Some("tiny-skia"),
    ))
    .unwrap()
}

#[test]
fn every_schema_constructs_round_trips_and_has_native_layout() {
    let lua = lua();
    let renderer = renderer();
    let mut kinds = std::collections::HashSet::new();
    for source in EXAMPLES {
        let value: Value = lua.load(format!("return {source}")).eval().unwrap();
        let node = parse_node(&value).unwrap();
        kinds.insert(node.kind());
        let exported = super::encode::to_table(&lua, &node).unwrap();
        assert_eq!(
            parse_node(&Value::Table(exported)).unwrap(),
            node,
            "{source}"
        );
        let mut element =
            realize(&node, 13.0, std::sync::Arc::new(|action| action), true, 1.0).unwrap();
        let mut tree = Tree::new(&element);
        let layout = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(400.0, 600.0)),
        );
        assert!(
            layout.size().width.is_finite() && layout.size().height.is_finite(),
            "{source}"
        );
    }
    assert_eq!(kinds.len(), super::components::COMPONENTS.len());
    assert!(
        lua.load("return ui == nil and iced.components.slider.rust_type == 'SliderNode'")
            .eval::<bool>()
            .unwrap()
    );
    assert!(
        lua.load("return iced.to_table(iced.text('x')):size(18)._properties.size == 18")
            .eval::<bool>()
            .unwrap()
    );
}

#[test]
fn canvas_drawings_are_strict_and_blank_by_default() {
    let lua = lua();
    let parse = |source: &str| {
        let value: Value = lua.load(source).eval().unwrap();
        parse_node(&value)
    };
    assert!(matches!(
        parse("return iced.canvas()").unwrap(),
        super::node::WidgetNode::Canvas { drawing, width, height }
            if drawing.lines.is_empty()
                && drawing.rects.is_empty()
                && width == super::node::NodeLength::Native
                && height == super::node::NodeLength::Native
    ));
    for source in [
        "return iced.canvas({lines='nope'})",
        "return iced.canvas({lines={{points={{x=0,y=0}}}}})",
        "return iced.canvas({lines={{points={{x=0,y=1/0},{x=1,y=1}},color='#fff'}}})",
        "return iced.canvas({rects={{x=0,y=0,width=-1,height=2,color='#fff'}}})",
        "return iced.canvas({rects={{x=0,y=0,width=2,height=2}}})",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
}

#[test]
fn typed_structs_cross_vms_without_retaining_lua_values() {
    let node = {
        let lua = lua();
        let value = lua
            .load("return iced.text('owned'):width(size.fill):wrap('word'):anchor('right'):margin(16):on_press('title')")
            .eval()
            .unwrap();
        super::components::TextNode::from_lua(value, &lua).unwrap()
    };
    let other = lua();
    let expected: super::node::WidgetNode = node.clone().into();
    assert_eq!(
        parse_node(&node.into_lua(&other).unwrap()).unwrap(),
        expected
    );
    let wrong = other.load("return iced.space()").eval().unwrap();
    assert!(super::components::SliderNode::from_lua(wrong, &other).is_err());
}

#[test]
fn invalid_control_properties_fail_before_native_realization() {
    let lua = lua();
    for source in [
        "iced.slider(1,1,1)",
        "iced.slider(0,1,0):step(0)",
        "iced.slider(0,1,0):value(0/0)",
        "iced.slider(-1e38,3e38,0)",
        "iced.checkbox('x','yes')",
        "iced.toggler('x',1)",
        "iced.pick_list({'one'},'missing')",
        "iced.pick_list({'one','one'},nil)",
        "iced.pick_list({1},nil)",
        "iced.text_input('x',''):on_input(function() end)",
        "iced.radio('x',42,nil)",
        "iced.text_input('x',''):size(-1)",
    ] {
        let value = lua.load(format!("return {source}")).eval().unwrap();
        assert!(parse_node(&value).is_err(), "{source}");
    }
    assert!(
        lua.load("return iced.text('x'):on_toggle('wrong')")
            .eval::<Value>()
            .is_err()
    );
    assert!(
        lua.load("return iced.define('slider', function() return iced.space() end)")
            .exec()
            .is_err()
    );
}

fn click(
    element: &mut Element<'static, Action>,
    tree: &mut Tree,
    node: &layout::Node,
    renderer: &iced::Renderer,
    point: Point,
    messages: &mut Vec<Action>,
) {
    for event in [
        mouse::Event::ButtonPressed(mouse::Button::Left),
        mouse::Event::ButtonReleased(mouse::Button::Left),
    ] {
        element.as_widget_mut().update(
            tree,
            &Event::Mouse(event),
            Layout::new(node),
            mouse::Cursor::Available(point),
            renderer,
            &mut clipboard::Null,
            &mut Shell::new(messages),
            &Rectangle::with_size(Size::new(300.0, 300.0)),
        );
    }
}

#[test]
fn stack_routes_clicks_to_the_top_layer() {
    let lua = lua();
    let renderer = renderer();
    let node = parse_node(
        &lua.load(
            "return iced.stack({iced.button('base','base'):width(120):height(60), iced.button('top','top'):width(120):height(60)})",
        )
        .eval()
        .unwrap(),
    )
    .unwrap();
    let mut element =
        realize(&node, 13.0, std::sync::Arc::new(|action| action), true, 1.0).unwrap();
    let mut tree = Tree::new(&element);
    let layout = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(300.0, 300.0)),
    );
    let mut messages = Vec::new();
    click(
        &mut element,
        &mut tree,
        &layout,
        &renderer,
        layout.bounds().center(),
        &mut messages,
    );
    assert_eq!(messages, [Action::from("top")]);
}

#[test]
fn native_boolean_numeric_and_radio_events_have_owned_values_and_disabled_controls_are_inert() {
    let lua = lua();
    let renderer = renderer();
    for (source, expected) in [
        (
            "iced.checkbox('Enable',false):on_toggle('check')",
            ActionValue::Boolean(true),
        ),
        (
            "iced.toggler('Enable',false):on_toggle('check')",
            ActionValue::Boolean(true),
        ),
        (
            "iced.slider(0,100,0):width(200):on_change('check')",
            ActionValue::Number(50.0),
        ),
        (
            "iced.radio('Choice','choice',nil):on_select('check')",
            ActionValue::Text("choice".into()),
        ),
    ] {
        let node = parse_node(&lua.load(format!("return {source}")).eval().unwrap()).unwrap();
        for enabled in [true, false] {
            let mut element = realize(
                &node,
                13.0,
                std::sync::Arc::new(|action| action),
                enabled,
                1.0,
            )
            .unwrap();
            let mut tree = Tree::new(&element);
            let layout = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, Size::new(300.0, 300.0)),
            );
            let mut messages = Vec::new();
            click(
                &mut element,
                &mut tree,
                &layout,
                &renderer,
                layout.bounds().center(),
                &mut messages,
            );
            if enabled {
                assert_eq!(
                    messages,
                    [Action::change("check".into(), expected.clone())],
                    "{source}"
                );
            } else {
                assert!(messages.is_empty(), "disabled {source}");
            }
        }
    }
}

#[test]
fn text_input_emits_text_and_submit_and_pick_list_emits_selection() {
    let lua = lua();
    let renderer = renderer();
    let node = parse_node(
        &lua.load(
            "return iced.text_input('Search',''):on_input('query'):on_submit('search'):width(200)",
        )
        .eval()
        .unwrap(),
    )
    .unwrap();
    let mut element =
        realize(&node, 13.0, std::sync::Arc::new(|action| action), true, 1.0).unwrap();
    let mut tree = Tree::new(&element);
    let limits = layout::Limits::new(Size::ZERO, Size::new(300.0, 300.0));
    let layout = element
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
    let mut messages = Vec::new();
    click(
        &mut element,
        &mut tree,
        &layout,
        &renderer,
        layout.bounds().center(),
        &mut messages,
    );
    use iced::keyboard::{
        Key, Location, Modifiers,
        key::{Code, Named, Physical},
    };
    for (key, code, text) in [
        (Key::Character("x".into()), Code::KeyX, Some("x".into())),
        (Key::Named(Named::Enter), Code::Enter, None),
    ] {
        let event = Event::Keyboard(iced::keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: Physical::Code(code),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text,
            repeat: false,
        });
        element.as_widget_mut().update(
            &mut tree,
            &event,
            Layout::new(&layout),
            mouse::Cursor::Available(layout.bounds().center()),
            &renderer,
            &mut clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::with_size(Size::new(300.0, 300.0)),
        );
    }
    assert_eq!(
        messages,
        [
            Action::change("query".into(), ActionValue::Text("x".into())),
            Action::from("search")
        ]
    );

    let node = parse_node(
        &lua.load("return iced.pick_list({'one','two'},nil):on_select('pick'):width(200)")
            .eval()
            .unwrap(),
    )
    .unwrap();
    let mut element =
        realize(&node, 13.0, std::sync::Arc::new(|action| action), true, 1.0).unwrap();
    let mut tree = Tree::new(&element);
    let layout = element
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
    let mut messages = Vec::new();
    click(
        &mut element,
        &mut tree,
        &layout,
        &renderer,
        layout.bounds().center(),
        &mut messages,
    );
    let viewport = Rectangle::with_size(Size::new(300.0, 300.0));
    let mut overlay = element
        .as_widget_mut()
        .overlay(
            &mut tree,
            Layout::new(&layout),
            &renderer,
            &viewport,
            iced::Vector::ZERO,
        )
        .expect("pick-list menu");
    let menu = overlay.as_overlay_mut().layout(&renderer, viewport.size());
    let point = Point::new(menu.bounds().x + 10.0, menu.bounds().y + 10.0);
    for event in [
        mouse::Event::CursorMoved { position: point },
        mouse::Event::ButtonPressed(mouse::Button::Left),
    ] {
        overlay.as_overlay_mut().update(
            &Event::Mouse(event),
            Layout::new(&menu),
            mouse::Cursor::Available(point),
            &renderer,
            &mut clipboard::Null,
            &mut Shell::new(&mut messages),
        );
    }
    assert_eq!(
        messages,
        [Action::change(
            "pick".into(),
            ActionValue::Text("one".into())
        )]
    );
}
