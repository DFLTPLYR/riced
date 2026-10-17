//! Desktop app hosting and two-click placement. Capture surfaces own input only
//! for the lifetime of a session; the Settings toplevel remains intact beneath.
use super::{Plant, Plots, windows::PlotInfo};
use crate::lua::widgets::{EntryContext, ResolvedWidget, WidgetState};
use crate::{
    config::{ConfigPatch, DesktopWidget},
    ui::action::Action,
};
use iced::{
    Element, Length, Point, Task,
    widget::{Space, button, column, container, mouse_area, row, stack, text},
    window,
};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings,
    OutputOption,
};
use iced_wayland_subscriber::OutputId;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

#[derive(Debug, Default)]
pub(crate) struct State {
    widgets: WidgetState,
    entries: HashMap<String, Cached>,
    library: Option<SystemTime>,
    pressed: Option<String>,
}

#[derive(Debug)]
struct Cached {
    path: PathBuf,
    input: serde_json::Value,
    rendered: crate::lua::composable::Rendered,
    last_run: Instant,
    interval: Duration,
}

impl State {
    fn forget(&mut self, id: &str) {
        if self.pressed.as_deref() == Some(id) {
            self.pressed = None;
        }
        self.entries.remove(id);
        self.widgets.instances.remove(id);
        self.widgets.hosts.remove(id);
        self.widgets.hovered.remove(id);
        self.widgets.errors.remove(id);
    }

