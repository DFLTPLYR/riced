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

mod popup;
#[cfg(test)]
mod tests;
pub(crate) use popup::{close_children, dismiss as dismiss_popup, is_popup};

#[derive(Debug, Clone)]
pub enum AppMessage {
    Managed(Delivery),
    Tick,
    Changed,
    Redraw(window::Id),
    DismissPopup {
        surface: window::Id,
        reason: &'static str,
    },
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
    pending_closes: Vec<window::Id>,
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
    context: Value,
    props: Value,
    output_name: String,
    hover: crate::ui::hover::HoverSet,
    error: Option<String>,
    service_error: Option<String>,
    popup: Option<popup::Popup>,
    dismissed_popup: Option<String>,
    /// Declared layers, keyed by surface id. One entry per native surface the
    /// app owns; each carries its own list state and measurement frame.
    roots: BTreeMap<String, Root>,
    /// Surfaces whose layer the app stopped declaring; reconcile closes them.
    retired: Vec<window::Id>,
}

/// One declared layer and the native state that belongs to its surface.
struct Root {
    root: LayerRoot,
    surface: Option<window::Id>,
    applied: Option<Surface>,
    lists: Lists,
    revision: u64,
    frame: SharedFrame,
}

impl Instance {
    fn owns(&self, surface: window::Id) -> bool {
        self.root_by_surface(surface).is_some()
            || self.popup.as_ref().is_some_and(|p| p.surface == surface)
    }
    fn root_by_surface(&self, surface: window::Id) -> Option<&Root> {
        self.roots
            .values()
            .find(|root| root.surface == Some(surface))
    }
    fn surfaces(&self) -> Vec<window::Id> {
        self.roots
            .values()
            .filter_map(|root| root.surface)
            .chain(self.popup.as_ref().map(|p| p.surface))
            .collect()
    }
    /// Instance identity from its context, used to scope per-layer list state.
    fn identity(&self) -> &str {
        self.context
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or(&self.output_name)
    }
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
        let roots = match runtime.layer_view(
            services,
            &self.output_name,
            &self.props,
            &self.context,
            &self.hover,
        ) {
            Ok(roots) => roots,
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
        let modified = match self.update_roots(roots, motion, replaced) {
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
            self.dismissed_popup = None;
        }
        Ok(modified)
    }

