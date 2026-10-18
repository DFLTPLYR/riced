//! Reusable fullscreen overlay host. Scenes own Lua/IR; services own policy.
use super::{Plant, Plots, windows::PlotInfo};
use crate::lua::composable::source_path;
use crate::{
    config,
    lua::LuaRuntime,
    services::ServiceCtx,
    ui::{
        action::{Action, Sender},
        node::WidgetNode,
        scene::Lists,
        widgets::measured::{self, Measurement, SharedFrame},
    },
};
use iced::{Element, Length, Task, window};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings,
    OutputOption, WlRegion,
};
use iced_wayland_subscriber::OutputId;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};

pub(crate) type Input = Arc<dyn Fn(&mlua::Lua) -> mlua::Result<mlua::Table> + Send + Sync>;
pub(crate) struct Spec {
    pub id: String,
    pub source: String,
    pub input: Input,
}
#[derive(Debug, Clone)]
pub enum OverlayEvent {
    Tick,
    Changed,
    Action {
        surface: window::Id,
        output: OutputId,
        scene: String,
        action: Action,
    },
    Measured {
        surface: window::Id,
        output: OutputId,
        scene: String,
        revision: u64,
        data: Measurement,
    },
    Hover {
        surface: window::Id,
        output: OutputId,
        scene: String,
        list: String,
        key: String,
        entered: bool,
    },
}

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(crate) struct State {
    revision: u64,
    scenes: BTreeMap<(OutputId, String), Scene>,
    pub masks: HashMap<OutputId, Vec<(i32, i32, i32, i32)>>,
}
impl std::fmt::Debug for State {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("OverlayState")
            .field("scenes", &self.scenes.len())
            .finish()
    }
}
#[derive(Default)]
struct Scene {
    runtime: Option<LuaRuntime>,
    code: Option<(PathBuf, String, Option<SystemTime>)>,
    attempted: Option<(PathBuf, String, Option<SystemTime>)>,
    node: Option<WidgetNode>,
    revision: u64,
    lists: Lists,
    frame: SharedFrame,
    regions: Vec<iced::Rectangle>,
    error: Option<String>,
    /// Listview rows hovered in this scene, keyed `(list id, item key)`.
    /// Decoded into `item.hovered`; dropped with the scene.
    hovered: crate::ui::hover::HoverSet,
    rendered_hovered: crate::ui::hover::HoverSet,
}
impl Scene {
    fn commit(
        &mut self,
        node: WidgetNode,
        motion: &mut crate::ui::anim::AnimRuntime,
        replaced: bool,
    ) -> Result<bool, String> {
        let modified = replaced || self.node.as_ref() != Some(&node);
        if modified {
            let measured = self
                .frame
                .lock()
                .map_err(|_| "overlay measurement lock")?
                .last
                .clone()
                .unwrap_or_default();
            self.lists.synchronize(
                self.node.as_ref(),
                &node,
                &measured,
                motion,
                std::time::Duration::from_millis(250),
            )?;
            self.node = Some(node);
        }
        Ok(modified)
    }
    fn render(
        &mut self,
        spec: &Spec,
        services: &ServiceCtx<'_>,
        output: &str,
        motion: &mut crate::ui::anim::AnimRuntime,
        force: bool,
    ) -> bool {
        // Hover changes re-decode the view with new `item.hovered` flags,
        // without invalidating the runtime or its cached Lua state.
        let hover_changed = self.hovered != self.rendered_hovered;
        let result = (|| -> Result<bool, String> {
            let path = source_path(&spec.source);
            let source = std::fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let signature = (path, source, config::components_mtime());
            let changed = self.code.as_ref() != Some(&signature);
            if changed && !force && self.attempted.as_ref() == Some(&signature) {
                let Some(runtime) = self.runtime.as_mut() else {
                    return Ok(false);
                };
                if !runtime.has_handler("on_tick") {
                    return Ok(false);
                }
                runtime
                    .scene_tick(services, output)
                    .map_err(|error| error.to_string())?;
                let node = runtime
                    .scene_view(services, output, |lua| (spec.input)(lua), &self.hovered)
                    .map_err(|error| error.to_string())?;
                return self.commit(node, motion, false);
            }
            let ticking = !changed
                && !force
                && self
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.has_handler("on_tick"));
            if !changed && !force && !ticking && !hover_changed {
                return Ok(false);
            }
            let mut candidate = if changed {
                self.attempted = Some(signature.clone());
                let mut runtime = LuaRuntime::with_profile(
                    crate::lua::sandbox::Profile::Widget,
                    config::component_files(),
                )
                .map_err(|error| error.to_string())?;
                runtime
                    .load_named(&signature.1, &signature.0.display().to_string())
                    .map_err(|error| error.to_string())?;
                Some(runtime)
            } else {
                None
            };
            let runtime = candidate
                .as_mut()
                .or(self.runtime.as_mut())
                .ok_or("overlay runtime missing")?;
            if ticking {
                runtime
                    .scene_tick(services, output)
                    .map_err(|error| error.to_string())?;
            }
            let node = runtime
                .scene_view(services, output, |lua| (spec.input)(lua), &self.hovered)
                .map_err(|error| error.to_string())?;
            let modified = self.commit(node, motion, changed)?;
            if let Some(candidate) = candidate {
                self.runtime = Some(candidate);
            }
            self.code = Some(signature);
            self.rendered_hovered = self.hovered.clone();
            self.error = None;
            Ok(modified)
        })();
        match result {
            Ok(changed) => changed,
            Err(error) => {
                crate::lua::error::report_once(&mut self.error, "overlay: ", error);
                if force
                    && let Some(runtime) = self.runtime.as_mut()
                    && let Ok(node) =
                        runtime.scene_view(services, output, |lua| (spec.input)(lua), &self.hovered)
                {
                    return self.commit(node, motion, false).unwrap_or(false);
                }
                false
            }
        }
    }
}
impl State {
    pub(crate) fn has_callbacks(&self) -> bool {
        self.scenes
            .values()
            .any(|scene| scene.lists.has_callbacks())
    }
    pub(crate) fn has_images(&self, output: OutputId) -> bool {
        self.scenes.iter().any(|((owner, _), scene)| {
            *owner == output
                && (scene
                    .node
                    .as_ref()
                    .is_some_and(crate::ui::node::contains_images)
                    || scene.lists.has_images())
        })
    }
    pub(crate) fn forget(&mut self, output: OutputId, motion: &mut crate::ui::anim::AnimRuntime) {
        self.scenes.retain(|(owner, _), scene| {
            if *owner == output {
                scene.lists.clear(motion);
                false
            } else {
                true
            }
        });
        self.masks.remove(&output);
    }
    pub(crate) fn invalidate(&mut self) {
        for scene in self.scenes.values_mut() {
            scene.code = None;
        }
    }
}

