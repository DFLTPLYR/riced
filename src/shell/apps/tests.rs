use super::*;
use iced::futures::StreamExt;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    project: Project,
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
    let crate::ui::node::WidgetNode::Text { content, .. } =
        &instance.root.as_ref().unwrap().content
    else {
        panic!("text root")
    };
    content
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
        .surface
        .unwrap();
    // Every view increments count, making unrelated executions observable.
    let other_revision = state.instances[&(OutputId(2), "bar".into())].revision;
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
        plots.apps.instances[&(OutputId(2), "bar".into())].revision,
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
fn background_example_mounts_handles_local_gestures_and_builds_its_menu() {
    let fixture = Fixture::new(include_str!(
        "../../../config/riced/example/apps/background/app.lua"
    ));
    std::fs::write(
        fixture.project.app_root("bar").join("components/menu.lua"),
        include_str!("../../../config/riced/example/apps/background/components/menu.lua"),
    )
    .unwrap();
    let mut app = instance("DP-1");
    app.context["output"] = json!({"name":"DP-1","x":0,"y":0,"width":1920,"height":1080});
    app.props["output"] = app.context["output"].clone();
    fixture.render(&mut app, false).unwrap();
    assert!(app.root.as_ref().unwrap().surface.pointer_events);
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
    let config =
        crate::config::test_parse_config(include_str!("../../../config/riced/example/config.lua"))
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
        .surface
        .unwrap();
    let mut commands = Vec::new();
    assert!(!reconcile(&mut plots, &mut state, &mut commands));
    assert!(commands.is_empty());
    state
        .instances
        .get_mut(&(OutputId(1), "bar".into()))
        .unwrap()
        .root
        .as_mut()
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
    app.surface = Some(surface);
    plots
        .windows
        .ids
        .insert(surface, PlotInfo::App(OutputId(1)));
    app.root = None;
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
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/framework"),
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
    assert_eq!(app.root.as_ref().unwrap().surface.exclusive_zone, 32);
    let (_, receiver) = iced_wayland_subscriber::shell::channel();
    let mut plots = Plots::new(receiver);
    let surface = window::Id::unique();
    app.surface = Some(surface);
    plots
        .apps
        .instances
        .insert((OutputId(1), "bar".into()), app);
    let _element = view(&plots, surface);
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
    app.surface = Some(surface);
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
    app.surface = Some(surface);
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
