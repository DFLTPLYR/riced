use super::*;
use iced::futures::StreamExt;
use mlua::LuaSerdeExt;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    project: Project,
}

fn install_daemon_builder(lua: &mlua::Lua) {
    let riced: mlua::Table = lua.globals().get("riced").unwrap();
    riced
        .set(
            "daemon",
            lua.load(include_str!("../../lua/daemon.lua"))
                .eval::<mlua::Function>()
                .unwrap(),
        )
        .unwrap();
}

#[test]
fn daemon_keeps_its_startup_vm_alive_without_outputs() {
    let fixture = Fixture::new("return {view=function() end}");
    let source = r#"return riced.daemon(function() return {} end,
        function() return 'test' end, function() end, function() return nil end):run()"#;
    let path = fixture.project.root.join("shell.lua");
    std::fs::write(&path, source).unwrap();
    let mut runtime = LuaRuntime::with_profile(
        crate::lua::sandbox::Profile::Widget,
        crate::config::component_files(),
    )
    .unwrap();
    runtime.set_app_root(&fixture.project.root).unwrap();
    runtime.load_path(&path).unwrap();
    let generation = runtime.managed_generation();
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    plots
        .apps
        .install_daemon(runtime, &fixture.project)
        .unwrap();
    let _ = refresh_project(&mut plots, &fixture.project, true);
    let _ = refresh_project(&mut plots, &fixture.project, true);
    assert_eq!(plots.apps.instances.len(), 1);
    assert_eq!(
        plots.apps.daemon_runtime().unwrap().managed_generation(),
        generation
    );
}

#[test]
fn daemon_shares_state_between_outputs_and_survives_surface_close_and_bad_reload() {
    let fixture = Fixture::new(
        r#"
        return riced.daemon(function() return {count=0} end,
            function() return 'test' end,
            function(self, event) self.count=self.count+1 end,
            function(self, props)
                local layers={}
                for _, output in ipairs(props.outputs) do
                    layers[#layers+1]=iced.layer({id=output.name,output=output.name,width=100,height=30,
                        content=iced.text(tostring(self.count))})
                end
                return layers
            end):run()
    "#,
    );
    let mut app = instance("");
    app.props["outputs"] = json!([{"name":"DP-1"},{"name":"DP-2"}]);
    fixture.render(&mut app, false).unwrap();
    let generation = app.runtime.as_ref().unwrap().managed_generation();
    with_services(|services| {
        app.runtime.as_mut().unwrap().app_event(
            services,
            "DP-1",
            &app.context,
            &app.props,
            AppEvent::Event(&json!({"type":"increment"})),
        )
    })
    .unwrap();
    fixture.render(&mut app, false).unwrap();
    for root in app.roots.values() {
        assert!(
            matches!(&root.root.content, crate::ui::node::WidgetNode::Text {content, ..} if content == "1")
        );
    }
    let closed = window::Id::unique();
    app.roots.get_mut("DP-1").unwrap().surface = Some(closed);
    let mut state = State {
        daemon: true,
        ..Default::default()
    };
    state.instances.insert((OutputId(0), "root".into()), app);
    with_services(|services| state.forget(closed, services, &mut Default::default()));
    let app = state
        .instances
        .get_mut(&(OutputId(0), "root".into()))
        .unwrap();
    app.props["outputs"] = json!([{"name":"DP-2"}]);
    fixture.render(app, false).unwrap();
    assert_eq!(app.roots.len(), 1);
    assert_eq!(
        app.runtime.as_ref().unwrap().managed_generation(),
        generation
    );
    fixture.write("error('broken reload')");
    assert!(fixture.render(app, false).is_err());
    assert_eq!(text(app), "1");
}

#[test]
fn shared_pointer_delivery_translates_logical_coordinates_and_is_app_scoped() {
    let fixture = Fixture::new(
        r#"
        return {
            actions=function(self, event)
                self.seen=string.format('%s:%g:%g', event.source_output or '', event.x, event.y)
            end,
            view=function(self) return iced.layer({id='background',layer='background',
                anchors={top=true,right=true,bottom=true,left=true},exclusive_zone=-1,pointer_events=true,shared_pointer_events=self.props.shared ~= false,
                content=iced.text(self.seen or 'untouched')}) end,
        }
    "#,
    );
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let mut state = State::default();
    for (output, owner, x, y) in [
        (1, "bar", -1280, 100),
        (2, "bar", 0, 0),
        (3, "other", 1920, 0),
        (4, "bar", 0, 800),
    ] {
        let mut app = instance(&format!("DP-{output}"));
        app.props["shared"] = json!(output != 4);
        app.props["output"] =
            json!({"name":format!("DP-{output}"),"x":x,"y":y,"width":1280,"height":720});
        app.context["output"] = app.props["output"].clone();
        fixture.render(&mut app, false).unwrap();
        state
            .instances
            .insert((OutputId(output), owner.into()), app);
    }
    reconcile(&mut plots, &mut state, &mut Vec::new());
    let surface = state.instances[&(OutputId(1), "bar".into())]
        .surface()
        .unwrap();
    plots.apps = state;
    let _ = update(
        &mut plots,
        AppMessage::Action {
            surface,
            action: Action {
                key: String::new(),
                value: None,
                event: crate::ui::action::Event::Mouse {
                    state: "move".into(),
                    button: None,
                    position: Some(iced::Point::new(1500.0, -50.0)),
                    origin: Some(iced::Point::new(20.0, 10.0)),
                    bounds: None,
                },
            },
        },
    );
    assert_eq!(
        text(&plots.apps.instances[&(OutputId(1), "bar".into())]),
        "DP-1:1500:-50"
    );
    assert_eq!(
        text(&plots.apps.instances[&(OutputId(2), "bar".into())]),
        "DP-1:220:50"
    );
    assert_eq!(
        text(&plots.apps.instances[&(OutputId(3), "other".into())]),
        "untouched"
    );
    assert_eq!(
        text(&plots.apps.instances[&(OutputId(4), "bar".into())]),
        "untouched"
    );
    let _ = update(
        &mut plots,
        AppMessage::Action {
            surface,
            action: Action {
                key: String::new(),
                value: None,
                event: crate::ui::action::Event::Mouse {
                    state: "move".into(),
                    button: None,
                    position: Some(iced::Point::new(50.0, 10.0)),
                    origin: None,
                    bounds: None,
                },
            },
        },
    );
    assert_eq!(
        text(&plots.apps.instances[&(OutputId(1), "bar".into())]),
        ":50:10"
    );
    assert_eq!(
        text(&plots.apps.instances[&(OutputId(2), "bar".into())]),
        "DP-1:220:50",
        "idle motion must not rebuild other outputs"
    );
}

#[test]
fn bundled_selection_uses_global_geometry_and_only_background_left_drags() {
    let lua = crate::lua::widgets::new_widget_lua().unwrap();
    lua.load("riced = { root = function(app) return app end }; require = function() return {} end")
        .exec()
        .unwrap();
    install_daemon_builder(&lua);
    let app: mlua::Table = lua
        .load(include_str!("../../../scripts/frontend/shell.lua"))
        .eval()
        .unwrap();
    lua.globals().set("app", app).unwrap();
    lua.load(
        r#"
        app.props = {output={name='left', x=-1280, y=100, width=1280, height=720}}
        local update = app.actions
        function app:actions(event)
            event.output = self.props.output
            event.layer = 'test.' .. event.layer
            return update(self, event)
        end
        app:actions({type='press',layer='bar-top',button='left',x=20,y=30})
        assert(not app.selection)
        app:actions({type='press',layer='background',button='right',x=20,y=30})
        assert(not app.selection)
        app.menu = {x=20,y=30}
        app:actions({type='press',layer='background',button='left',x=900,y=300})
        assert(app.selecting and not app.menu)
        app:actions({type='move',layer='background',x=200,y=50})
        local s = app.selection
        assert(s.x == -1080 and s.y == 150 and s.width == 700 and s.height == 250)
        app:actions({type='exit',layer='background'})
        assert(app.selecting, 'cross-output motion must not cancel selection')
        app:actions({type='move',layer='background',x=1500,y=-50})
        assert(s.x == -380 and s.y == 50 and s.width == 600 and s.height == 350)
        app:actions({type='release',layer='background',button='right'})
        assert(app.selecting)
        app:actions({type='release',layer='background',button='left',x=1550,y=-100})
        assert(not app.selecting and s.width == 650 and s.height == 400)
        local previous = s.id
        app:actions({type='press',layer='background',button='left',x=10,y=10})
        assert(app.selecting and app.selection.id ~= previous)
        app:actions({type='cancel',layer='background'})
        assert(not app.selecting)
    "#,
    )
    .exec()
    .unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn deferred_reclamation_collects_discarded_framework_notification_items() {
    const CHILD: &str = "RICED_NOTIFICATION_RSS_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "shell::apps::tests::deferred_reclamation_collects_discarded_framework_notification_items", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "isolated notification memory probe failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let rss = || -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|line| line.starts_with("VmRSS:"))
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap()
    };
    struct Owner(std::sync::Arc<Vec<u8>>);
    impl mlua::UserData for Owner {}
    let fixture = Fixture::new(
        r#"
        return {view=function()
            local items = payload and {{id=1, image=payload}} or {}
            return iced.layer({id='notifications',width=360,height=480,
                content=iced.listview(items):id('notifications'):key('id')
                    :delegate(function(item) return iced.text(item.id) end)})
        end}
    "#,
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let owner = std::sync::Arc::new(vec![7u8; 32 * 1024 * 1024]);
    let weak = std::sync::Arc::downgrade(&owner);
    let payload = Owner(owner);
    assert_eq!(std::sync::Arc::strong_count(&payload.0), 1);
    app.runtime
        .as_ref()
        .unwrap()
        .lua
        .globals()
        .set("payload", payload)
        .unwrap();
    fixture.render(&mut app, false).unwrap();
    app.runtime
        .as_ref()
        .unwrap()
        .lua
        .globals()
        .set("payload", mlua::Value::Nil)
        .unwrap();
    fixture.render(&mut app, false).unwrap();
    // Settle legitimate exit owners before asking the host to collect Lua garbage.
    let mut motion = crate::ui::anim::AnimRuntime::default();
    for root in app.roots.values_mut() {
        root.lists.clear(&mut motion);
    }
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    plots
        .apps
        .instances
        .insert((OutputId(1), "bar".into()), app);
    let peak = rss();
    let _ = plots.update(Plant::Config(
        super::super::ConfigEvent::ReclaimImageMemory { redraw: true },
    ));
    assert!(
        weak.upgrade().is_none(),
        "discarded Lua items still retain native resources after dismissal cleanup"
    );
    let after = rss();
    eprintln!("notification native/Lua reclamation: peak {peak} KiB -> {after} KiB");
    assert!(
        peak.saturating_sub(after) > 16 * 1024,
        "dismissal retained native/Lua RSS: peak {peak}, after {after} KiB"
    );
}

