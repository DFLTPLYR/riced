use super::*;

#[test]
fn bundled_fragments_match_the_uniform_abi_and_fragment_contract() {
    assert_eq!(size_of::<gpu::Uniforms>(), 112);
    for (_, source) in crate::config::SHADER_SEEDS {
        validate(source).unwrap();
    }
    for source in [
        "fn shade(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(unknown); }",
        "fn shade(uv: vec2<f32>) -> f32 { return 1.0; }",
        "@group(1) @binding(0) var tex: texture_2d<f32>; fn shade(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }",
    ] {
        assert!(validate(source).is_err(), "{source}");
    }
}

#[test]
fn paths_resolve_names_aliases_and_absolute_paths_without_changing_extensions() {
    let root = Path::new("/tmp/riced/shaders");
    for name in ["plasma", "plasma.wgsl", "/shaders/plasma.wgsl"] {
        assert_eq!(resolve_in(root, name).unwrap(), root.join("plasma.wgsl"));
    }
    assert_eq!(
        resolve_in(root, "file:///tmp/other.wgsl").unwrap(),
        Path::new("/tmp/other.wgsl")
    );
    for name in ["", "  ", "plasma.glsl", "/shaders/", "file://relative.wgsl"] {
        assert!(resolve_in(root, name).is_err());
    }
}

#[test]
fn source_reloads_valid_edits_and_preserves_last_good_code_on_errors_or_deletion() {
    let path =
        std::env::temp_dir().join(format!("riced-shader-reload-{}.wgsl", std::process::id()));
    let first = "fn shade(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";
    let second = first.replace("0.0", "0.5");
    std::fs::write(&path, first).unwrap();
    let source = Source::open(path.clone()).unwrap();
    let mut now = Instant::now();
    let a = source.code(now);
    std::fs::write(&path, &second).unwrap();
    now += RELOAD_INTERVAL;
    let b = source.code(now);
    assert_ne!(a, b);
    std::fs::write(&path, "invalid").unwrap();
    now += RELOAD_INTERVAL;
    assert_eq!(source.code(now), b);
    std::fs::remove_file(&path).unwrap();
    now += RELOAD_INTERVAL;
    assert_eq!(source.code(now), b);
    std::fs::write(&path, first).unwrap();
    now += RELOAD_INTERVAL;
    assert_eq!(source.code(now), a);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn shader_descriptions_reject_bad_parameters_and_unknown_fields_before_realization() {
    let lua = mlua::Lua::new();
    crate::ui::dsl::inject_ui_base(&lua).unwrap();
    lua.globals()
        .set(
            "path",
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/shaders/gradient.wgsl"),
        )
        .unwrap();
    for source in [
        "iced.shader(path):params({1,nil,3})",
        "iced.shader(path):params({x=1})",
        "iced.shader(path):params({1/0})",
        "iced.shader(path):params({true})",
        "iced.shader(path):params({[17]=1})",
        "iced.shader(path):params(false)",
        "iced.shader(path):speed(0/0)",
        "iced.shader(path):speed(function() end)",
        "iced.shader(path):animated(1)",
        "iced.shader(path):width(0)",
        "iced.shader(path):height(1/0)",
        "iced.shader(path):width(function() end)",
        "{type='shader',path=path,typo=true}",
        "{type='shader',path=path,_properties={typo=true}}",
        "{type='shader',path=path,_properties=false}",
        "iced.shader(42)",
    ] {
        let value = lua.load(format!("return {source}")).eval().unwrap();
        assert!(crate::ui::decode::parse_node(&value).is_err(), "{source}");
    }
    assert!(
        lua.load("return iced.shader(path, 'extra')")
            .eval::<mlua::Value>()
            .is_err()
    );
    assert!(
        lua.load("return iced.shader(path):unknown(true)")
            .eval::<mlua::Value>()
            .is_err()
    );
}

#[test]
fn shaders_build_in_every_host_and_software_rendering_does_not_schedule_frames() {
    use iced::advanced::{Layout, Shell, renderer::Headless};
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(13.0),
        Some("tiny-skia"),
    ))
    .unwrap();
    let lua = mlua::Lua::new();
    crate::ui::dsl::inject_ui_base(&lua).unwrap();
    lua.globals()
        .set(
            "path",
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/shaders/gradient.wgsl"),
        )
        .unwrap();
    let value = lua.load("return iced.listview({{id=1}}):id('shaders'):key('id'):delegate(function() return iced.shader(path):width(80):height(40) end)").eval().unwrap();
    let node = crate::ui::decode::parse_node(&value).unwrap();
    for mut element in [
        crate::ui::build::build_node(&node, 13.0, None).unwrap(),
        crate::ui::scene::Lists::default()
            .build(
                &node,
                &Default::default(),
                Arc::new(|_| crate::shell::Plant::Tend),
                None,
            )
            .unwrap(),
        crate::shell::screens::top::build_with_lists(
            &node,
            "test",
            13.0,
            None,
            &Default::default(),
            &Default::default(),
            None,
        )
        .unwrap(),
    ] {
        let mut tree = Tree::new(&element);
        let limits = layout::Limits::new(Size::ZERO, Size::new(80.0, 40.0));
        let node = element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &limits);
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Window(iced::window::Event::RedrawRequested(Instant::now())),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &renderer,
            &mut advanced::clipboard::Null,
            &mut shell,
            &Rectangle::with_size(Size::new(80.0, 40.0)),
        );
        assert_eq!(shell.redraw_request(), iced::window::RedrawRequest::Wait);
        assert!(!shell.is_event_captured());
        assert!(shell.is_empty());
    }
}