pub(crate) fn ensure(plots: &mut Plots, output: OutputId) -> Task<Plant> {
    if plots.windows.overlays.contains_key(&output) {
        return Task::none();
    }
    let id = window::Id::unique();
    plots.windows.overlays.insert(output, id);
    plots.windows.ids.insert(id, PlotInfo::Overlay(output));
    Task::done(Plant::NewLayerShell {
        id,
        settings: NewLayerShellSettings {
            anchor: Anchor::all(),
            layer: Layer::Overlay,
            exclusive_zone: Some(-1),
            size: LayerSize::FILL,
            output_option: OutputOption::GlobalName(output.0),
            margin: Some((0, 0, 0, 0)),
            namespace: Some(format!("Riced - Overlay {output:?}")),
            keyboard_interactivity: KeyboardInteractivity::None,
            events_transparent: true,
            blur_option: BlurOption::None,
        },
    })
}
pub(crate) fn refresh(plots: &mut Plots, force: bool) -> Task<Plant> {
    use mlua::LuaSerdeExt;
    let mut commands = Vec::new();
    let mut changed = false;
    let outputs: Vec<_> = plots.windows.output_infos.keys().copied().collect();
    for output in outputs {
        let info = &plots.windows.output_infos[&output];
        let output_name = info.name.clone().unwrap_or_default();
        let mut specs = Vec::new();
        let notification = super::screens::notification::scene_spec(plots, output);
        let service_input = notification.input.clone();
        if plots.config.notifications.enabled && !plots.config.notifications.source.is_empty() {
            specs.push(notification);
        }
        for config in &plots.config.overlays {
            if !config.output.is_empty() && config.output != output_name {
                continue;
            }
            let props = config.props.clone();
            let service_input = service_input.clone();
            specs.push(Spec {
                id: format!(
                    "overlay:{}",
                    if config.id.is_empty() {
                        &config.source
                    } else {
                        &config.id
                    }
                ),
                source: config.source.clone(),
                input: Arc::new(move |lua| {
                    let mlua::Value::Table(props) = lua.to_value(&props)? else {
                        return Err(mlua::Error::RuntimeError(
                            "scene props must be a table".into(),
                        ));
                    };
                    let services = service_input(lua)?;
                    props.set(
                        "notifications",
                        services.get::<mlua::Value>("notifications")?,
                    )?;
                    Ok(props)
                }),
            });
        }
        let mut ids = HashSet::new();
        specs.retain(|spec| ids.insert(spec.id.clone()));
        plots.desktop.overlays.scenes.retain(|(owner, id), scene| {
            if *owner == output && !ids.contains(id) {
                scene.lists.clear(&mut plots.animation.motion);
                changed = true;
                false
            } else {
                true
            }
        });
        let geometry = crate::shared::geometry::output_geometry(info);
        for spec in specs {
            let input = spec.input.clone();
            let name = output_name.clone();
            let spec = Spec {
                input: Arc::new(move |lua| {
                    let props = input(lua)?;
                    let output = lua.create_table()?;
                    output.set("name", name.as_str())?;
                    output.set("width", geometry.2)?;
                    output.set("height", geometry.3)?;
                    output.set("x", geometry.0)?;
                    output.set("y", geometry.1)?;
                    props.set("output", output)?;
                    Ok(props)
                }),
                ..spec
            };
            let services = ServiceCtx {
                sys: &plots.services.system,
                gpu: plots.services.gpu(),
                sampled: plots.services.sampled(),
                theme: &plots.config.theme,
                outputs: &plots.windows.output_infos,
                notifications: &plots.notification.queue,
                toplevels: &plots.services.toplevels,
                workspaces: &plots.services.workspaces,
            };
            let state = &mut plots.desktop.overlays;
            let scene = state.scenes.entry((output, spec.id.clone())).or_default();
            scene.lists.scope = format!("overlay:{}:{}", output.0, spec.id);
            if scene.render(
                &spec,
                &services,
                &output_name,
                &mut plots.animation.motion,
                force,
            ) {
                state.revision = state.revision.wrapping_add(1);
                scene.revision = state.revision;
                changed = true;
                for id in scene.lists.take_resets() {
                    commands.push(iced::widget::operation::scroll_to(
                        id,
                        iced::widget::operation::AbsoluteOffset { x: 0.0, y: 0.0 },
                    ));
                }
            }
        }
        if !ids.is_empty() {
            commands.push(ensure(plots, output));
        }
        commands.push(push_mask(plots, output));
    }
    if changed {
        commands.push(Task::done(Plant::Overlay(OverlayEvent::Changed)));
    }
    Task::batch(commands)
}