#[test]
fn bundled_bars_are_scoped_to_safe_output_identities() {
    let lua = mlua::Lua::new();
    lua.load(r#"
        files = { ['data/bars/top.lua'] = {edge='top'} }
        riced = {
            ipc = function(handlers) ipc = handlers end,
            read = function(path) return assert(files[path], 'missing') end,
            write = function(path, data) files[path] = data end,
            remove = function(path) files[path] = nil end,
            list = function() local names={} for path in pairs(files) do names[#names+1]=path:sub(11) end return names end,
            layer = function(path, props) props.id = props.id or path:match('([^/]+)%.lua$'); return props end,
        }
    "#).exec().unwrap();
    let state: mlua::Table = lua
        .load(include_str!("../../../scripts/frontend/state.lua"))
        .eval()
        .unwrap();
    lua.globals().set("state", state).unwrap();
    lua.load("riced.root = function(app) return app end; require = function() return state end")
        .exec()
        .unwrap();
    install_daemon_builder(&lua);
    let app: mlua::Table = lua
        .load(include_str!("../../../scripts/frontend/shell.lua"))
        .eval()
        .unwrap();
    lua.globals().set("app", app).unwrap();
    lua.load(
        r#"
        local odd = '../Display: with spaces/é'
        ipc.add(app, {'--anchor','top','--output','DP-1'})
        ipc.add(app, {'--anchor','top','--output',odd})
        local count=0
        for path, bar in pairs(files) do
            count=count+1
            assert(path:match('^data/bars/[%w%-]+%.lua$'), path)
            if bar.output then assert(bar.output == 'DP-1' or bar.output == odd) end
        end
        assert(count == 3, 'targeted bars must not collide with all-output bars')
        assert(not pcall(ipc.add, app, {'--anchor','top','--output','DP-1'}))
        local function visible(output)
            local count=0
            for _, layer in ipairs(app:view({outputs={{name=output}}})) do
                if layer.id:find('bar-', 1, true) then count=count+1 end
            end
            return count
        end
        assert(visible('DP-1') == 2)
        assert(visible('DP-2') == 1)
        assert(visible(odd) == 2)
        assert(visible('DP-1') == 2, 'reconnect restores saved bar')
        ipc.remove(app, {'--anchor','top','--output','DP-2'})
        assert(files['data/bars/top.lua'], 'targeted miss must not delete shared bar')
        ipc.remove(app, {'--anchor','top','--output','DP-1'})
        assert(visible('DP-1') == 1)
        assert(visible(odd) == 2)
        ipc.remove(app, {'--anchor','top'})
        assert(visible('DP-2') == 0)
        assert(visible(odd) == 1)
    "#,
    )
    .exec()
    .unwrap();
}

#[test]
fn bundled_shell_owns_dynamic_generation_policy() {
    let mut runtime = LuaRuntime::with_profile(
        crate::lua::sandbox::Profile::Widget,
        crate::config::component_files(),
    )
    .unwrap();
    runtime
        .load_named(
            &include_str!("../../../scripts/frontend/shell.lua")
                .replace("require(\"data.state\")", "{}")
                .replace("require(\"components.settings.window\")", "{}"),
            "shell.lua",
        )
        .unwrap();
    with_services(|services| {
        for (kind, name, generates) in [
            ("wallpaper_applied", "dynamic", true),
            ("wallpaper_applied", "gruvbox", false),
            ("config_changed", "dynamic", false),
            ("output_changed", "dynamic", false),
        ] {
            let result = runtime
                .app_event(
                    services,
                    "",
                    &json!({}),
                    &json!({}),
                    AppEvent::Event(&json!({"type":kind,"theme":{"name":name}})),
                )
                .unwrap();
            assert_eq!(
                matches!(&result, mlua::Value::UserData(value) if value.is::<crate::theme_gen::GenerateTheme>()),
                generates,
                "{kind}: {name}"
            );
        }
    });
}

#[test]
fn bundled_root_preserves_notification_action_keys() {
    let lua = crate::lua::widgets::new_widget_lua().unwrap();
    lua.load("riced = { root = function(app) return app end }; require = function() return {} end")
        .exec()
        .unwrap();
    install_daemon_builder(&lua);
    let app: mlua::Table = lua
        .load(include_str!("../../../scripts/frontend/shell.lua"))
        .eval()
        .unwrap();
    let actions: mlua::Function = app.get("actions").unwrap();
    let value: mlua::Value = actions
        .call((
            app.clone(),
            lua.to_value(&json!({"key":"invoke:42:open:details"}))
                .unwrap(),
        ))
        .unwrap();
    assert_eq!(
        crate::lua::notifications::notification_action(&value),
        Some(crate::lua::notifications::NotificationAction::Invoke(
            42,
            "open:details".into()
        ))
    );
    for key in [
        json!({}),
        json!({"key":"invoke:no:open"}),
        json!({"key":"unknown"}),
    ] {
        let value: mlua::Value = actions
            .call((app.clone(), lua.to_value(&key).unwrap()))
            .unwrap();
        assert!(value.is_nil());
    }
    let value: mlua::Value = actions
        .call((
            app.clone(),
            lua.to_value(&json!({"key":"dismiss:42"})).unwrap(),
        ))
        .unwrap();
    assert_eq!(
        crate::lua::notifications::notification_action(&value),
        Some(crate::lua::notifications::NotificationAction::Dismiss(42))
    );
}

#[test]
fn root_state_module_registers_ipc_and_failed_reload_keeps_handlers() {
    let directory = tempfile::tempdir_in("/tmp/opencode").unwrap();
    std::fs::write(
        directory.path().join("state.lua"),
        r#"
        riced.ipc({ remove = function(self, args)
            self.seen = args[2]
            return {seen = args[2]}
        end })
        return {}
    "#,
    )
    .unwrap();
    let mut runtime =
        LuaRuntime::with_profile(crate::lua::sandbox::Profile::Widget, vec![]).unwrap();
    runtime.set_app_root(directory.path()).unwrap();
    runtime
        .load("local state = require('state'); return {view=function() return nil end}")
        .unwrap();
    assert!(runtime.has_ipc_handler("remove"));
    assert!(!runtime.has_ipc_handler("unknown"));
    assert!(
        runtime
            .load("riced.ipc({bad=function() end}); error('broken')")
            .is_err()
    );
    assert!(runtime.has_ipc_handler("remove"));
    assert!(!runtime.has_ipc_handler("bad"));
    runtime
        .load("local state = require('state'); return {view=function() return nil end}")
        .unwrap();
    assert!(runtime.has_ipc_handler("remove"));
    let result = with_services(|services| {
        runtime.app_event(
            services,
            "DP-1",
            &json!({}),
            &json!({}),
            AppEvent::Ipc {
                command: "remove",
                args: &["--output".into(), "Display with spaces".into()],
            },
        )
    })
    .unwrap();
    let seen: serde_json::Value = runtime.lua.from_value(result).unwrap();
    assert_eq!(seen["seen"], "Display with spaces");
}

#[test]
fn installed_root_declares_layers_and_dispatches_settings() {
    let directory = tempfile::tempdir_in("/tmp/opencode").unwrap();
    crate::config::initialize::initialize_in(
        directory.path(),
        crate::config::initialize::Mode::Startup,
    )
    .unwrap();
    let project = Project {
        root: directory.path().to_path_buf(),
    };
    let sources = project.sources("root");
    let config = AppConfig {
        name: "root".into(),
        ..Default::default()
    };
    let mut app = instance("DP-1");
    let prefix = "DP-1"
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let (background, bar_top, bar_bottom) = (
        format!("{prefix}.background"),
        format!("{prefix}.bar-top"),
        format!("{prefix}.bar-bottom"),
    );
    app.props["outputs"] = json!([{
        "name": "DP-1", "x": -1280, "y": 100, "width": 1280, "height": 720
    }]);
    let render = |app: &mut Instance| {
        with_services(|services| {
            app.render(
                &Request {
                    project: &project,
                    config: &config,
                    sources: &sources,
                    components: &[],
                    services,
                },
                &mut Default::default(),
                false,
            )
        })
        .unwrap();
    };
    render(&mut app);
    assert!(app.roots.contains_key(&background));
    assert!(app.roots.contains_key(&bar_top));
    assert!(app.roots.contains_key(&format!("{prefix}.notifications")));
    assert!(app.runtime.as_ref().unwrap().has_ipc_handler("settings"));
    assert!(app.runtime.as_ref().unwrap().has_ipc_handler("add"));
    assert!(app.runtime.as_ref().unwrap().has_ipc_handler("remove"));

    // The bundled background renders global selection geometry without seam borders.
    app.context["output"] = json!({"name":"DP-1","x":-1280,"y":100,"width":1280,"height":720});
    app.props["output"] = app.context["output"].clone();
    app.props["selection_fade_ms"] = json!(150);
    for (kind, point) in [
        ("press", iced::Point::new(100.0, 100.0)),
        ("move", iced::Point::new(-50.0, 20.0)),
        ("release", iced::Point::new(-50.0, 20.0)),
    ] {
        let action = Action {
            key: String::new(),
            value: None,
            event: crate::ui::action::Event::Mouse {
                state: kind.into(),
                button: (kind != "move").then(|| "left".into()),
                position: Some(point),
                origin: None,
                bounds: None,
            },
        };
        let mut context = app.context.clone();
        context["layer"] = json!(background);
        with_services(|services| {
            app.runtime.as_mut().unwrap().app_event(
                services,
                &app.output_name,
                &context,
                &app.props,
                AppEvent::Action(&action),
            )
        })
        .unwrap();
        render(&mut app);
        if kind == "press" {
            continue;
        }
        let crate::ui::node::WidgetNode::Stack { children, .. } =
            &app.roots[&background].root.content
        else {
            panic!("background stack")
        };
        let (spec, child) = children
            .iter()
            .find_map(|node| match node {
                crate::ui::node::WidgetNode::Animated { spec, child } => Some((spec, child)),
                _ => None,
            })
            .expect("selection animation");
        assert_eq!(
            spec.properties[2].as_ref().unwrap().to,
            if kind == "move" { 1.0 } else { 0.0 }
        );
        let crate::ui::node::WidgetNode::Canvas { drawing, .. } = child.as_ref() else {
            panic!("selection canvas")
        };
        assert_eq!(
            (
                drawing.rects[0].x,
                drawing.rects[0].y,
                drawing.rects[0].width,
                drawing.rects[0].height
            ),
            (-50.0, 20.0, 150.0, 80.0)
        );
        assert_eq!(drawing.lines[0].points[0], (-50.0, 20.0));
        crate::ui::build::build_node(child, 13.0, None).unwrap();
    }

    // Lua owns visibility; the next view declares a native window.
    let value = with_services(|services| {
        app.runtime.as_mut().unwrap().app_event(
            services,
            &app.output_name,
            &app.context,
            &app.props,
            AppEvent::Ipc {
                command: "settings",
                args: &[],
            },
        )
    })
    .unwrap();
    assert!(value.is_nil());
    render(&mut app);
    assert_eq!(
        app.roots["settings"]
            .root
            .surface
            .window
            .as_ref()
            .unwrap()
            .title,
        "Riced Settings"
    );

    // Adding a bar edits the app's own data and the next view declares it.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for delivery in managed_deliveries(&mut app).await {
            with_services(|services| app.deliver(services, &delivery).unwrap().unwrap());
        }
    });
    let value = with_services(|services| {
        app.runtime.as_mut().unwrap().app_event(
            services,
            &app.output_name,
            &app.context,
            &app.props,
            AppEvent::Ipc {
                command: "add",
                args: &[
                    "--anchor".into(),
                    "bottom".into(),
                    "--thickness".into(),
                    "28".into(),
                ],
            },
        )
    })
    .unwrap();
    assert!(value.is_nil());
    render(&mut app);
    assert!(app.roots.contains_key(&bar_bottom));
}

