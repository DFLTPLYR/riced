//! Popup geometry, list synchronization, and registration over narrow owners.
use super::{Popup, PopupContent};
use crate::{
    lua::widgets::ResolvedWidget,
    shell::{
        Plant,
        screens::{Background, top::animation::ListContext},
        windows::{PlotInfo, WindowState},
    },
};
use iced::{Element, Point, Size, Task, window};
use iced_exwlshell::{
    actions::IcedNewPopupSettings,
    reexport::{
        Anchor, BlurOption, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings,
        OutputOption, PixelSize, PopupAnchor, PopupConstraintAdjustment, PopupGravity,
    },
};
use iced_wayland_subscriber::OutputId;

pub(super) struct SurfaceContext<'a> {
    pub windows: &'a mut WindowState,
    pub animation: ListContext<'a>,
}

pub(super) struct OpenRequest {
    pub bar: window::Id,
    pub output: OutputId,
    /// Reporting widget rect in bar-local px; the popup anchors to its
    /// center. `None` falls back to the bar center.
    pub anchor: Option<(f32, f32, f32, f32)>,
}

/// An output-local anchor for an exit-animated, host-owned popup surface.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Hosted {
    anchor: (i32, i32),
}

impl Hosted {
    pub(super) fn new(anchor: (i32, i32)) -> Self {
        Self { anchor }
    }

    fn settings(output: OutputId) -> NewLayerShellSettings {
        NewLayerShellSettings {
            anchor: Anchor::all(),
            layer: Layer::Overlay,
            size: LayerSize::FILL,
            exclusive_zone: Some(-1),
            output_option: OutputOption::GlobalName(output.0),
            margin: Some((0, 0, 0, 0)),
            namespace: Some("Riced - Popup".into()),
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            events_transparent: false,
            blur_option: BlurOption::None,
        }
    }

    fn origin(&self, viewport: Size, bar: Size, edge: Anchor, content: Size) -> Point {
        let horizontal = edge == Anchor::Top || edge == Anchor::Bottom;
        let (x, y) = if horizontal {
            (
                (viewport.width - bar.width) / 2.0 + self.anchor.0 as f32 - content.width / 2.0,
                if edge == Anchor::Top {
                    bar.height
                } else {
                    viewport.height - bar.height - content.height
                },
            )
        } else {
            (
                if edge == Anchor::Left {
                    bar.width
                } else {
                    viewport.width - bar.width - content.width
                },
                (viewport.height - bar.height) / 2.0 + self.anchor.1 as f32 - content.height / 2.0,
            )
        };
        Point::new(
            x.clamp(0.0, (viewport.width - content.width).max(0.0)),
            y.clamp(0.0, (viewport.height - content.height).max(0.0)),
        )
    }

    pub(super) fn view(
        &self,
        popup: &Popup,
        plots: &crate::shell::state::Plots,
        content: Element<'static, Plant>,
    ) -> Element<'static, Plant> {
        use iced::widget::{Space, mouse_area, opaque, stack};
        let viewport = plots
            .windows
            .ids
            .get(&popup.win_id)
            .and_then(|info| match info {
                PlotInfo::Popup(output) => {
                    Background::available_rect(*output, &plots.windows.output_infos)
                }
                _ => None,
            })
            .map(|(_, _, w, h)| Size::new(w, h))
            .unwrap_or(Size::new(popup.max_width, popup.max_height));
        let (bar, edge) =
            plots
                .windows
                .tops
                .get(&popup.bar_id)
                .map_or((Size::ZERO, Anchor::Top), |top| {
                    let (w, h) =
                        top.local
                            .px_size(viewport.width, viewport.height, top.is_horizontal());
                    (Size::new(w as f32, h as f32), top.anchor())
                });
        let origin = self.origin(
            viewport,
            bar,
            edge,
            Size::new(popup.w as f32, popup.h as f32),
        );
        let close = Plant::TopPlot(crate::shell::TopEvent::Widget(
            crate::shell::WidgetEvent::PopupCloseRequested(popup.win_id),
        ));
        let backdrop = mouse_area(Space::new().width(iced::Fill).height(iced::Fill))
            .on_press(close.clone())
            .on_right_press(close.clone())
            .on_middle_press(close);
        // Opaque before anchoring: blank space inside the card consumes clicks,
        // while the fullscreen area outside it belongs to the dismiss backdrop.
        let body = crate::ui::widgets::anchored::anchored(
            opaque(content),
            origin,
            Point::ORIGIN,
            Default::default(),
        );
        stack![backdrop, body].into()
    }
}

