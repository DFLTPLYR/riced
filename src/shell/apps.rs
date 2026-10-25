//! Opt-in framework apps: one Lua instance and layer root per app/output.
use super::{Plant, Plots, windows::PlotInfo};
use crate::{
    framework::tasks::{Delivery, DeliveryKind, Owner},
    framework::{AppConfig, LayerRoot, Project, Surface},
    lua::{AppEvent, LuaRuntime},
    services::ServiceCtx,
    ui::{
        action::Action,
        scene::Lists,
        widgets::measured::{self, Measurement, SharedFrame},
    },
};
use iced::{Element, Task, window};
use iced_exwlshell::reexport::WlRegion;
use iced_wayland_subscriber::OutputId;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum AppMessage {
    Managed(Delivery),
    Tick,
    Changed,
    Redraw(window::Id),
    Action {
        surface: window::Id,
        action: Action,
    },
    Hover {
        surface: window::Id,
        list: String,
        key: String,
        entered: bool,
    },
    Measured {
        surface: window::Id,
        revision: u64,
        data: Measurement,
    },
}

type Sources = Vec<(PathBuf, String)>;
#[derive(Default)]
pub(crate) struct State {
    instances: BTreeMap<(OutputId, String), Instance>,
}
impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Apps")
            .field("instances", &self.instances.len())
            .finish()
    }
}
#[derive(Default)]
struct Instance {
    managed: Owner,
    runtime: Option<LuaRuntime>,
    accepted: Option<Sources>,
    attempted: Option<(Sources, Value)>,
    root: Option<LayerRoot>,
    surface: Option<window::Id>,
    applied: Option<Surface>,
    context: Value,
    props: Value,
    output_name: String,
    lists: Lists,
    frame: SharedFrame,
    hover: crate::ui::hover::HoverSet,
    revision: u64,
    error: Option<String>,
    service_error: Option<String>,
}
impl Instance {
    fn unmount(&mut self, services: &ServiceCtx<'_>, reason: &str) {
        self.managed.cancel_all();
        if let Some(runtime) = &mut self.runtime
            && let Err(error) = runtime.app_event(
                services,
                &self.output_name,
                &self.context,
                &self.props,
                AppEvent::Unmount(reason),
            )
        {
            eprintln!("app {} unmount: {error}", self.context["id"]);
        }
        self.runtime = None;
    }
    fn render(
        &mut self,
        request: &Request<'_>,
        motion: &mut crate::ui::anim::AnimRuntime,
        tick: bool,
    ) -> Result<bool, String> {
        let Request {
            project,
            config,
            sources,
            components,
            services,
        } = request;
        let sources = sources.as_ref().map_err(Clone::clone)?;
        let mut signature = sources.clone();
        signature.extend_from_slice(components);
        let changed = self.accepted.as_ref() != Some(&signature);
        let retry = !self
            .attempted
            .as_ref()
            .is_some_and(|(sources, props)| sources == &signature && props == &self.props);
        let mut candidate = if changed && retry {
            self.attempted = Some((signature.clone(), self.props.clone()));
            let mut runtime =
                LuaRuntime::with_profile(crate::lua::sandbox::Profile::Widget, components.to_vec())
                    .map_err(|e| e.to_string())?;
            runtime
                .set_app_root(&project.app_root(&config.name))
                .map_err(|e| e.to_string())?;
            let entry = project.source(&config.name);
            let source = sources
                .iter()
                .find(|(path, _)| *path == entry)
                .ok_or("app entry point missing")?;
            runtime
                .load_named(&source.1, &entry.display().to_string())
                .map_err(|e| e.to_string())?;
            if let Err(error) = runtime.app_event(
                services,
                &self.output_name,
                &self.context,
                &self.props,
                AppEvent::Mount,
            ) {
                let _ = runtime.app_event(
                    services,
                    &self.output_name,
                    &self.context,
                    &self.props,
                    AppEvent::Unmount("failed_mount"),
                );
                return Err(error.to_string());
            }
            Some(runtime)
        } else {
            None
        };
        let replaced = candidate.is_some();
        let runtime = candidate
            .as_mut()
            .or(self.runtime.as_mut())
            .ok_or("app has no working runtime")?;
        if !replaced && tick {
            runtime
                .app_event(
                    services,
                    &self.output_name,
                    &self.context,
                    &self.props,
                    AppEvent::Tick,
                )
                .map_err(|e| e.to_string())?;
        }
        let root = match runtime.layer_view(
            services,
            &self.output_name,
            &self.props,
            &self.context,
            &self.hover,
        ) {
            Ok(root) => root,
            Err(error) => {
                if let Some(runtime) = candidate.as_mut() {
                    let _ = runtime.app_event(
                        services,
                        &self.output_name,
                        &self.context,
                        &self.props,
                        AppEvent::Unmount("failed_mount"),
                    );
                }
                return Err(error.to_string());
            }
        };
        let modified = match self.update_root(root, motion, replaced) {
            Ok(modified) => modified,
            Err(error) => {
                if let Some(runtime) = candidate.as_mut() {
                    let _ = runtime.app_event(
                        services,
                        &self.output_name,
                        &self.context,
                        &self.props,
                        AppEvent::Unmount("failed_mount"),
                    );
                }
                return Err(error);
            }
        };
        if let Some(runtime) = candidate {
            self.unmount(services, "reload");
            self.runtime = Some(runtime);
            self.accepted = Some(signature);
            self.error = None;
        }
        Ok(modified)
    }

