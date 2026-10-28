mod sizing;
mod surface;
use super::top::Top;
use crate::shell::state::Plots;
use crate::shell::{Plant, TopEvent, WidgetEvent};
use crate::ui::decode::parse_node_hovered;
use crate::ui::node::{NodeLength, WidgetNode};
use crate::ui::{animations, components::PopupLifecycle};
use iced::window;
use iced::{Element, Length, Task as Command};
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use iced_wayland_subscriber::OutputId;
pub(crate) use sizing::MeasurementCache;
use std::time::Duration;

/// In-flight close transition: the popup stays mounted until it finishes.
#[derive(Debug, Clone)]
pub(crate) struct Closing {
    revision: u64,
    tree: WidgetNode,
    callback: Option<crate::lua::callbacks::Callback>,
}

/// Widget-placement popup, anchored to the reporting widget's measured bounds.
/// Exit-animated popups use a host-owned layer so native grabs cannot unmap
/// them before their transition finishes. Other popups use `xdg_popup`.
///
/// Opened from `actions(events)` by calling `self:popup()` on a click;
/// the box renders whatever `popup()` returns (icons included). Clicking
/// the same placement toggles it; outside clicks dismiss it.
#[derive(Debug, Clone)]
pub struct Popup {
    /// Own window id (item clicks address their popup).
    pub win_id: window::Id,
    /// Bar that spawned this popup (toggle handling).
    pub bar_id: window::Id,
    /// Placement id whose `popup()` renders the body.
    pub placement: String,
    /// Action within the placement that opened this popup (e.g. a tray icon).
    pub opener: String,
    /// Last rendered popup root, including its user-owned presentation.
    pub tree: Option<WidgetNode>,
    /// Listview rows hovered in this popup, keyed `(list id, item key)`.
    /// Re-parsed into `item.hovered` on change; dropped with the popup.
    pub hovered: crate::ui::hover::HoverSet,
    /// Text size, from the widget def at open time.
    pub size: f32,
    /// Box size in px.
    pub w: u32,
    pub h: u32,
    pub auto_width: bool,
    pub max_width: f32,
    pub auto_height: bool,
    pub max_height: f32,
    pub sizing_revision: u64,
    pub measured_revision: Option<u64>,
    pub positioner: iced_exwlshell::actions::IcedNewPopupSettings,
    pub(crate) hosted: Option<surface::Hosted>,
    /// Declared `:on_open(...)` transition/callback, kept across body refreshes.
    pub on_open: Option<PopupLifecycle>,
    /// Declared `:on_close(...)` transition/callback.
    pub on_close: Option<PopupLifecycle>,
    /// Pending open completion `(revision, callback)`.
    pub opening: Option<(u64, crate::lua::callbacks::Callback)>,
    /// Pending close transition, if the body is sliding/fading out.
    pub closing: Option<Closing>,
    /// Monotonic phase token, so stale timers cannot complete a new phase.
    pub phase: u64,
}

/// A validated popup root and its surface sizing policy. None means no popup.
#[derive(Debug, Clone, Default)]
pub(crate) struct PopupContent {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub tree: Option<WidgetNode>,
    pub on_open: Option<PopupLifecycle>,
    pub on_close: Option<PopupLifecycle>,
}

fn root_spec(node: &WidgetNode) -> Option<&animations::Spec> {
    match node {
        WidgetNode::Animated { spec, .. } => Some(spec),
        _ => None,
    }
}

/// Swap the popup's root transition while preserving its tween identity.
fn with_root_spec(node: WidgetNode, mut spec: animations::Spec) -> WidgetNode {
    if let WidgetNode::Animated {
        child,
        spec: current,
    } = node
    {
        spec.owner = current.owner;
        spec.scope = current.scope;
        WidgetNode::Animated { child, spec }
    } else {
        WidgetNode::Animated {
            child: Box::new(node),
            spec,
        }
    }
}

impl Popup {
    /// Provisional surface bounds until the renderer measures omitted axes.
    /// Unconfirmed content stays invisible; no padding estimate affects size.
    pub(crate) fn content_size(sw: f32, sh: f32, content: &PopupContent) -> (u32, u32) {
        let dimension = |value: Option<f32>, max: f32| {
            value.unwrap_or(max).ceil().clamp(1.0, max.max(1.0)) as u32
        };
        (dimension(content.width, sw), dimension(content.height, sh))
    }

    pub(crate) fn initial_size(
        cache: &mut MeasurementCache,
        sw: f32,
        sh: f32,
        content: &PopupContent,
        size: f32,
    ) -> (u32, u32) {
        let (width, height) = Self::content_size(sw, sh, content);
        if content.height.is_some() && content.width.is_some() {
            return (width, height);
        }
        let key = sizing::measurement_key(
            content.tree.as_ref(),
            content.width,
            content.height,
            size.max(1.0),
            sw.max(1.0),
            sh.max(1.0),
        );
        cache.get(key).unwrap_or((width, height))
    }

    /// Only a popup root or nil is a valid surface description.
    pub(crate) fn parse_popup_content(
        value: mlua::Value,
        hover: &crate::ui::hover::HoverSet,
    ) -> Result<PopupContent, String> {
        use mlua::Value;
        if value == Value::Nil {
            return Ok(PopupContent::default());
        }
        let Value::Table(table) = &value else {
            return Err("app:popup() must return iced.popup(child) or nil".into());
        };
        if table
            .raw_get::<Option<String>>("type")
            .map_err(|e| e.to_string())?
            .as_deref()
            != Some("popup")
        {
            return Err("app:popup() must return iced.popup(child) or nil; descriptor tables are no longer supported".into());
        }
        let tree = parse_node_hovered(&value, hover)?;
        let mut root = &tree;
        while let WidgetNode::Anchored { child, .. }
        | WidgetNode::Clickable { child, .. }
        | WidgetNode::Aligned { child, .. }
        | WidgetNode::Animated { child, .. } = root
        {
            root = child;
        }
        let WidgetNode::Popup {
            width,
            height,
            on_open,
            on_close,
            ..
        } = root
        else {
            return Err("expected popup root".into());
        };
        fn dimension(length: &NodeLength) -> Option<f32> {
            match length {
                NodeLength::Fixed(value) => Some(*value),
                NodeLength::Fill => Some(f32::MAX),
                _ => None,
            }
        }
        let on_open = on_open.clone();
        let on_close = on_close.clone();
        Ok(PopupContent {
            width: dimension(width),
            height: dimension(height),
            tree: Some(tree),
            on_open,
            on_close,
        })
    }

    /// GPU busy % from DRM sysfs (AMD + Intel expose `gpu_busy_percent`
    /// per card; NVIDIA needs NVML and reads as unavailable). Busiest
    /// card wins on multi-GPU setups.
    pub(crate) fn gpu_usage_percent() -> Option<f32> {
        crate::services::gpu::gpu_usage_percent()
    }

    #[cfg(test)]
    pub(crate) fn gpu_usage_in(drm: &std::path::Path) -> Option<f32> {
        crate::services::gpu::gpu_usage_in(drm)
    }