pub(crate) fn view(plots: &Plots, output: OutputId) -> Element<'static, Plant> {
    let Some(surface) = plots.windows.overlays.get(&output).copied() else {
        return iced::widget::Space::new().into();
    };
    let mut layer = iced::widget::stack![
        iced::widget::Space::new()
            .width(Length::Fill)
            .height(Length::Fill)
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .clip(false);
    for ((owner, id), scene) in &plots.desktop.overlays.scenes {
        if *owner != output {
            continue;
        }
        let Some(node) = &scene.node else {
            continue;
        };
        let name = id.clone();
        let sender: Sender = Arc::new(move |action| {
            Plant::Overlay(OverlayEvent::Action {
                surface,
                output,
                scene: name.clone(),
                action,
            })
        });
        let hover_name = id.clone();
        let hover: crate::ui::action::HoverSender = Arc::new(move |list, key, entered| {
            Plant::Overlay(OverlayEvent::Hover {
                surface,
                output,
                scene: hover_name.clone(),
                list,
                key,
                entered,
            })
        });
        if let Ok(element) = scene
            .lists
            .build(node, &plots.animation.motion, sender, Some(hover))
        {
            let name = id.clone();
            let revision = scene.revision;
            layer = layer.push(measured::root(
                element,
                scene.frame.clone(),
                revision,
                Arc::new(move |data| {
                    Plant::Overlay(OverlayEvent::Measured {
                        surface,
                        output,
                        scene: name.clone(),
                        revision,
                        data,
                    })
                }),
            ));
        }
    }
    layer.into()
}
pub(crate) fn push_mask(plots: &mut Plots, output: OutputId) -> Task<Plant> {
    let Some(id) = plots.windows.overlays.get(&output).copied() else {
        return Task::none();
    };
    let mut rects: Vec<_> = plots
        .desktop
        .overlays
        .scenes
        .iter()
        .filter(|((owner, _), _)| *owner == output)
        .flat_map(|(_, scene)| scene.regions.iter())
        .map(|rect| {
            let x = rect.x.floor() as i32;
            let y = rect.y.floor() as i32;
            (
                x,
                y,
                (rect.x + rect.width).ceil() as i32 - x,
                (rect.y + rect.height).ceil() as i32 - y,
            )
        })
        .collect();
    rects.sort_unstable();
    rects.dedup();
    if plots.desktop.overlays.masks.get(&output) == Some(&rects) {
        return Task::none();
    }
    plots.desktop.overlays.masks.insert(output, rects.clone());
    Task::done(Plant::SetInputRegion {
        id,
        callback: iced_exwlshell::actions::ActionCallback::new(move |region: &WlRegion| {
            for (x, y, w, h) in &rects {
                region.add(*x, *y, *w, *h);
            }
        }),
    })
}
pub(crate) fn sweep(plots: &mut Plots) -> Task<Plant> {
    let mut callbacks = Vec::new();
    for scene in plots.desktop.overlays.scenes.values_mut() {
        scene.lists.sweep(&mut plots.animation.motion);
        callbacks.extend(scene.lists.take_completed());
    }
    for list in plots.animation.widgets.values_mut() {
        callbacks.extend(list.take_completed());
    }
    super::animation::complete(plots, callbacks)
}
pub(crate) fn update(plots: &mut Plots, event: OverlayEvent) -> Task<Plant> {
    match event {
        OverlayEvent::Tick => refresh(plots, false),
        OverlayEvent::Changed => Task::none(),
        OverlayEvent::Measured {
            surface,
            output,
            scene,
            revision,
            data,
        } => {
            if plots.windows.overlays.get(&output) != Some(&surface) {
                return Task::none();
            }
            let Some(scene) = plots.desktop.overlays.scenes.get_mut(&(output, scene)) else {
                return Task::none();
            };
            if revision != scene.revision {
                return Task::none();
            }
            scene.regions = data.regions.clone();
            let moved = scene.lists.layout(
                &data,
                &mut plots.animation.motion,
                std::time::Duration::from_millis(250),
            );
            let mask = push_mask(plots, output);
            if moved {
                Task::batch([mask, Task::done(Plant::Overlay(OverlayEvent::Changed))])
            } else {
                mask
            }
        }
        OverlayEvent::Hover {
            surface,
            output,
            scene,
            list,
            key,
            entered,
        } => {
            if plots.windows.overlays.get(&output) != Some(&surface) {
                return Task::none();
            }
            let Some(scene) = plots.desktop.overlays.scenes.get_mut(&(output, scene)) else {
                return Task::none();
            };
            let owned = (list, key);
            let mut changed = false;
            if entered {
                // Single pointer: entering one row clears the rest of the scene.
                for other in scene
                    .hovered
                    .iter()
                    .filter(|entry| **entry != owned)
                    .cloned()
                    .collect::<Vec<_>>()
                {
                    scene.hovered.remove(&other);
                    changed = true;
                }
                changed |= scene.hovered.insert(owned);
            } else {
                changed = scene.hovered.remove(&owned);
            }
            if changed {
                refresh(plots, false)
            } else {
                Task::none()
            }
        }
        OverlayEvent::Action {
            surface,
            output,
            scene,
            action,
        } => {
            if plots.windows.overlays.get(&output) != Some(&surface) {
                return Task::none();
            }
            let reload = refresh(plots, false);
            let services = ServiceCtx {
                sys: &plots.services.system,
                gpu: plots.services.gpu(),
                sampled: plots.services.sampled(),
                theme: &plots.config.theme,
                outputs: &plots.windows.output_infos,
                notifications: &plots.notification.queue,
                toplevels: &plots.services.toplevels,
                workspaces: &plots.services.workspaces,
            };
            let name = plots
                .windows
                .output_infos
                .get(&output)
                .and_then(|info| info.name.as_deref())
                .unwrap_or("");
            let result = plots
                .desktop
                .overlays
                .scenes
                .get_mut(&(output, scene.clone()))
                .and_then(|scene| scene.runtime.as_mut())
                .map(|runtime| runtime.scene_action(&services, name, &action));
            let command = match result {
                Some(Ok(value)) => super::screens::notification::command_from_action(&value, plots),
                Some(Err(error)) => {
                    eprintln!("overlay action {scene}: {error}");
                    Task::none()
                }
                _ => return reload,
            };
            Task::batch([reload, command, refresh(plots, true)])
        }
    }
}