    fn update_root(
        &mut self,
        root: Option<LayerRoot>,
        motion: &mut crate::ui::anim::AnimRuntime,
        replaced: bool,
    ) -> Result<bool, String> {
        let modified = replaced || self.root != root;
        if modified {
            if let Some(root) = &root {
                let frame = self
                    .frame
                    .lock()
                    .map_err(|_| "app measurement lock")?
                    .last
                    .clone()
                    .unwrap_or_default();
                let synchronized = self.lists.synchronize(
                    self.root.as_ref().map(|r| &r.content),
                    &root.content,
                    &frame,
                    motion,
                    Duration::from_millis(250),
                );
                synchronized?;
            } else {
                self.lists.clear(motion);
                self.hover.clear();
            }
            self.root = root;
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(modified)
    }

    fn managed_work(&mut self, services: &ServiceCtx<'_>) -> Vec<Task<Plant>> {
        let Some(runtime) = self.runtime.as_ref() else {
            return Vec::new();
        };
        let Some(generation) = runtime.managed_generation() else {
            return Vec::new();
        };
        self.managed.reset(generation);
        self.managed.apply(runtime.managed_commands());
        let mut tasks: Vec<_> = self
            .managed
            .jobs()
            .into_iter()
            .map(|job| {
                Task::perform(job.run(), |delivery| {
                    Plant::App(AppMessage::Managed(delivery))
                })
            })
            .collect();
        match runtime.managed_services(services) {
            Ok(deliveries) => {
                self.service_error = None;
                tasks.extend(
                    deliveries
                        .into_iter()
                        .map(|delivery| Task::done(Plant::App(AppMessage::Managed(delivery)))),
                );
            }
            Err(error) => crate::lua::error::report_once(
                &mut self.service_error,
                "app service subscription: ",
                error.to_string(),
            ),
        }
        tasks
    }

    fn deliver(
        &mut self,
        services: &ServiceCtx<'_>,
        delivery: &Delivery,
    ) -> Option<mlua::Result<mlua::Value>> {
        let runtime = self.runtime.as_mut()?;
        if !runtime.accepts_managed(delivery) {
            return None;
        }
        if delivery.kind == DeliveryKind::Job && !self.managed.complete(delivery) {
            return None;
        }
        Some(runtime.app_event(
            services,
            &self.output_name,
            &self.context,
            &self.props,
            AppEvent::Managed(delivery),
        ))
    }
}
struct Request<'a> {
    project: &'a Project,
    config: &'a AppConfig,
    sources: &'a Result<Sources, String>,
    components: &'a [(PathBuf, String)],
    services: &'a ServiceCtx<'a>,
}

impl State {
    pub fn has_callbacks(&self) -> bool {
        self.instances.values().any(|i| i.lists.has_callbacks())
    }
    pub fn forget(
        &mut self,
        surface: window::Id,
        services: &ServiceCtx<'_>,
        motion: &mut crate::ui::anim::AnimRuntime,
    ) {
        self.instances.retain(|_, instance| {
            if instance.surface == Some(surface) {
                instance.unmount(services, "closed");
                instance.lists.clear(motion);
                false
            } else {
                true
            }
        });
    }
}

fn services(plots: &Plots) -> ServiceCtx<'_> {
    ServiceCtx::from_plots(plots, plots.services.gpu())
}

pub(crate) fn refresh(plots: &mut Plots, tick: bool) -> Task<Plant> {
    refresh_project(plots, &Project::installed(), tick)
}
fn refresh_project(plots: &mut Plots, project: &Project, tick: bool) -> Task<Plant> {
    refresh_target(plots, project, tick, None)
}

