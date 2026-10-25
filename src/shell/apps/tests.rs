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
