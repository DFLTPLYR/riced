//! Desktop app hosting, hold-drag-release, and Settings placement. Capture surfaces own input only
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
    next_revision: u64,
    previews: HashMap<String, PreviewKey>,
    settings_outputs: HashMap<window::Id, OutputId>,
}

#[derive(Debug)]
struct Cached {
    path: PathBuf,
    input: serde_json::Value,
    rendered: crate::lua::composable::Rendered,
    last_run: Instant,
    interval: Duration,
    revision: u64,
    source: Option<SystemTime>,
    library: Option<SystemTime>,
}

impl State {
    fn forget(&mut self, id: &str) {
        self.previews.remove(id);
        self.entries.remove(id);
        self.widgets.instances.remove(id);
        self.widgets.hosts.remove(id);
        self.widgets.hovered.remove(id);
        self.widgets.errors.remove(id);
    }

    fn forget_settings(&mut self, settings: window::Id) {
        let ids: Vec<_> = self
            .previews
            .iter()
            .filter(|(_, key)| key.settings == settings)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.forget(&id);
        }
        self.settings_outputs.remove(&settings);
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
                let source = self.widgets.revisions.get(&resolved.path).copied();
                let revision = match self.entries.get(&resolved.id) {
                    Some(entry)
                        if entry.input == input
                            && entry.rendered.node == rendered.node
                            && entry.rendered.props == rendered.props
                            && entry.source == source
                            && entry.library == library =>
                    {
                        entry.revision
                    }
                    _ => {
                        self.next_revision = self.next_revision.wrapping_add(1);
                        self.next_revision
                    }
                };
                self.entries.insert(
                    resolved.id.clone(),
                    Cached {
                        path: resolved.path.clone(),
                        input,
                        rendered: rendered.clone(),
                        last_run: now,
                        interval,
                        revision,
                        source,
                        library,
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
    PreviewChanged(window::Id),
    SourceMotion {
        source: window::Id,
        position: Point,
    },
    Pick {
        settings: window::Id,
        widget: String,
    },
    Reposition {
        settings: window::Id,
        placement: String,
    },
    PreviewOutput {
        settings: window::Id,
        output: OutputId,
    },
    PreviewPick {
        key: PreviewKey,
        revision: u64,
        grab: Point,
    },
    Move {
        surface: window::Id,
        output: OutputId,
        position: Point,
    },
    Drop {
        surface: window::Id,
        output: OutputId,
        position: Point,
        session: Option<window::Id>,
    },
    Release {
        source: window::Id,
        button: iced::mouse::Button,
        session: window::Id,
        position: Option<Point>,
    },
    Cancel {
        surface: window::Id,
    },
    Hover {
        instance: String,
        entered: bool,
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
    pub placement: Option<String>,
    pub cursor: Option<(OutputId, Point)>,
    pub pickup: Pickup,
}

#[derive(Debug, Default)]
pub(crate) struct Pickup {
    owner: Option<HeldGesture>,
    grab: Point,
    cursor: Option<(OutputId, Point)>,
    preview: Option<crate::lua::composable::Rendered>,
    revision: Option<SystemTime>,
    library: Option<SystemTime>,
    original: Option<DesktopWidget>,
    native_limit: iced::Size,
}

#[derive(Debug, Clone)]
struct HeldGesture {
    id: window::Id,
    instance: String,
    output: OutputId,
    button: iced::mouse::Button,
}

impl DragSession {
    fn accepts_release(
        &self,
        source: window::Id,
        button: iced::mouse::Button,
        session: window::Id,
    ) -> bool {
        self.source == source
            && self
                .pickup
                .owner
                .as_ref()
                .is_some_and(|owner| owner.id == session && owner.button == button)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewTarget {
    Available(String),
    Placed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewKey {
    settings: window::Id,
    target: PreviewTarget,
    output: OutputId,
}

impl PreviewKey {
    fn id(&self) -> String {
        format!(
            "settings/{:?}/{}/{:?}",
            self.settings, self.output.0, self.target
        )
    }
}

fn preview_widget(plots: &Plots, target: &PreviewTarget) -> Option<DesktopWidget> {
    match target {
        PreviewTarget::Available(name) => source(name).map(|_| DesktopWidget {
            widget: name.clone(),
            ..Default::default()
        }),
        PreviewTarget::Placed(id) => plots
            .config
            .desktop
            .iter()
            .find(|widget| widget.id == *id)
            .cloned(),
    }
}

fn preview_output(plots: &Plots, settings: window::Id, widget: &DesktopWidget) -> Option<OutputId> {
    preview_output_selected(plots, &plots.desktop.widgets.borrow(), settings, widget)
}

fn preview_output_selected(
    plots: &Plots,
    state: &State,
    settings: window::Id,
    widget: &DesktopWidget,
) -> Option<OutputId> {
    let selected = state.settings_outputs.get(&settings).copied();
    let mut outputs: Vec<_> = plots.windows.output_infos.keys().copied().collect();
    outputs.sort();
    outputs
        .iter()
        .copied()
        .find(|output| !widget.output.is_empty() && matches_output(plots, *output, widget))
        .or_else(|| selected.filter(|output| outputs.contains(output)))
        .or_else(|| outputs.first().copied())
}

fn source_revision(widget: &str) -> Option<SystemTime> {
    source(widget).and_then(|path| std::fs::metadata(path).ok()?.modified().ok())
}

fn origin(pointer: Point, grab: Point) -> Point {
    Point::new(pointer.x - grab.x, pointer.y - grab.y)
}

fn drag_widget(config: &crate::config::Config, drag: &DragSession) -> Option<DesktopWidget> {
    match &drag.placement {
        Some(id) => config
            .desktop
            .iter()
            .find(|widget| widget.id == *id)
            .cloned(),
        None => Some(DesktopWidget {
            widget: drag.widget.clone(),
            ..Default::default()
        }),
    }
}

/// Capture points are already output-local logical coordinates: compositor
/// output offsets and physical pixel density must not be added a second time.
fn placement(
    config: &crate::config::Config,
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
    let mut widget = drag_widget(config, drag)?;
    if drag.placement.is_none() {
        widget.id = crate::config::fresh_placement_id();
    }
    widget.output = name.into();
    let origin = origin(point, drag.pickup.grab);
    if !origin.x.is_finite()
        || !origin.y.is_finite()
        || origin.x.abs() > 32768.0
        || origin.y.abs() > 32768.0
    {
        return None;
    }
    widget.x = origin.x;
    widget.y = origin.y;
    Some(widget)
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
            gpu: plots.services.gpu(),
            sampled: plots.services.sampled(),
            theme: &plots.config.theme,
            outputs: &plots.windows.output_infos,
            notifications: &plots.notification.queue,
            toplevels: &plots.services.toplevels,
            workspaces: &plots.services.workspaces,
            tray: &plots.services.tray,
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

fn widget_content(
    rendered: &crate::lua::composable::Rendered,
    sender: Option<crate::ui::action::Sender>,
) -> Option<Element<'static, Plant>> {
    let width = rendered.props["width"]
        .as_f64()
        .map(|value| Length::Fixed(value as f32))
        .unwrap_or(Length::Shrink);
    let height = rendered.props["height"]
        .as_f64()
        .map(|value| Length::Fixed(value as f32))
        .unwrap_or(Length::Shrink);
    let size = rendered
        .props
        .get("size")
        .and_then(serde_json::Value::as_f64)
        .filter(|size| size.is_finite() && *size > 0.0)
        .unwrap_or(13.0) as f32;
    let content = crate::ui::build::build_node(&rendered.node, size, sender).ok()?;
    Some(
        container(content)
            .width(width)
            .height(height)
            .clip(true)
            .into(),
    )
}

fn inert_content(rendered: &crate::lua::composable::Rendered) -> Option<Element<'static, Plant>> {
    widget_content(
        rendered,
        Some(Arc::new(|_| Plant::Desktop(DesktopEvent::Changed))),
    )
}

fn positioned(
    content: Element<'static, Plant>,
    position: Point,
    interactive: bool,
) -> Element<'static, Plant> {
    crate::ui::widgets::panel::panel()
        .content(crate::ui::motion::shifted(
            content,
            position.x,
            position.y,
            interactive,
        ))
        .into()
}

fn app_view(
    plots: &Plots,
    output: OutputId,
    widget: &DesktopWidget,
    id: String,
    interactive: bool,
) -> Option<Element<'static, Plant>> {
    let rendered = render(plots, output, widget, &id)?;
    let target = id.clone();
    let sender: crate::ui::action::Sender = Arc::new(move |action| {
        Plant::Desktop(DesktopEvent::Action {
            instance: target.clone(),
            output,
            action,
        })
    });
    let content = widget_content(&rendered, Some(sender.clone()))?;
    let content = if interactive {
        let content = crate::ui::widgets::input::with_hold(
            content,
            sender,
            plots.config.interaction.hold.duration(),
        );
        mouse_area(content)
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
    let picked_up = interactive
        && plots.desktop.drag.as_ref().is_some_and(|drag| {
            drag.pickup.owner.is_some() && drag.placement.as_deref() == Some(widget.id.as_str())
        });
    Some(positioned(
        crate::ui::motion::visibility(content, !picked_up),
        Point::new(widget.x, widget.y),
        interactive,
    ))
}

fn settings_preview(
    settings: window::Id,
    plots: &Plots,
    target: PreviewTarget,
) -> Element<'static, Plant> {
    let Some(widget) = preview_widget(plots, &target) else {
        return text("Widget source unavailable").into();
    };
    let Some(output) = preview_output(plots, settings, &widget) else {
        return text("Connect a display to preview this widget").into();
    };
    let key = PreviewKey {
        settings,
        target,
        output,
    };
    let id = key.id();
    let Some(rendered) = render(plots, output, &widget, &id) else {
        return text("Widget preview could not be loaded").into();
    };
    let Some(content) = inert_content(&rendered) else {
        return text("Widget preview could not be built").into();
    };
    let mut state = plots.desktop.widgets.borrow_mut();
    let revision = state.entries[&id].revision;
    state.previews.insert(id, key.clone());
    drop(state);
    let (_, _, width, height) =
        super::screens::Background::output_geometry(&plots.windows.output_infos[&output]);
    crate::ui::widgets::widget_preview::preview(
        content,
        iced::Size::new(width, height),
        Arc::new(move |grab| {
            Plant::Desktop(DesktopEvent::PreviewPick {
                key: key.clone(),
                revision,
                grab,
            })
        }),
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
        .filter(|widget| {
            interactive
                || plots
                    .desktop
                    .drag
                    .as_ref()
                    .and_then(|drag| drag.placement.as_deref())
                    != Some(widget.id.as_str())
        })
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
        .filter(|id| {
            (id.starts_with("desktop/") && !live.contains(*id))
                || state.previews.get(*id).is_some_and(|key| {
                    !plots.windows.settings.contains_key(&key.settings)
                        || !plots.windows.output_infos.contains_key(&key.output)
                        || preview_widget(plots, &key.target).is_none()
                        || preview_widget(plots, &key.target).and_then(|widget| {
                            preview_output_selected(plots, &state, key.settings, &widget)
                        }) != Some(key.output)
                })
        })
        .cloned()
        .collect();
    for id in stale {
        state.forget(&id);
    }
    state
        .settings_outputs
        .retain(|id, _| plots.windows.settings.contains_key(id));
}

pub(crate) fn forget_settings(plots: &Plots, settings: window::Id) {
    plots.desktop.widgets.borrow_mut().forget_settings(settings);
}

pub(crate) fn settings_have_images(plots: &Plots, settings: window::Id) -> bool {
    let state = plots.desktop.widgets.borrow();
    state.previews.iter().any(|(id, key)| {
        key.settings == settings
            && state
                .entries
                .get(id)
                .is_some_and(|entry| crate::ui::node::contains_images(&entry.rendered.node))
    })
}

fn across_outputs(
    source: Point,
    point: Point,
    outputs: impl IntoIterator<Item = (OutputId, iced::Rectangle)>,
) -> Option<(OutputId, Point)> {
    let global = Point::new(source.x + point.x, source.y + point.y);
    outputs.into_iter().find_map(|(output, rect)| {
        rect.contains(global)
            .then(|| (output, Point::new(global.x - rect.x, global.y - rect.y)))
    })
}

/// The initiating Wayland surface retains its pointer grab until release.
pub(crate) fn held_source(plots: &Plots) -> Option<window::Id> {
    plots
        .desktop
        .drag
        .as_ref()
        .filter(|drag| drag.pickup.owner.is_some())
        .map(|drag| drag.source)
}

pub(crate) fn source_motion(plots: &mut Plots, source: window::Id, point: Point) -> bool {
    if plots
        .desktop
        .drag
        .as_ref()
        .is_none_or(|drag| drag.source != source)
    {
        return false;
    }
    let Some(PlotInfo::Background(output)) = plots.windows.ids.get(&source).copied() else {
        return false;
    };
    let Some(info) = plots.windows.output_infos.get(&output) else {
        return false;
    };
    let (x, y, _, _) = super::screens::Background::output_geometry(info);
    let mut outputs: Vec<_> = plots
        .windows
        .output_infos
        .iter()
        .map(|(id, info)| {
            let (x, y, width, height) = super::screens::Background::output_geometry(info);
            (
                *id,
                iced::Rectangle::new(Point::new(x, y), iced::Size::new(width, height)),
            )
        })
        .collect();
    outputs.sort_by_key(|(output, _)| *output);
    let cursor = across_outputs(Point::new(x, y), point, outputs);
    if let Some(drag) = plots.desktop.drag.as_mut() {
        drag.cursor = cursor;
    }
    true
}

fn refresh_previews(plots: &Plots) -> bool {
    let previews: Vec<_> = plots
        .desktop
        .widgets
        .borrow()
        .previews
        .values()
        .cloned()
        .collect();
    let mut changed = false;
    for key in previews {
        let Some(widget) = preview_widget(plots, &key.target) else {
            continue;
        };
        let id = key.id();
        let before = plots
            .desktop
            .widgets
            .borrow()
            .entries
            .get(&id)
            .map(|entry| entry.revision);
        let _ = render(plots, key.output, &widget, &id);
        changed |= before
            != plots
                .desktop
                .widgets
                .borrow()
                .entries
                .get(&id)
                .map(|entry| entry.revision);
    }
    changed
}

pub(crate) fn settings(id: window::Id, plots: &Plots) -> Element<'static, Plant> {
    use crate::config::{HoldDuration, HoldSpeed};
    let current = plots.config.interaction.hold;
    let mut speeds = row![text("Hold duration")].spacing(8);
    for (label, duration) in [
        ("Fast (250 ms)", HoldDuration::Preset(HoldSpeed::Fast)),
        ("Medium (500 ms)", HoldDuration::Preset(HoldSpeed::Medium)),
        ("Slow (1000 ms)", HoldDuration::Preset(HoldSpeed::Slow)),
        ("Custom", HoldDuration::Custom(current.milliseconds())),
    ] {
        speeds = speeds.push(
            button(label)
                .on_press(Plant::Config(super::ConfigEvent::Patch(
                    ConfigPatch::HoldDuration(duration),
                )))
                .style(crate::theme::nav_button(current == duration)),
        );
    }
    let mut content = column![
        speeds,
        text("Applies to widget holds, including desktop drag pickup.")
    ]
    .spacing(12);
    if let HoldDuration::Custom(ms) = current {
        content = content.push(
            row![
                text("Milliseconds"),
                crate::ui::widgets::spin_box::spin_box(
                    ms as f64,
                    1.0..=60000.0,
                    25.0,
                    0,
                    |value| Plant::Config(super::ConfigEvent::Patch(ConfigPatch::HoldDuration(
                        HoldDuration::Custom(value as u32)
                    )))
                )
            ]
            .spacing(8),
        );
    }
    content = content.push(text("Available desktop widgets").size(18));
    let mut outputs: Vec<_> = plots.windows.output_infos.iter().collect();
    outputs.sort_by_key(|(output, _)| **output);
    let mut choices = row![text("Preview display")].spacing(8);
    let selected = preview_output(plots, id, &DesktopWidget::default());
    for (output, info) in outputs {
        choices = choices.push(
            button(text(
                info.name
                    .clone()
                    .unwrap_or_else(|| format!("Display {}", output.0)),
            ))
            .on_press(Plant::Desktop(DesktopEvent::PreviewOutput {
                settings: id,
                output: *output,
            }))
            .style(crate::theme::nav_button(selected == Some(*output))),
        );
    }
    content = content.push(choices).push(text("Click inside a widget preview to pick it up at that point. Release, then click the desktop to place."));
    for (name, _) in crate::config::discovery::discover_desktop_widget_files() {
        content = content.push(
            button(text(format!("Place {name}"))).on_press(Plant::Desktop(DesktopEvent::Pick {
                settings: id,
                widget: name.clone(),
            })),
        );
        content = content.push(settings_preview(id, plots, PreviewTarget::Available(name)));
    }
    content = content.push(text("Placed widgets").size(18));
    for (index, widget) in plots.config.desktop.iter().enumerate() {
        let patch = |patch| Plant::Config(super::ConfigEvent::Patch(patch));
        content = content.push(settings_preview(
            id,
            plots,
            PreviewTarget::Placed(widget.id.clone()),
        ));
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
                button("Reposition").on_press(Plant::Desktop(DesktopEvent::Reposition {
                    settings: id,
                    placement: widget.id.clone()
                })),
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
        DesktopEvent::Changed | DesktopEvent::PreviewChanged(_) => Task::none(),
        DesktopEvent::SourceMotion { source, position } => {
            let previous = plots
                .desktop
                .drag
                .as_ref()
                .and_then(|drag| drag.cursor.map(|(output, _)| output));
            if !source_motion(plots, source, position) {
                return Task::none();
            }
            let output = plots
                .desktop
                .drag
                .as_ref()
                .and_then(|drag| drag.cursor.map(|(output, _)| output));
            let event = output
                .filter(|output| Some(*output) == previous)
                .and_then(|output| plots.windows.drags.get(&output).copied())
                .map(DesktopEvent::PreviewChanged)
                .unwrap_or(DesktopEvent::Changed);
            Task::done(Plant::Desktop(event))
        }
        DesktopEvent::Tick => {
            retain(plots);
            plots.services.refresh_system();
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
            changed |= refresh_previews(plots);
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
            if !plots.windows.settings.contains_key(&settings) {
                return Task::none();
            }
            let preview = DesktopWidget {
                widget,
                ..Default::default()
            };
            start(plots, settings, preview, None, Pickup::default())
        }
        DesktopEvent::Reposition {
            settings,
            placement,
        } => {
            if !plots.windows.settings.contains_key(&settings) {
                return Task::none();
            }
            let Some(preview) = plots
                .config
                .desktop
                .iter()
                .find(|widget| widget.id == placement)
                .cloned()
            else {
                return Task::none();
            };
            start(plots, settings, preview, Some(placement), Pickup::default())
        }
        DesktopEvent::PreviewOutput { settings, output } => {
            if plots.windows.settings.contains_key(&settings)
                && plots.windows.output_infos.contains_key(&output)
            {
                plots
                    .desktop
                    .widgets
                    .borrow_mut()
                    .settings_outputs
                    .insert(settings, output);
                retain(plots);
            }
            Task::none()
        }
        DesktopEvent::PreviewPick {
            key,
            revision,
            grab,
        } => {
            if !plots.windows.settings.contains_key(&key.settings)
                || !plots.windows.output_infos.contains_key(&key.output)
                || !grab.x.is_finite()
                || !grab.y.is_finite()
                || grab.x < 0.0
                || grab.y < 0.0
            {
                return Task::none();
            }
            let id = key.id();
            if plots.desktop.widgets.borrow().previews.get(&id) != Some(&key) {
                return Task::none();
            }
            let Some(widget) = preview_widget(plots, &key.target) else {
                return Task::none();
            };
            if preview_output(plots, key.settings, &widget) != Some(key.output) {
                return Task::none();
            }
            let Some(rendered) = render(plots, key.output, &widget, &id) else {
                return Task::none();
            };
            let valid = plots
                .desktop
                .widgets
                .borrow()
                .entries
                .get(&id)
                .is_some_and(|entry| {
                    entry.revision == revision
                        && entry.source == source_revision(&widget.widget)
                        && entry.library == plots.catalog.library_revision
                });
            if !valid {
                return Task::none();
            }
            let placement = match key.target {
                PreviewTarget::Available(_) => None,
                PreviewTarget::Placed(id) => Some(id),
            };
            start(
                plots,
                key.settings,
                widget,
                placement,
                Pickup {
                    grab,
                    preview: Some(rendered),
                    ..Default::default()
                },
            )
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
                let changed_output = drag.cursor.is_some_and(|(owner, _)| owner != output);
                drag.cursor = Some((output, position));
                if changed_output {
                    // Clear the previous output's ghost and placement highlight too.
                    return Task::done(Plant::Desktop(DesktopEvent::Changed));
                }
            }
            Task::none()
        }
        DesktopEvent::Drop {
            surface,
            output,
            position,
            session,
        } => {
            if plots.windows.drags.get(&output) != Some(&surface) {
                return Task::none();
            }
            let Some(drag) = plots.desktop.drag.as_mut() else {
                return Task::none();
            };
            if drag.pickup.owner.as_ref().map(|owner| owner.id) != session {
                return Task::none();
            }
            // Commit the actual click point, not the last motion notification.
            drag.cursor = Some((output, position));
            let Some(drag) = plots.desktop.drag.as_ref() else {
                return Task::none();
            };
            if drag.pickup.preview.is_some()
                && (drag.pickup.revision != source_revision(&drag.widget)
                    || drag.pickup.library != plots.catalog.library_revision
                    || drag.pickup.original.as_ref().is_some_and(|original| {
                        !plots.config.desktop.iter().any(|widget| widget == original)
                    }))
            {
                return finish(plots);
            }
            if !plots.windows.ids.contains_key(&drag.source) {
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
            if drag
                .placement
                .as_ref()
                .is_some_and(|id| !plots.config.desktop.iter().any(|widget| widget.id == *id))
            {
                return finish(plots);
            }
            let Some(widget) = placement(
                &plots.config,
                drag,
                output,
                &name,
                iced::Size::new(width, height),
            ) else {
                return Task::none();
            };
            // New placements have no last-good runtime: validate the saved
            // source rather than silently accepting a retained preview.
            plots.desktop.widgets.borrow_mut().forget("drag-preview");
            if render(plots, output, &widget, "drag-preview").is_none() {
                return Task::none();
            }
            let patch = match &drag.placement {
                Some(id) => ConfigPatch::RepositionDesktopWidget {
                    id: id.clone(),
                    output: widget.output,
                    x: widget.x,
                    y: widget.y,
                },
                None => ConfigPatch::AddDesktopWidget(widget),
            };
            let cleanup = finish(plots);
            Task::batch([
                cleanup,
                Task::done(Plant::Config(super::ConfigEvent::Patch(patch))),
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
        DesktopEvent::Action {
            instance,
            output,
            action,
        } => widget_action(plots, &instance, output, &action, None),
        DesktopEvent::Release {
            source,
            button,
            session,
            position,
        } => release(plots, source, button, session, position),
    }
}

fn start(
    plots: &mut Plots,
    source: window::Id,
    preview: DesktopWidget,
    placement: Option<String>,
    mut pickup: Pickup,
) -> Task<Plant> {
    let Some(output) = pickup
        .cursor
        .map(|(output, _)| output)
        .filter(|output| plots.windows.output_infos.contains_key(output))
        .or_else(|| preview_output(plots, source, &preview))
    else {
        return Task::none();
    };
    // Validate before opening capture surfaces; failed reloads retain the session.
    plots.desktop.widgets.borrow_mut().forget("drag-preview");
    if pickup.preview.is_none() {
        pickup.preview = render(plots, output, &preview, "drag-preview");
    }
    if pickup.preview.is_none() || !pickup.grab.x.is_finite() || !pickup.grab.y.is_finite() {
        return Task::none();
    }
    pickup.revision = source_revision(&preview.widget);
    pickup.library = plots.catalog.library_revision;
    pickup.original = placement.as_ref().map(|_| preview.clone());
    let (_, _, width, height) =
        super::screens::Background::output_geometry(&plots.windows.output_infos[&output]);
    pickup.native_limit = iced::Size::new(width, height);
    let cursor = pickup.cursor.or(Some((
        output,
        Point::new(preview.x + pickup.grab.x, preview.y + pickup.grab.y),
    )));
    let cleanup = finish(plots);
    plots.desktop.drag = Some(DragSession {
        source,
        widget: preview.widget,
        placement,
        cursor,
        pickup,
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
            settings: preview_surface_settings(
                output,
                plots
                    .desktop
                    .drag
                    .as_ref()
                    .is_some_and(|drag| drag.pickup.owner.is_some()),
            ),
        }));
    }
    Task::batch(tasks)
}

fn preview_surface_settings(output: OutputId, held: bool) -> NewLayerShellSettings {
    NewLayerShellSettings {
        anchor: Anchor::all(),
        layer: Layer::Overlay,
        exclusive_zone: Some(-1),
        size: LayerSize::FILL,
        output_option: OutputOption::GlobalName(output.0),
        namespace: Some("Riced - Desktop Placement".into()),
        // A held gesture must keep its initiating surface's implicit pointer grab.
        keyboard_interactivity: if held {
            KeyboardInteractivity::None
        } else {
            KeyboardInteractivity::Exclusive
        },
        events_transparent: held,
        blur_option: BlurOption::None,
        ..Default::default()
    }
}

fn widget_action(
    plots: &mut Plots,
    instance: &str,
    output: OutputId,
    action: &Action,
    session: Option<window::Id>,
) -> Task<Plant> {
    let dropping =
        matches!(&action.event, crate::ui::action::Event::Mouse { state, .. } if state == "drop");
    if plots.desktop.drag.as_ref().is_some_and(|drag| {
        !dropping
            || drag.pickup.owner.as_ref().is_none_or(|owner| {
                owner.instance != instance || owner.output != output || Some(owner.id) != session
            })
    }) {
        return Task::none();
    }
    let mut action = action.clone();
    if let crate::ui::action::Event::Mouse { state, button, .. } = &mut action.event
        && state == "hold"
        && button.as_deref() == Some("left")
    {
        *state = "drag".into();
    }
    if !plots.config.desktop.iter().any(|widget| {
        matches_output(plots, output, widget) && self::instance(output, widget) == instance
    }) {
        return Task::none();
    }
    let services = crate::services::ServiceCtx {
        sys: &plots.services.system,
        gpu: plots.services.gpu(),
        sampled: plots.services.sampled(),
        theme: &plots.config.theme,
        outputs: &plots.windows.output_infos,
        notifications: &plots.notification.queue,
        toplevels: &plots.services.toplevels,
        workspaces: &plots.services.workspaces,
        tray: &plots.services.tray,
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
        state.widgets.ui_action(&resolved, &context, &action)
    };
    match result {
        Some(Ok(value)) => {
            let command = super::actions::command_from_action(&value, plots);
            if dropping
                && matches!(&value, mlua::Value::Table(table) if table.get::<bool>("drop").unwrap_or(false))
            {
                let target = plots.desktop.drag.as_ref().and_then(|drag| drag.cursor);
                if let Some((output, position)) = target
                    && let Some(surface) = plots.windows.drags.get(&output).copied()
                {
                    return Task::batch([
                        command,
                        update(
                            plots,
                            DesktopEvent::Drop {
                                surface,
                                output,
                                position,
                                session,
                            },
                        ),
                    ]);
                }
                return Task::batch([command, finish(plots)]);
            }
            let reposition = matches!(&value, mlua::Value::Table(table) if table.get::<bool>("drag").unwrap_or(false))
                && matches!(&action.event, crate::ui::action::Event::Mouse {state,..} if state == "drag");
            let reposition = if reposition {
                let preview = plots
                    .config
                    .desktop
                    .iter()
                    .find(|widget| self::instance(output, widget) == instance)
                    .cloned();
                match (preview, plots.windows.background_ids.get(&output).copied()) {
                    (Some(preview), Some(source)) => {
                        let placement = preview.id.clone();
                        let (grab, cursor) = match &action.event {
                            crate::ui::action::Event::Mouse {
                                origin, position, ..
                            } => (
                                origin.or(*position).unwrap_or(Point::ORIGIN),
                                position.map(|point| {
                                    (output, Point::new(preview.x + point.x, preview.y + point.y))
                                }),
                            ),
                            _ => (Point::ORIGIN, None),
                        };
                        let snapshot = render(plots, output, &preview, instance);
                        start(
                            plots,
                            source,
                            preview,
                            Some(placement),
                            Pickup {
                                owner: Some(HeldGesture {
                                    id: window::Id::unique(),
                                    instance: instance.into(),
                                    output,
                                    button: iced::mouse::Button::Left,
                                }),
                                grab,
                                cursor,
                                preview: snapshot,
                                ..Default::default()
                            },
                        )
                    }
                    _ => Task::none(),
                }
            } else {
                Task::none()
            };
            Task::batch([command, reposition])
        }
        Some(Err(error)) => {
            eprintln!("desktop action: {error}");
            Task::none()
        }
        None => Task::none(),
    }
}

/// Snapshot the matching native release before another gesture can begin.
pub(crate) fn request_release(
    plots: &Plots,
    source: window::Id,
    button: iced::mouse::Button,
) -> Option<Task<Plant>> {
    let drag = plots.desktop.drag.as_ref()?;
    let owner = drag.pickup.owner.as_ref()?;
    if !drag.accepts_release(source, button, owner.id) {
        return None;
    }
    let position = super::screens::background::LAST_CURSOR_GLOBAL
        .lock()
        .ok()
        .and_then(|cursors| cursors.get(&source).copied());
    Some(Task::done(Plant::Desktop(DesktopEvent::Release {
        source,
        button,
        session: owner.id,
        position,
    })))
}

/// Only the source surface's matching release can finish a held drag.
fn release(
    plots: &mut Plots,
    source: window::Id,
    button: iced::mouse::Button,
    session: window::Id,
    position: Option<Point>,
) -> Task<Plant> {
    let Some(drag) = plots
        .desktop
        .drag
        .as_ref()
        .filter(|drag| drag.accepts_release(source, button, session))
    else {
        return Task::none();
    };
    let Some(owner) = drag.pickup.owner.clone() else {
        return Task::none();
    };
    let grab = drag.pickup.grab;
    if let Some(point) = position {
        source_motion(plots, source, point);
    }
    let action = Action {
        key: String::new(),
        value: None,
        event: crate::ui::action::Event::Mouse {
            state: "drop".into(),
            button: Some("left".into()),
            position: Some(grab),
            origin: Some(grab),
            bounds: None,
        },
    };
    let result = widget_action(plots, &owner.instance, owner.output, &action, Some(session));
    // Declining the drop (or an errored callback) cancels instead of leaving a stuck grab.
    Task::batch([result, finish(plots)])
}

/// Held-drag overlays paint only the moving widget and placement cues. Settings
/// placement paints a desktop snapshot because its toplevel remains mapped.
pub(crate) fn view(
    plots: &Plots,
    surface: window::Id,
    output: OutputId,
) -> Element<'static, Plant> {
    let held = plots
        .desktop
        .drag
        .as_ref()
        .is_some_and(|drag| drag.pickup.owner.is_some());
    let mut layers: Vec<Element<'static, Plant>> = vec![
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| {
                iced::widget::container::Style::default().background(if held {
                    iced::Color::TRANSPARENT
                } else {
                    iced::Color::BLACK
                })
            })
            .into(),
    ];
    if !held {
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
    }
    if let Some(drag) = &plots.desktop.drag
        && let Some((owner, point)) = drag.cursor
        && owner == output
    {
        if let Some(snapshot) = &drag.pickup.preview
            && let Some(ghost) = inert_content(snapshot)
        {
            layers.push(
                crate::ui::widgets::panel::panel()
                    .content(crate::ui::motion::snapshot(
                        ghost,
                        origin(point, drag.pickup.grab),
                        drag.pickup.native_limit,
                    ))
                    .into(),
            );
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
            "{output_name} · {} · Right-click to cancel",
            if plots
                .desktop
                .drag
                .as_ref()
                .is_some_and(|drag| drag.pickup.owner.is_some())
            {
                "Release to drop"
            } else {
                "Click to place"
            }
        )))
        .padding(12)
        .style(crate::theme::menu_box)
        .into(),
    );
    let content = stack(layers)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    if held {
        return content;
    }
    crate::ui::widgets::input::input(
        content,
        Arc::new(move |action| {
            let event = match action.event {
                crate::ui::action::Event::Mouse {
                    state,
                    position: Some(position),
                    button,
                    ..
                } => match (state.as_str(), button.as_deref()) {
                    ("move", _) => DesktopEvent::Move {
                        surface,
                        output,
                        position,
                    },
                    ("press", Some("left")) => DesktopEvent::Drop {
                        surface,
                        output,
                        position,
                        session: None,
                    },
                    ("press", Some("right")) => DesktopEvent::Cancel { surface },
                    _ => DesktopEvent::Changed,
                },
                _ => DesktopEvent::Changed,
            };
            Plant::Desktop(event)
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_drag_survives_focus_exit_foreign_buttons_and_stale_releases() {
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = Plots {
            windows: Default::default(),
            shell_events: receiver,
            input: Default::default(),
            desktop: Default::default(),
            config: crate::config::Config::default(),
            config_mtime: None,
            catalog: super::super::catalog::WidgetCatalog::new(Vec::new()),
            placements: Default::default(),
            animation: Default::default(),
            services: crate::services::state::ServiceState::new(),
            config_jobs: Default::default(),
            theme_mtime: None,
            theme_jobs: Default::default(),
            notification: Default::default(),
        };
        let source = window::Id::unique();
        let capture = window::Id::unique();
        let foreign = window::Id::unique();
        let output = OutputId(1);
        let session = window::Id::unique();
        let old_session = window::Id::unique();
        plots.windows.drags.insert(output, capture);
        plots.windows.ids.insert(capture, PlotInfo::Drag(output));
        plots.desktop.drag = Some(DragSession {
            source,
            widget: "removed-widget".into(),
            placement: Some("removed-placement".into()),
            cursor: Some((output, Point::new(100.0, 200.0))),
            pickup: Pickup {
                owner: Some(HeldGesture {
                    id: session,
                    instance: "missing-instance".into(),
                    output,
                    button: iced::mouse::Button::Left,
                }),
                ..Default::default()
            },
        });
        assert!(request_release(&plots, foreign, iced::mouse::Button::Left).is_none());
        assert!(request_release(&plots, capture, iced::mouse::Button::Left).is_none());
        assert!(request_release(&plots, source, iced::mouse::Button::Middle).is_none());
        for event in [
            iced::Event::Window(iced::window::Event::Unfocused),
            iced::Event::Mouse(iced::mouse::Event::CursorLeft),
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                iced::mouse::Button::Middle,
            )),
        ] {
            let _ = plots.update(Plant::Graft(source, event));
            assert!(plots.desktop.drag.is_some());
        }
        for (surface, button, token) in [
            (capture, iced::mouse::Button::Left, session),
            (source, iced::mouse::Button::Middle, session),
            (source, iced::mouse::Button::Left, old_session),
        ] {
            let _ = update(
                &mut plots,
                DesktopEvent::Release {
                    source: surface,
                    button,
                    session: token,
                    position: None,
                },
            );
            assert!(plots.desktop.drag.is_some());
        }
        for token in [None, Some(old_session)] {
            let _ = update(
                &mut plots,
                DesktopEvent::Drop {
                    surface: capture,
                    output,
                    position: Point::ORIGIN,
                    session: token,
                },
            );
            assert!(plots.desktop.drag.is_some());
        }
        assert!(request_release(&plots, source, iced::mouse::Button::Left).is_some());
        assert!(plots.desktop.drag.is_some());
        let _ = update(
            &mut plots,
            DesktopEvent::Release {
                source,
                button: iced::mouse::Button::Left,
                session,
                position: None,
            },
        );
        assert!(plots.desktop.drag.is_none());
        assert!(plots.windows.drags.is_empty());
        assert!(request_release(&plots, source, iced::mouse::Button::Left).is_none());
        plots.desktop.drag = Some(DragSession {
            source,
            widget: "new-widget".into(),
            placement: None,
            cursor: None,
            pickup: Default::default(),
        });
        let _ = update(
            &mut plots,
            DesktopEvent::Release {
                source,
                button: iced::mouse::Button::Left,
                session,
                position: None,
            },
        );
        assert!(request_release(&plots, source, iced::mouse::Button::Left).is_none());
        assert!(plots.desktop.drag.is_some());
        assert!(plots.config.desktop.is_empty());
    }

    #[test]
    fn held_drag_previews_preserve_input_and_settings_capture_remains_interactive() {
        let passive = preview_surface_settings(OutputId(1), true);
        assert!(passive.events_transparent);
        assert_eq!(passive.keyboard_interactivity, KeyboardInteractivity::None);
        let capture = preview_surface_settings(OutputId(1), false);
        assert!(!capture.events_transparent);
        assert_eq!(
            capture.keyboard_interactivity,
            KeyboardInteractivity::Exclusive
        );
    }

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
                sampled: 0.0,
                theme: &theme,
                outputs: &outputs,
                notifications: &notifications,
                toplevels: &toplevels,
                workspaces: &workspaces,
                tray: &Default::default(),
            },
            output: "DP-1",
        });
    }

    #[test]
    fn grab_offsets_survive_cross_output_placement_and_negative_origins() {
        let original = DesktopWidget {
            id: "grabbed".into(),
            widget: "clock".into(),
            output: "DP-1".into(),
            x: 100.0,
            y: 200.0,
            ..Default::default()
        };
        let config = crate::config::Config {
            desktop: vec![original.clone()],
            ..Default::default()
        };
        let mut drag = DragSession {
            source: window::Id::unique(),
            widget: "clock".into(),
            placement: Some(original.id.clone()),
            cursor: Some((OutputId(2), Point::new(300.0, 400.0))),
            pickup: Pickup {
                grab: Point::new(70.0, 25.0),
                ..Default::default()
            },
        };
        let moved = placement(
            &config,
            &drag,
            OutputId(2),
            "DP-2",
            iced::Size::new(1080.0, 1920.0),
        )
        .unwrap();
        assert_eq!((moved.x, moved.y), (230.0, 375.0));
        assert_eq!(
            origin(Point::new(300.0, 400.0), drag.pickup.grab),
            Point::new(moved.x, moved.y)
        );
        drag.cursor = Some((OutputId(2), Point::new(10.0, 5.0)));
        let moved = placement(
            &config,
            &drag,
            OutputId(2),
            "DP-2",
            iced::Size::new(1080.0, 1920.0),
        )
        .unwrap();
        assert_eq!((moved.x, moved.y), (-60.0, -20.0));
        assert_eq!(config.desktop, [original]);
    }

    #[test]
    fn initial_pointer_grab_motion_uses_logical_output_origins_once() {
        let outputs = [
            (
                OutputId(1),
                iced::Rectangle::new(Point::new(0.0, 840.0), iced::Size::new(1920.0, 1080.0)),
            ),
            (
                OutputId(2),
                iced::Rectangle::new(Point::new(1920.0, 0.0), iced::Size::new(1080.0, 1920.0)),
            ),
        ];
        assert_eq!(
            across_outputs(Point::new(0.0, 840.0), Point::new(1950.0, 60.0), outputs),
            Some((OutputId(2), Point::new(30.0, 900.0)))
        );
        assert!(across_outputs(Point::new(0.0, 840.0), Point::new(-50.0, 0.0), outputs).is_none());
    }

    #[test]
    fn settings_previews_keep_independent_state_and_release_their_own_vms() {
        with_context(|context| {
            let dir = std::env::temp_dir()
                .join(format!("riced-preview-isolation-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("counter.desktop.lua");
            std::fs::write(&path,"return {count=0,defaults={width=640,height=360},view=function(self,props) self.count=self.count+1; return iced.text(tostring(self.count)) end}").unwrap();
            let settings = window::Id::unique();
            let key = PreviewKey {
                settings,
                target: PreviewTarget::Available("counter".into()),
                output: OutputId(1),
            };
            let id = key.id();
            let resolved = |id: &str| ResolvedWidget {
                id: id.into(),
                name: "counter".into(),
                path: path.clone(),
                interval: 1.0,
                size: 13.0,
                props: HashMap::new(),
            };
            let mut state = State::default();
            let now = Instant::now();
            state
                .render(
                    &resolved("desktop/1/live"),
                    serde_json::json!({}),
                    context,
                    None,
                    now,
                )
                .unwrap();
            let preview = state
                .render(
                    &resolved(&id),
                    serde_json::json!({"width":320,"height":180}),
                    context,
                    None,
                    now,
                )
                .unwrap();
            state.previews.insert(id.clone(), key);
            assert_eq!(preview.props["width"], 320);
            assert_eq!(preview.props["height"], 180);
            state
                .render(
                    &resolved(&id),
                    serde_json::json!({"width":320,"height":180}),
                    context,
                    None,
                    now + Duration::from_secs(2),
                )
                .unwrap();
            let count = |state: &State, id: &str| {
                state.widgets.instances[id]
                    .named_registry_value::<mlua::Table>("riced.widget.app")
                    .unwrap()
                    .get::<u32>("count")
                    .unwrap()
            };
            assert_eq!(count(&state, &id), 2);
            assert_eq!(count(&state, "desktop/1/live"), 1);
            let accepted = state.entries[&id].revision;
            std::fs::write(&path, "return {view=function() return 'new source' end}").unwrap();
            let stamp = state.widgets.revisions[&path] + Duration::from_secs(1);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(stamp))
                .unwrap();
            state
                .render(
                    &resolved(&id),
                    serde_json::json!({"width":320,"height":180}),
                    context,
                    None,
                    now + Duration::from_secs(3),
                )
                .unwrap();
            assert_ne!(state.entries[&id].revision, accepted);
            state.forget_settings(settings);
            assert!(!state.entries.contains_key(&id));
            assert!(!state.widgets.instances.contains_key(&id));
            assert!(state.entries.contains_key("desktop/1/live"));
            std::fs::remove_dir_all(dir).unwrap();
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
    fn reposition_preserves_identity_dimensions_props_and_order_until_drop() {
        let original = DesktopWidget {
            id: "existing".into(),
            widget: "rectangle".into(),
            output: "DP-1".into(),
            x: 10.0,
            y: 20.0,
            width: 320.0,
            height: 180.0,
            z: 3,
            props: HashMap::from([(
                "color".into(),
                crate::config::PropValue::Text("#123456".into()),
            )]),
        };
        let mut config = crate::config::Config {
            desktop: vec![original.clone(), DesktopWidget::default()],
            ..Default::default()
        };
        let drag = DragSession {
            source: window::Id::unique(),
            widget: original.widget.clone(),
            placement: Some(original.id.clone()),
            cursor: Some((OutputId(2), Point::new(70.0, 90.0))),
            pickup: Pickup::default(),
        };
        let moved = placement(
            &config,
            &drag,
            OutputId(2),
            "DP-2",
            iced::Size::new(1080.0, 1920.0),
        )
        .unwrap();
        assert_eq!(
            moved,
            DesktopWidget {
                output: "DP-2".into(),
                x: 70.0,
                y: 90.0,
                ..original.clone()
            }
        );
        assert_eq!(config.desktop[0], original);
        config.desktop.swap(0, 1);
        config.apply(ConfigPatch::RepositionDesktopWidget {
            id: original.id.clone(),
            output: moved.output,
            x: moved.x,
            y: moved.y,
        });
        assert_eq!(
            config.desktop[1],
            DesktopWidget {
                output: "DP-2".into(),
                x: 70.0,
                y: 90.0,
                ..original.clone()
            }
        );
        config.desktop.remove(1);
        assert!(
            placement(
                &config,
                &drag,
                OutputId(2),
                "DP-2",
                iced::Size::new(1080.0, 1920.0)
            )
            .is_none()
        );
        config.apply(ConfigPatch::RepositionDesktopWidget {
            id: original.id,
            output: "DP-1".into(),
            x: 0.0,
            y: 0.0,
        });
        assert_eq!(config.desktop.len(), 1);
    }

    #[test]
    fn drop_uses_logical_local_coordinates_and_rejects_stale_outputs() {
        let config = crate::config::Config::default();
        let mut drag = DragSession {
            source: window::Id::unique(),
            widget: "rectangle".into(),
            placement: None,
            cursor: Some((OutputId(2), Point::new(40.0, 60.0))),
            pickup: Pickup::default(),
        };
        let size = iced::Size::new(1080.0, 1920.0);
        let widget = placement(&config, &drag, OutputId(2), "DP-2", size).unwrap();
        assert_eq!((widget.x, widget.y), (40.0, 60.0));
        assert_eq!(widget.output, "DP-2");
        assert!(!widget.id.is_empty());
        assert!(placement(&config, &drag, OutputId(1), "DP-1", size).is_none());
        assert!(placement(&config, &drag, OutputId(2), "", size).is_none());
        for point in [
            Point::new(-1.0, 1.0),
            Point::new(1080.0, 0.0),
            Point::new(f32::NAN, 2.0),
        ] {
            drag.cursor = Some((OutputId(2), point));
            assert!(placement(&config, &drag, OutputId(2), "DP-2", size).is_none());
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