    /// Anchor point (bar-local px) for the popup: the center of the
    /// reporting widget along the bar axis, pinned to the bar's outer
    /// edge on the cross axis, so the menu grows off the edge from the
    /// widget. The compositor flips/slides the box into the output.
    pub(crate) fn popup_anchor(
        bar: (f32, f32),
        horizontal: bool,
        first_side: bool,
        rect: (i32, i32, u32, u32),
    ) -> (i32, i32) {
        let (bw, bh) = bar;
        let (rx, ry, rw, rh) = rect;
        let (cx, cy) = (rx as f32 + rw as f32 / 2.0, ry as f32 + rh as f32 / 2.0);
        if horizontal {
            let x = cx.clamp(0.0, (bw - 1.0).max(0.0)).round() as i32;
            let y = if first_side { bh as i32 - 1 } else { 0 };
            (x, y.max(0))
        } else {
            let y = cy.clamp(0.0, (bh - 1.0).max(0.0)).round() as i32;
            let x = if first_side { bw as i32 - 1 } else { 0 };
            (x.max(0), y)
        }
    }

    /// Open a popup for one placement's `popup()` body, anchored to the
    /// widget rect (bar-local px) reported by the opening click. Falls
    /// back to the cursor when a caller has no bounds. Returns `None`
    /// when the placement cannot pop up or renders nothing.
    pub(crate) fn open_for(
        plots: &mut Plots,
        bar_id: window::Id,
        output: OutputId,
        placement_id: &str,
        opener: &str,
        bounds: Option<(f32, f32, f32, f32)>,
    ) -> Option<Command<Plant>> {
        // Fresh state for the placement: edits apply on open, not on the
        // placement's next (maybe 60s) render tick.
        let resolved = Top::resolve_placement_id(plots, placement_id)?;
        Top::sync_script_state(plots, &resolved);
        // A caller without measured bounds (e.g. a synthetic click) still
        // opens at the last cursor position rather than a stale slot.
        let bounds = bounds.or_else(|| {
            plots
                .input
                .cursors
                .get(&bar_id)
                .map(|p| (p.x, p.y, 1.0, 1.0))
        });
        if let Err(e) = Top::ensure_widget_lua(plots, &resolved) {
            Top::note_widget_error(plots, &resolved.label(), e);
            return None;
        }
        let label = resolved.label();
        let output_name = Top::output_name(plots, bar_id);
        let entry = crate::lua::widgets::EntryContext {
            services: crate::services::ServiceCtx::from_plots(plots, Self::gpu_usage_percent()),
            output: &output_name,
        };
        let body = match plots
            .placements
            .popup(&resolved, &entry)?
            .and_then(|value| Self::parse_popup_content(value, &Default::default()))
        {
            Ok(content) => content,
            Err(error) => {
                Top::note_widget_error(plots, &label, error);
                return None;
            }
        };
        body.tree.as_ref()?;
        match Self::surface_context(plots).open(
            surface::OpenRequest {
                bar: bar_id,
                output,
                opener: opener.to_owned(),
                anchor: bounds,
            },
            &resolved,
            body,
        ) {
            Ok(command) => command,
            Err(error) => {
                Top::note_widget_error(plots, &label, error);
                None
            }
        }
    }