    /// Adopt the layers the app declared this render.
    ///
    /// Layers keep their native surface and list state across renders; a layer
    /// the app stopped declaring has its surface retired for reconcile to close.
    fn update_roots(
        &mut self,
        roots: Vec<LayerRoot>,
        motion: &mut crate::ui::anim::AnimRuntime,
        replaced: bool,
    ) -> Result<bool, String> {
        let mut modified = replaced;
        let mut declared: BTreeMap<String, LayerRoot> = BTreeMap::new();
        for root in roots {
            declared.insert(root.surface.id.clone(), root);
        }
        let scope = format!("app:{}:{}", self.output_name, self.identity());
        let stale: Vec<String> = self
            .roots
            .keys()
            .filter(|id| !declared.contains_key(*id))
            .cloned()
            .collect();
        for id in stale {
            if let Some(mut root) = self.roots.remove(&id) {
                root.lists.clear(motion);
                self.retired.extend(root.surface);
                if self.popup.as_ref().is_some_and(|popup| popup.owner == id)
                    && let Some(mut popup) = self.popup.take()
                {
                    popup.lists.clear(motion);
                    self.retired.push(popup.surface);
                }
                modified = true;
            }
        }
        for (id, root) in declared {
            match self.roots.get_mut(&id) {
                Some(existing) => {
                    if existing.root == root {
                        continue;
                    }
                    let frame = existing
                        .frame
                        .lock()
                        .map_err(|_| "app measurement lock")?
                        .last
                        .clone()
                        .unwrap_or_default();
                    existing.lists.synchronize(
                        Some(&existing.root.content),
                        &root.content,
                        &frame,
                        motion,
                        Duration::from_millis(250),
                    )?;
                    existing.root = root;
                    existing.revision = existing.revision.wrapping_add(1);
                }
                None => {
                    let mut lists = Lists::default();
                    lists.scope = format!("{scope}:{id}");
                    lists.synchronize(
                        None,
                        &root.content,
                        &Default::default(),
                        motion,
                        Duration::from_millis(250),
                    )?;
                    self.roots.insert(
                        id,
                        Root {
                            root,
                            surface: None,
                            applied: None,
                            lists,
                            revision: 0,
                            frame: Default::default(),
                        },
                    );
                }
            }
            modified = true;
        }
        if modified && self.roots.is_empty() {
            self.hover.clear();
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

/// Single-layer accessors so tests that assert one root stay readable.
#[cfg(test)]
impl Instance {
    fn root(&self) -> Option<&LayerRoot> {
        self.roots.values().next().map(|root| &root.root)
    }
    fn root_mut(&mut self) -> Option<&mut LayerRoot> {
        self.roots.values_mut().next().map(|root| &mut root.root)
    }
    fn surface(&self) -> Option<window::Id> {
        self.roots.values().next().and_then(|root| root.surface)
    }
    /// Place the single declared layer on a chosen surface.
    fn set_surface(&mut self, surface: window::Id) {
        if let Some(root) = self.roots.values_mut().next() {
            root.surface = Some(surface);
        }
    }
    fn revision(&self) -> u64 {
        self.roots
            .values()
            .map(|root| root.revision)
            .max()
            .unwrap_or_default()
    }
    /// Stop declaring every layer through the real path, so surfaces retire.
    fn drop_roots(&mut self, motion: &mut crate::ui::anim::AnimRuntime) {
        let _ = self.update_roots(Vec::new(), motion, false);
    }
}

impl State {
    pub fn has_callbacks(&self) -> bool {
        self.instances.values().any(|i| {
            i.roots.values().any(|root| root.lists.has_callbacks())
                || i.popup.as_ref().is_some_and(|p| p.lists.has_callbacks())
        })
    }
    pub fn forget(
        &mut self,
        surface: window::Id,
        services: &ServiceCtx<'_>,
        motion: &mut crate::ui::anim::AnimRuntime,
    ) {
        let pending = &mut self.pending_closes;
        self.instances.retain(|_, instance| {
            // Closing any declared layer closes the whole app, as before: the
            // app declares its layers, so a partial set has no meaning.
            if instance.root_by_surface(surface).is_some() {
                if let Some(child) = popup::detach(instance, motion) {
                    pending.push(child);
                }
                instance.unmount(services, "closed");
                for root in instance.roots.values_mut() {
                    root.lists.clear(motion);
                }
                false
            } else {
                if instance
                    .popup
                    .as_ref()
                    .is_some_and(|p| p.surface == surface)
                {
                    instance.dismissed_popup =
                        instance.popup.as_ref().map(|p| p.root.geometry.id.clone());
                    popup::detach(instance, motion);
                }
                true
            }
        });
    }
}

/// A complete bar shape for apps.
///
/// `TopConfig` serializes through the file format, which omits fields equal to
/// their default, so serializing it directly hands Lua a partial object with a
/// nil `anchor`. Apps get every field they may read instead.
fn bar_json(bar: &crate::config::TopConfig) -> serde_json::Value {
    json!({
        "anchor": bar.anchor,
        "output": bar.output,
        "length": bar.length,
        "thickness": bar.thickness,
        "slots": bar.slots,
        "aligns": bar.aligns,
        "opacity": bar.opacity,
        "floating": bar.floating,
    })
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
    if let Some(instance) = state.instances.values_mut().find(|i| i.owns(surface)) {
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
                .and_then(|roots| instance.update_roots(roots, &mut plots.animation.motion, false))
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
        for instance in plots.apps.instances.values().filter(|i| i.owns(surface)) {
            commands.extend(
                instance
                    .surfaces()
                    .into_iter()
                    .map(|surface| Task::done(Plant::App(AppMessage::Redraw(surface)))),
            );
        }
    }
    Task::batch(commands)
}
/// Reconcile every app against the project after a config edit, so a changed
/// `config.bar` re-declares the app's layers.
pub(crate) fn reload(plots: &mut Plots) -> Task<Plant> {
    refresh_target(plots, &Project::installed(), false, None)
}

/// IPC is a shell command, delivered once rather than once per output.
pub(crate) fn ipc(plots: &mut Plots, command: &str, args: &[String]) -> Task<Plant> {
    let mut state = std::mem::take(&mut plots.apps);
    let selected = state
        .instances
        .iter()
        .filter(|(_, instance)| {
            instance
                .runtime
                .as_ref()
                .is_some_and(|runtime| runtime.has_ipc_handler(command))
        })
        .min_by_key(|((_, id), instance)| (id.as_str(), instance.output_name.as_str()))
        .map(|(key, _)| key.clone());
    let result = selected.and_then(|key| {
        let instance = state.instances.get_mut(&key)?;
        instance.runtime.as_mut().map(|runtime| {
            runtime.app_event(
                &services(plots),
                &instance.output_name,
                &instance.context,
                &instance.props,
                AppEvent::Ipc { command, args },
            )
        })
    });
    plots.apps = state;
    match result {
        Some(Ok(value)) => {
            let action = super::actions::command_from_action(&value, plots);
            Task::batch([action, reload(plots)])
        }
        Some(Err(error)) => {
            tracing::warn!(%command, %error, "IPC handler failed");
            Task::none()
        }
        None => {
            // Projects predating root.lua still use the native Settings host.
            // Root projects must explicitly register their opening policy.
            if command == "settings" && !Project::installed().root.join("root.lua").is_file() {
                return Task::done(Plant::Sprout);
            }
            tracing::warn!(%command, "No Lua IPC handler registered");
            Task::none()
        }
    }
}

fn refresh_target(
    plots: &mut Plots,
    project: &Project,
    tick: bool,
    target: Option<u64>,
) -> Task<Plant> {
    let configs = if project.root.join("root.lua").is_file() {
        vec![AppConfig {
            name: "root".into(),
            ..Default::default()
        }]
    } else {
        plots.config.apps.clone()
    };
    if configs.is_empty() && plots.apps.instances.is_empty() && plots.apps.pending_closes.is_empty()
    {
        return Task::none();
    }
    let mut state = std::mem::take(&mut plots.apps);
    let components = crate::config::component_files();
    let mut desired = HashSet::new();
    let mut commands = Vec::new();
    let mut changed = false;
    let mut redraws = Vec::new();
    for config in &configs {
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
            // Which native hosts own their surface. A root app uses these to
            // decide whether to compose its `layers/` modules or leave the
            // surface to the established Rust host.
            instance.props["bar_host"] = json!(plots.config.bar_host);
            instance.props["background_disabled"] = json!(plots.config.background.disabled);
            // The configured wallpaper source for this output. The layer draws
            // it immediately, so a background is visible before (and without) a
            // successful bake; the baked tile still draws over it.
            instance.props["wallpaper"] = json!(
                match plots.config.background.mode {
                    crate::config::WallpaperMode::PerMonitor => plots
                        .config
                        .background
                        .monitor
                        .iter()
                        .find(|entry| entry.output.trim().eq_ignore_ascii_case(name))
                        .map(|entry| entry.path.clone()),
                    crate::config::WallpaperMode::FreeForm => plots
                        .config
                        .background
                        .image
                        .first()
                        .map(|image| image.path.clone()),
                }
                .unwrap_or_default()
            );
            // The single source of truth for bars: Settings edits `config.bar`
            // and the app renders the entries that match this output.
            instance.props["bars"] = json!(
                plots
                    .config
                    .bar
                    .iter()
                    .filter(|bar| {
                        bar.output.trim().is_empty() || bar.output.trim().eq_ignore_ascii_case(name)
                    })
                    .map(bar_json)
                    .collect::<Vec<_>>()
            );
            instance.output_name = name.to_string();
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
                    redraws.extend(instance.surfaces());
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
            if let Some(child) = popup::detach(&mut instance, &mut plots.animation.motion) {
                commands.push(close(plots, child));
            }
            instance.unmount(&services(plots), "removed");
            for surface in instance.surfaces() {
                if plots.windows.ids.contains_key(&surface) {
                    commands.push(close(plots, surface));
                }
            }
            for root in instance.roots.values_mut() {
                root.lists.clear(&mut plots.animation.motion);
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
    for surface in state.pending_closes.drain(..) {
        if plots.windows.ids.contains_key(&surface) {
            commands.push(close(plots, surface));
            changed = true;
        }
    }
    for ((output, id), instance) in &mut state.instances {
        for surface in instance.retired.drain(..) {
            if plots.windows.ids.contains_key(&surface) {
                commands.push(close(plots, surface));
                changed = true;
            }
        }
        if instance.roots.is_empty() {
            if let Some(child) = popup::detach(instance, &mut plots.animation.motion) {
                commands.push(close(plots, child));
                changed = true;
            }
            instance.dismissed_popup = None;
        }
        for (root_id, layer) in &mut instance.roots {
            let surface_desc = layer.root.surface.clone();
            // Changing the declared identity creates a new native surface, not
            // a new VM.
            if layer
                .applied
                .as_ref()
                .is_some_and(|old| old.id != surface_desc.id)
                && layer.surface.take().is_some()
            {
                changed = true;
            }
            let settings = surface_desc.settings(output.0, format!("riced:app:{id}:{root_id}"));
            match layer.surface {
                Some(surface) => {
                    if layer.applied.as_ref() != Some(&surface_desc) {
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
                                zone_size: surface_desc.exclusive_zone,
                            }),
                            Task::done(Plant::MarginChange {
                                id: surface,
                                margin: settings.margin.unwrap_or_default(),
                            }),
                            Task::done(Plant::KeyboardInteractivityChange {
                                id: surface,
                                keyboard_interactivity: settings.keyboard_interactivity,
                            }),
                            input_region(surface, &surface_desc),
                        ]);
                    }
                }
                None => {
                    let surface = window::Id::unique();
                    layer.surface = Some(surface);
                    plots.windows.ids.insert(surface, PlotInfo::App(*output));
                    commands.push(Task::done(Plant::NewLayerShell {
                        id: surface,
                        settings,
                    }));
                    changed = true;
                }
            }
            layer.applied = Some(surface_desc);
            for id in layer.lists.take_resets() {
                commands.push(iced::widget::operation::scroll_to(
                    id,
                    iced::widget::operation::AbsoluteOffset { x: 0.0, y: 0.0 },
                ));
            }
        }
        changed |= popup::reconcile(plots, instance, *output, id, commands);
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
    if is_popup(plots, surface) {
        return input_region(surface, &Surface::default());
    }
    plots
        .apps
        .instances
        .values()
        .find(|i| i.owns(surface))
        .and_then(|i| i.root_by_surface(surface))
        .map_or_else(Task::none, |root| input_region(surface, &root.root.surface))
}
/// Host slots a framework layer can embed. `desktop-widgets` composites the
/// legacy desktop host, which has no description equivalent: widget instances,
/// service snapshots, hold-drag and placement all stay native.
fn app_native<'a>(
    plots: &'a Plots,
    surface: window::Id,
    output: Option<OutputId>,
) -> impl FnMut(&str, Option<&serde_json::Value>) -> Result<Element<'static, Plant>, String> + 'a {
    move |id, args| match (id, output) {
        ("wallpaper", Some(output)) => Ok(iced::widget::Stack::with_children(
            super::screens::Background::wallpaper_views(plots, output),
        )
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .into()),
        ("wallpaper", None) => Err("iced.native(\"wallpaper\") needs an output-bound app".into()),
        ("desktop-widgets", Some(output)) => {
            crate::shell::desktop_widgets::retain(plots);
            let mut stack = iced::widget::Stack::with_capacity(1);
            for view in crate::shell::desktop_widgets::views(plots, output, true) {
                stack = stack.push(view);
            }
            Ok(stack
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .into())
        }
        ("desktop-widgets", None) => {
            Err("iced.native(\"desktop-widgets\") needs an output-bound app".into())
        }
        ("widget", Some(_)) => {
            let args = args.ok_or_else(|| "iced.native(\"widget\") needs args".to_owned())?;
            let name = args
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "iced.native(\"widget\") needs a name".to_owned())?;
            let id = args
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(name)
                .to_owned();
            let props = args
                .get("props")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let instance = plots
                .apps
                .instances
                .values()
                .find(|instance| instance.owns(surface))
                .ok_or_else(|| "iced.native(\"widget\") needs an owning app".to_owned())?;
            let runtime = instance
                .runtime
                .as_ref()
                .ok_or_else(|| "iced.native(\"widget\") needs a live app".to_owned())?;
            let path = crate::config::discovery::widget_file_path(name, &format!("{name}.lua"));
            let resolved = crate::lua::widgets::ResolvedWidget {
                id: id.clone(),
                name: name.to_owned(),
                path,
                interval: 1.0,
                size: 13.0,
                props: crate::lua::embedded::json_props(&props),
            };
            let services = crate::services::ServiceCtx::from_plots(plots, plots.services.gpu());
            let context = crate::lua::widgets::EntryContext {
                services,
                output: instance.output_name.as_str(),
            };
            let value = runtime
                .widgets
                .borrow_mut()
                .render(&resolved, &context)
                .map_err(|error| error.to_string())?;
            let node = crate::ui::decode::parse_node(&value)?;
            let target = id.clone();
            let sender: crate::ui::action::Sender = Arc::new(move |action| {
                let mut action = action;
                action.key = format!("widget:{target}:{}", action.key);
                Plant::App(AppMessage::Action { surface, action })
            });
            let content = crate::ui::build::build_node(&node, 13.0, Some(sender.clone()))?;
            Ok(crate::ui::widgets::input::with_hold(
                content,
                sender,
                plots.config.interaction.hold.duration(),
            ))
        }
        ("widget", None) => Err("iced.native(\"widget\") needs an output-bound app".into()),
        (other, _) => Err(format!(
            "iced.native({other:?}) is not a framework host slot"
        )),
    }
}