#[test]
fn framework_actions_include_the_identity_of_the_source_layer() {
    let fixture = Fixture::new(
        r#"
        return {
            actions=function(self,event) self.source=event.layer end,
            view=function(self) return {
                iced.layer({id='one',width=100,height=32,content=iced.text(self.source or '')}),
                iced.layer({id='two',width=100,height=32,content=iced.text(self.source or '')}),
            } end,
        }
    "#,
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let first = window::Id::unique();
    let second = window::Id::unique();
    app.roots.get_mut("one").unwrap().surface = Some(first);
    app.roots.get_mut("two").unwrap().surface = Some(second);
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    plots
        .apps
        .instances
        .insert((OutputId(1), "bar".into()), app);
    let _ = messages(update(
        &mut plots,
        AppMessage::Action {
            surface: second,
            action: "clicked".into(),
        },
    ));
    let content = &plots.apps.instances[&(OutputId(1), "bar".into())].roots["one"]
        .root
        .content;
    assert!(matches!(content, crate::ui::node::WidgetNode::Text {content,..} if content == "two"));
}

#[test]
fn framework_wallpaper_slot_uses_settings_tiles_when_native_background_is_disabled() {
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    plots.config.background.disabled = true;
    let output = OutputId(1);
    assert!(crate::shell::screens::Background::wallpaper_views(&plots, output).is_empty());
    plots.desktop.wallpaper.applied.insert(
        output,
        crate::wallpaper::Tile {
            spec: crate::wallpaper::OutputSpec {
                key: "DP-1".into(),
                rect: (0.0, 0.0, 1.0, 1.0),
                pixels: (1, 1),
            },
            path: PathBuf::from("test-tile.png"),
            handle: iced::widget::image::Handle::from_rgba(1, 1, vec![255; 4]),
        },
    );
    assert_eq!(
        crate::shell::screens::Background::wallpaper_views(&plots, output).len(),
        1
    );
    assert!(app_native(&plots, window::Id::unique(), Some(output))("wallpaper", None).is_ok());
    assert!(app_native(&plots, window::Id::unique(), None)("wallpaper", None).is_err());
}
impl Fixture {
    fn new(source: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from("/tmp/opencode").join(format!(
            "riced-framework-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let project = Project { root };
        std::fs::create_dir_all(project.app_root("bar").join("components")).unwrap();
        std::fs::write(project.source("bar"), source).unwrap();
        Self { project }
    }
    fn write(&self, source: &str) {
        std::fs::write(self.project.source("bar"), source).unwrap();
    }
    fn render(&self, instance: &mut Instance, tick: bool) -> Result<bool, String> {
        let sources = self.project.sources("bar");
        let config = AppConfig {
            name: "bar".into(),
            ..Default::default()
        };
        with_services(|services| {
            instance.render(
                &Request {
                    project: &self.project,
                    config: &config,
                    sources: &sources,
                    components: &[],
                    services,
                },
                &mut Default::default(),
                tick,
            )
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.project.root);
    }
}
fn with_services<T>(f: impl FnOnce(&ServiceCtx<'_>) -> T) -> T {
    let sys = sysinfo::System::new();
    let ctx = ServiceCtx {
        sys: &sys,
        gpu: None,
        sampled: 0.0,
        theme: &Default::default(),
        component_defaults: &Default::default(),
        outputs: &Default::default(),
        notifications: &Default::default(),
        toplevels: &Default::default(),
        workspaces: &Default::default(),
        tray: &Default::default(),
    };
    f(&ctx)
}
fn instance(output: &str) -> Instance {
    Instance {
        output_name: output.into(),
        props: json!({"output":{"name":output}, "height":32}),
        context: json!({"id":"bar","name":"bar","output":{"name":output}}),
        ..Default::default()
    }
}
fn text(instance: &Instance) -> &str {
    let crate::ui::node::WidgetNode::Text { content, .. } = &instance.root().unwrap().content
    else {
        panic!("text root")
    };
    content
}
/// Native slot ids anywhere in a widget tree.
fn native_slots(node: &crate::ui::node::WidgetNode, out: &mut Vec<String>) {
    use crate::ui::node::WidgetNode;
    match node {
        WidgetNode::Native { id, .. } => out.push(id.clone()),
        WidgetNode::Column { children, .. }
        | WidgetNode::Row { children, .. }
        | WidgetNode::Stack { children, .. }
        | WidgetNode::Masonry { children, .. } => {
            for child in children {
                native_slots(child, out);
            }
        }
        WidgetNode::Container { child, .. }
        | WidgetNode::Scrollable { child, .. }
        | WidgetNode::Clickable { child, .. }
        | WidgetNode::Anchored { child, .. }
        | WidgetNode::Aligned { child, .. }
        | WidgetNode::Animated { child, .. } => native_slots(child, out),
        _ => {}
    }
}
/// Button labels anywhere in a widget tree.
fn buttons(node: &crate::ui::node::WidgetNode, out: &mut Vec<String>) {
    use crate::ui::node::WidgetNode;
    match node {
        WidgetNode::Button { label, .. } => out.push(label.clone()),
        WidgetNode::Column { children, .. }
        | WidgetNode::Row { children, .. }
        | WidgetNode::Stack { children, .. }
        | WidgetNode::Masonry { children, .. } => {
            for child in children {
                buttons(child, out);
            }
        }
        WidgetNode::Container { child, .. }
        | WidgetNode::Scrollable { child, .. }
        | WidgetNode::Clickable { child, .. }
        | WidgetNode::Anchored { child, .. }
        | WidgetNode::Aligned { child, .. }
        | WidgetNode::Animated { child, .. } => buttons(child, out),
        _ => {}
    }
}
fn messages(task: Task<Plant>) -> Vec<iced_runtime::Action<Plant>> {
    iced_runtime::task::into_stream(task)
        .map(|stream| iced::futures::executor::block_on(stream.collect()))
        .unwrap_or_default()
}
const COUNTER: &str = r#"
return {
    defaults={height=10},
    mount=function(self,context) self.count=0; self.ticks=0; assert(context.output.name==self.props.output.name) end,
    on_tick=function(self) self.ticks=self.ticks+1 end,
    actions=function(self,event) assert(self.props.height==32); self.count=self.count+1 end,
    view=function(self,props)
        assert(props.height==32)
        assert(self.context.output.name==props.output.name)
        return iced.layer({height=props.height,anchors={top=true,left=true,right=true},exclusive_zone=props.height,
            content=iced.text(props.output.name..':'..self.count..':'..self.ticks)})
    end
}
"#;

const WINDOW_APP: &str = r#"
return {
    mount=function(self) self.show=true; self.width=640 end,
    actions=function(self,event)
        if event.type=='window_closed' then
            assert(event.id=='settings')
            self.show=false
        elseif event.key=='settings' then self.show=not self.show
        elseif event.key=='resize' then self.width=800 end
    end,
    view=function(self)
        local roots={iced.layer({id='bar',width=100,height=32,content=iced.text('bar')})}
        if self.show then roots[#roots+1]=iced.window({id='settings',title='Editor',width=self.width,height=480,content=riced.plugin('settings')}) end
        return roots
    end,
}
"#;

#[test]
fn native_windows_reuse_resize_close_and_reopen_without_unmounting_the_shell() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let messages = |task| {
        runtime.block_on(async {
            match iced_runtime::task::into_stream(task) {
                Some(stream) => stream.collect::<Vec<_>>().await,
                None => Vec::new(),
            }
        })
    };
    let fixture = Fixture::new(WINDOW_APP);
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let mut state = State::default();
    for output in [1, 2] {
        let mut app = instance(&format!("DP-{output}"));
        fixture.render(&mut app, false).unwrap();
        state
            .instances
            .insert((OutputId(output), "bar".into()), app);
    }
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    let created = messages(Task::batch(commands));
    assert_eq!(
        created
            .iter()
            .filter(|action| matches!(
                action,
                iced_runtime::Action::Output(Plant::NewBaseWindow { .. })
            ))
            .count(),
        2
    );
    let key = (OutputId(1), "bar".into());
    let editor = state.instances[&key].roots["settings"].surface.unwrap();
    let bar = state.instances[&key].roots["bar"].surface.unwrap();
    let other = state.instances[&(OutputId(2), "bar".into())].roots["settings"]
        .surface
        .unwrap();
    plots.apps = state;
    assert_eq!(plots.title(editor).as_deref(), Some("Editor"));
    assert!(plots.windows.settings.contains_key(&editor));
    assert!(messages(opened(&mut plots, editor)).is_empty());
    let resized = messages(update(
        &mut plots,
        AppMessage::Action {
            surface: editor,
            action: "resize".into(),
        },
    ));
    assert!(resized.iter().any(|action| matches!(action, iced_runtime::Action::Window(iced_runtime::window::Action::Resize(id, size)) if *id == editor && size.width == 800.0)));
    assert_eq!(
        plots.apps.instances[&key].roots["settings"].surface,
        Some(editor)
    );
    let closed = messages(plots.close_surface(editor, false));
    assert!(!closed.iter().any(|action| matches!(action, iced_runtime::Action::Window(iced_runtime::window::Action::Close(id)) if *id == editor)));
    assert!(!plots.windows.settings.contains_key(&editor));
    assert!(plots.windows.ids.contains_key(&bar));
    assert!(plots.windows.ids.contains_key(&other));
    assert!(!plots.apps.instances[&key].roots.contains_key("settings"));
    let reopened = messages(update(
        &mut plots,
        AppMessage::Action {
            surface: bar,
            action: "settings".into(),
        },
    ));
    assert!(reopened.iter().any(|action| matches!(
        action,
        iced_runtime::Action::Output(Plant::NewBaseWindow { .. })
    )));
    let new_editor = plots.apps.instances[&key].roots["settings"]
        .surface
        .unwrap();
    assert_ne!(editor, new_editor);
    let hidden = messages(update(
        &mut plots,
        AppMessage::Action {
            surface: bar,
            action: "settings".into(),
        },
    ));
    assert!(hidden.iter().any(|action| matches!(action, iced_runtime::Action::Window(iced_runtime::window::Action::Close(id)) if *id == new_editor)));
    assert!(!plots.windows.settings.contains_key(&new_editor));
    assert!(plots.windows.ids.contains_key(&other));
}

#[test]
fn failing_window_close_handler_does_not_reopen_the_window() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let messages = |task| {
        runtime.block_on(async {
            match iced_runtime::task::into_stream(task) {
                Some(stream) => stream.collect::<Vec<_>>().await,
                None => Vec::new(),
            }
        })
    };
    let fixture = Fixture::new(&WINDOW_APP.replace("self.show=false", "error('close failed')"));
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let key = (OutputId(1), "bar".into());
    let mut state = State::default();
    state.instances.insert(key.clone(), app);
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    let _ = messages(Task::batch(commands));
    let editor = state.instances[&key].roots["settings"].surface.unwrap();
    let bar = state.instances[&key].roots["bar"].surface.unwrap();
    plots.apps = state;
    let _ = messages(plots.close_surface(editor, false));
    let refresh = messages(refresh_surface(&mut plots, bar));
    assert!(!refresh.iter().any(|action| matches!(
        action,
        iced_runtime::Action::Output(Plant::NewBaseWindow { .. })
    )));
    assert!(plots.apps.instances[&key].roots["settings"].dismissed);
    assert!(!plots.windows.ids.contains_key(&editor));
    assert!(plots.windows.ids.contains_key(&bar));
}

const POPUP_APP: &str = r#"
return {
    mount=function(self) self.count=0; self.open=true; self.reason='none' end,
    actions=function(self,event)
        if event.type=='popup_closed' then self.reason=event.reason; self.open=false
        elseif event.key=='increment' then self.count=self.count+1
        elseif event.key=='show' then self.open=true
        elseif event.key=='hide' then self.open=false end
    end,
    view=function(self)
        return iced.layer({width=100,height=32,content=iced.text(self.count..':'..self.reason),
            popup=self.open and {id='menu',x=12,y=48,width=200,height=100,
                content=iced.button('Increment','increment')} or nil})
    end,
}
"#;

fn popup_fixture(source: &str) -> (Fixture, Plots, window::Id, window::Id) {
    let fixture = Fixture::new(source);
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let mut state = State::default();
    state.instances.insert((OutputId(1), "bar".into()), app);
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    let app = &state.instances[&(OutputId(1), "bar".into())];
    let parent = app.surface().unwrap();
    let surfaces: Vec<_> = app.roots.values().filter_map(|root| root.surface).collect();
    plots.apps = state;
    for surface in surfaces {
        let _ = messages(opened(&mut plots, surface));
    }
    let child = plots.apps.instances[&(OutputId(1), "bar".into())]
        .popup
        .as_ref()
        .unwrap()
        .surface;
    (fixture, plots, parent, child)
}

#[test]
fn app_popup_creation_and_geometry_updates_use_native_popup_actions() {
    let source = POPUP_APP.replace(
        "x=12,y=48,width=200,height=100",
        "x=self.props.x or 12,y=-100,width=self.props.width or 200,height=100",
    );
    let fixture = Fixture::new(&source);
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let mut state = State::default();
    state.instances.insert((OutputId(1), "bar".into()), app);
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    let app = &state.instances[&(OutputId(1), "bar".into())];
    let parent = app.surface().unwrap();
    assert!(
        app.popup.is_none(),
        "native children must wait for the parent's open event"
    );
    plots.apps = state;
    let created = messages(opened(&mut plots, parent));
    let child = plots.apps.instances[&(OutputId(1), "bar".into())]
        .popup
        .as_ref()
        .unwrap()
        .surface;
    assert!(created.iter().any(|action| matches!(action,
        iced_runtime::Action::Output(Plant::NewPopUp { id, settings })
        if *id == child && settings.parent == Some(parent)
            && settings.size == iced_exwlshell::reexport::PixelSize::try_px(200, 100).unwrap()
            && settings.placement == iced_exwlshell::reexport::PopupPlacement::Position((12, -100))
    )), "{created:?}");
    let mut state = std::mem::take(&mut plots.apps);
    let app = state
        .instances
        .get_mut(&(OutputId(1), "bar".into()))
        .unwrap();
    app.props["x"] = json!(24);
    app.props["width"] = json!(260);
    fixture.render(app, false).unwrap();
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    let updated = messages(Task::batch(commands));
    assert_eq!(
        state.instances[&(OutputId(1), "bar".into())]
            .popup
            .as_ref()
            .unwrap()
            .surface,
        child
    );
    assert!(updated.iter().any(|action| matches!(action,
        iced_runtime::Action::Output(Plant::PopUpReposition { id, settings })
        if *id == child && settings.parent == Some(parent)
            && settings.size == iced_exwlshell::reexport::PixelSize::try_px(260, 100).unwrap()
            && settings.placement == iced_exwlshell::reexport::PopupPlacement::Position((24, -100))
    )), "{updated:?}");
}

#[test]
fn app_popup_follows_its_declaring_layer_not_its_siblings() {
    let source = r#"return {view=function(self)
        local popup = {id='menu',x=0,y=32,width=200,height=100,content=iced.text('menu')}
        return {
            iced.layer({id='main',width=100,height=32,content=iced.text('main'),
                popup=not self.props.other and popup or nil}),
            iced.layer({id='other',width=100,height=32,content=iced.text('other'),
                popup=self.props.other and popup or nil}),
        }
    end}"#;
    let (fixture, mut plots, parent, child) = popup_fixture(source);
    let sibling = plots.apps.instances[&(OutputId(1), "bar".into())].roots["other"]
        .surface
        .unwrap();
    assert!(messages(close_children(&mut plots, sibling)).is_empty());
    assert!(is_popup(&plots, child));
    let mut state = std::mem::take(&mut plots.apps);
    let app = state
        .instances
        .get_mut(&(OutputId(1), "bar".into()))
        .unwrap();
    app.props["other"] = json!(true);
    fixture.render(app, false).unwrap();
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    let popup = state.instances[&(OutputId(1), "bar".into())]
        .popup
        .as_ref()
        .unwrap();
    assert_ne!(popup.surface, child);
    assert_eq!(popup.positioner.parent, Some(sibling));
    assert_ne!(popup.positioner.parent, Some(parent));
    assert!(!plots.windows.ids.contains_key(&child));
    let new_child = popup.surface;
    plots.apps = state;
    let _ = close_children(&mut plots, sibling);
    assert!(!is_popup(&plots, new_child));
}

#[test]
fn app_popup_actions_share_parent_state_and_reuse_the_child_surface() {
    let (_fixture, mut plots, parent, child) = popup_fixture(POPUP_APP);
    let _ = view(&plots, child);
    let actions = messages(update(
        &mut plots,
        AppMessage::Action {
            surface: child,
            action: "increment".into(),
        },
    ));
    let app = &plots.apps.instances[&(OutputId(1), "bar".into())];
    assert_eq!(text(app), "1:none");
    assert_eq!(app.popup.as_ref().unwrap().surface, child);
    assert!(actions.iter().any(|a| matches!(a, iced_runtime::Action::Output(Plant::App(AppMessage::Redraw(id))) if *id == parent)));
    assert!(actions.iter().any(|a| matches!(a, iced_runtime::Action::Output(Plant::App(AppMessage::Redraw(id))) if *id == child)));
}

#[test]
fn app_popup_dismissal_notifies_only_the_owner_and_stale_events_are_ignored() {
    let (_fixture, mut plots, parent, child) = popup_fixture(POPUP_APP);
    let _ = messages(update(
        &mut plots,
        AppMessage::DismissPopup {
            surface: child,
            reason: "outside",
        },
    ));
    let app = &plots.apps.instances[&(OutputId(1), "bar".into())];
    assert_eq!(text(app), "0:outside");
    assert!(app.popup.is_none());
    assert!(app.runtime.is_some());
    assert!(!plots.windows.ids.contains_key(&child));
    assert!(
        messages(update(
            &mut plots,
            AppMessage::Action {
                surface: child,
                action: "increment".into()
            }
        ))
        .is_empty()
    );
    let _ = messages(update(
        &mut plots,
        AppMessage::Action {
            surface: parent,
            action: "show".into(),
        },
    ));
    let replacement = plots.apps.instances[&(OutputId(1), "bar".into())]
        .popup
        .as_ref()
        .unwrap()
        .surface;
    assert_ne!(replacement, child);
    assert!(
        messages(update(
            &mut plots,
            AppMessage::DismissPopup {
                surface: child,
                reason: "escape"
            }
        ))
        .is_empty()
    );
}

#[test]
fn app_popup_stays_closed_even_if_the_dismissal_handler_fails() {
    let source = POPUP_APP.replace(
        "self.reason=event.reason; self.open=false",
        "error('dismissal failed')",
    );
    let (_fixture, mut plots, parent, child) = popup_fixture(&source);
    let _ = messages(update(
        &mut plots,
        AppMessage::DismissPopup {
            surface: child,
            reason: "escape",
        },
    ));
    let _ = messages(refresh_surface(&mut plots, parent));
    let app = &plots.apps.instances[&(OutputId(1), "bar".into())];
    assert!(app.root().unwrap().popup.is_some());
    assert!(app.popup.is_none());
    assert_eq!(app.dismissed_popup.as_deref(), Some("menu"));
}

#[test]
fn app_popup_native_close_does_not_unmount_the_parent() {
    let (_fixture, mut plots, parent, child) = popup_fixture(POPUP_APP);
    let _ = messages(plots.update(Plant::Wayland(crate::shell::LandEvent::Closed(child))));
    let app = &plots.apps.instances[&(OutputId(1), "bar".into())];
    assert_eq!(app.surface(), Some(parent));
    assert_eq!(text(app), "0:closed");
    assert!(app.popup.is_none());
}

#[test]
fn escape_key_dismisses_only_the_framework_popup() {
    use iced::keyboard::{self, Key, key};
    let (_fixture, mut plots, parent, child) = popup_fixture(POPUP_APP);
    let event = iced::Event::Keyboard(keyboard::Event::KeyPressed {
        key: Key::Named(key::Named::Escape),
        modified_key: Key::Named(key::Named::Escape),
        physical_key: key::Physical::Code(key::Code::Escape),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::default(),
        text: None,
        repeat: false,
    });
    let _ = messages(plots.update(Plant::Graft(child, event)));
    let app = &plots.apps.instances[&(OutputId(1), "bar".into())];
    assert_eq!(text(app), "0:escape");
    assert!(app.popup.is_none());
    assert!(plots.windows.ids.contains_key(&parent));
}

#[test]
fn closing_the_parent_closes_its_popup_child_and_retires_the_vm() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (_fixture, mut plots, parent, child) = popup_fixture(POPUP_APP);
    let actions: Vec<_> = runtime.block_on(async {
        iced_runtime::task::into_stream(
            plots.update(Plant::Wayland(crate::shell::LandEvent::Closed(parent))),
        )
        .unwrap()
        .collect()
        .await
    });
    assert!(plots.apps.instances.is_empty());
    assert!(!plots.windows.ids.contains_key(&parent));
    assert!(!plots.windows.ids.contains_key(&child));
    assert!(actions.iter().any(|a| matches!(a, iced_runtime::Action::Window(iced_runtime::window::Action::Close(id)) if *id == child)));
}

#[test]
fn app_reload_replaces_popup_identity_and_invalid_candidates_preserve_the_old_popup() {
    let (fixture, mut plots, _parent, child) = popup_fixture(POPUP_APP);
    let mut state = std::mem::take(&mut plots.apps);
    let app = state
        .instances
        .get_mut(&(OutputId(1), "bar".into()))
        .unwrap();
    fixture.write(&POPUP_APP.replace("width=200,height=100", "width=0,height=100"));
    assert!(fixture.render(app, false).is_err());
    assert_eq!(app.popup.as_ref().unwrap().surface, child);
    fixture.write(&POPUP_APP.replace("self.count=0", "self.count=10"));
    fixture.render(app, false).unwrap();
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    assert_ne!(
        state.instances[&(OutputId(1), "bar".into())]
            .popup
            .as_ref()
            .unwrap()
            .surface,
        child
    );
    assert!(!plots.windows.ids.contains_key(&child));
    plots.apps = state;
}

#[test]
fn popup_instances_on_other_outputs_are_unaffected_by_dismissal() {
    let (fixture, mut plots, _parent, child) = popup_fixture(POPUP_APP);
    let mut other = instance("DP-2");
    fixture.render(&mut other, false).unwrap();
    let mut state = std::mem::take(&mut plots.apps);
    state.instances.insert((OutputId(2), "bar".into()), other);
    reconcile(&mut plots, &mut state, &mut Vec::new());
    let parent = state.instances[&(OutputId(2), "bar".into())]
        .surface()
        .unwrap();
    plots.apps = state;
    let _ = messages(opened(&mut plots, parent));
    let peer = plots.apps.instances[&(OutputId(2), "bar".into())]
        .popup
        .as_ref()
        .unwrap()
        .surface;
    let _ = messages(update(
        &mut plots,
        AppMessage::DismissPopup {
            surface: child,
            reason: "outside",
        },
    ));
    assert!(is_popup(&plots, peer));
    assert_eq!(
        text(&plots.apps.instances[&(OutputId(2), "bar".into())]),
        "0:none"
    );
}

#[test]
fn hiding_a_parent_releases_its_child_without_unmounting_the_app() {
    let (_fixture, mut plots, parent, child) = popup_fixture(POPUP_APP);
    let mut state = std::mem::take(&mut plots.apps);
    let mut motion = std::mem::take(&mut plots.animation.motion);
    state
        .instances
        .get_mut(&(OutputId(1), "bar".into()))
        .unwrap()
        .drop_roots(&mut motion);
    plots.animation.motion = motion;
    reconcile(&mut plots, &mut state, &mut Vec::new());
    let app = &state.instances[&(OutputId(1), "bar".into())];
    assert!(app.runtime.is_some());
    assert!(app.popup.is_none());
    assert!(!plots.windows.ids.contains_key(&parent));
    assert!(!plots.windows.ids.contains_key(&child));
    plots.apps = state;
}

#[test]
fn bundled_popup_example_opens_a_child_and_builds_its_content() {
    let source = include_str!("../../../examples/apps/popup/app.lua");
    let fixture = Fixture::new(source);
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    assert!(app.root().unwrap().popup.is_none());
    with_services(|services| {
        app.runtime
            .as_mut()
            .unwrap()
            .app_event(
                services,
                &app.output_name,
                &app.context,
                &app.props,
                AppEvent::Action(&"toggle".into()),
            )
            .unwrap()
    });
    fixture.render(&mut app, false).unwrap();
    let popup = app.root().unwrap().popup.as_ref().unwrap();
    assert_eq!(popup.geometry.id, "counter");
    crate::ui::build::build_node(&popup.content, 13.0, None).unwrap();
}

#[test]
fn input_refreshes_only_the_owning_instance_and_redraws_its_surface() {
    let fixture =
        Fixture::new(&COUNTER.replace("assert(props.height==32)", "self.count=self.count+1"));
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let mut state = State::default();
    for output in [1, 2] {
        let mut app = instance(&format!("DP-{output}"));
        fixture.render(&mut app, false).unwrap();
        state
            .instances
            .insert((OutputId(output), "bar".into()), app);
    }
    reconcile(&mut plots, &mut state, &mut Vec::new());
    let surface = state.instances[&(OutputId(1), "bar".into())]
        .surface()
        .unwrap();
    // Every view increments count, making unrelated executions observable.
    let other_revision = state.instances[&(OutputId(2), "bar".into())].revision();
    plots.apps = state;
    let actions = messages(update(
        &mut plots,
        AppMessage::Action {
            surface,
            action: "drag".into(),
        },
    ));
    assert_eq!(
        text(&plots.apps.instances[&(OutputId(1), "bar".into())]),
        "DP-1:3:0"
    );
    assert_eq!(
        plots.apps.instances[&(OutputId(2), "bar".into())].revision(),
        other_revision
    );
    assert_eq!(
        text(&plots.apps.instances[&(OutputId(2), "bar".into())]),
        "DP-2:1:0"
    );
    assert!(
        matches!(&actions[..], [iced_runtime::Action::Output(Plant::App(AppMessage::Redraw(id)))] if *id == surface)
    );
    assert_eq!(
        super::super::state::redraw_scope(&Plant::App(AppMessage::Redraw(surface))),
        iced_exwlshell::redraw::Scope::Window(surface)
    );
}

#[test]
fn palette_example_renders_before_and_after_native_generation() {
    let fixture = Fixture::new(include_str!("../../../examples/apps/palette/app.lua"));
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    assert_eq!(app.root().unwrap().surface.width, Some(300));
    let runtime = app.runtime.as_ref().unwrap();
    let mut owner = Owner::default();
    owner.reset(runtime.managed_generation().unwrap());
    owner.apply(runtime.managed_commands());
    let job = owner.jobs().pop().unwrap();
    let delivery = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(job.run());
    assert_eq!(delivery.payload["ok"], true);
    // Deliver the callback without committing shared state; owner publication
    // is covered separately, keeping this UI test independent of other apps.
    with_services(|services| {
        app.runtime
            .as_mut()
            .unwrap()
            .app_event(
                services,
                &app.output_name,
                &app.context,
                &app.props,
                AppEvent::Managed(&delivery),
            )
            .unwrap()
    });
    fixture.render(&mut app, false).unwrap();
    crate::ui::build::build_node(&app.root().unwrap().content, 13.0, None).unwrap();
}

#[test]
fn background_example_mounts_handles_local_gestures_and_builds_its_menu() {
    let fixture = Fixture::new(include_str!("../../../examples/apps/background/app.lua"));
    // The app composes its layers from its own component library.
    for (relative, source) in [
        (
            "menu.lua",
            include_str!("../../../examples/apps/background/components/menu.lua"),
        ),
        (
            "layers/wallpaper.lua",
            include_str!("../../../examples/apps/background/components/layers/wallpaper.lua"),
        ),
        (
            "layers/bar.lua",
            include_str!("../../../examples/apps/background/components/layers/bar.lua"),
        ),
    ] {
        let target = fixture
            .project
            .app_root("bar")
            .join("components")
            .join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, source).unwrap();
    }
    let mut app = instance("DP-1");
    app.context["output"] = json!({"name":"DP-1","x":0,"y":0,"width":1920,"height":1080});
    app.props["output"] = app.context["output"].clone();
    fixture.render(&mut app, false).unwrap();
    assert!(app.roots["background"].root.surface.pointer_events);
    for (state, button, x, y) in [
        ("press", Some("left"), 10.0, 20.0),
        ("move", None, 100.0, 200.0),
        ("release", Some("left"), 100.0, 200.0),
        ("press", Some("right"), 1900.0, 1070.0),
    ] {
        let action = Action {
            key: String::new(),
            value: None,
            event: crate::ui::action::Event::Mouse {
                state: state.into(),
                button: button.map(str::to_string),
                position: Some(iced::Point::new(x, y)),
                origin: None,
                bounds: None,
            },
        };
        with_services(|services| {
            app.runtime.as_mut().unwrap().app_event(
                services,
                &app.output_name,
                &app.context,
                &app.props,
                AppEvent::Action(&action),
            )
        })
        .unwrap();
        fixture.render(&mut app, false).unwrap();
    }
    // The context menu offers the same host commands the native menu had.
    let mut labels = Vec::new();
    buttons(&app.roots["background"].root.content, &mut labels);
    assert!(labels.iter().any(|label| label == "Add Top"), "{labels:?}");
    assert!(
        labels.iter().any(|label| label == "Open Settings"),
        "{labels:?}"
    );
    // The layer embeds the host's desktop-widget slot.
    let mut slots = Vec::new();
    native_slots(&app.roots["background"].root.content, &mut slots);
    assert_eq!(slots, ["desktop-widgets"]);
    // The app owns its layer set: "Add Top" appends to app state and the next
    // view declares an extra bar layer. No host command is involved.
    assert!(!app.roots.contains_key("bar-1"), "{:?}", app.roots.keys());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // Let the app's own documents load, then add a bar through state.
            for delivery in managed_deliveries(&mut app).await {
                with_services(|services| app.deliver(services, &delivery).unwrap().unwrap());
            }
            with_services(|services| {
                app.runtime.as_mut().unwrap().app_event(
                    services,
                    &app.output_name,
                    &app.context,
                    &app.props,
                    AppEvent::Action(&Action::from("add-top".to_string())),
                )
            })
            .unwrap();
            for delivery in managed_deliveries(&mut app).await {
                with_services(|services| app.deliver(services, &delivery).unwrap().unwrap());
            }
        });
    fixture.render(&mut app, false).unwrap();
    let bar = app.roots.get("bar-1").expect("bar layer");
    // The right-click was at (1900, 1070) on a 1920x1080 output, so the Lua
    // policy docks the new bar to the bottom edge.
    assert_eq!(bar.root.surface.layer, "top");
    assert!(bar.root.surface.anchors.bottom);
    assert_eq!(bar.root.surface.exclusive_zone, 32);

    // Opening Settings changes Lua's declared windows.
    let value = with_services(|services| {
        app.runtime.as_mut().unwrap().app_event(
            services,
            &app.output_name,
            &app.context,
            &app.props,
            AppEvent::Action(&Action::from("open-settings".to_string())),
        )
    })
    .unwrap();
    assert!(value.is_nil());
    fixture.render(&mut app, false).unwrap();
    assert!(app.roots["settings"].root.surface.window.is_some());
    let config = crate::config::test_parse_config(include_str!(
        "../../../config/riced/example/data/config.lua"
    ))
    .unwrap();
    assert!(config.background.disabled);
    let roundtrip = crate::config::test_format_config(&config).unwrap();
    assert!(
        crate::config::test_parse_config(&roundtrip)
            .unwrap()
            .background
            .disabled
    );
}

