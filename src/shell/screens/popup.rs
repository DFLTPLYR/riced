mod sizing;
mod surface;
use super::top::Top;
use crate::shell::state::Plots;
use crate::shell::{Plant, TopEvent, WidgetEvent};
use crate::ui::decode::parse_node_hovered;
use crate::ui::node::{NodeLength, WidgetNode};
use iced::window;
use iced::{Element, Length, Task as Command};
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use iced_wayland_subscriber::OutputId;
pub(crate) use sizing::MeasurementCache;
#[cfg(test)]
use std::collections::HashMap;

/// QML-`Menu`-style popup for widget placements, backed by a native
/// `xdg_popup` (`Plant::NewPopUp`): an exactly-sized surface anchored
/// to the reporting widget's measured bounds, with the compositor
/// flipping/sliding it into view on overflow. No fullscreen overlay, no
/// cursor math.
///
/// Opened from `actions(events)` by calling `self:popup()` on a click;
/// the box renders whatever `popup()` returns (icons included). Clicking
/// the same placement toggles it; the compositor dismisses on outside
/// clicks (menu-grab semantics).
#[derive(Debug, Clone)]
pub struct Popup {
    /// Own window id (item clicks address their popup).
    pub win_id: window::Id,
    /// Bar that spawned this popup (toggle handling).
    pub bar_id: window::Id,
    /// Placement id whose `popup()` renders the body.
    pub placement: String,
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
}

/// A validated popup root and its surface sizing policy. None means no popup.
#[derive(Debug, Clone, Default)]
pub(crate) struct PopupContent {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub tree: Option<WidgetNode>,
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
        let WidgetNode::Popup { width, height, .. } = root else {
            return Err("expected popup root".into());
        };
        fn dimension(length: &NodeLength) -> Option<f32> {
            match length {
                NodeLength::Fixed(value) => Some(*value),
                NodeLength::Fill => Some(f32::MAX),
                _ => None,
            }
        }
        Ok(PopupContent {
            width: dimension(width),
            height: dimension(height),
            tree: Some(tree),
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
            size: iced::Size::new(self.w, self.h),
            max: iced::Size::new(self.max_width, self.max_height),
            auto_width: self.auto_width,
            auto_height: self.auto_height,
        })
    }

    fn body_view(&self, plots: &Plots, intrinsic: bool) -> Element<'static, Plant> {
        use crate::shell::TopEvent;
        let win_id = self.win_id;
        if let Some(tree) = &self.tree {
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
            if let Ok(node) = super::top::build_with_lists(
                tree,
                &format!("popup:{win_id:?}"),
                self.size,
                Some(msg),
                &plots.animation.motion,
                &plots.animation.widgets,
                Some(hover),
            ) {
                return node;
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
        let Some(size) = iced_exwlshell::reexport::PixelSize::try_px(self.w, self.h) else {
            return Command::none();
        };
        self.positioner.size = size;
        Command::done(Plant::PopUpReposition {
            settings: self.positioner,
            id: self.win_id,
        })
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
                cmds.push(Self::handle_dismiss(plots, pid));
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
                    cmds.push(Self::handle_dismiss(plots, pid))
                }
                Some(content) => match Self::surface_context(plots).refresh(pid, content) {
                    Ok(Some(command)) => cmds.push(command),
                    Ok(None) => {}
                    Err(error) => Top::note_widget_error(plots, &label, error),
                },
                None => cmds.push(Self::handle_dismiss(plots, pid)),
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
                positioner: iced_exwlshell::actions::IcedNewPopupSettings::at_position(
                    window::Id::unique(),
                    iced_exwlshell::reexport::PixelSize::try_px(220, 137).unwrap(),
                    (0, 0),
                ),
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
                positioner: iced_exwlshell::actions::IcedNewPopupSettings::at_position(
                    window::Id::unique(),
                    iced_exwlshell::reexport::PixelSize::try_px(220, 137).unwrap(),
                    (0, 0),
                ),
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
        assert!(context.refresh(id, content.clone()).unwrap().is_some());
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
        let mut lists = HashMap::new();
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
        let menu = &node.children()[0].children()[2];
        assert_eq!(menu.children().len(), 4);
        assert!(menu.size().width > 100.0, "collapsed menu: {menu:?}");
        for child in menu.children() {
            assert!(
                child.size().width > 100.0 && child.size().height > 20.0,
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