impl SurfaceContext<'_> {
    pub fn open(
        &mut self,
        request: OpenRequest,
        resolved: &ResolvedWidget,
        body: PopupContent,
    ) -> Result<Option<Task<Plant>>, String> {
        let OpenRequest {
            bar,
            output,
            anchor,
        } = request;
        let Some(top) = self.windows.tops.get(&bar) else {
            return Ok(None);
        };
        let Some((_, _, sw, sh)) = Background::available_rect(output, &self.windows.output_infos)
        else {
            return Ok(None);
        };
        let horizontal = top.is_horizontal();
        let (bw, bh) = top.local.px_size(sw, sh, horizontal);
        let rect = anchor
            .map(|(x, y, w, h)| {
                (
                    x.round() as i32,
                    y.round() as i32,
                    w.round().max(1.0) as u32,
                    h.round().max(1.0) as u32,
                )
            })
            .unwrap_or((bw as i32 / 2, bh as i32 / 2, 1, 1));
        let (w, h) = Popup::initial_size(
            &mut self.windows.popup_heights,
            sw,
            sh,
            &body,
            resolved.size,
        );
        let Some(size) = PixelSize::try_px(w, h) else {
            return Ok(None);
        };
        let first_side = top.anchor() == Anchor::Top || top.anchor() == Anchor::Left;
        let anchor_at = Popup::popup_anchor((bw as f32, bh as f32), horizontal, first_side, rect);
        let Some(anchor_size) = PixelSize::try_px(1, 1) else {
            return Ok(None);
        };
        let (anchor, gravity) = if horizontal {
            if first_side {
                (PopupAnchor::Bottom, PopupGravity::Bottom)
            } else {
                (PopupAnchor::Top, PopupGravity::Top)
            }
        } else if first_side {
            (PopupAnchor::Right, PopupGravity::Right)
        } else {
            (PopupAnchor::Left, PopupGravity::Left)
        };
        let settings = IcedNewPopupSettings::new(bar, size, anchor_at, anchor_size)
            .anchor(anchor)
            .gravity(gravity)
            .constraint_adjustment(
                PopupConstraintAdjustment::FlipX
                    | PopupConstraintAdjustment::FlipY
                    | PopupConstraintAdjustment::SlideX
                    | PopupConstraintAdjustment::SlideY,
            );
        let id = window::Id::unique();
        // Native grabs dismiss on an icon click before Lua can toggle.
        // Keep exit-animated surfaces under host control until completion.
        let hosted = body.on_close.as_ref().map(|_| Hosted::new(anchor_at));
        let PopupContent {
            width,
            height,
            tree,
            on_open,
            on_close,
        } = body;
        let tree = match (
            tree,
            on_open
                .as_ref()
                .and_then(|life| life.animate.as_deref().cloned()),
        ) {
            (Some(tree), Some(spec)) => Some(super::with_root_spec(tree, spec)),
            (tree, _) => tree,
        };
        if let Some(tree) = &tree {
            self.animation
                .synchronize(&format!("popup:{id:?}"), None, tree)?;
        }
        let callback = on_open.as_ref().and_then(|life| life.callback.clone());
        let phase = u64::from(callback.is_some());
        let duration = tree
            .as_ref()
            .and_then(super::root_spec)
            .map(super::animations::Spec::total_duration)
            .unwrap_or_default();
        self.windows.register_popup(
            output,
            Popup {
                win_id: id,
                bar_id: bar,
                placement: resolved.id.clone(),
                tree,
                hovered: Default::default(),
                size: resolved.size.max(1.0),
                w,
                h,
                auto_width: width.is_none(),
                max_width: sw.max(1.0),
                auto_height: height.is_none(),
                max_height: sh.max(1.0),
                sizing_revision: 0,
                measured_revision: None,
                positioner: settings,
                hosted,
                on_open,
                on_close,
                opening: callback.map(|callback| (phase, callback)),
                closing: None,
                phase,
            },
        );
        let open = Task::done(if hosted.is_some() {
            Plant::NewLayerShell {
                settings: Hosted::settings(output),
                id,
            }
        } else {
            Plant::NewPopUp { settings, id }
        });
        Ok(Some(if phase == 0 {
            open
        } else {
            Task::batch(vec![open, Popup::phase_timer(id, phase, duration, true)])
        }))
    }

    pub fn refresh(
        &mut self,
        id: window::Id,
        content: PopupContent,
    ) -> Result<Option<Task<Plant>>, String> {
        let output = self.windows.ids.get(&id).and_then(|info| match info {
            PlotInfo::Popup(output) => Some(*output),
            _ => None,
        });
        let (sw, sh) = output
            .and_then(|output| Background::available_rect(output, &self.windows.output_infos))
            .map(|(_, _, width, height)| (width, height))
            .unwrap_or((1920.0, 1080.0));
        let (mut w, mut h) = Popup::content_size(sw, sh, &content);
        let Some(popup) = self.windows.popups.get_mut(&id) else {
            return Ok(None);
        };
        // The exit transition owns the body until it completes.
        if popup.closing.is_some() {
            return Ok(None);
        }
        let auto_height = content.height.is_none();
        let auto_width = content.width.is_none();
        if auto_width && popup.auto_width {
            w = popup.w.min(sw.max(1.0) as u32);
        }
        if auto_height && popup.auto_height {
            h = popup.h.min(sh.max(1.0) as u32);
        }
        let PopupContent {
            tree,
            on_open,
            on_close,
            ..
        } = content;
        let tree = match (
            tree,
            on_open
                .as_ref()
                .and_then(|life| life.animate.as_deref().cloned()),
        ) {
            (Some(tree), Some(spec)) => Some(super::with_root_spec(tree, spec)),
            (tree, _) => tree,
        };
        if let Some(tree) = &tree {
            self.animation
                .synchronize(&format!("popup:{id:?}"), popup.tree.as_ref(), tree)?;
        }
        if popup.tree == tree
            && popup.w == w
            && popup.h == h
            && popup.auto_height == auto_height
            && popup.auto_width == auto_width
            && popup.max_width == sw.max(1.0)
            && popup.max_height == sh.max(1.0)
        {
            // Fresh parses mint new callback tokens; keep the live declarations
            // without treating token identity as a layout change.
            popup.on_open = on_open;
            popup.on_close = on_close;
            return Ok(None);
        }
        popup.tree = tree;
        popup.on_open = on_open;
        popup.on_close = on_close;
        popup.w = w;
        popup.h = h;
        popup.auto_height = auto_height;
        popup.auto_width = auto_width;
        popup.max_width = sw.max(1.0);
        popup.max_height = sh.max(1.0);
        popup.sizing_revision += 1;
        Ok(Some(popup.reposition()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_surface_captures_input_without_a_native_popup_grab_or_exclusive_zone() {
        let settings = Hosted::settings(OutputId(7));
        assert_eq!(settings.anchor, Anchor::all());
        assert_eq!(settings.layer, Layer::Overlay);
        assert_eq!(settings.size, LayerSize::FILL);
        assert_eq!(settings.exclusive_zone, Some(-1));
        assert_eq!(settings.output_option, OutputOption::GlobalName(7));
        assert_eq!(
            settings.keyboard_interactivity,
            KeyboardInteractivity::Exclusive
        );
        assert!(!settings.events_transparent);
    }

    #[test]
    fn hosted_placement_follows_all_bar_edges_and_clamps_to_the_output() {
        let viewport = Size::new(1000.0, 600.0);
        let content = Size::new(100.0, 100.0);
        for (edge, bar, anchor, expected) in [
            (
                Anchor::Top,
                Size::new(600.0, 50.0),
                (100, 49),
                Point::new(250.0, 50.0),
            ),
            (
                Anchor::Bottom,
                Size::new(600.0, 50.0),
                (100, 0),
                Point::new(250.0, 450.0),
            ),
            (
                Anchor::Left,
                Size::new(50.0, 400.0),
                (49, 100),
                Point::new(50.0, 150.0),
            ),
            (
                Anchor::Right,
                Size::new(50.0, 400.0),
                (0, 100),
                Point::new(850.0, 150.0),
            ),
            (
                Anchor::Top,
                Size::new(1000.0, 50.0),
                (0, 49),
                Point::new(0.0, 50.0),
            ),
            (
                Anchor::Top,
                Size::new(1000.0, 50.0),
                (999, 49),
                Point::new(900.0, 50.0),
            ),
        ] {
            assert_eq!(
                Hosted::new(anchor).origin(viewport, bar, edge, content),
                expected
            );
        }
    }
}