#[test]
fn instances_mount_once_keep_independent_state_and_tick_only_on_poll() {
    let fixture = Fixture::new(COUNTER);
    let mut first = instance("DP-1");
    let mut second = instance("DP-2");
    fixture.render(&mut first, false).unwrap();
    fixture.render(&mut second, false).unwrap();
    with_services(|services| {
        first
            .runtime
            .as_mut()
            .unwrap()
            .app_event(
                services,
                &first.output_name,
                &first.context,
                &first.props,
                AppEvent::Action(&Action::from("increment".to_string())),
            )
            .unwrap();
    });
    fixture.render(&mut first, false).unwrap();
    assert_eq!(text(&first), "DP-1:1:0");
    assert_eq!(text(&second), "DP-2:0:0");
    fixture.render(&mut first, true).unwrap();
    assert_eq!(text(&first), "DP-1:1:1");
}

#[test]
fn invalid_reloads_preserve_the_last_working_app_and_valid_reloads_retire_it() {
    let fixture = Fixture::new(
        r#"
        local function trace(event) local f=assert(io.open(riced.path('events'),'a')); f:write(event..'\n'); f:close() end
        return {
            mount=function() trace('mount') end,
            unmount=function(self,reason) trace('unmount:'..reason) end,
            view=function() return iced.layer({width=100,height=32,content=iced.text('working')}) end
        }
    "#,
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    fixture.write("this is invalid Lua");
    assert!(fixture.render(&mut app, false).is_err());
    assert_eq!(text(&app), "working");
    fixture.render(&mut app, false).unwrap();
    assert_eq!(text(&app), "working");
    fixture.write("return {view=function() return iced.layer({width=100,height=32,content=iced.text('replacement')}) end}");
    fixture.render(&mut app, false).unwrap();
    assert_eq!(text(&app), "replacement");
    assert_eq!(
        std::fs::read_to_string(fixture.project.app_root("bar").join("events")).unwrap(),
        "mount\nunmount:reload\n"
    );
}

#[test]
fn invalid_first_views_unmount_candidates_without_replacing_active_state() {
    let fixture = Fixture::new(COUNTER);
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    fixture.write(r#"return {
        unmount=function(self,reason) local f=assert(io.open(riced.path('failed'),'w')); f:write(reason); f:close() end,
        view=function() return iced.layer({width=0,height=32,content=iced.text('bad')}) end
    }"#);
    assert!(fixture.render(&mut app, false).is_err());
    assert_eq!(text(&app), "DP-1:0:0");
    assert_eq!(
        std::fs::read_to_string(fixture.project.app_root("bar").join("failed")).unwrap(),
        "failed_mount"
    );
}

#[test]
fn local_component_edits_reload_and_context_exists_without_mount_hook() {
    let fixture = Fixture::new(
        r#"return {view=function(self)
        assert(self.context.output.name=='DP-1')
        return iced.layer({width=100,height=32,content=riced.component('status.lua')})
    end}"#,
    );
    let component = fixture
        .project
        .app_root("bar")
        .join("components/status.lua");
    std::fs::write(
        &component,
        "return {view=function() return iced.text('before') end}",
    )
    .unwrap();
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    assert_eq!(text(&app), "before");
    std::fs::write(
        &component,
        "return {view=function() return iced.text('after') end}",
    )
    .unwrap();
    fixture.render(&mut app, false).unwrap();
    assert_eq!(text(&app), "after");
}