    fn render(
        &mut self,
        resolved: &ResolvedWidget,
        host: serde_json::Value,
        context: &EntryContext<'_>,
        library: Option<SystemTime>,
        now: Instant,
    ) -> Option<crate::lua::composable::Rendered> {
        if self.library != library {
            self.widgets.reset();
            self.library = library;
        }
        let ids: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.path == resolved.path)
            .map(|(id, _)| id.clone())
            .chain(std::iter::once(resolved.id.clone()))
            .collect();
        self.widgets
            .sync_revision(&resolved.path, ids.iter().map(String::as_str));
        let input = serde_json::json!({"host":host,"theme":crate::theme::lua_palette(context.services.theme),"library":format!("{library:?}"),"hovered":self.widgets.hovered.get(&resolved.id).copied().unwrap_or(false)});
        if self.widgets.instances.contains_key(&resolved.id)
            && let Some(entry) = self.entries.get(&resolved.id)
            && entry.input == input
            && now.saturating_duration_since(entry.last_run) < entry.interval
        {
            return Some(entry.rendered.clone());
        }
        let result = (|| -> Result<_, String> {
            let value = self.widgets.render_with_host(resolved, context, host)?;
            let node = crate::ui::decode::parse_node(&value)?;
            let props = self.widgets.published_props(&resolved.id)?;
            let rendered = crate::lua::composable::Rendered { node, props };
            valid_size(&rendered)?;
            let (defaults, _) = crate::lua::metadata::load_widget_meta(
                &self.widgets.instances[&resolved.id],
                &resolved.name,
            );
            let seconds = defaults.interval;
            let seconds = if seconds.is_finite() {
                seconds.clamp(0.25, 86400.0)
            } else {
                1.0
            };
            Ok((rendered, Duration::from_secs_f32(seconds)))
        })();
        match result {
            Ok((rendered, interval)) => {
                self.widgets.errors.remove(&resolved.id);
                self.entries.insert(
                    resolved.id.clone(),
                    Cached {
                        path: resolved.path.clone(),
                        input,
                        rendered: rendered.clone(),
                        last_run: now,
                        interval,
                    },
                );
                Some(rendered)
            }
            Err(error) => {
                if self.widgets.errors.get(&resolved.id) != Some(&error) {
                    eprintln!(
                        "widget {}: keeping last working view: {error}",
                        resolved.path.display()
                    );
                    self.widgets.errors.insert(resolved.id.clone(), error);
                }
                self.entries
                    .get(&resolved.id)
                    .map(|entry| entry.rendered.clone())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum DesktopEvent {
    Tick,
    Changed,
    Pick {
        settings: window::Id,
        widget: String,
    },
    Move {
        surface: window::Id,
        output: OutputId,
        position: Point,
    },
    Drop {
        surface: window::Id,
        output: OutputId,
    },
    Cancel {
        surface: window::Id,
    },
    Hover {
        instance: String,
        entered: bool,
    },
    Click {
        instance: String,
        output: OutputId,
        pressed: bool,
    },
    Action {
        instance: String,
        output: OutputId,
        action: Action,
    },
}

#[derive(Debug)]
pub(crate) struct DragSession {
    pub source: window::Id,
    pub widget: String,
    pub cursor: Option<(OutputId, Point)>,
}

/// Capture points are already output-local logical coordinates: compositor
/// output offsets and physical pixel density must not be added a second time.
fn placement(
    drag: &DragSession,
    output: OutputId,
    name: &str,
    size: iced::Size,
) -> Option<DesktopWidget> {
    let (owner, point) = drag.cursor?;
    if owner != output
        || name.is_empty()
        || !point.x.is_finite()
        || !point.y.is_finite()
        || point.x < 0.0
        || point.y < 0.0
        || point.x >= size.width
        || point.y >= size.height
    {
        return None;
    }
    Some(DesktopWidget {
        id: crate::config::fresh_placement_id(),
        widget: drag.widget.clone(),
        output: name.into(),
        x: point.x,
        y: point.y,
        ..Default::default()
    })
}

fn source(name: &str) -> Option<std::path::PathBuf> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
        return None;
    }
    let path = crate::config::discovery::widget_file_path(name, &format!("{name}.desktop.lua"));
    path.is_file().then_some(path)
}

fn instance(output: OutputId, widget: &DesktopWidget) -> String {
    format!("desktop/{}/{}", output.0, widget.id)
}

fn matches_output(plots: &Plots, output: OutputId, widget: &DesktopWidget) -> bool {
    widget.output.is_empty()
        || plots
            .windows
            .output_infos
            .get(&output)
            .and_then(|info| info.name.as_deref())
            == Some(widget.output.as_str())
}

fn valid_size(rendered: &crate::lua::composable::Rendered) -> Result<(), String> {
    for key in ["width", "height"] {
        if rendered.props.get(key).is_none() {
            continue;
        }
        let size = rendered.props.get(key).and_then(serde_json::Value::as_f64);
        if !size.is_some_and(|size| size.is_finite() && size > 0.0 && size <= 32768.0) {
            return Err(format!(
                "desktop app {key} must be a finite positive number (at most 32768)"
            ));
        }
    }
    Ok(())
}

fn render(
    plots: &Plots,
    output: OutputId,
    widget: &DesktopWidget,
    id: &str,
) -> Option<crate::lua::composable::Rendered> {
    let path = source(&widget.widget)?;
    let info = plots.windows.output_infos.get(&output)?;
    let (x, y, width, height) = super::screens::Background::output_geometry(info);
    let mut host = serde_json::to_value(&widget.props).ok()?;
    host["x"] = widget.x.into();
    host["y"] = widget.y.into();
    host["output"] =
        serde_json::json!({"name":info.name,"x":x,"y":y,"width":width,"height":height});
    if widget.width > 0.0 {
        host["width"] = widget.width.into();
    }
    if widget.height > 0.0 {
        host["height"] = widget.height.into();
    }
    let resolved = ResolvedWidget {
        id: id.into(),
        name: widget.widget.clone(),
        path,
        interval: 1.0,
        size: 13.0,
        props: widget.props.clone(),
    };
    let context = EntryContext {
        services: crate::services::ServiceCtx {
            sys: &plots.services.system,
            gpu: None,
            theme: &plots.config.theme,
            outputs: &plots.windows.output_infos,
            notifications: &plots.notification.queue,
            toplevels: &plots.services.toplevels,
            workspaces: &plots.services.workspaces,
        },
        output: info.name.as_deref().unwrap_or(""),
    };
    plots.desktop.widgets.borrow_mut().render(
        &resolved,
        host,
        &context,
        plots.catalog.library_revision,
        Instant::now(),
    )
}

fn app_view(
    plots: &Plots,
    output: OutputId,
    widget: &DesktopWidget,
    id: String,
    interactive: bool,
) -> Option<Element<'static, Plant>> {
    let rendered = render(plots, output, widget, &id)?;
    let width = rendered.props["width"]
        .as_f64()
        .map(|value| Length::Fixed(value as f32))
        .unwrap_or(Length::Shrink);
    let height = rendered.props["height"]
        .as_f64()
        .map(|value| Length::Fixed(value as f32))
        .unwrap_or(Length::Shrink);
    let sender = interactive.then(|| {
        let instance = id.clone();
        Arc::new(move |action| {
            Plant::Desktop(DesktopEvent::Action {
                instance: instance.clone(),
                output,
                action,
            })
        }) as crate::ui::action::Sender
    });
    let size = rendered
        .props
        .get("size")
        .and_then(serde_json::Value::as_f64)
        .filter(|size| size.is_finite() && *size > 0.0)
        .unwrap_or(13.0) as f32;
    let content = crate::ui::build::build_node(&rendered.node, size, sender).ok()?;
    let content: Element<'static, Plant> = container(content)
        .width(width)
        .height(height)
        .clip(true)
        .into();
    let content = if interactive {
        mouse_area(content)
            .on_press(Plant::Desktop(DesktopEvent::Click {
                instance: id.clone(),
                output,
                pressed: true,
            }))
            .on_release(Plant::Desktop(DesktopEvent::Click {
                instance: id.clone(),
                output,
                pressed: false,
            }))
            .on_enter(Plant::Desktop(DesktopEvent::Hover {
                instance: id.clone(),
                entered: true,
            }))
            .on_exit(Plant::Desktop(DesktopEvent::Hover {
                instance: id,
                entered: false,
            }))
            .into()
    } else {
        content
    };
    // A panel applies output-local positioning without anchoring/clamping the
    // saved origin when the app's dimensions change on a successful reload.
    Some(
        crate::ui::widgets::panel::panel()
            .content(content)
            .padding(iced::Padding {
                left: widget.x,
                top: widget.y,
                right: 0.0,
                bottom: 0.0,
            })
            .into(),
    )
}