// Input does not need source discovery/hot reload or unrelated app views.
fn refresh_surface(plots: &mut Plots, surface: window::Id) -> Task<Plant> {
    let mut state = std::mem::take(&mut plots.apps);
    let mut commands = Vec::new();
    let mut modified = false;
    if let Some(instance) = state
        .instances
        .values_mut()
        .find(|i| i.surface == Some(surface))
    {
        let result = instance.runtime.as_mut().map(|runtime| {
            runtime.layer_view(
                &services(plots),
                &instance.output_name,
                &instance.props,
                &instance.context,
                &instance.hover,
            )
        });
        if let Some(result) = result {
            match result
                .map_err(|e| e.to_string())
                .and_then(|root| instance.update_root(root, &mut plots.animation.motion, false))
            {
                Ok(changed) => modified = changed,
                Err(error) => {
                    crate::lua::error::report_once(&mut instance.error, "app render: ", error)
                }
            }
        }
        commands.extend(instance.managed_work(&services(plots)));
    }
    let lifecycle_changed = reconcile(plots, &mut state, &mut commands);
    plots.apps = state;
    if lifecycle_changed {
        commands.push(Task::done(Plant::App(AppMessage::Changed)));
    } else if modified {
        commands.push(Task::done(Plant::App(AppMessage::Redraw(surface))));
    }
    Task::batch(commands)
}
fn refresh_target(
    plots: &mut Plots,
    project: &Project,
    tick: bool,
    target: Option<u64>,
) -> Task<Plant> {
    if plots.config.apps.is_empty() && plots.apps.instances.is_empty() {
        return Task::none();
    }
    let mut state = std::mem::take(&mut plots.apps);
    let components = crate::config::component_files();
    let mut desired = HashSet::new();
    let mut commands = Vec::new();
    let mut changed = false;
    let mut redraws = Vec::new();
    for config in &plots.config.apps {
        let sources = project.sources(&config.name);
        for (&output, info) in &plots.windows.output_infos {
            let name = info.name.as_deref().unwrap_or("");
            if !config.output.is_empty() && config.output != name {
                continue;
            }
            let key = (output, config.identity().to_string());
            desired.insert(key.clone());
            let instance = state.instances.entry(key).or_default();
            if target.is_some_and(|target| {
                instance
                    .runtime
                    .as_ref()
                    .and_then(LuaRuntime::managed_generation)
                    != Some(target)
            }) {
                continue;
            }
            let geometry = crate::shared::geometry::output_geometry(info);
            let output_context = json!({"name":name,"x":geometry.0,"y":geometry.1,"width":geometry.2,"height":geometry.3});
            instance.context = json!({"id":config.identity(),"name":config.name,"root":project.app_root(&config.name),"output":output_context});
            instance.props = serde_json::to_value(&config.props).unwrap_or_else(|_| json!({}));
            instance.props["output"] = output_context;
            instance.output_name = name.to_string();
            instance.lists.scope = format!("app:{}:{}", output.0, config.identity());
            let service_ctx = ServiceCtx {
                sys: &plots.services.system,
                gpu: plots.services.gpu(),
                sampled: plots.services.sampled(),
                theme: &plots.config.theme,
                component_defaults: &plots.config.component_defaults,
                outputs: &plots.windows.output_infos,
                notifications: &plots.notification.queue,
                toplevels: &plots.services.toplevels,
                workspaces: &plots.services.workspaces,
                tray: &plots.services.tray,
            };
            let request = Request {
                project,
                config,
                sources: &sources,
                components: &components,
                services: &service_ctx,
            };
            match instance.render(&request, &mut plots.animation.motion, tick) {
                Ok(true) => {
                    if let Some(surface) = instance.surface {
                        redraws.push(surface);
                    }
                }
                Ok(false) => {}
                Err(error) => crate::lua::error::report_once(
                    &mut instance.error,
                    &format!("app {} on {name}: ", config.identity()),
                    error,
                ),
            }
            commands.extend(instance.managed_work(&service_ctx));
        }
    }
    let obsolete: Vec<_> = state
        .instances
        .keys()
        .filter(|key| !desired.contains(*key))
        .cloned()
        .collect();
    for key in obsolete {
        if let Some(mut instance) = state.instances.remove(&key) {
            instance.unmount(&services(plots), "removed");
            instance.lists.clear(&mut plots.animation.motion);
            if let Some(surface) = instance.surface.take() {
                commands.push(close(plots, surface));
            }
            changed = true;
        }
    }
    changed |= reconcile(plots, &mut state, &mut commands);
    plots.apps = state;
    if changed {
        commands.push(Task::done(Plant::App(AppMessage::Changed)));
    } else {
        commands.extend(
            redraws
                .into_iter()
                .map(|surface| Task::done(Plant::App(AppMessage::Redraw(surface)))),
        );
    }
    Task::batch(commands)
}
fn reconcile(plots: &mut Plots, state: &mut State, commands: &mut Vec<Task<Plant>>) -> bool {
    let mut changed = false;
    for ((output, id), instance) in &mut state.instances {
        let Some(root) = &instance.root else {
            if let Some(surface) = instance.surface.take() {
                commands.push(close(plots, surface));
                changed = true;
            }
            instance.applied = None;
            continue;
        };
        // Changing the declared identity creates a new native surface, not a new VM.
        if instance
            .applied
            .as_ref()
            .is_some_and(|old| old.id != root.surface.id)
            && let Some(surface) = instance.surface.take()
        {
            commands.push(close(plots, surface));
        }
        let settings = root
            .surface
            .settings(output.0, format!("riced:app:{id}:{}", root.surface.id));
        if let Some(surface) = instance.surface {
            if instance.applied.as_ref() != Some(&root.surface) {
                changed = true;
                commands.extend([
                    Task::done(Plant::LayoutChange {
                        id: surface,
                        anchor: settings.anchor,
                        size: settings.size,
                    }),
                    Task::done(Plant::LayerChange {
                        id: surface,
                        layer: settings.layer,
                    }),
                    Task::done(Plant::ExclusiveZoneChange {
                        id: surface,
                        zone_size: root.surface.exclusive_zone,
                    }),
                    Task::done(Plant::MarginChange {
                        id: surface,
                        margin: settings.margin.unwrap_or_default(),
                    }),
                    Task::done(Plant::KeyboardInteractivityChange {
                        id: surface,
                        keyboard_interactivity: settings.keyboard_interactivity,
                    }),
                    input_region(surface, &root.surface),
                ]);
            }
        } else {
            let surface = window::Id::unique();
            instance.surface = Some(surface);
            plots.windows.ids.insert(surface, PlotInfo::App(*output));
            commands.push(Task::done(Plant::NewLayerShell {
                id: surface,
                settings,
            }));
            changed = true;
        }
        instance.applied = Some(root.surface.clone());
        for id in instance.lists.take_resets() {
            commands.push(iced::widget::operation::scroll_to(
                id,
                iced::widget::operation::AbsoluteOffset { x: 0.0, y: 0.0 },
            ));
        }
    }
    changed
}
fn close(plots: &mut Plots, surface: window::Id) -> Task<Plant> {
    plots.forget_window(surface);
    iced_runtime::task::effect(iced_runtime::Action::Window(
        iced_runtime::window::Action::Close(surface),
    ))
}
fn input_region(surface: window::Id, settings: &Surface) -> Task<Plant> {
    let accepts = settings.input == "all";
    Task::done(Plant::SetInputRegion {
        id: surface,
        callback: iced_exwlshell::actions::ActionCallback::new(move |region: &WlRegion| {
            if accepts {
                region.add(0, 0, 32768, 32768);
            }
        }),
    })
}
pub(crate) fn opened(plots: &Plots, surface: window::Id) -> Task<Plant> {
    plots
        .apps
        .instances
        .values()
        .find(|i| i.surface == Some(surface))
        .and_then(|i| i.root.as_ref())
        .map_or_else(Task::none, |root| input_region(surface, &root.surface))
}
pub(crate) fn view(plots: &Plots, surface: window::Id) -> Element<'static, Plant> {
    let Some(instance) = plots
        .apps
        .instances
        .values()
        .find(|i| i.surface == Some(surface))
    else {
        return iced::widget::Space::new().into();
    };
    let Some(root) = &instance.root else {
        return iced::widget::Space::new().into();
    };
    let sender = Arc::new(move |action| Plant::App(AppMessage::Action { surface, action }));
    let hover = Arc::new(move |list, key, entered| {
        Plant::App(AppMessage::Hover {
            surface,
            list,
            key,
            entered,
        })
    });
    match instance.lists.build(
        &root.content,
        &plots.animation.motion,
        sender.clone(),
        Some(hover),
    ) {
        Ok(element) => {
            let element = if root.surface.pointer_events {
                crate::ui::widgets::input::frame_input(element, sender)
            } else {
                element
            };
            let revision = instance.revision;
            measured::root(
                element,
                instance.frame.clone(),
                revision,
                Arc::new(move |data| {
                    Plant::App(AppMessage::Measured {
                        surface,
                        revision,
                        data,
                    })
                }),
            )
        }
        Err(error) => {
            eprintln!("app render: {error}");
            iced::widget::Space::new().into()
        }
    }
}
pub(crate) fn sweep(plots: &mut Plots) -> Task<Plant> {
    let mut completed = Vec::new();
    for instance in plots.apps.instances.values_mut() {
        instance.lists.sweep(&mut plots.animation.motion);
        completed.extend(instance.lists.take_completed());
    }
    super::animation::complete(plots, completed)
}
pub(crate) fn forget(plots: &mut Plots, surface: window::Id) {
    if !plots
        .apps
        .instances
        .values()
        .any(|instance| instance.surface == Some(surface))
    {
        return;
    }
    let mut state = std::mem::take(&mut plots.apps);
    // Resource teardown must run even when a Lua unmount hook fails.
    let mut motion = std::mem::take(&mut plots.animation.motion);
    state.forget(surface, &services(plots), &mut motion);
    plots.animation.motion = motion;
    plots.apps = state;
}
pub(crate) fn update(plots: &mut Plots, message: AppMessage) -> Task<Plant> {
    match message {
        AppMessage::Managed(delivery) => {
            let mut state = std::mem::take(&mut plots.apps);
            let mut refresh = false;
            let mut continuation = Vec::new();
            let result = state.instances.values_mut().find_map(|instance| {
                let result = instance.deliver(&services(plots), &delivery)?;
                refresh = instance
                    .runtime
                    .as_ref()
                    .is_some_and(LuaRuntime::managed_was_invoked);
                if !refresh {
                    continuation.extend(instance.managed_work(&services(plots)));
                }
                Some(result)
            });
            plots.apps = state;
            if !refresh {
                if let Some(Err(error)) = result {
                    eprintln!("app callback: {error}");
                }
                return Task::batch(continuation);
            }
            let command = match result {
                Some(Ok(value)) => super::actions::command_from_action(&value, plots),
                Some(Err(error)) => {
                    eprintln!("app callback: {error}");
                    Task::none()
                }
                None => return Task::none(),
            };
            Task::batch([
                command,
                refresh_target(
                    plots,
                    &Project::installed(),
                    false,
                    Some(delivery.generation),
                ),
            ])
        }
        AppMessage::Tick => {
            // A framework-only project must not depend on the legacy widget timer.
            if plots.catalog.definitions.is_empty() {
                plots.services.refresh_system();
            }
            refresh(plots, true)
        }
        AppMessage::Changed | AppMessage::Redraw(_) => Task::none(),
        AppMessage::Measured {
            surface,
            revision,
            data,
        } => {
            let Some(instance) = plots
                .apps
                .instances
                .values_mut()
                .find(|i| i.surface == Some(surface) && i.revision == revision)
            else {
                return Task::none();
            };
            if instance.lists.layout(
                &data,
                &mut plots.animation.motion,
                Duration::from_millis(250),
            ) {
                Task::done(Plant::App(AppMessage::Redraw(surface)))
            } else {
                Task::none()
            }
        }
        AppMessage::Hover {
            surface,
            list,
            key,
            entered,
        } => {
            let Some(instance) = plots
                .apps
                .instances
                .values_mut()
                .find(|i| i.surface == Some(surface))
            else {
                return Task::none();
            };
            let key = (list, key);
            if entered {
                instance.hover.clear();
                instance.hover.insert(key);
            } else {
                instance.hover.remove(&key);
            }
            refresh_surface(plots, surface)
        }
        AppMessage::Action { surface, action } => {
            if action.is_animation_frame() {
                return Task::none();
            }
            let mut state = std::mem::take(&mut plots.apps);
            let result = state
                .instances
                .values_mut()
                .find(|i| i.surface == Some(surface))
                .and_then(|instance| {
                    instance.runtime.as_mut().map(|runtime| {
                        runtime.app_event(
                            &services(plots),
                            &instance.output_name,
                            &instance.context,
                            &instance.props,
                            AppEvent::Action(&action),
                        )
                    })
                });
            plots.apps = state;
            let command = match result {
                Some(Ok(value)) => super::actions::command_from_action(&value, plots),
                Some(Err(error)) => {
                    eprintln!("app action: {error}");
                    Task::none()
                }
                None => return Task::none(),
            };
            Task::batch([command, refresh_surface(plots, surface)])
        }
    }
}