#[test]
fn native_reconciliation_reuses_surfaces_and_updates_layout_without_restarting_apps() {
    let fixture = Fixture::new(COUNTER);
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let mut state = State::default();
    for output in [1, 2] {
        let mut app = instance(&format!("DP-{output}"));
        fixture.render(&mut app, false).unwrap();
        state
            .instances
            .insert((OutputId(output), "bar".into()), app);
    }
    let mut commands = Vec::new();
    assert!(reconcile(&mut plots, &mut state, &mut commands));
    let created = messages(Task::batch(commands));
    assert_eq!(
        created
            .iter()
            .filter(|m| matches!(m, iced_runtime::Action::Output(Plant::NewLayerShell { .. })))
            .count(),
        2
    );
    let first = state.instances[&(OutputId(1), "bar".into())]
        .surface()
        .unwrap();
    let mut commands = Vec::new();
    assert!(!reconcile(&mut plots, &mut state, &mut commands));
    assert!(commands.is_empty());
    state
        .instances
        .get_mut(&(OutputId(1), "bar".into()))
        .unwrap()
        .root_mut()
        .unwrap()
        .surface
        .height = Some(48);
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    let updates = messages(Task::batch(commands));
    assert!(updates.iter().any(|m| matches!(m, iced_runtime::Action::Output(Plant::LayoutChange { id, size, .. }) if *id == first && size.to_set() == (0,48))));
    assert!(
        !updates
            .iter()
            .any(|m| matches!(m, iced_runtime::Action::Output(Plant::NewLayerShell { .. })))
    );
    assert_eq!(plots.windows.on_output(OutputId(1)), vec![first]);
}