pub(crate) fn views(
    plots: &Plots,
    output: OutputId,
    interactive: bool,
) -> Vec<Element<'static, Plant>> {
    let mut widgets: Vec<_> = plots
        .config
        .desktop
        .iter()
        .filter(|widget| matches_output(plots, output, widget))
        .collect();
    widgets.sort_by_key(|widget| widget.z);
    widgets
        .into_iter()
        .filter_map(|widget| app_view(plots, output, widget, instance(output, widget), interactive))
        .collect()
}

pub(crate) fn retain(plots: &Plots) {
    let live: HashSet<_> = plots
        .windows
        .output_infos
        .keys()
        .flat_map(|output| {
            plots
                .config
                .desktop
                .iter()
                .filter(|widget| matches_output(plots, *output, widget))
                .map(|widget| instance(*output, widget))
        })
        .collect();
    let mut state = plots.desktop.widgets.borrow_mut();
    let stale: Vec<_> = state
        .entries
        .keys()
        .filter(|id| id.starts_with("desktop/") && !live.contains(*id))
        .cloned()
        .collect();
    for id in stale {
        state.forget(&id);
    }
}

pub(crate) fn settings(id: window::Id, plots: &Plots) -> Element<'static, Plant> {
    let mut content = column![text("Available desktop widgets").size(18)].spacing(12);
    for (name, _) in crate::config::discovery::discover_desktop_widget_files() {
        content = content.push(
            button(text(format!("Place {name}"))).on_press(Plant::Desktop(DesktopEvent::Pick {
                settings: id,
                widget: name,
            })),
        );
    }
    content = content.push(text("Placed widgets").size(18));
    for (index, widget) in plots.config.desktop.iter().enumerate() {
        let patch = |patch| Plant::Config(super::ConfigEvent::Patch(patch));
        content = content.push(
            row![
                text(format!(
                    "{} · {} · ({:.0}, {:.0})",
                    widget.widget,
                    if widget.output.is_empty() {
                        "all outputs"
                    } else {
                        &widget.output
                    },
                    widget.x,
                    widget.y
                ))
                .width(Length::Fill),
                button("Remove").on_press(patch(ConfigPatch::RemoveDesktopWidget { index })),
            ]
            .spacing(8),
        );
        let x = widget.x;
        let y = widget.y;
        content = content.push(
            row![
                text("X"),
                crate::ui::widgets::spin_box::spin_box(
                    x as f64,
                    -32768.0..=32768.0,
                    1.0,
                    0,
                    move |x| Plant::Config(super::ConfigEvent::Patch(
                        ConfigPatch::MoveDesktopWidget {
                            index,
                            x: x as f32,
                            y
                        }
                    ))
                ),
                text("Y"),
                crate::ui::widgets::spin_box::spin_box(
                    y as f64,
                    -32768.0..=32768.0,
                    1.0,
                    0,
                    move |y| Plant::Config(super::ConfigEvent::Patch(
                        ConfigPatch::MoveDesktopWidget {
                            index,
                            x,
                            y: y as f32
                        }
                    ))
                ),
            ]
            .spacing(8),
        );
        let width = widget.width;
        let height = widget.height;
        content = content.push(
            row![
                text("Width (0 = Lua)"),
                crate::ui::widgets::spin_box::spin_box(
                    width as f64,
                    0.0..=32768.0,
                    1.0,
                    0,
                    move |width| Plant::Config(super::ConfigEvent::Patch(
                        ConfigPatch::SetDesktopWidgetSize {
                            index,
                            width: width as f32,
                            height
                        }
                    ))
                ),
                text("Height (0 = Lua)"),
                crate::ui::widgets::spin_box::spin_box(
                    height as f64,
                    0.0..=32768.0,
                    1.0,
                    0,
                    move |height| Plant::Config(super::ConfigEvent::Patch(
                        ConfigPatch::SetDesktopWidgetSize {
                            index,
                            width,
                            height: height as f32
                        }
                    ))
                ),
            ]
            .spacing(8),
        );
    }
    content.into()
}

