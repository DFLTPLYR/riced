//! Popup geometry, list synchronization, and registration over narrow owners.
use super::{Popup, PopupContent, TopLocal};
use crate::{
    lua::widgets::ResolvedWidget,
    shell::{
        Plant,
        screens::{Background, top::animation::ListContext},
        windows::{PlotInfo, WindowState},
    },
};
use iced::{Task, window};
use iced_exwlshell::{
    actions::IcedNewPopupSettings,
    reexport::{Anchor, PixelSize, PopupAnchor, PopupConstraintAdjustment, PopupGravity},
};
use iced_runtime::{Action, window::Action as WindowAction};
use iced_wayland_subscriber::OutputId;

pub(super) struct SurfaceContext<'a> {
    pub windows: &'a mut WindowState,
    pub animation: ListContext<'a>,
}

pub(super) struct OpenRequest {
    pub bar: window::Id,
    pub output: OutputId,
    pub slot: usize,
    pub cursor: Option<(f32, f32)>,
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
            slot,
            cursor,
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
        let (pl, pt, pr, pb) = if top.local.floating {
            let margins = top.local.margins;
            (
                margins.left.max(0) as f32,
                margins.top.max(0) as f32,
                margins.right.max(0) as f32,
                margins.bottom.max(0) as f32,
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        let gap = top.local.slot_spacing.clamp(0.0, TopLocal::MAX_SLOT_GAP);
        let count = top.local.slots.clamp(1, TopLocal::MAX_SLOTS) as usize;
        let rect = Popup::slot_rect(
            (pl, pt, bw as f32 - pl - pr, bh as f32 - pt - pb),
            count,
            gap,
            horizontal,
            slot,
        );
        let (w, h) = Popup::content_size(sw, sh, &body);
        let Some(size) = PixelSize::try_px(w, h) else {
            return Ok(None);
        };
        let first_side = top.anchor() == Anchor::Top || top.anchor() == Anchor::Left;
        let anchor_at =
            Popup::popup_anchor((bw as f32, bh as f32), horizontal, first_side, rect, cursor);
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
        if let Some(tree) = &body.tree {
            self.animation
                .synchronize(&format!("popup:{id:?}"), None, tree)?;
        }
        self.windows.register_popup(
            output,
            Popup {
                win_id: id,
                bar_id: bar,
                slot,
                placement: resolved.id.clone(),
                body: body.text,
                items: body.items,
                tree: body.tree,
                size: resolved.size.max(1.0),
                w,
                h,
            },
        );
        Ok(Some(Task::done(Plant::NewPopUp { settings, id })))
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
        let (w, h) = Popup::content_size(sw, sh, &content);
        let Some(popup) = self.windows.popups.get_mut(&id) else {
            return Ok(None);
        };
        if let Some(tree) = &content.tree {
            self.animation
                .synchronize(&format!("popup:{id:?}"), popup.tree.as_ref(), tree)?;
        }
        if popup.body == content.text
            && popup.items == content.items
            && popup.tree == content.tree
            && popup.w == w
            && popup.h == h
        {
            return Ok(None);
        }
        popup.body = content.text;
        popup.items = content.items;
        popup.tree = content.tree;
        popup.w = w;
        popup.h = h;
        Ok(Some(iced_runtime::task::effect(Action::Window(
            WindowAction::Resize(id, iced::Size::new(w as f32, h as f32)),
        ))))
    }
}