#[test]
fn hiding_and_stale_events_do_not_resurrect_or_invoke_retired_instances() {
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let surface = window::Id::unique();
    assert!(
        messages(update(
            &mut plots,
            AppMessage::Action {
                surface,
                action: "stale".to_string().into()
            }
        ))
        .is_empty()
    );
    let fixture = Fixture::new(COUNTER);
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    app.set_surface(surface);
    plots
        .windows
        .ids
        .insert(surface, PlotInfo::App(OutputId(1)));
    app.drop_roots(&mut plots.animation.motion);
    let mut state = State::default();
    state.instances.insert((OutputId(1), "bar".into()), app);
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    assert!(!plots.windows.ids.contains_key(&surface));
    assert!(
        state.instances[&(OutputId(1), "bar".into())]
            .runtime
            .is_some()
    );
}

#[test]
fn bundled_bar_loads_and_builds_without_a_special_rust_bar_host() {
    let project = Project {
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples"),
    };
    let sources = project.sources("bar").unwrap();
    let config = AppConfig {
        name: "bar".into(),
        ..Default::default()
    };
    let mut app = instance("DP-1");
    with_services(|services| {
        app.render(
            &Request {
                project: &project,
                config: &config,
                sources: &Ok(sources),
                components: &[],
                services,
            },
            &mut Default::default(),
            false,
        )
    })
    .unwrap();
    assert_eq!(app.root().unwrap().surface.exclusive_zone, 32);
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let surface = window::Id::unique();
    app.set_surface(surface);
    plots
        .apps
        .instances
        .insert((OutputId(1), "bar".into()), app);
    let _element = view(&plots, surface);
}