pub(crate) fn finish(plots: &mut Plots) -> Task<Plant> {
    if plots.desktop.drag.is_none() && plots.windows.drags.is_empty() {
        return Task::none();
    }
    plots.desktop.drag = None;
    plots.desktop.widgets.borrow_mut().forget("drag-preview");
    plots.desktop.widgets.borrow_mut().widgets.hovered.clear();
    plots.desktop.widgets.borrow_mut().pressed = None;
    plots
        .desktop
        .composable_runtime
        .borrow_mut()
        .hovered
        .clear();
    plots.placements.hovered.clear();
    let windows = std::mem::take(&mut plots.windows.drags);
    let tasks: Vec<_> = windows
        .into_values()
        .map(|id| {
            plots.windows.ids.remove(&id);
            plots.input.cursors.remove(&id);
            iced::window::close(id)
        })
        .collect();
    Task::batch(tasks)
}

pub(crate) fn update(plots: &mut Plots, event: DesktopEvent) -> Task<Plant> {
    match event {
        DesktopEvent::Changed => Task::none(),
        DesktopEvent::Tick => {
            retain(plots);
            let mut changed = false;
            for output in plots.windows.output_infos.keys().copied() {
                for widget in plots
                    .config
                    .desktop
                    .iter()
                    .filter(|widget| matches_output(plots, output, widget))
                {
                    let id = instance(output, widget);
                    let before = plots
                        .desktop
                        .widgets
                        .borrow()
                        .entries
                        .get(&id)
                        .map(|entry| entry.rendered.clone());
                    let after = render(plots, output, widget, &id);
                    changed |= match (before, after) {
                        (Some(before), Some(after)) => {
                            before.node != after.node || before.props != after.props
                        }
                        (None, Some(_)) => true,
                        _ => false,
                    };
                }
            }
            if changed {
                Task::done(Plant::Desktop(DesktopEvent::Changed))
            } else {
                Task::none()
            }
        }
        DesktopEvent::Cancel { surface } => {
            if plots.windows.drags.values().any(|id| *id == surface) {
                finish(plots)
            } else {
                Task::none()
            }
        }
        DesktopEvent::Pick { settings, widget } => {
            if !plots.windows.settings.contains_key(&settings)
                || plots.windows.output_infos.is_empty()
            {
                return Task::none();
            }
            let Some(output) = plots.windows.output_infos.keys().next().copied() else {
                return Task::none();
            };
            let preview = DesktopWidget {
                widget: widget.clone(),
                ..Default::default()
            };
            // A broken app never opens a full-screen capture session.
            plots.desktop.widgets.borrow_mut().forget("drag-preview");
            if render(plots, output, &preview, "drag-preview").is_none() {
                return Task::none();
            }
            let cleanup = finish(plots);
            plots.desktop.drag = Some(DragSession {
                source: settings,
                widget,
                cursor: None,
            });
            let mut tasks = vec![cleanup];
            for output in plots
                .windows
                .output_infos
                .keys()
                .copied()
                .collect::<Vec<_>>()
            {
                let id = window::Id::unique();
                plots.windows.drags.insert(output, id);
                plots.windows.ids.insert(id, PlotInfo::Drag(output));
                tasks.push(Task::done(Plant::NewLayerShell {
                    id,
                    settings: NewLayerShellSettings {
                        anchor: Anchor::all(),
                        layer: Layer::Overlay,
                        exclusive_zone: Some(-1),
                        size: LayerSize::FILL,
                        output_option: OutputOption::GlobalName(output.0),
                        namespace: Some("Riced - Desktop Placement".into()),
                        keyboard_interactivity: KeyboardInteractivity::Exclusive,
                        events_transparent: false,
                        blur_option: BlurOption::None,
                        ..Default::default()
                    },
                }));
            }
            Task::batch(tasks)
        }
        DesktopEvent::Move {
            surface,
            output,
            position,
        } => {
            if plots.windows.drags.get(&output) == Some(&surface)
                && position.x.is_finite()
                && position.y.is_finite()
                && let Some(drag) = &mut plots.desktop.drag
            {
                drag.cursor = Some((output, position));
            }
            Task::none()
        }
        DesktopEvent::Drop { surface, output } => {
            if plots.windows.drags.get(&output) != Some(&surface) {
                return Task::none();
            }
            let Some(drag) = plots.desktop.drag.as_ref() else {
                return Task::none();
            };
            if !plots.windows.settings.contains_key(&drag.source) {
                return finish(plots);
            }
            let Some((owner, _)) = drag.cursor else {
                return Task::none();
            };
            if owner != output {
                return Task::none();
            }
            let Some(info) = plots.windows.output_infos.get(&output) else {
                return finish(plots);
            };
            let Some(name) = info.name.clone().filter(|name| !name.is_empty()) else {
                return finish(plots);
            };
            let (_, _, width, height) = super::screens::Background::output_geometry(info);
            let Some(widget) = placement(drag, output, &name, iced::Size::new(width, height))
            else {
                return Task::none();
            };
            // New placements have no last-good runtime: validate the saved
            // source rather than silently accepting a retained preview.
            plots.desktop.widgets.borrow_mut().forget("drag-preview");
            if render(plots, output, &widget, "drag-preview").is_none() {
                return Task::none();
            }
            let cleanup = finish(plots);
            Task::batch([
                cleanup,
                Task::done(Plant::Config(super::ConfigEvent::Patch(
                    ConfigPatch::AddDesktopWidget(widget),
                ))),
            ])
        }
        DesktopEvent::Hover { instance, entered } => {
            if !plots.windows.output_infos.keys().any(|output| {
                plots.config.desktop.iter().any(|widget| {
                    matches_output(plots, *output, widget)
                        && self::instance(*output, widget) == instance
                })
            }) {
                return Task::none();
            }
            let mut state = plots.desktop.widgets.borrow_mut();
            if entered {
                state.widgets.hovered.insert(instance, true);
            } else {
                state.widgets.hovered.remove(&instance);
            }
            Task::none()
        }
        DesktopEvent::Click {
            instance,
            output,
            pressed,
        } => {
            if !plots.config.desktop.iter().any(|widget| {
                matches_output(plots, output, widget) && self::instance(output, widget) == instance
            }) {
                return Task::none();
            }
            let mut state = plots.desktop.widgets.borrow_mut();
            if pressed {
                state.pressed = Some(instance);
                return Task::none();
            }
            if state.pressed.take().as_deref() != Some(instance.as_str()) {
                return Task::none();
            }
            drop(state);
            widget_action(plots, &instance, output, None)
        }
        DesktopEvent::Action {
            instance,
            output,
            action,
        } => widget_action(plots, &instance, output, Some(&action)),
    }
}