fn assert_native_frame_cadence(renderer: &iced::Renderer, source: &Source) {
    use iced::advanced::{Layout, Shell};
    use iced::window::RedrawRequest;
    for (animated, speed, opacity, viewport, delay) in [
        (
            true,
            1.0,
            1.0,
            Rectangle::with_size(Size::new(80.0, 40.0)),
            Some(Duration::from_millis(16)),
        ),
        (
            false,
            1.0,
            1.0,
            Rectangle::with_size(Size::new(80.0, 40.0)),
            Some(RELOAD_INTERVAL),
        ),
        (
            true,
            0.0,
            1.0,
            Rectangle::with_size(Size::new(80.0, 40.0)),
            Some(RELOAD_INTERVAL),
        ),
        (
            true,
            1.0,
            0.0,
            Rectangle::with_size(Size::new(80.0, 40.0)),
            None,
        ),
        (
            true,
            1.0,
            1.0,
            Rectangle {
                x: 100.0,
                y: 100.0,
                width: 10.0,
                height: 10.0,
            },
            None,
        ),
    ] {
        let mut element: Element<'static, ()> = element(
            source.clone(),
            speed,
            &[],
            animated,
            Length::Fixed(80.0),
            Length::Fixed(40.0),
            opacity,
        );
        let mut tree = Tree::new(&element);
        let node = element.as_widget_mut().layout(
            &mut tree,
            renderer,
            &layout::Limits::new(Size::ZERO, Size::new(80.0, 40.0)),
        );
        let now = Instant::now();
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Window(iced::window::Event::RedrawRequested(now)),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            renderer,
            &mut advanced::clipboard::Null,
            &mut shell,
            &viewport,
        );
        assert_eq!(
            shell.redraw_request(),
            delay.map_or(RedrawRequest::Wait, |delay| RedrawRequest::At(now + delay))
        );
        assert!(!shell.is_event_captured());
        assert!(shell.is_empty());
    }
}