#[test]
fn bundled_editors_app_builds_and_spin_box_actions_update_its_layout() {
    fn collect_texts(node: &crate::ui::node::WidgetNode, out: &mut Vec<String>) {
        use crate::ui::node::WidgetNode;
        match node {
            WidgetNode::Text { content, .. } => out.push(content.clone()),
            WidgetNode::Column { children, .. }
            | WidgetNode::Row { children, .. }
            | WidgetNode::Stack { children, .. }
            | WidgetNode::Masonry { children, .. } => {
                for child in children {
                    collect_texts(child, out);
                }
            }
            WidgetNode::Container { child, .. }
            | WidgetNode::Scrollable { child, .. }
            | WidgetNode::Clickable { child, .. }
            | WidgetNode::Anchored { child, .. }
            | WidgetNode::Aligned { child, .. }
            | WidgetNode::Animated { child, .. } => collect_texts(child, out),
            _ => {}
        }
    }
    let project = Project {
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples"),
    };
    let sources = project.sources("editors").unwrap();
    let config = AppConfig {
        name: "editors".into(),
        ..Default::default()
    };
    let mut app = instance("DP-1");
    with_services(|services| {
        app.render(
            &Request {
                project: &project,
                config: &config,
                sources: &Ok(sources),
                components: &[],
                services,
            },
            &mut Default::default(),
            false,
        )
    })
    .unwrap();
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let surface = window::Id::unique();
    app.set_surface(surface);
    plots
        .apps
        .instances
        .insert((OutputId(1), "editors".into()), app);
    let _element = view(&plots, surface);
    let mut texts = Vec::new();
    collect_texts(
        &plots.apps.instances[&(OutputId(1), "editors".into())]
            .root()
            .unwrap()
            .content,
        &mut texts,
    );
    assert!(texts.iter().any(|text| text == "140 px"), "{texts:?}");

    // `iced.spin_box(...):on_change(key)` emits a numeric action; the app owns
    // the value and re-renders its masonry layout from it.
    let _ = messages(update(
        &mut plots,
        AppMessage::Action {
            surface,
            action: Action::change(
                "column_width".into(),
                crate::ui::action::ActionValue::Number(220.0),
            ),
        },
    ));
    let mut texts = Vec::new();
    collect_texts(
        &plots.apps.instances[&(OutputId(1), "editors".into())]
            .root()
            .unwrap()
            .content,
        &mut texts,
    );
    assert!(texts.iter().any(|text| text == "220 px"), "{texts:?}");
}

#[test]
fn framework_host_slots_resolve_desktop_widgets_and_reject_unknown_ids() {
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let plots = Plots::new(receiver);
    // The legacy desktop host has no description equivalent, so it is a slot.
    let mut native = app_native(&plots, window::Id::unique(), Some(OutputId(1)));
    let element = native("desktop-widgets", None).unwrap();
    drop(element);
    // Without an output there is nothing to composite.
    let mut native = app_native(&plots, window::Id::unique(), None);
    let error = native("desktop-widgets", None).err().unwrap();
    assert!(error.contains("output-bound"), "{error}");
    // Unknown ids are reported, never silently dropped.
    let mut native = app_native(&plots, window::Id::unique(), Some(OutputId(1)));
    let error = native("wallpaper-map", None).err().unwrap();
    assert!(error.contains("unknown plugin"), "{error}");
}

#[test]
fn framework_apps_build_a_layer_that_embeds_the_desktop_widget_slot() {
    let fixture = Fixture::new(
        "return {view=function() return iced.layer({width=200,height=100,\
         content=riced.plugin('desktop-widgets')}) end}",
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let surface = window::Id::unique();
    app.set_surface(surface);
    plots
        .apps
        .instances
        .insert((OutputId(1), "bar".into()), app);
    plots
        .windows
        .ids
        .insert(surface, PlotInfo::App(OutputId(1)));
    let _element = view(&plots, surface);
}

#[test]
fn layer_region_input_masks_only_measured_widgets() {
    let rects = input_rects(
        &Surface {
            input: "regions".into(),
            ..Surface::default()
        },
        &[
            Rectangle::new(iced::Point::new(10.2, 20.7), iced::Size::new(100.2, 40.1)),
            Rectangle::new(iced::Point::new(10.2, 20.7), iced::Size::new(100.2, 40.1)),
            Rectangle::new(iced::Point::new(300.0, 400.0), iced::Size::new(0.2, 2.0)),
        ],
    );
    assert_eq!(rects, [(10, 20, 101, 41), (300, 400, 1, 2)]);
    assert!(
        input_rects(
            &Surface {
                input: "none".into(),
                ..Surface::default()
            },
            &[]
        )
        .is_empty()
    );
    assert_eq!(
        input_rects(
            &Surface {
                input: "all".into(),
                ..Surface::default()
            },
            &[]
        ),
        [(0, 0, 32768, 32768)]
    );
}

#[test]
fn an_app_can_declare_several_layers_and_each_gets_its_own_surface() {
    let fixture = Fixture::new(
        r#"
        return {
            view = function(self)
                return {
                    iced.layer({ id = "wallpaper", layer = "background",
                        anchors = { top = true, right = true, bottom = true, left = true },
                        exclusive_zone = -1, content = iced.text("wallpaper") }),
                    iced.layer({ id = "bar", layer = "top",
                        anchors = { top = true, left = true, right = true },
                        height = 32, exclusive_zone = 32, content = iced.text("bar") }),
                    iced.layer({ id = "hud", layer = "overlay", input = "none",
                        anchors = { bottom = true, left = true },
                        width = 120, height = 40, content = iced.text("hud") }),
                }
            end,
        }
        "#,
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    assert_eq!(app.roots.len(), 3, "{:?}", app.roots.keys());
    let mut state = State::default();
    state.instances.insert((OutputId(1), "bar".into()), app);
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let mut commands = Vec::new();
    assert!(reconcile(&mut plots, &mut state, &mut commands));
    let app = &state.instances[&(OutputId(1), "bar".into())];
    // One native surface per declared layer, each keyed by its layer id.
    let surfaces: Vec<_> = app.roots.keys().cloned().collect();
    assert_eq!(surfaces, ["bar", "hud", "wallpaper"]);
    for root in app.roots.values() {
        assert!(root.surface.is_some(), "{:?}", root.root.surface.id);
        assert!(plots.windows.ids.contains_key(&root.surface.unwrap()));
    }
    // Each surface renders only its own layer's content.
    for (id, layer) in &app.roots {
        let crate::ui::node::WidgetNode::Text { content, .. } = &layer.root.content else {
            panic!("{id} is not text")
        };
        assert_eq!(content, id);
    }
    // Dropping a layer retires exactly its surface.
    let dropped = state
        .instances
        .get_mut(&(OutputId(1), "bar".into()))
        .unwrap()
        .roots
        .remove("hud")
        .unwrap()
        .surface
        .unwrap();
    state
        .instances
        .get_mut(&(OutputId(1), "bar".into()))
        .unwrap()
        .retired
        .push(dropped);
    let mut commands = Vec::new();
    reconcile(&mut plots, &mut state, &mut commands);
    assert!(!plots.windows.ids.contains_key(&dropped));
    assert_eq!(state.instances[&(OutputId(1), "bar".into())].roots.len(), 2);
}

#[test]
fn an_app_composes_a_layer_from_its_own_file() {
    let fixture = Fixture::new(
        r#"
        return {
            view = function(self)
                return {
                    iced.layer({ id = "background", width = 100, height = 100,
                        content = iced.text("own") }),
                    riced.layer("layers/hud.lua", { label = "hud" }),
                }
            end,
        }
        "#,
    );
    let layer = fixture
        .project
        .app_root("bar")
        .join("components/layers/hud.lua");
    std::fs::create_dir_all(layer.parent().unwrap()).unwrap();
    std::fs::write(
        &layer,
        r#"
        return {
            view = function(self, props)
                return iced.layer({
                    id = "hud", layer = "overlay", input = "none",
                    anchors = { bottom = true, left = true },
                    width = 120, height = 40,
                    content = iced.text(props.label),
                })
            end,
        }
        "#,
    )
    .unwrap();
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    // The layer file owns its surface, not just its content.
    assert_eq!(app.roots.len(), 2, "{:?}", app.roots.keys());
    let hud = app.roots.get("hud").expect("hud layer");
    assert_eq!(hud.root.surface.layer, "overlay");
    assert_eq!(hud.root.surface.input, "none");
    let crate::ui::node::WidgetNode::Text { content, .. } = &hud.root.content else {
        panic!("hud content")
    };
    assert_eq!(content, "hud");

    // A layer file that returns a widget tree is rejected by name.
    std::fs::write(
        &layer,
        "return { view = function(self) return iced.text('nope') end }",
    )
    .unwrap();
    let mut app = instance("DP-1");
    let error = fixture.render(&mut app, false).unwrap_err();
    assert!(
        error.contains("layers/hud.lua must return iced.layer"),
        "{error}"
    );
}

#[test]
fn duplicate_layer_ids_and_duplicate_list_ids_are_rejected() {
    let fixture = Fixture::new(
        r#"
        return {
            view = function(self)
                return {
                    iced.layer({ id = "same", width = 10, height = 10, content = iced.text("a") }),
                    iced.layer({ id = "same", width = 10, height = 10, content = iced.text("b") }),
                }
            end,
        }
        "#,
    );
    let mut app = instance("DP-1");
    let error = fixture.render(&mut app, false).unwrap_err();
    assert!(error.contains("duplicate layer id"), "{error}");
}

#[test]
fn corrected_configuration_retries_a_failed_candidate_without_a_source_edit() {
    let fixture = Fixture::new(
        "return {view=function(self,props) return iced.layer({width=100,height=props.height,content=iced.text('valid')}) end}",
    );
    let mut app = instance("DP-1");
    app.props["height"] = json!(0);
    assert!(fixture.render(&mut app, false).is_err());
    app.props["height"] = json!(32);
    fixture.render(&mut app, false).unwrap();
    assert_eq!(text(&app), "valid");
}

#[test]
fn context_updates_on_non_ticking_refreshes_without_remounting() {
    let fixture = Fixture::new(
        "return {mount=function(self) self.mounts=1 end,view=function(self) return iced.layer({width=100,height=32,content=iced.text(self.context.output.name..':'..self.mounts)}) end}",
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    app.context["output"]["name"] = json!("renamed");
    fixture.render(&mut app, false).unwrap();
    assert_eq!(text(&app), "renamed:1");
}

#[test]
fn native_close_releases_the_instance_even_if_unmount_fails() {
    let fixture = Fixture::new(
        "return {unmount=function() error('cleanup failed') end,view=function() return iced.layer({width=100,height=32,content=iced.text('valid')}) end}",
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let surface = window::Id::unique();
    app.set_surface(surface);
    plots
        .apps
        .instances
        .insert((OutputId(1), "bar".into()), app);
    plots
        .windows
        .ids
        .insert(surface, PlotInfo::App(OutputId(1)));
    plots.forget_window(surface);
    assert!(plots.apps.instances.is_empty());
    assert!(!plots.windows.ids.contains_key(&surface));
}

async fn managed_deliveries(instance: &mut Instance) -> Vec<Delivery> {
    let tasks = with_services(|services| instance.managed_work(services));
    let Some(stream) = iced_runtime::task::into_stream(Task::batch(tasks)) else {
        return Vec::new();
    };
    stream
        .filter_map(|action| async move {
            match action {
                iced_runtime::Action::Output(Plant::App(AppMessage::Managed(delivery))) => {
                    Some(delivery)
                }
                _ => None,
            }
        })
        .collect()
        .await
}

#[test]
fn palette_extension_callbacks_apply_only_for_live_owners() {
    let source = r##"return {
        mount=function(self)
            self:palette_job()
        end,
        palette_job=function(self)
            self.palette=riced.service('palette')
            self.palette:command('generate',{seed='#89b4fa',dark=true},function(result)
                assert(result.ok)
                return iced.palette(result.data.colors)
            end)
        end,
        actions=function(self,event)
            if event.key=='generate' then self:palette_job() end
        end,
        view=function() return iced.layer({width=100,height=32,content=iced.text('preview')}) end,
    }"##;
    let fixture = Fixture::new(source);
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let deliveries = managed_deliveries(&mut app).await;
            assert_eq!(deliveries.len(), 1);
            let value =
                with_services(|services| app.deliver(services, &deliveries[0]).unwrap().unwrap());
            let mlua::Value::UserData(patch) = value else {
                panic!("expected a native palette value")
            };
            assert!(
                !patch
                    .borrow::<crate::theme::palette::PaletteOverrides>()
                    .unwrap()
                    .is_empty()
            );
            assert!(with_services(|services| app.deliver(services, &deliveries[0])).is_none());

            with_services(|services| {
                app.runtime.as_mut().unwrap().app_event(
                    services,
                    &app.output_name,
                    &app.context,
                    &app.props,
                    AppEvent::Action(&"generate".into()),
                )
            })
            .unwrap();
            let pending = managed_deliveries(&mut app).await;
            assert_eq!(pending.len(), 1);
            // A different VM must never accept an unconsumed previous owner's job.
            fixture.write(&source.replace("'preview'", "'reloaded'"));
            fixture.render(&mut app, false).unwrap();
            assert!(with_services(|services| app.deliver(services, &pending[0])).is_none());
        });
}

#[test]
fn process_results_enter_the_owning_app_and_callbacks_can_schedule_timers() {
    let fixture = Fixture::new(
        r#"return {
        mount=function(self)
            self.message='pending'
            self:spawn({'/bin/sh','-c','printf ready'}, {timeout_ms=1000}, function(result)
                assert(result.ok)
                self.message=result.stdout
                self:after(1,function() self.message='timer' end)
            end)
        end,
        view=function(self) return iced.layer({width=100,height=32,content=iced.text(self.message)}) end
    }"#,
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    assert_eq!(text(&app), "pending");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let deliveries = managed_deliveries(&mut app).await;
            assert_eq!(deliveries.len(), 1);
            with_services(|services| app.deliver(services, &deliveries[0]).unwrap().unwrap());
            fixture.render(&mut app, false).unwrap();
            assert_eq!(text(&app), "ready");
            let deliveries = managed_deliveries(&mut app).await;
            with_services(|services| app.deliver(services, &deliveries[0]).unwrap().unwrap());
            fixture.render(&mut app, false).unwrap();
            assert_eq!(text(&app), "timer");
        });
}