fn widget_action(
    plots: &mut Plots,
    instance: &str,
    output: OutputId,
    action: Option<&Action>,
) -> Task<Plant> {
    if !plots.config.desktop.iter().any(|widget| {
        matches_output(plots, output, widget) && self::instance(output, widget) == instance
    }) {
        return Task::none();
    }
    let services = crate::services::ServiceCtx {
        sys: &plots.services.system,
        gpu: None,
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
    let widget = plots
        .config
        .desktop
        .iter()
        .find(|widget| self::instance(output, widget) == instance)
        .expect("validated placement");
    let Some(path) = source(&widget.widget) else {
        return Task::none();
    };
    let resolved = ResolvedWidget {
        id: instance.into(),
        name: widget.widget.clone(),
        path,
        props: widget.props.clone(),
        interval: 1.0,
        size: 13.0,
    };
    let context = EntryContext {
        services,
        output: name,
    };
    let result = {
        let mut state = plots.desktop.widgets.borrow_mut();
        if let Some(entry) = state.entries.get_mut(instance) {
            entry.input = serde_json::Value::Null;
        }
        if let Some(action) = action {
            state.widgets.ui_action(&resolved, &context, action)
        } else if crate::lua::widgets::lua_has_func(&state.widgets.instances, instance, "on_press")
        {
            state
                .widgets
                .press(&resolved, &context)
                .map(|result| result.map(|_| mlua::Value::Nil))
        } else {
            None
        }
    };
    match result {
        Some(Ok(value)) => super::screens::notification::command_from_action(&value, plots),
        Some(Err(error)) => {
            eprintln!("desktop action: {error}");
            Task::none()
        }
        None => Task::none(),
    }
}

/// Capture surfaces paint a snapshot of the desktop, visually revealing it
/// without unmapping the Settings toplevel (the backend cannot minimize it).
pub(crate) fn view(
    plots: &Plots,
    surface: window::Id,
    output: OutputId,
) -> Element<'static, Plant> {
    let mut layers: Vec<Element<'static, Plant>> = vec![
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| iced::widget::container::Style::default().background(iced::Color::BLACK))
            .into(),
    ];
    if let Some(tile) = plots.desktop.wallpaper.applied.get(&output) {
        layers.push(
            iced::widget::image(tile.handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Fill)
                .into(),
        );
    }
    layers.extend(views(plots, output, false));
    if let Some(drag) = &plots.desktop.drag
        && let Some((owner, point)) = drag.cursor
        && owner == output
    {
        let widget = DesktopWidget {
            widget: drag.widget.clone(),
            x: point.x,
            y: point.y,
            ..Default::default()
        };
        if let Some(ghost) = app_view(plots, output, &widget, "drag-preview".into(), false) {
            layers.push(ghost);
        }
        layers.push(
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| iced::widget::container::Style {
                    border: iced::Border {
                        color: crate::theme::active().primary,
                        width: 2.0,
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into(),
        );
    }
    let output_name = plots
        .windows
        .output_infos
        .get(&output)
        .and_then(|info| info.name.as_deref())
        .unwrap_or("display");
    layers.push(
        container(text(format!(
            "{output_name} · Click to place · Right-click or Escape to cancel"
        )))
        .padding(12)
        .style(crate::theme::menu_box)
        .into(),
    );
    mouse_area(stack(layers).width(Length::Fill).height(Length::Fill))
        .on_move(move |position| {
            Plant::Desktop(DesktopEvent::Move {
                surface,
                output,
                position,
            })
        })
        .on_press(Plant::Desktop(DesktopEvent::Drop { surface, output }))
        .on_right_press(Plant::Desktop(DesktopEvent::Cancel { surface }))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_context(run: impl FnOnce(&EntryContext<'_>)) {
        let system = sysinfo::System::new();
        let theme = crate::config::ThemeConfig::default();
        let outputs = Default::default();
        let notifications = Default::default();
        let toplevels = Default::default();
        let workspaces = Default::default();
        run(&EntryContext {
            services: crate::services::ServiceCtx {
                sys: &system,
                gpu: None,
                theme: &theme,
                outputs: &outputs,
                notifications: &notifications,
                toplevels: &toplevels,
                workspaces: &workspaces,
            },
            output: "DP-1",
        });
    }

    #[test]
    fn dock_and_desktop_use_the_same_widget_environment_and_props_contract() {
        with_context(|context| {
            let dir =
                std::env::temp_dir().join(format!("riced-shared-widget-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("clock.desktop.lua");
            std::fs::write(
                &path,
                r#"return {
                defaults={width=240,height=160,props={format='%H:%M:%S'}},
                view=function(self, props)
                    assert(props == self.props)
                    assert(type(os.date)=='function' and type(io.popen)=='function')
                    assert(type(system)=='table' and bar.output=='DP-1')
                    assert(props.width == 240)
                    return iced.text(os.date(props.format))
                end,
            }"#,
            )
            .unwrap();
            let resolved = |id: &str| ResolvedWidget {
                id: id.into(),
                name: "clock".into(),
                path: path.clone(),
                interval: 1.0,
                size: 13.0,
                props: HashMap::new(),
            };
            let mut dock = WidgetState::default();
            let value = dock.render(&resolved("dock"), context).unwrap();
            assert!(
                matches!(crate::ui::decode::parse_node(&value).unwrap(), crate::ui::node::WidgetNode::Text { content, .. } if content.len()==8)
            );
            let mut desktop = State::default();
            let rendered = desktop
                .render(
                    &resolved("desktop"),
                    serde_json::json!({"x":30,"y":50}),
                    context,
                    None,
                    Instant::now(),
                )
                .unwrap();
            assert!(
                matches!(rendered.node, crate::ui::node::WidgetNode::Text { content, .. } if content.len()==8)
            );
            assert_eq!(rendered.props["width"], 240);
            assert_eq!(rendered.props["x"], 30);
            std::fs::remove_dir_all(dir).unwrap();
        });
    }

    #[test]
    fn desktop_intervals_refresh_without_hover_and_preserve_independent_widget_state() {
        with_context(|context| {
            let dir =
                std::env::temp_dir().join(format!("riced-desktop-interval-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("clock.desktop.lua");
            std::fs::write(
                &path,
                r#"return {count=0, defaults={interval=0.5}, view=function(self, props)
                self.count=self.count+1
                return iced.text(self.count..':'..os.date('%H:%M:%S'))
            end}"#,
            )
            .unwrap();
            let resolved = |id: &str| ResolvedWidget {
                id: id.into(),
                name: "clock".into(),
                path: path.clone(),
                interval: 1.0,
                size: 13.0,
                props: HashMap::new(),
            };
            let mut state = State::default();
            let now = Instant::now();
            let read = |state: &mut State, id: &str, now| {
                let rendered = state
                    .render(&resolved(id), serde_json::json!({}), context, None, now)
                    .unwrap();
                let crate::ui::node::WidgetNode::Text { content, .. } = rendered.node else {
                    panic!("clock text")
                };
                content
            };
            assert!(read(&mut state, "a", now).starts_with("1:"));
            assert!(read(&mut state, "a", now + Duration::from_millis(499)).starts_with("1:"));
            assert!(read(&mut state, "a", now + Duration::from_millis(500)).starts_with("2:"));
            state.widgets.hovered.insert("a".into(), true);
            assert!(read(&mut state, "a", now + Duration::from_millis(501)).starts_with("3:"));
            assert!(read(&mut state, "b", now + Duration::from_millis(501)).starts_with("1:"));
            std::fs::write(&path, "this is broken Lua").unwrap();
            assert!(read(&mut state, "a", now + Duration::from_secs(2)).starts_with("3:"));
            std::fs::remove_dir_all(dir).unwrap();
        });
    }

    #[test]
    fn drop_uses_logical_local_coordinates_and_rejects_stale_outputs() {
        let mut drag = DragSession {
            source: window::Id::unique(),
            widget: "rectangle".into(),
            cursor: Some((OutputId(2), Point::new(40.0, 60.0))),
        };
        let size = iced::Size::new(1080.0, 1920.0);
        let widget = placement(&drag, OutputId(2), "DP-2", size).unwrap();
        assert_eq!((widget.x, widget.y), (40.0, 60.0));
        assert_eq!(widget.output, "DP-2");
        assert!(!widget.id.is_empty());
        assert!(placement(&drag, OutputId(1), "DP-1", size).is_none());
        assert!(placement(&drag, OutputId(2), "", size).is_none());
        for point in [
            Point::new(-1.0, 1.0),
            Point::new(1080.0, 0.0),
            Point::new(f32::NAN, 2.0),
        ] {
            drag.cursor = Some((OutputId(2), point));
            assert!(placement(&drag, OutputId(2), "DP-2", size).is_none());
        }
    }

    #[test]
    fn rectangle_seed_has_lua_owned_geometry_hover_and_host_overrides() {
        let mut runtime = crate::lua::LuaRuntime::new().unwrap();
        runtime
            .load(include_str!("../../scripts/widgets/rectangle.desktop.lua"))
            .unwrap();
        let theme = crate::config::ThemeConfig::default();
        let (node, props) = runtime
            .component_view(
                &serde_json::json!({}),
                &serde_json::json!({"x":25,"y":50}),
                &theme,
                false,
            )
            .unwrap();
        let rendered = crate::lua::composable::Rendered { node, props };
        valid_size(&rendered).unwrap();
        assert_eq!(rendered.props["width"], 240);
        assert_eq!(rendered.props["height"], 160);
        let (hovered, props) = runtime
            .component_view(
                &serde_json::json!({}),
                &serde_json::json!({"width":320,"height":200}),
                &theme,
                true,
            )
            .unwrap();
        assert_ne!(hovered, rendered.node);
        assert_eq!(props["width"], 320);
        assert_eq!(props["height"], 200);
        assert!(
            valid_size(&crate::lua::composable::Rendered {
                node: hovered,
                props: serde_json::json!({"width":0,"height":160})
            })
            .is_err()
        );
    }
}