    fn surface_context(plots: &mut Plots) -> surface::SurfaceContext<'_> {
        surface::SurfaceContext {
            windows: &mut plots.windows,
            animation: super::top::animation::ListContext {
                lists: &mut plots.animation.widgets,
                motion: &mut plots.animation.motion,
                duration: plots.config.animation.speed.duration(),
            },
        }
    }

    pub fn view(&self, plots: &Plots) -> Element<'static, Plant> {
        let content = self.content_view(plots);
        match self.hosted {
            Some(hosted) => hosted.view(self, plots, content),
            None => content,
        }
    }

    fn content_view(&self, plots: &Plots) -> Element<'static, Plant> {
        use iced::widget::container;
        let content = container(self.body_view(plots, false))
            .width(Length::Fixed(self.w as f32))
            .height(Length::Fixed(self.h as f32))
            .clip(true)
            .into();
        if !self.auto_height && !self.auto_width {
            return content;
        }
        let intrinsic = container(self.body_view(plots, true))
            .width(if self.auto_width {
                Length::Shrink
            } else {
                Length::Fixed(self.w as f32)
            })
            .into();
        Element::new(sizing::MeasuredPopup {
            content,
            intrinsic,
            id: self.win_id,
            revision: self.sizing_revision,
            confirmed: self.measured_revision == Some(self.sizing_revision),
            visible: self.measured_revision.is_some(),
            size: iced::Size::new(self.w, self.h),
            max: iced::Size::new(self.max_width, self.max_height),
            auto_width: self.auto_width,
            auto_height: self.auto_height,
        })
    }

    /// Active description: the exit-transition tree while closing.
    fn display_tree(&self) -> Option<&WidgetNode> {
        self.closing
            .as_ref()
            .map(|closing| &closing.tree)
            .or(self.tree.as_ref())
    }

    fn body_view(&self, plots: &Plots, intrinsic: bool) -> Element<'static, Plant> {
        use crate::shell::TopEvent;
        let win_id = self.win_id;
        if let Some(tree) = self.display_tree() {
            let scope = format!("popup:{win_id:?}");
            let (tree, retiring) = if self.closing.is_some() {
                (tree, false)
            } else {
                plots.animation.widgets.display(&scope, tree)
            };
            let measured;
            let tree = if intrinsic {
                measured = sizing::intrinsic_size(tree, self.auto_width, self.auto_height);
                &measured
            } else {
                tree
            };
            // Trees failing to build were rejected at parse/refresh.
            // Buttons route like item rows: the click carries this
            // popup, so `on_action` receives the key.
            let msg: crate::ui::action::Sender = std::sync::Arc::new(move |action| {
                Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupSelect(win_id, action)))
            });
            let hover: crate::ui::action::HoverSender =
                std::sync::Arc::new(move |list, key, entered| {
                    Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupHover(
                        win_id, list, key, entered,
                    )))
                });
            if let Ok(node) = super::top::build_with_lists_opacity(
                tree,
                &scope,
                self.size,
                (!retiring).then_some(msg),
                &plots.animation.motion,
                &plots.animation.widgets,
                (!retiring).then_some(hover),
                1.0,
            ) {
                return crate::ui::motion::shifted(node, 0.0, 0.0, !retiring);
            }
        }
        iced::widget::Space::new().into()
    }

    pub(crate) fn handle_measured(
        plots: &mut Plots,
        id: window::Id,
        revision: u64,
        measured: iced::Size<u32>,
    ) -> Command<Plant> {
        let Some(popup) = plots.windows.popups.get_mut(&id) else {
            return Command::none();
        };
        if (!popup.auto_height && !popup.auto_width) || revision != popup.sizing_revision {
            return Command::none();
        }
        let width = if popup.auto_width {
            measured.width.max(1).min(popup.max_width.max(1.0) as u32)
        } else {
            popup.w
        };
        let height = if popup.auto_height {
            measured.height.max(1).min(popup.max_height.max(1.0) as u32)
        } else {
            popup.h
        };
        let key = sizing::measurement_key(
            popup.tree.as_ref(),
            (!popup.auto_width).then_some(popup.w as f32),
            (!popup.auto_height).then_some(popup.h as f32),
            popup.size,
            popup.max_width,
            popup.max_height,
        );
        plots.windows.popup_heights.remember(key, (width, height));
        popup.measured_revision = Some(revision);
        if height == popup.h && width == popup.w {
            return Command::none();
        }
        popup.h = height;
        popup.w = width;
        popup.reposition()
    }

    /// xdg_popup resizing uses its positioner, not iced's generic window API.
    fn reposition(&mut self) -> Command<Plant> {
        if self.hosted.is_some() {
            return Command::none();
        }
        let Some(size) = iced_exwlshell::reexport::PixelSize::try_px(self.w, self.h) else {
            return Command::none();
        };
        self.positioner.size = size;
        Command::done(Plant::PopUpReposition {
            settings: self.positioner,
            id: self.win_id,
        })
    }

    pub(super) fn phase_timer(
        id: window::Id,
        revision: u64,
        duration: Duration,
        opened: bool,
    ) -> Command<Plant> {
        Command::perform(
            async move { tokio::time::sleep(duration).await },
            move |()| {
                Plant::TopPlot(TopEvent::Widget(if opened {
                    WidgetEvent::PopupOpened(id, revision)
                } else {
                    WidgetEvent::PopupClosed(id, revision)
                }))
            },
        )
    }

    fn close_spec(&self) -> Option<animations::Spec> {
        let declared = self.on_close.as_ref()?;
        let root = self.tree.as_ref().and_then(root_spec);
        match &declared.animate {
            Some(spec) => {
                let mut spec = (**spec).clone();
                // Reuse the open tween so an interrupted opening retargets
                // from its current position instead of jumping.
                if let Some(root) = root {
                    spec.id.clone_from(&root.id);
                    spec.owner = root.owner.clone();
                    spec.scope = root.scope.clone();
                }
                Some(spec)
            }
            None => {
                let root = root?;
                // Only a declared `from` gives the close transition a start
                // state to return to; otherwise the popup is already home.
                if root.properties.iter().flatten().all(|p| p.from.is_none()) {
                    return None;
                }
                Some(root.reversed())
            }
        }
    }

    /// Start the close transition (killing any pending open completion).
    /// Popups without `:on_close(...)` still close immediately.
    pub(crate) fn request_close(plots: &mut Plots, id: window::Id) -> Command<Plant> {
        let Some(popup) = plots.windows.popups.get_mut(&id) else {
            return Command::none();
        };
        if popup.closing.is_some() {
            return Command::none();
        }
        popup.opening = None;
        let callback = popup
            .on_close
            .as_ref()
            .and_then(|life| life.callback.clone());
        let Some(spec) = popup.close_spec() else {
            let close = Self::handle_dismiss(plots, id);
            let run = callback
                .as_ref()
                .map(|callback| Self::run_callback(plots, callback))
                .unwrap_or_else(Command::none);
            return Command::batch(vec![close, run]);
        };
        let Some(tree) = popup.tree.clone() else {
            return Self::handle_dismiss(plots, id);
        };
        popup.phase = popup.phase.wrapping_add(1);
        let revision = popup.phase;
        popup.closing = Some(Closing {
            revision,
            tree: with_root_spec(tree, spec.clone()),
            callback,
        });
        if spec.total_duration().is_zero() {
            Self::handle_closed(plots, id, revision)
        } else {
            Self::phase_timer(id, revision, spec.total_duration(), false)
        }
    }

    /// Open transition finished: run its callback once, if still current.
    pub(crate) fn handle_opened(
        plots: &mut Plots,
        id: window::Id,
        revision: u64,
    ) -> Command<Plant> {
        let callback = {
            let Some(popup) = plots.windows.popups.get_mut(&id) else {
                return Command::none();
            };
            if popup.closing.is_some()
                || popup.opening.as_ref().map(|(value, _)| *value) != Some(revision)
            {
                return Command::none();
            }
            popup.opening.take().map(|(_, callback)| callback)
        };
        match callback {
            Some(callback) => Command::batch(vec![
                Self::run_callback(plots, &callback),
                Self::refresh_bodies(plots),
            ]),
            None => Command::none(),
        }
    }

    /// Close transition finished: drop the popup, then run its callback once.
    pub(crate) fn handle_closed(
        plots: &mut Plots,
        id: window::Id,
        revision: u64,
    ) -> Command<Plant> {
        let callback = {
            let Some(popup) = plots.windows.popups.get_mut(&id) else {
                return Command::none();
            };
            if popup.closing.as_ref().map(|closing| closing.revision) != Some(revision) {
                return Command::none();
            }
            popup
                .closing
                .take()
                .and_then(|closing: Closing| closing.callback)
        };
        let close = Self::handle_dismiss(plots, id);
        match callback {
            Some(callback) => Command::batch(vec![close, Self::run_callback(plots, &callback)]),
            None => close,
        }
    }

    /// Dispatch a lifecycle callback from the update phase only.
    fn run_callback(
        plots: &mut Plots,
        callback: &crate::lua::callbacks::Callback,
    ) -> Command<Plant> {
        match callback.invoke(
            &crate::services::ServiceCtx::from_plots(plots, Self::gpu_usage_percent()),
            |_| true,
        ) {
            Ok(Some(value)) => crate::shell::actions::command_from_action(&value, plots),
            Ok(None) => Command::none(),
            Err(error) => {
                eprintln!("popup lifecycle callback: {error}");
                Command::none()
            }
        }
    }

    /// Dismiss a popup: drop tracking + cursor state and close its window
    /// (idempotent, like every other layer close).
    pub(crate) fn handle_dismiss(plots: &mut Plots, id: window::Id) -> Command<Plant> {
        plots.forget_window(id);
        iced_runtime::task::effect(Action::Window(WindowAction::Close(id)))
    }

    /// Re-render open popup bodies with live data (called when widget
    /// outputs move, so menus tick too). Each body re-syncs its script
    /// first, so `popup()` edits apply without waiting out the
    /// placement's render interval. Bodies that break keep their last
    /// good content; popups whose placement left the config are
    /// dismissed. Returns close commands for dismissals (possibly none).
    pub(crate) fn refresh_bodies(plots: &mut Plots) -> Command<Plant> {
        if !plots.windows.popups.is_empty() {
            plots.services.refresh_system();
        }
        Self::refresh_service_bodies(plots)
    }

    /// Service events reuse the current samples rather than advancing the
    /// periodic CPU/GPU sampling clock on every workspace/window change.
    pub(crate) fn refresh_service_bodies(plots: &mut Plots) -> Command<Plant> {
        let open: Vec<(window::Id, String, window::Id)> = plots
            .windows
            .popups
            .iter()
            .filter(|(_, popup)| popup.closing.is_none())
            .map(|(id, popup)| (*id, popup.placement.clone(), popup.bar_id))
            .collect();
        if open.is_empty() {
            return Command::none();
        }
        let gpu = plots.services.gpu();
        let mut cmds = Vec::new();
        for (pid, placement_id, bar) in open {
            // Gone from the config: dismiss. Anything else keeps its
            // last good body on error (a typo mid-save must not close
            // the menu you're editing).
            let Some(resolved) = Top::resolve_placement_id(plots, &placement_id) else {
                cmds.push(Self::request_close(plots, pid));
                continue;
            };
            Top::sync_script_state(plots, &resolved);
            let label = resolved.label();
            let content = match Top::ensure_widget_lua(plots, &resolved) {
                Err(e) => {
                    Top::note_widget_error(plots, &label, e);
                    continue;
                }
                Ok(()) => {
                    let output = Top::output_name(plots, bar);
                    let entry = crate::lua::widgets::EntryContext {
                        services: crate::services::ServiceCtx::from_plots(plots, gpu),
                        output: &output,
                    };
                    let hover = plots
                        .windows
                        .popups
                        .get(&pid)
                        .map(|popup| popup.hovered.clone())
                        .unwrap_or_default();
                    match plots.placements.popup(&resolved, &entry) {
                        Some(result) => match result
                            .and_then(|value| Self::parse_popup_content(value, &hover))
                        {
                            Ok(content) => Some(content),
                            Err(error) => {
                                Top::note_widget_error(plots, &label, error);
                                continue;
                            }
                        },
                        None => continue,
                    }
                }
            };
            match content {
                Some(content) if content.tree.is_none() => {
                    cmds.push(Self::request_close(plots, pid))
                }
                Some(content) => match Self::surface_context(plots).refresh(pid, content) {
                    Ok(Some(command)) => cmds.push(command),
                    Ok(None) => {}
                    Err(error) => Top::note_widget_error(plots, &label, error),
                },
                None => cmds.push(Self::request_close(plots, pid)),
            }
        }
        if cmds.is_empty() {
            Command::none()
        } else {
            Command::batch(cmds)
        }
    }

    /// Pointer entered or left one popup listview row: re-parse that popup
    /// with the row's `item.hovered` set. Unknown popups and unchanged
    /// flags are ignored. Never invokes Lua.
    pub(crate) fn handle_hover(
        plots: &mut Plots,
        id: window::Id,
        list: String,
        key: String,
        hovered: bool,
    ) -> Command<Plant> {
        let Some(popup) = plots.windows.popups.get_mut(&id) else {
            return Command::none();
        };
        if popup.closing.is_some() {
            return Command::none();
        }
        let owned = (list, key);
        let mut changed = false;
        if hovered {
            // Single pointer: entering one row clears the rest of the popup.
            for other in popup
                .hovered
                .iter()
                .filter(|entry| **entry != owned)
                .cloned()
                .collect::<Vec<_>>()
            {
                popup.hovered.remove(&other);
                changed = true;
            }
            changed |= popup.hovered.insert(owned);
        } else {
            changed = popup.hovered.remove(&owned);
        }
        if changed {
            Self::refresh_bodies(plots)
        } else {
            Command::none()
        }
    }

    /// Click a popup item: run the placement's `on_action()` with the
    /// item key, then re-render the menu and the cell (a toggle flips
    /// both). Unknown popups are ignored.
    pub(crate) fn handle_select(
        plots: &mut Plots,
        id: window::Id,
        action: crate::ui::action::Action,
    ) -> Command<Plant> {
        let Some(placement_id) = plots.windows.popups.get(&id).map(|p| p.placement.clone()) else {
            return Command::none();
        };
        if plots
            .windows
            .popups
            .get(&id)
            .is_some_and(|popup| popup.closing.is_some())
        {
            return Command::none();
        }
        let bar = plots.windows.popups.get(&id).map(|p| p.bar_id);
        // Fresh handler for the click: an edited `on_action()` applies
        // immediately (in-memory toggle state resets on reload, same
        // as editing any widget script).
        let resolved = Top::resolve_placement_id(plots, &placement_id);
        if let Some(resolved) = resolved.as_ref() {
            Top::sync_script_state(plots, resolved);
            if let Err(e) = Top::ensure_widget_lua(plots, resolved) {
                Top::note_widget_error(plots, &resolved.label(), e);
                return Command::none();
            }
        }
        let outcome = resolved.as_ref().and_then(|resolved| {
            let output = bar.map(|b| Top::output_name(plots, b)).unwrap_or_default();
            plots.placements.ui_action(
                resolved,
                &crate::lua::widgets::EntryContext {
                    services: crate::services::ServiceCtx::from_plots(
                        plots,
                        Self::gpu_usage_percent(),
                    ),
                    output: &output,
                },
                &action,
            )
        });
        match outcome {
            Some(Ok(value)) => {
                plots.placements.errors.remove(&placement_id);
                let notif = crate::shell::actions::command_from_action(&value, plots);
                let refresh = Self::refresh_bodies(plots);
                // Tree-aware cell refresh (same as the cell-action
                // path): a tree render() must update widget_trees, not
                // just the text cache.
                let gpu = Self::gpu_usage_percent();
                if Top::refresh_widget(plots, &placement_id, gpu) {
                    return Command::batch(vec![
                        refresh,
                        Command::done(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Changed))),
                        notif,
                    ]);
                }
                return Command::batch(vec![refresh, notif]);
            }
            Some(Err(e)) => {
                let label = resolved
                    .map(|r| r.label())
                    .unwrap_or_else(|| placement_id.clone());
                Top::note_widget_error(plots, &label, e)
            }
            None => {}
        }
        Command::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::node::NodeLength;

    #[test]
    fn popup_hover_reparses_with_item_hovered_and_ignores_stale_popups() {
        use crate::config::{WidgetDefinition, WidgetPlacement};
        let dir = std::env::temp_dir().join(format!("riced-popup-hover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hover.lua");
        std::fs::write(
            &script,
            "return {view=function() return 'x' end, popup=function() return iced.popup(iced.listview({{id=1},{id=2}}):id('rows'):key('id'):delegate(function(n) return iced.text(n.hovered and 'hot' or 'cold') end)) end}",
        )
        .unwrap();
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = crate::shell::state::Plots::new(receiver);
        let bar = window::Id::unique();
        let mut top = Top::new();
        top.local.widgets = vec![vec![WidgetPlacement {
            id: "whover".into(),
            name: "hoverpop".into(),
            file: Some(script.to_string_lossy().into()),
            ..Default::default()
        }]];
        plots.windows.tops.insert(bar, top);
        plots.catalog.definitions.push(WidgetDefinition {
            name: "hoverpop".into(),
            file: script.clone(),
            defaults: Default::default(),
            schema: Default::default(),
        });
        let pid = window::Id::unique();
        plots.windows.popups.insert(
            pid,
            Popup {
                win_id: pid,
                bar_id: bar,
                placement: "whover".into(),
                opener: String::new(),
                auto_width: false,
                max_width: 1920.0,
                tree: None,
                hovered: Default::default(),
                size: 13.0,
                w: 220,
                h: 137,
                auto_height: true,
                max_height: 1080.0,
                sizing_revision: 0,
                measured_revision: None,
                hosted: None,
                positioner: iced_exwlshell::actions::IcedNewPopupSettings::at_position(
                    window::Id::unique(),
                    iced_exwlshell::reexport::PixelSize::try_px(220, 137).unwrap(),
                    (0, 0),
                ),
                on_open: None,
                on_close: None,
                opening: None,
                closing: None,
                phase: 0,
            },
        );
        let labels = |plots: &crate::shell::state::Plots| {
            let WidgetNode::Popup { child, .. } = plots.windows.popups[&pid].tree.as_ref().unwrap()
            else {
                panic!("popup");
            };
            let WidgetNode::ListView { items, .. } = child.as_ref() else {
                panic!("list");
            };
            items
                .iter()
                .map(|(_, node)| match node {
                    WidgetNode::Text { content, .. } => content.clone(),
                    other => panic!("text, got {other:?}"),
                })
                .collect::<Vec<_>>()
        };
        // Stale popups are ignored without touching Lua.
        let _ = Popup::handle_hover(
            &mut plots,
            window::Id::unique(),
            "rows".into(),
            "2".into(),
            true,
        );
        assert!(plots.windows.popups[&pid].hovered.is_empty());
        let _ = Popup::handle_hover(&mut plots, pid, "rows".into(), "2".into(), true);
        assert!(
            plots.windows.popups[&pid]
                .hovered
                .contains(&("rows".to_string(), "2".to_string()))
        );
        assert_eq!(labels(&plots), ["cold", "hot"]);
        // Entering the sibling clears the first; repeats and exits settle.
        let _ = Popup::handle_hover(&mut plots, pid, "rows".into(), "1".into(), true);
        assert_eq!(labels(&plots), ["hot", "cold"]);
        let _ = Popup::handle_hover(&mut plots, pid, "rows".into(), "1".into(), true);
        let _ = Popup::handle_hover(&mut plots, pid, "rows".into(), "1".into(), false);
        assert!(plots.windows.popups[&pid].hovered.is_empty());
        assert_eq!(labels(&plots), ["cold", "cold"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn popup_roots_are_strict_and_roundtrip_without_losing_properties() {
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        let content = Popup::parse_popup_content(
            lua.load("return iced.popup(iced.button('Go', 'go')):width(90):height(40):padding(0):align_x('center')").eval().unwrap(),
            &Default::default(),
        )
        .unwrap();
        assert_eq!((content.width, content.height), (Some(90.0), Some(40.0)));
        let node = content.tree.unwrap();
        let exported = crate::ui::encode::to_table(&lua, &node).unwrap();
        assert_eq!(exported.raw_get::<String>("type").unwrap(), "popup");
        let decoded =
            Popup::parse_popup_content(mlua::Value::Table(exported), &Default::default()).unwrap();
        assert_eq!(decoded.tree, Some(node));
        for source in [
            "'hi'",
            "{ui=iced.text('old'),width=90}",
            "{items={}}",
            "iced.container(iced.text('not a popup'))",
            "iced.popup(iced.text('x')):width('fill:2')",
            "iced.popup(iced.text('x')):width(0)",
            "iced.popup(iced.text('x')):height(1/0)",
            "iced.popup(iced.text('x')):padding(function() end)",
            "{type='popup',child=iced.text('x'),typo=true}",
            "iced.popup(iced.row({iced.popup(iced.text('nested'))}))",
        ] {
            let value = lua.load(format!("return {source}")).eval().unwrap();
            assert!(
                Popup::parse_popup_content(value, &Default::default()).is_err(),
                "{source}"
            );
        }
        for source in [
            "iced.row({iced.popup(iced.text('x'))})",
            "iced.container(iced.popup(iced.text('x')))",
            "iced.stack({iced.popup(iced.text('x'))})",
        ] {
            let value = lua.load(format!("return {source}")).eval().unwrap();
            assert!(crate::ui::decode::parse_node(&value).is_err(), "{source}");
        }
        assert!(
            Popup::parse_popup_content(mlua::Value::Nil, &Default::default())
                .unwrap()
                .tree
                .is_none()
        );
    }

    #[test]
    fn popup_refresh_preserves_measured_height_and_tracks_explicit_height_changes() {
        let id = window::Id::unique();
        let mut windows = crate::shell::windows::WindowState::default();
        windows.popups.insert(
            id,
            Popup {
                win_id: id,
                bar_id: window::Id::unique(),
                placement: "session".into(),
                opener: String::new(),
                auto_width: false,
                max_width: 1920.0,
                tree: None,
                hovered: Default::default(),
                size: 13.0,
                w: 220,
                h: 137,
                auto_height: true,
                max_height: 1080.0,
                sizing_revision: 0,
                measured_revision: None,
                hosted: None,
                positioner: iced_exwlshell::actions::IcedNewPopupSettings::at_position(
                    window::Id::unique(),
                    iced_exwlshell::reexport::PixelSize::try_px(220, 137).unwrap(),
                    (0, 0),
                ),
                on_open: None,
                on_close: None,
                opening: None,
                closing: None,
                phase: 0,
            },
        );
        let mut lists = super::super::top::animation::WidgetLists::new();
        let mut motion = crate::ui::anim::AnimRuntime::default();
        let mut context = surface::SurfaceContext {
            windows: &mut windows,
            animation: super::super::top::animation::ListContext {
                lists: &mut lists,
                motion: &mut motion,
                duration: std::time::Duration::ZERO,
            },
        };
        let content = PopupContent {
            tree: Some(WidgetNode::Space {
                width: NodeLength::Shrink,
                height: NodeLength::Fixed(20.0),
            }),
            width: Some(220.0),
            ..Default::default()
        };
        assert!(context.refresh(id, content.clone()).unwrap().is_none());
        assert_eq!(context.windows.popups[&id].h, 137);
        assert_eq!(context.windows.popups[&id].sizing_revision, 1);
        assert!(context.refresh(id, content.clone()).unwrap().is_none());
        let content = PopupContent {
            height: Some(210.0),
            ..content
        };
        assert!(context.refresh(id, content.clone()).unwrap().is_some());
        assert_eq!(context.windows.popups[&id].h, 210);
        assert!(!context.windows.popups[&id].auto_height);
        assert!(context.refresh(id, content).unwrap().is_none());
    }

    #[test]
    fn system_menu_popup_is_sized_to_fit_all_rows() {
        use crate::lua::{
            test_support::load_seed_components,
            widgets::{call_lua_value, load_widget_script, new_widget_lua},
        };
        let lua = new_widget_lua().unwrap();
        load_seed_components(&lua);
        load_widget_script(&lua, "system", crate::config::SEED_SYSTEM_LUA).unwrap();
        let content =
            Popup::parse_popup_content(call_lua_value(&lua, "popup").unwrap(), &Default::default())
                .unwrap();
        let (_, height) = Popup::content_size(1920.0, 1080.0, &content);
        let needed = crate::ui::build::estimate_height(content.tree.as_ref().unwrap(), 13.0);
        assert!(height as f32 >= needed, "popup {height}px clips {needed}px");
        assert!(height > 150, "four menu rows: {height}");
    }

    #[test]
    fn session_menu_delegates_have_visible_layout_bounds() {
        use crate::{shell::screens::top, ui::listview::ListView};
        use iced::advanced::{layout, renderer::Headless, widget::Tree};
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        for (_, source) in crate::config::builtin_component_files() {
            lua.load(&source).exec().unwrap();
        }
        crate::lua::widgets::load_widget_script(&lua, "system", crate::config::SEED_SYSTEM_LUA)
            .unwrap();
        let content = Popup::parse_popup_content(
            crate::lua::widgets::call_lua_value(&lua, "popup").unwrap(),
            &Default::default(),
        )
        .unwrap();
        let tree = content.tree.unwrap();
        let runtime = aura_anim::core::runtime::MotionRuntime::new();
        let mut lists = top::animation::WidgetLists::new();
        lists.insert("session/session-actions".into(), ListView::new(32.0));
        let popup_id = window::Id::unique();
        let handler: crate::ui::action::Sender = std::sync::Arc::new(move |key| {
            Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupSelect(popup_id, key)))
        });
        let mut element = top::build_with_lists(
            &tree,
            "session",
            13.0,
            Some(handler),
            &runtime,
            &lists,
            None,
        )
        .unwrap();
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .expect("software renderer");
        let mut state = Tree::new(&element);
        let node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, iced::Size::new(220.0, 300.0)),
        );
        // popup container -> card surface container -> column -> menu
        let menu = &node.children()[0].children()[0].children()[2];
        assert_eq!(menu.children().len(), 4);
        assert!(menu.size().width > 60.0, "collapsed menu: {menu:?}");
        for child in menu.children() {
            assert!(
                child.size().width > 60.0 && child.size().height > 20.0,
                "invisible menu delegate: {child:?}"
            );
        }
        let mut messages = Vec::new();
        let viewport = iced::Rectangle::with_size(iced::Size::new(220.0, 300.0));
        for child in menu.children() {
            let point = iced::Point::new(
                menu.bounds().x + child.bounds().x + child.size().width / 2.0,
                menu.bounds().y + child.bounds().y + child.size().height / 2.0,
            );
            for event in [
                iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
                iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
            ] {
                element.as_widget_mut().update(
                    &mut state,
                    &iced::Event::Mouse(event),
                    iced::advanced::Layout::new(&node),
                    iced::mouse::Cursor::Available(point),
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut iced::advanced::Shell::new(&mut messages),
                    &viewport,
                );
            }
        }
        let actions: Vec<_> = messages
            .into_iter()
            .filter_map(|message| match message {
                Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupSelect(id, key)))
                    if id == popup_id =>
                {
                    Some(key)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            actions
                .iter()
                .map(|action| action.key.as_str())
                .collect::<Vec<_>>(),
            ["suspend", "hibernate", "reboot", "poweroff"]
        );
    }

    #[test]
    fn popup_explicit_dimensions_are_outer_sizes_clamped_to_the_output() {
        let content = PopupContent {
            width: Some(500.0),
            height: Some(10.0),
            ..Default::default()
        };
        assert_eq!(Popup::content_size(1920.0, 1080.0, &content), (500, 10));
        assert_eq!(Popup::content_size(200.0, 100.0, &content), (200, 10));
    }

    fn test_popup(
        id: window::Id,
        tree: Option<WidgetNode>,
        on_open: Option<PopupLifecycle>,
        on_close: Option<PopupLifecycle>,
    ) -> Popup {
        Popup {
            win_id: id,
            bar_id: window::Id::unique(),
            placement: "clock".into(),
            opener: String::new(),
            tree,
            hovered: Default::default(),
            size: 13.0,
            w: 100,
            h: 100,
            auto_width: false,
            max_width: 1920.0,
            auto_height: false,
            max_height: 1080.0,
            sizing_revision: 0,
            measured_revision: None,
            hosted: None,
            positioner: iced_exwlshell::actions::IcedNewPopupSettings::at_position(
                window::Id::unique(),
                iced_exwlshell::reexport::PixelSize::try_px(100, 100).unwrap(),
                (0, 0),
            ),
            on_open,
            on_close,
            opening: None,
            closing: None,
            phase: 0,
        }
    }

    #[test]
    fn different_tray_icons_replace_the_menu_instead_of_toggling_the_placement() {
        use crate::config::{WidgetDefinition, WidgetPlacement};
        use crate::shell::windows::PlotInfo;

        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = Plots::new(receiver);
        let bar = window::Id::unique();
        let mut top = Top::new();
        top.local.widgets = vec![vec![WidgetPlacement {
            id: "tray".into(),
            name: "tray".into(),
            ..Default::default()
        }]];
        plots.windows.tops.insert(bar, top);
        plots.windows.ids.insert(bar, PlotInfo::Top(OutputId(1)));
        plots.catalog.definitions.push(WidgetDefinition {
            name: "tray".into(),
            file: "/not-real-tray.lua".into(),
            defaults: Default::default(),
            schema: Default::default(),
        });
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        crate::lua::widgets::load_widget_script(
            &lua,
            "tray",
            r#"return {
            rendered=0,
            view=function() return iced.text('tray') end,
            actions=function(self, events) self.selected=events.key; return {popup=true} end,
            popup=function(self)
                self.rendered=self.rendered+1
                return iced.popup(iced.scrollable(iced.text(self.selected)):height(360)):width(280)
            end
        }"#,
        )
        .unwrap();
        let app: mlua::Table = lua.named_registry_value("riced.widget.app").unwrap();
        plots.placements.instances.insert("tray".into(), lua);

        let id = window::Id::unique();
        let mut popup = test_popup(id, None, None, None);
        popup.bar_id = bar;
        popup.placement = "tray".into();
        popup.opener = "first".into();
        plots.windows.popups.insert(id, popup.clone());
        let _ = Top::handle_cell_action(&mut plots, bar, "tray".into(), "first".into());
        assert!(plots.windows.popups.is_empty());
        assert_eq!(app.get::<u32>("rendered").unwrap(), 0);

        plots.windows.popups.insert(id, popup);
        let _ = Top::handle_cell_action(&mut plots, bar, "tray".into(), "second".into());
        assert!(!plots.windows.popups.contains_key(&id));
        // There is no compositor output in this test, but the replacement must
        // still render the newly selected body before attempting surface creation.
        assert_eq!(app.get::<u32>("rendered").unwrap(), 1);
        assert_eq!(app.get::<String>("selected").unwrap(), "second");
    }

    #[test]
    fn popup_lifecycle_setters_are_strict_and_roundtrip() {
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        let source = "return iced.popup(iced.text('x')):width(100):animate({id='menu',y=iced.animations.y({from=-160,to=0,duration=250})}):on_open(function() end):on_close({})";
        let content =
            Popup::parse_popup_content(lua.load(source).eval().unwrap(), &Default::default())
                .unwrap();
        let on_open = content.on_open.clone().unwrap();
        assert!(on_open.callback.is_some() && on_open.animate.is_none());
        assert!(content.on_close.is_some());
        let node = content.tree.clone().unwrap();
        let encoded = crate::ui::encode::to_table(&lua, &node).unwrap();
        let decoded =
            Popup::parse_popup_content(mlua::Value::Table(encoded), &Default::default()).unwrap();
        assert_eq!(decoded.tree, Some(node));
        assert!(decoded.on_open.unwrap().callback.is_some());
        assert!(decoded.on_close.is_some());
        for source in [
            "return iced.popup(iced.text('x')):on_open(42)",
            "return iced.popup(iced.text('x')):on_open({typo=true})",
            "return iced.popup(iced.text('x')):on_close({callback=42})",
            "return iced.popup(iced.text('x')):on_close({animate='slide'})",
            "return iced.popup(iced.text('x')):on_open({animate={y={to=0}}})",
        ] {
            let value = lua.load(source).eval().unwrap();
            assert!(
                Popup::parse_popup_content(value, &Default::default()).is_err(),
                "{source}"
            );
        }
        // Without a root animation the lifecycle may declare its own `id`.
        let explicit = Popup::parse_popup_content(
            lua.load("return iced.popup(iced.text('x')):on_open({animate={id='open',y={from=-20,to=0}}})")
                .eval()
                .unwrap(),
            &Default::default(),
        )
        .unwrap();
        assert!(
            explicit
                .on_open
                .unwrap()
                .animate
                .is_some_and(|spec| spec.id == "open")
        );
        let no_args = Popup::parse_popup_content(
            lua.load("return iced.popup(iced.text('x')):animate({id='a',y=iced.animations.y({from=10,to=0})}):on_close()")
                .eval()
                .unwrap(),
            &Default::default(),
        )
        .unwrap();
        assert!(no_args.on_close.is_some());
    }

    #[test]
    fn list_dismissals_keep_measured_popup_visible_without_replaying_open() {
        use iced::advanced::{Layout, renderer::Headless, renderer::Style, widget::Tree};

        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        let content = |items: &str| {
            Popup::parse_popup_content(
                lua.load(format!(r#"local items = {{{items}}}
                    return iced.popup(#items == 0 and iced.space() or iced.listview(items)
                    :id('notifications'):key('id'):pitch(32)
                    :delegate(function(item)
                        return iced.container(iced.space():width(100):height(32)):background('#00ff00')
                    end)
                    :onEntered({{duration=0}})
                    :onExit({{opacity={{to=0}},duration=250}})
                    :onDisplaced({{duration=250}}))
                    :width(100):on_open({{animate={{id='open',y={{from=-180,to=0,duration=180}}}}}})"#))
                    .eval().unwrap(),
                &Default::default(),
            ).unwrap()
        };
        let initial = content("{id=1},{id=2},{id=3}");
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = Plots::new(receiver);
        let id = window::Id::unique();
        let spec = initial.on_open.as_ref().unwrap().animate.as_ref().unwrap();
        let mut popup = test_popup(
            id,
            Some(with_root_spec(initial.tree.unwrap(), (**spec).clone())),
            initial.on_open,
            None,
        );
        popup.auto_height = true;
        popup.measured_revision = Some(0);
        Popup::surface_context(&mut plots)
            .animation
            .synchronize(&format!("popup:{id:?}"), None, popup.tree.as_ref().unwrap())
            .unwrap();
        plots.windows.popups.insert(id, popup);
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let bounds = iced::Rectangle::with_size(iced::Size::new(100.0, 100.0));
        let mut element = plots.windows.popups[&id].view(&plots);
        let mut tree = Tree::new(&element);
        let layout = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &iced::advanced::layout::Limits::new(iced::Size::ZERO, bounds.size()),
        );
        element.as_widget_mut().update(
            &mut tree,
            &iced::Event::Window(window::Event::RedrawRequested(
                std::time::Instant::now() + Duration::from_millis(180),
            )),
            Layout::new(&layout),
            iced::mouse::Cursor::Unavailable,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut iced::advanced::Shell::new(&mut Vec::new()),
            &bounds,
        );

        for items in ["{id=2},{id=3}", "{id=3}", ""] {
            let previous_spec =
                root_spec(plots.windows.popups[&id].tree.as_ref().unwrap()).cloned();
            assert!(
                Popup::surface_context(&mut plots)
                    .refresh(id, content(items))
                    .unwrap()
                    .is_none()
            );
            let popup = &plots.windows.popups[&id];
            assert_eq!(
                previous_spec.as_ref(),
                root_spec(popup.tree.as_ref().unwrap())
            );
            assert_ne!(popup.measured_revision, Some(popup.sizing_revision));
            let mut refreshed = popup.view(&plots);
            tree.diff(&refreshed);
            let layout = refreshed.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &iced::advanced::layout::Limits::new(iced::Size::ZERO, bounds.size()),
            );
            iced::advanced::Renderer::reset(&mut renderer, bounds);
            refreshed.as_widget().draw(
                &tree,
                &mut renderer,
                &iced::Theme::Dark,
                &Style::default(),
                Layout::new(&layout),
                iced::mouse::Cursor::Unavailable,
                &bounds,
            );
            let pixels =
                renderer.screenshot(iced::Size::new(100, 100), 1.0, iced::Color::TRANSPARENT);
            assert!(
                pixels.chunks_exact(4).any(|pixel| pixel[3] > 0),
                "popup must remain visible during dismissal, including the last row's exit: {items}"
            );
        }
    }

    #[test]
    fn explicit_close_draws_the_phase_animation_without_a_root_animate() {
        use iced::advanced::{Layout, renderer::Headless, renderer::Style, widget::Tree};
        use std::time::Instant;

        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        let content = Popup::parse_popup_content(
            lua.load("return iced.popup(iced.container(iced.space():width(100):height(100)):background('#00ff00')):width(100):on_open({animate={id='test',y=iced.animations.y({from=-180,to=0,duration=180})}}):on_close({animate={id='test',y=iced.animations.y({from=0,to=-180,duration=180})}})")
                .eval()
                .unwrap(),
            &Default::default(),
        )
        .unwrap();
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = crate::shell::state::Plots::new(receiver);
        let id = window::Id::unique();
        let spec = content.on_open.as_ref().unwrap().animate.as_ref().unwrap();
        let mut popup = test_popup(
            id,
            Some(with_root_spec(content.tree.unwrap(), (**spec).clone())),
            content.on_open,
            content.on_close,
        );
        popup.auto_height = true;
        popup.measured_revision = Some(0);
        popup.hosted = Some(surface::Hosted::new((0, 0)));
        popup.max_width = 100.0;
        popup.max_height = 100.0;
        plots.windows.popups.insert(id, popup);

        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let bounds = iced::Rectangle::with_size(iced::Size::new(100.0, 100.0));
        let mut element = plots.windows.popups[&id].view(&plots);
        let mut tree = Tree::new(&element);
        let layout = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &iced::advanced::layout::Limits::new(iced::Size::ZERO, bounds.size()),
        );
        let opened = Instant::now() + Duration::from_millis(180);
        let frame = |element: &mut Element<'static, Plant>,
                     tree: &mut Tree,
                     renderer: &iced::Renderer,
                     now| {
            element.as_widget_mut().update(
                tree,
                &iced::Event::Window(window::Event::RedrawRequested(now)),
                Layout::new(&layout),
                iced::mouse::Cursor::Unavailable,
                renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut Vec::new()),
                &bounds,
            );
        };
        frame(&mut element, &mut tree, &renderer, opened);
        let _ = Popup::request_close(&mut plots, id);
        let mut closing = plots.windows.popups[&id].view(&plots);
        tree.diff(&closing);
        let mut visible = Vec::new();
        for millis in [0, 30, 180] {
            frame(
                &mut closing,
                &mut tree,
                &renderer,
                opened + Duration::from_millis(millis),
            );
            iced::advanced::Renderer::reset(&mut renderer, bounds);
            closing.as_widget().draw(
                &tree,
                &mut renderer,
                &iced::Theme::Dark,
                &Style::default(),
                Layout::new(&layout),
                iced::mouse::Cursor::Unavailable,
                &bounds,
            );
            let pixels =
                renderer.screenshot(iced::Size::new(100, 100), 1.0, iced::Color::TRANSPARENT);
            visible.push(pixels.chunks_exact(4).filter(|pixel| pixel[3] > 0).count());
        }
        assert_eq!(visible[0], 10_000);
        assert!(visible[1] > 0 && visible[1] < visible[0], "{visible:?}");
        assert_eq!(visible[2], 0);
    }

    fn hosted_test_popup() -> (Plots, window::Id) {
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        let content = Popup::parse_popup_content(
            lua.load("return iced.popup(iced.space():width(100):height(100)):width(100):height(100):on_close({animate={id='hosted',y=iced.animations.y({from=0,to=-180,duration=180})}})")
                .eval()
                .unwrap(),
            &Default::default(),
        )
        .unwrap();
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = Plots::new(receiver);
        let id = window::Id::unique();
        let mut popup = test_popup(id, content.tree, content.on_open, content.on_close);
        popup.hosted = Some(surface::Hosted::new((100, 49)));
        popup.max_width = 1000.0;
        popup.max_height = 600.0;
        plots.windows.tops.insert(popup.bar_id, Top::new());
        plots.windows.register_popup(OutputId(7), popup);
        (plots, id)
    }

    #[test]
    fn hosted_icon_and_outside_clicks_animate_but_blank_card_space_does_not_dismiss() {
        use iced::advanced::{Layout, renderer::Headless, widget::Tree};
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        for (point, dismiss) in [
            (iced::Point::new(100.0, 25.0), true), // The spawning icon, outside the card.
            (iced::Point::new(900.0, 500.0), true),
            (iced::Point::new(75.0, 75.0), false), // Blank content inside the card.
        ] {
            let (mut plots, id) = hosted_test_popup();
            let mut element = plots.windows.popups[&id].view(&plots);
            let mut tree = Tree::new(&element);
            let viewport = iced::Rectangle::with_size(iced::Size::new(1000.0, 600.0));
            let layout = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &iced::advanced::layout::Limits::new(iced::Size::ZERO, viewport.size()),
            );
            let mut messages = Vec::new();
            element.as_widget_mut().update(
                &mut tree,
                &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
                Layout::new(&layout),
                iced::mouse::Cursor::Available(point),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
                &viewport,
            );
            assert_eq!(messages.len(), usize::from(dismiss), "{point:?}");
            if dismiss {
                assert!(
                    matches!(&messages[0], Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupCloseRequested(target))) if *target == id)
                );
                let _ = plots.update(messages.remove(0));
                let popup = &plots.windows.popups[&id];
                let revision = popup
                    .closing
                    .as_ref()
                    .expect("exit transition keeps the surface mounted")
                    .revision;
                let _ = plots.update(Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupClosed(
                    id, revision,
                ))));
                assert!(!plots.windows.popups.contains_key(&id));
            }
        }
    }

    #[test]
    fn hosted_escape_uses_the_exit_transition_and_repaints_its_surface() {
        use iced::futures::StreamExt;
        use iced::keyboard::{self, Key, Location, Modifiers, key};
        let (mut plots, id) = hosted_test_popup();
        let message = Plant::Graft(
            id,
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: Key::Named(key::Named::Escape),
                modified_key: Key::Named(key::Named::Escape),
                physical_key: key::Physical::Code(key::Code::Escape),
                location: Location::Standard,
                modifiers: Modifiers::default(),
                text: None,
                repeat: false,
            }),
        );
        let command = plots.update(message);
        let actions = iced::futures::executor::block_on(
            iced_runtime::task::into_stream(command)
                .unwrap()
                .collect::<Vec<_>>(),
        );
        let [Action::Output(request)] = actions.as_slice() else {
            panic!("Escape must request a host-owned close");
        };
        assert_eq!(
            crate::shell::state::redraw_scope(request),
            iced_exwlshell::redraw::Scope::Window(id)
        );
        let _ = plots.update(request.clone());
        let phase = plots.windows.popups[&id].phase;
        assert!(plots.windows.popups[&id].closing.is_some());
        let _ = plots.update(request.clone());
        assert_eq!(plots.windows.popups[&id].phase, phase);
    }

    #[test]
    fn hosted_measurement_does_not_reposition_a_native_popup() {
        let (mut plots, id) = hosted_test_popup();
        let popup = plots.windows.popups.get_mut(&id).unwrap();
        popup.w = 200;
        assert!(iced_runtime::task::into_stream(popup.reposition()).is_none());
    }

    #[test]
    fn close_transition_reverses_the_root_animation_and_keeps_the_surface() {
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        let content = Popup::parse_popup_content(
            lua.load("return iced.popup(iced.text('x')):width(100):animate({id='menu',y=iced.animations.y({from=-160,to=0,duration=250})}):on_close()")
                .eval()
                .unwrap(),
            &Default::default(),
        )
        .unwrap();
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = crate::shell::state::Plots::new(receiver);
        let id = window::Id::unique();
        plots.windows.popups.insert(
            id,
            test_popup(
                id,
                content.tree.clone(),
                content.on_open.clone(),
                content.on_close.clone(),
            ),
        );
        let _ = Popup::request_close(&mut plots, id);
        let popup = &plots.windows.popups[&id];
        assert!(popup.tree.is_some());
        let closing = popup.closing.as_ref().expect("close transition");
        let spec = root_spec(&closing.tree).expect("root animation");
        assert_eq!(spec.id, "menu");
        let y = spec.properties[1].as_ref().expect("y");
        assert_eq!((y.from, y.to), (None, -160.0));
        assert_eq!(spec.total_duration(), std::time::Duration::from_millis(250));
        // Without `:on_close(...)` the surface closes immediately.
        plots
            .windows
            .popups
            .insert(id, test_popup(id, content.tree, content.on_open, None));
        let _ = Popup::request_close(&mut plots, id);
        assert!(!plots.windows.popups.contains_key(&id));
    }

    #[test]
    fn lifecycle_callbacks_run_once_from_update_handlers() {
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        crate::lua::widgets::load_widget_script(
            &lua,
            "lifecycle",
            "hits=0; return {view=function() return 'x' end, popup=function() return iced.popup(iced.text('x')):width(100):animate({id='life',y=iced.animations.y({from=-20,to=0,duration=50})}):on_open(function() hits=hits+1 end):on_close({callback=function() hits=hits+10 end}) end}",
        )
        .unwrap();
        let content = Popup::parse_popup_content(
            crate::lua::widgets::call_lua_value(&lua, "popup").unwrap(),
            &Default::default(),
        )
        .unwrap();
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = crate::shell::state::Plots::new(receiver);
        let id = window::Id::unique();
        let opening = content.on_open.clone().unwrap().callback.unwrap();
        let mut popup = test_popup(
            id,
            content.tree.clone(),
            content.on_open.clone(),
            content.on_close.clone(),
        );
        popup.opening = Some((1, opening));
        popup.phase = 1;
        plots.windows.popups.insert(id, popup);
        let hits = || lua.globals().get::<u32>("hits").unwrap();
        let _ = Popup::handle_opened(&mut plots, id, 1);
        assert_eq!(hits(), 1);
        let _ = Popup::handle_opened(&mut plots, id, 1);
        assert_eq!(hits(), 1);
        let revision = plots.windows.popups[&id]
            .closing
            .as_ref()
            .expect("refresh starts the close transition")
            .revision;
        let _ = Popup::handle_closed(&mut plots, id, revision + 1);
        assert_eq!(hits(), 1);
        let _ = Popup::handle_closed(&mut plots, id, revision);
        assert_eq!(hits(), 11);
        assert!(!plots.windows.popups.contains_key(&id));
    }

    #[test]
    fn popup_anchor_centers_on_widget_at_bar_edge() {
        // Top bar: x is the widget center, y pins to the bottom edge.
        assert_eq!(
            Popup::popup_anchor((1920.0, 50.0), true, true, (50, 0, 100, 50)),
            (100, 49)
        );
        // Bottom bar: pins to the top edge instead.
        assert_eq!(
            Popup::popup_anchor((1920.0, 50.0), true, false, (50, 0, 100, 50)),
            (100, 0)
        );
        // Left bar: y is the widget center, x pins to the right edge.
        assert_eq!(
            Popup::popup_anchor((50.0, 1080.0), false, true, (0, 50, 50, 100)),
            (49, 100)
        );
        // The anchor clamps into the bar along the bar axis.
        assert_eq!(
            Popup::popup_anchor((1920.0, 50.0), true, true, (-100, 0, 1, 50)),
            (0, 49)
        );
    }

    #[test]
    fn gpu_reader_picks_busiest_card() {
        let dir = std::env::temp_dir().join(format!("riced-popup-gpu-{}", std::process::id()));
        let card0 = dir.join("card0/device");
        let card1 = dir.join("card1/device");
        let _ = std::fs::create_dir_all(&card0);
        let _ = std::fs::create_dir_all(&card1);
        // Connectors look like cards but are not all-digit suffixes.
        let _ = std::fs::create_dir_all(dir.join("card0-DP-1"));
        std::fs::write(card0.join("gpu_busy_percent"), "12\n").unwrap();
        std::fs::write(card1.join("gpu_busy_percent"), "78\n").unwrap();
        assert_eq!(Popup::gpu_usage_in(&dir), Some(78.0));
        assert_eq!(Popup::gpu_usage_in(&dir.join("missing")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