pub(crate) fn view(plots: &Plots, surface: window::Id) -> Element<'static, Plant> {
    let Some(instance) = plots.apps.instances.values().find(|i| i.owns(surface)) else {
        return iced::widget::Space::new().into();
    };
    if let Some(popup) = &instance.popup
        && popup.surface == surface
    {
        return popup::view(plots, instance, popup);
    }
    let Some(layer) = instance.root_by_surface(surface) else {
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
    // Host slots a framework layer can embed. `desktop-widgets` composites the
    // legacy desktop host, which has no description equivalent: widget
    // instances, service snapshots, hold-drag and placement all stay native.
    let output = match plots.windows.ids.get(&surface) {
        Some(PlotInfo::App(output)) => Some(*output),
        _ => None,
    };
    let mut native = app_native(plots, surface, output);
    match layer.lists.build(
        &layer.root.content,
        &plots.animation.motion,
        sender.clone(),
        Some(hover),
        &mut native,
    ) {
        Ok(element) => {
            let element = if layer.root.surface.pointer_events {
                crate::ui::widgets::input::frame_input(element, sender)
            } else {
                element
            };
            let revision = layer.revision;
            measured::root(
                element,
                layer.frame.clone(),
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
        for root in instance.roots.values_mut() {
            root.lists.sweep(&mut plots.animation.motion);
            completed.extend(root.lists.take_completed());
        }
        if let Some(popup) = &mut instance.popup {
            popup.lists.sweep(&mut plots.animation.motion);
            completed.extend(popup.lists.take_completed());
        }
    }
    super::animation::complete(plots, completed)
}
pub(crate) fn forget(plots: &mut Plots, surface: window::Id) {
    if !plots
        .apps
        .instances
        .values()
        .any(|instance| instance.owns(surface))
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
        AppMessage::DismissPopup { surface, reason } => dismiss_popup(plots, surface, reason, true),
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
            if let Some(popup) = plots
                .apps
                .instances
                .values_mut()
                .filter_map(|i| i.popup.as_mut())
                .find(|p| p.surface == surface && p.revision == revision)
            {
                return if popup.lists.layout(
                    &data,
                    &mut plots.animation.motion,
                    Duration::from_millis(250),
                ) {
                    Task::done(Plant::App(AppMessage::Redraw(surface)))
                } else {
                    Task::none()
                };
            }
            let Some(layer) = plots
                .apps
                .instances
                .values_mut()
                .find(|i| i.root_by_surface(surface).is_some())
                .and_then(|i| {
                    i.roots
                        .values_mut()
                        .find(|root| root.surface == Some(surface) && root.revision == revision)
                })
            else {
                return Task::none();
            };
            if layer.lists.layout(
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
            let Some(instance) = plots.apps.instances.values_mut().find(|i| i.owns(surface)) else {
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
            let result =
                state
                    .instances
                    .values_mut()
                    .find(|i| i.owns(surface))
                    .and_then(|instance| {
                        instance.runtime.as_mut().map(|runtime| {
                            let mut context = instance.context.clone();
                            if let Some(layer) = instance.roots.iter().find_map(|(id, root)| {
                                (root.surface == Some(surface)).then_some(id)
                            }) {
                                context["layer"] = json!(layer);
                            } else if let Some(popup) = &instance.popup {
                                context["layer"] = json!(popup.owner);
                            }
                            let fallback =
                                plots
                                    .input
                                    .cursors
                                    .get(&surface)
                                    .map(|point| iced::Rectangle {
                                        x: point.x,
                                        y: point.y,
                                        width: 1.0,
                                        height: 1.0,
                                    });
                            runtime.set_action_bounds(fallback);
                            runtime.app_event(
                                &services(plots),
                                &instance.output_name,
                                &context,
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