#[test]
fn reload_cancels_old_work_and_late_results_cannot_enter_the_new_vm() {
    let fixture = Fixture::new(
        r#"return {
        mount=function(self) self:after(60000,function() error('retired callback ran') end) end,
        view=function() return iced.layer({width=100,height=32,content=iced.text('old')}) end
    }"#,
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let tasks = with_services(|services| app.managed_work(services));
    fixture.write("return {view=function() return iced.layer({width=100,height=32,content=iced.text('new')}) end}");
    fixture.render(&mut app, false).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let stream = iced_runtime::task::into_stream(Task::batch(tasks)).unwrap();
            let actions: Vec<_> = tokio::time::timeout(Duration::from_secs(1), stream.collect())
                .await
                .unwrap();
            for action in actions {
                if let iced_runtime::Action::Output(Plant::App(AppMessage::Managed(delivery))) =
                    action
                {
                    assert_eq!(delivery.payload["error"]["code"], "cancelled");
                    assert!(with_services(|services| app.deliver(services, &delivery)).is_none());
                }
            }
        });
    assert_eq!(text(&app), "new");
}

#[test]
fn failed_candidates_never_launch_work_and_views_cannot_register_it() {
    let fixture = Fixture::new(
        r#"return {
        mount=function(self) self:spawn({'/bin/sh','-c','touch launched'},function() end) end,
        view=function() return iced.layer({width=0,height=32,content=iced.text('bad')}) end
    }"#,
    );
    let mut app = instance("DP-1");
    assert!(fixture.render(&mut app, false).is_err());
    assert!(with_services(|services| app.managed_work(services)).is_empty());
    assert!(!fixture.project.app_root("bar").join("launched").exists());
    fixture.write("return {view=function(self) self:after(1,function() end); return nil end}");
    assert!(
        fixture
            .render(&mut app, false)
            .unwrap_err()
            .contains("not views")
    );
}

#[test]
fn service_subscriptions_deliver_an_initial_detached_snapshot_only_once() {
    let fixture = Fixture::new(
        r#"return {
        mount=function(self)
            self.updates=0
            self:subscribe('system',function(snapshot)
                assert(snapshot.available and snapshot.name=='system')
                self.updates=self.updates+1
                self.cpu=snapshot.data.cpu_usage
            end)
        end,
        view=function(self) return iced.layer({width=100,height=32,content=iced.text(self.updates)}) end
    }"#,
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let deliveries = managed_deliveries(&mut app).await;
            assert_eq!(deliveries.len(), 1);
            with_services(|services| app.deliver(services, &deliveries[0]).unwrap().unwrap());
            fixture.render(&mut app, false).unwrap();
            assert_eq!(text(&app), "1");
            assert!(managed_deliveries(&mut app).await.is_empty());
        });
}

#[test]
fn file_callbacks_update_owned_cache_and_lua_data_saves_do_not_reload_app_code() {
    let fixture = Fixture::new(
        r#"return {
        mount=function(self)
            self.settings=riced.file({type='lua',path='settings.lua',write_mode='generated',defaults={count=0}})
            self.settings:reload(function(result) assert(result.ok) end)
        end,
        actions=function(self,event)
            self.settings:set('count',self.settings:get('count')+1)
            self.settings:save(function(result) assert(result.ok) end)
        end,
        view=function(self) return iced.layer({width=100,height=32,content=iced.text(self.settings:get('count'))}) end
    }"#,
    );
    let path = fixture.project.app_root("bar").join("settings.lua");
    std::fs::write(&path, "return {count=7}").unwrap();
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let generation = app.runtime.as_ref().unwrap().managed_generation();
    assert_eq!(text(&app), "0");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for delivery in managed_deliveries(&mut app).await {
                with_services(|services| app.deliver(services, &delivery).unwrap().unwrap());
            }
            fixture.render(&mut app, false).unwrap();
            assert_eq!(text(&app), "7");
            with_services(|services| {
                app.runtime
                    .as_mut()
                    .unwrap()
                    .app_event(
                        services,
                        &app.output_name,
                        &app.context,
                        &app.props,
                        AppEvent::Action(&Action::from("save".to_string())),
                    )
                    .unwrap()
            });
            for delivery in managed_deliveries(&mut app).await {
                with_services(|services| app.deliver(services, &delivery).unwrap().unwrap());
            }
            fixture.render(&mut app, false).unwrap();
            assert_eq!(text(&app), "8");
        });
    assert_eq!(
        app.runtime.as_ref().unwrap().managed_generation(),
        generation
    );
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains("Generated data")
    );
}

#[test]
fn unchanged_file_watch_deliveries_do_not_reexecute_lua_views() {
    let fixture = Fixture::new(
        r#"return {
        mount=function(self)
            self.views=0
            self.file=riced.file({type='json',path='settings.json'})
            self.file:reload(function(result)
                assert(result.ok)
                self.file:watch(function() error('unchanged file invoked callback') end)
            end)
        end,
        view=function(self) self.views=self.views+1; return iced.layer({width=100,height=32,content=iced.text(self.views)}) end
    }"#,
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for delivery in managed_deliveries(&mut app).await {
                with_services(|services| app.deliver(services, &delivery).unwrap().unwrap());
            }
            fixture.render(&mut app, false).unwrap();
            assert_eq!(text(&app), "2");
            let deliveries = managed_deliveries(&mut app).await;
            assert_eq!(deliveries.len(), 1);
            assert_eq!(deliveries[0].payload["unchanged"], true);
            let (_, receiver) = iced_wayland_subscriber::shell::channel();
            let mut plots = Plots::new(receiver);
            plots.config.apps.clear();
            plots
                .apps
                .instances
                .insert((OutputId(1), "bar".into()), app);
            assert!(
                messages(update(
                    &mut plots,
                    AppMessage::Managed(deliveries.into_iter().next().unwrap())
                ))
                .is_empty()
            );
            assert_eq!(
                text(&plots.apps.instances[&(OutputId(1), "bar".into())]),
                "2"
            );
        });
}

#[test]
fn app_teardown_cancels_file_watch_work_and_rejects_retired_deliveries() {
    let fixture = Fixture::new(
        r#"return {
        mount=function(self)
            self.file=riced.file({type='json',path='settings.json'})
            self.file:watch(function() error('retired watcher ran') end)
        end,
        view=function() return iced.layer({width=100,height=32,content=iced.text('watch')}) end
    }"#,
    );
    let mut app = instance("DP-1");
    fixture.render(&mut app, false).unwrap();
    let tasks = with_services(|services| app.managed_work(services));
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let surface = window::Id::unique();
    app.set_surface(surface);
    plots
        .apps
        .instances
        .insert((OutputId(1), "bar".into()), app);
    plots
        .windows
        .ids
        .insert(surface, PlotInfo::App(OutputId(1)));
    plots.forget_window(surface);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let stream = iced_runtime::task::into_stream(Task::batch(tasks)).unwrap();
            let actions: Vec<_> = tokio::time::timeout(Duration::from_secs(1), stream.collect())
                .await
                .unwrap();
            for action in actions {
                if let iced_runtime::Action::Output(Plant::App(AppMessage::Managed(delivery))) =
                    action
                {
                    assert_eq!(delivery.payload["error"]["code"], "cancelled");
                    assert!(messages(update(&mut plots, AppMessage::Managed(delivery))).is_empty());
                }
            }
        });
    assert!(plots.apps.instances.is_empty());
}