#[test]
#[ignore = "requires a working wgpu adapter; run explicitly for shader rendering changes"]
fn engine_renders_an_opaque_fractal_and_changes_with_time() {
    use iced::advanced::{Layout, renderer::Headless, renderer::Style};
    let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(13.0),
        Some("wgpu"),
    ))
    .expect("wgpu adapter");
    let path = std::env::temp_dir().join(format!("riced-engine-gpu-{}.wgsl", std::process::id()));
    let mut frames = Vec::new();
    for time in ["0.0", "4.0"] {
        std::fs::write(
            &path,
            include_str!("../../../scripts/shaders/engine.wgsl").replace("riced.time", time),
        )
        .unwrap();
        let mut content: Element<'static, ()> = element(
            Source::open(path.clone()).unwrap(),
            1.0,
            &[],
            false,
            Length::Fixed(64.0),
            Length::Fixed(48.0),
            1.0,
        );
        let bounds = Rectangle::with_size(Size::new(64.0, 48.0));
        let mut tree = Tree::new(&content);
        let node = content.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, bounds.size()),
        );
        advanced::Renderer::reset(&mut renderer, bounds);
        content.as_widget().draw(
            &tree,
            &mut renderer,
            &iced::Theme::Dark,
            &Style::default(),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &bounds,
        );
        let pixels = renderer.screenshot(iced::Size::new(64, 48), 1.0, iced::Color::TRANSPARENT);
        assert!(pixels.chunks_exact(4).all(|pixel| pixel[3] == 255));
        assert!(pixels.chunks_exact(4).any(|pixel| pixel != &pixels[..4]));
        frames.push(pixels);
    }
    assert_ne!(frames[0], frames[1]);
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore = "requires a working wgpu adapter; run explicitly for shader rendering changes"]
fn gpu_shader_instances_keep_separate_uniforms_and_follow_clipping_and_translation() {
    use iced::advanced::{Layout, renderer::Headless, renderer::Style};
    use iced::{Color, Point};
    let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(13.0),
        Some("wgpu"),
    ))
    .expect("wgpu adapter");
    assert!(matches!(
        renderer,
        iced_renderer::fallback::Renderer::Primary(_)
    ));
    let path = std::env::temp_dir().join(format!("riced-shader-gpu-{}.wgsl", std::process::id()));
    std::fs::write(
        &path,
        "fn shade(uv: vec2<f32>) -> vec4<f32> { return riced.params[0]; }",
    )
    .unwrap();
    let source = Source::open(path.clone()).unwrap();
    assert_native_frame_cadence(&renderer, &source);
    for scale in [1.0, 2.0] {
        advanced::Renderer::reset(&mut renderer, Rectangle::with_size(Size::new(80.0, 40.0)));
        for (position, params, opacity) in [
            (Point::new(4.0, 4.0), vec![1.0, 0.0, 0.0, 1.0], 1.0),
            (Point::new(36.0, 4.0), vec![0.0, 1.0, 0.0, 1.0], 0.5),
        ] {
            let content: Element<'static, ()> = element(
                source.clone(),
                1.0,
                &params,
                false,
                Length::Fixed(24.0),
                Length::Fixed(24.0),
                opacity,
            );
            let mut content = crate::ui::motion::shifted(content, position.x, position.y, true);
            let mut tree = Tree::new(&content);
            let node = content.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, Size::new(80.0, 40.0)),
            );
            advanced::Renderer::with_layer(
                &mut renderer,
                Rectangle::new(Point::new(0.0, 0.0), Size::new(52.0, 40.0)),
                |renderer| {
                    content.as_widget().draw(
                        &tree,
                        renderer,
                        &iced::Theme::Dark,
                        &Style::default(),
                        Layout::new(&node),
                        mouse::Cursor::Unavailable,
                        &Rectangle::with_size(Size::new(80.0, 40.0)),
                    );
                },
            );
        }
        let pixels = renderer.screenshot(
            Size::new((80.0 * scale) as u32, (40.0 * scale) as u32),
            scale,
            Color::TRANSPARENT,
        );
        let pixel = |x: usize, y: usize| {
            let index = ((y as f32 * scale) as usize * (80.0 * scale) as usize
                + (x as f32 * scale) as usize)
                * 4;
            &pixels[index..index + 4]
        };
        assert_eq!(pixel(8, 8), [255, 0, 0, 255]);
        let green = pixel(40, 8);
        assert!(
            green[0] == 0 && green[1] > 100 && green[2] == 0 && (126..=129).contains(&green[3]),
            "{green:?}"
        );
        assert_eq!(pixel(56, 8), [0, 0, 0, 0]);
        assert_eq!(pixel(0, 0), [0, 0, 0, 0]);
    }
    // A shader partially scrolled offscreen must retain its original UVs,
    // logical resolution and mouse coordinates, rather than stretching to the clip.
    let uv_path = path.with_extension("uv.wgsl");
    std::fs::write(&uv_path, "fn shade(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, riced.mouse.x / riced.resolution.x, 1.0); }").unwrap();
    let mut element: Element<'static, ()> = element(
        Source::open(uv_path.clone()).unwrap(),
        1.0,
        &[],
        false,
        Length::Fixed(24.0),
        Length::Fixed(24.0),
        1.0,
    );
    let mut tree = Tree::new(&element);
    let node = element
        .as_widget_mut()
        .layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(80.0, 40.0)),
        )
        .move_to(Point::new(-8.0, 0.0));
    advanced::Renderer::reset(&mut renderer, Rectangle::with_size(Size::new(80.0, 40.0)));
    element.as_widget().draw(
        &tree,
        &mut renderer,
        &iced::Theme::Dark,
        &Style::default(),
        Layout::new(&node),
        mouse::Cursor::Available(Point::new(4.0, 4.0)),
        &Rectangle::with_size(Size::new(8.0, 24.0)),
    );
    let pixels = renderer.screenshot(Size::new(80, 40), 1.0, Color::TRANSPARENT);
    let color = &pixels[(4 * 80 + 4) * 4..(4 * 80 + 4) * 4 + 4];
    assert!(
        (120..=145).contains(&color[0])
            && (40..=60).contains(&color[1])
            && (126..=129).contains(&color[2]),
        "{color:?}"
    );
    assert_eq!(
        &pixels[(4 * 80 + 12) * 4..(4 * 80 + 12) * 4 + 4],
        [0, 0, 0, 0]
    );
    std::fs::remove_file(uv_path).unwrap();
    std::fs::remove_file(path).unwrap();
}
