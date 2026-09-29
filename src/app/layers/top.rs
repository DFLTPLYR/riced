use super::background::Background;
use crate::app::Plant;
use crate::app::app::{PlotInfo, Plots};
use crate::composables::panel_window::top_window;
use crate::theme;
use iced::mouse::Button;
use iced::widget::{column, container, row};
use iced::window;
use iced::{Element, Fill, Point, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Layer margins (px). Only applied when the bar floats; a docked bar is
/// edge-pinned by the compositor and margins would fight the exclusive zone.
#[derive(Debug, Clone, Copy, Default)]
pub struct Margins {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

/// Per-corner rounding (px) painted on the bar backdrop.
#[derive(Debug, Clone, Copy, Default)]
pub struct CornerRadius {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_left: f32,
    pub bottom_right: f32,
}

#[derive(Debug, Clone)]
pub struct Top {
    anchor: Anchor,
    /// Local per-bar runtime data. Owned by the bar itself, never bound to
    /// the global `Config` (config.toml only holds theme/panel/menu/wallpaper
    /// settings). Lost on restart — tweaks are session-local.
    pub local: TopLocal,
}

/// Local data for one bar: size %, floating + margins, rounding.
/// Plain runtime state on `Top`, deliberately outside `Config` so bars stay
/// independent of the global config file and its hot-reload.
#[derive(Debug, Clone)]
pub struct TopLocal {
    /// Bar width as % of the output width (1–100, portrait caps at 20).
    pub width_pct: f32,
    /// Bar height as % of the output height (1–100, landscape caps at 20).
    pub height_pct: f32,
    /// Floating bars reserve no exclusive zone (they overlay the wallpaper)
    /// and honor `margins`. Docked bars are edge-pinned and reserve space.
    pub floating: bool,
    pub margins: Margins,
    pub radius: CornerRadius,
}

impl Default for TopLocal {
    fn default() -> Self {
        Self {
            // ~50px at 1080p, the old fixed thickness.
            width_pct: 100.0,
            height_pct: 5.0,
            floating: false,
            margins: Margins::default(),
            radius: CornerRadius::default(),
        }
    }
}

impl TopLocal {
    /// `%` fields resolved against an output size, in px (min 1px).
    pub(crate) fn px_size(&self, sw: f32, sh: f32) -> (u32, u32) {
        let w = ((sw * self.width_pct / 100.0).round() as u32).max(1);
        let h = ((sh * self.height_pct / 100.0).round() as u32).max(1);
        (w, h)
    }
}

impl Top {
    /// Hold threshold: press held >= this on release counts as hold.
    const HOLD_THRESHOLD: Duration = Duration::from_millis(500);

    pub fn new() -> Self {
        Self {
            anchor: Anchor::Top,
            local: TopLocal::default(),
        }
    }

    pub fn with_anchor(anchor: Anchor) -> Self {
        // Side bars default to full height + narrow width (≈50px at 1080p);
        // Top/Bottom bars default to full width + short height.
        let mut local = TopLocal::default();
        if anchor == Anchor::Left || anchor == Anchor::Right {
            local.width_pct = 3.0;
            local.height_pct = 100.0;
        }
        Self { anchor, local }
    }

    pub fn anchor(&self) -> Anchor {
        self.anchor
    }

    pub(crate) fn is_horizontal(&self) -> bool {
        !(self.anchor == Anchor::Left || self.anchor == Anchor::Right)
    }

    fn output_size(output_infos: &HashMap<OutputId, OutputInfo>, output: OutputId) -> (f32, f32) {
        output_infos
            .get(&output)
            .map(|info| {
                let (_, _, sw, sh) = Background::output_geometry(info);
                (sw, sh)
            })
            .unwrap_or((1920.0, 1080.0))
    }

    fn exclusive_px(top: &Top, w: u32, h: u32) -> i32 {
        if top.is_horizontal() {
            h as i32
        } else {
            w as i32
        }
    }

    pub(crate) fn anchor_label(&self) -> &'static str {
        if self.anchor == Anchor::Top {
            "TOP"
        } else if self.anchor == Anchor::Bottom {
            "BOTTOM"
        } else if self.anchor == Anchor::Left {
            "LEFT"
        } else if self.anchor == Anchor::Right {
            "RIGHT"
        } else {
            "BAR"
        }
    }

    /// Open a Top bar for a specific output (GlobalName) at `w`x`h` px
    /// (resolved from the `%` fields against the output size by the caller).
    /// Called on `WayEvent::OutputInsert` which fires at startup for each
    /// active output when `StartMode::AllScreens`.
    pub fn open(&self, output: u32, w: u32, h: u32) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();
        let edge = Self::exclusive_px(self, w, h);

        let settings = NewLayerShellSettings {
            anchor: self.anchor,
            layer: Layer::Top,
            exclusive_zone: Some(edge),
            size: LayerSize::px(w, h),
            output_option: OutputOption::GlobalName(output),
            margin: self.local.floating.then_some((
                self.local.margins.top,
                self.local.margins.right,
                self.local.margins.bottom,
                self.local.margins.left,
            )),
            namespace: Some(format!("Riced - {} {}", self.anchor_label(), output)),
            ..Default::default()
        };

        (id, settings)
    }

    /// Fallback for startup when no OutputId is known yet (uses Active output).
    /// Fixed 50px strip like the old default; replaced once outputs arrive.
    pub fn open_active(&self) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();

        let settings = NewLayerShellSettings {
            anchor: self.anchor,
            layer: Layer::Top,
            exclusive_zone: Some(50),
            size: LayerSize::fill_width(50),
            output_option: OutputOption::Active,
            namespace: Some(format!("Riced - {} Active", self.anchor_label())),
            blur_option: BlurOption::FullRegion,
            ..Default::default()
        };

        (id, settings)
    }

    pub fn view(&self, id: window::Id) -> Element<'_, Plant> {
        // Opaque bar background on purpose: the daemon clears transparent (for
        // the Settings panel's Hyprland blur), so this layer must paint its own
        // backdrop or the wallpaper would show through the bar.
        // Landscape (Top/Bottom) lays out horizontally, portrait
        // (Left/Right) lays out vertically.
        let content: Element<'_, Plant> = if self.is_horizontal() {
            row![].width(Fill).height(Fill).into()
        } else {
            column![].width(Fill).height(Fill).into()
        };
        let radius = self.local.radius;
        top_window(id)
            .content(
                container(content)
                    .width(Fill)
                    .height(Fill)
                    .center_x(Fill)
                    .center_y(Fill)
                    .style(move |theme: &iced::Theme| {
                        let mut s = theme::bar(theme);
                        s.border.radius = iced::border::Radius {
                            top_left: radius.top_left,
                            top_right: radius.top_right,
                            bottom_right: radius.bottom_right,
                            bottom_left: radius.bottom_left,
                        };
                        s
                    }),
            )
            .into()
    }

    // ------------------------------------------------------------------
    // Event handling — press/release arrive via PanelWindow (mouse_area);
    // CursorMoved still arrives via Plant::Graft for cursor bookkeeping.
    // Measures press duration for hold detection: press stores Instant,
    // release compares against HOLD_THRESHOLD and clears the entry.
    // ------------------------------------------------------------------

    fn handle_cursor_moved(plots: &mut Plots, id: window::Id, position: Point) -> Command<Plant> {
        plots.last_cursor.insert(id, position);
        Command::none()
    }

    pub(crate) fn handle_press(
        plots: &mut Plots,
        id: window::Id,
        button: Button,
    ) -> Command<Plant> {
        if !matches!(plots.id_info(id), Some(PlotInfo::Top(_))) {
            return Command::none();
        }
        plots.press_starts.insert(id, Instant::now());
        println!("top press {button:?} on {id:?}");
        Command::none()
    }

    /// Size/float/margin edits apply live to the layer window every tick, so
    /// sliders stay smooth (radius is view-live and needs nothing).
    /// Release does no layout work — persisting is the config's business.
    pub(crate) fn handle_set_width(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            // Portrait bars (Left/Right) are thin: cap width at 20%.
            let max = if top.is_horizontal() { 100.0 } else { 20.0 };
            top.local.width_pct = value.clamp(1.0, max);
        }
        Self::apply_layout(plots, id)
    }

    pub(crate) fn handle_set_height(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            // Landscape bars (Top/Bottom) are thin: cap height at 20%.
            let max = if top.is_horizontal() { 20.0 } else { 100.0 };
            top.local.height_pct = value.clamp(1.0, max);
        }
        Self::apply_layout(plots, id)
    }

    pub(crate) fn handle_set_floating(
        plots: &mut Plots,
        id: window::Id,
        value: bool,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.floating = value;
        }
        Self::apply_layout(plots, id)
    }

    pub(crate) fn handle_set_margin_top(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.top = value.max(0);
        }
        Self::apply_layout(plots, id)
    }

    pub(crate) fn handle_set_margin_right(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.right = value.max(0);
        }
        Self::apply_layout(plots, id)
    }

    pub(crate) fn handle_set_margin_bottom(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.bottom = value.max(0);
        }
        Self::apply_layout(plots, id)
    }

    pub(crate) fn handle_set_margin_left(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.left = value.max(0);
        }
        Self::apply_layout(plots, id)
    }

    pub(crate) fn handle_set_radius_tl(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.top_left = value.max(0.0);
        }
        Command::none()
    }

    pub(crate) fn handle_set_radius_tr(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.top_right = value.max(0.0);
        }
        Command::none()
    }

    pub(crate) fn handle_set_radius_bl(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.bottom_left = value.max(0.0);
        }
        Command::none()
    }

    pub(crate) fn handle_set_radius_br(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.bottom_right = value.max(0.0);
        }
        Command::none()
    }

    /// Push the bar's current size/exclusive/margins to its live window.
    /// Same window id throughout — no close/reopen flicker. Skips sentinel
    /// windows (fixed fallback until outputs arrive and replace them).
    pub(crate) fn apply_layout(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        // Safety net for bars sized before the 20% thin-dimension cap:
        // clamp stale values so the live window never renders oversized.
        if let Some(top) = plots.tops.get_mut(&bar_id) {
            let horizontal = top.is_horizontal();
            let max_w = if horizontal { 100.0 } else { 20.0 };
            let max_h = if horizontal { 20.0 } else { 100.0 };
            top.local.width_pct = top.local.width_pct.clamp(1.0, max_w);
            top.local.height_pct = top.local.height_pct.clamp(1.0, max_h);
        }
        let (output, top) = match plots.ids.get(&bar_id).copied() {
            Some(PlotInfo::Top(o)) => match plots.tops.get(&bar_id).cloned() {
                Some(t) => (o, t),
                None => return Command::none(),
            },
            _ => return Command::none(),
        };
        if output == OutputId(u32::MAX) {
            return Command::none();
        }
        let (sw, sh) = Self::output_size(&plots.output_infos, output);
        let (w, h) = top.local.px_size(sw, sh);
        let mut cmds = vec![
            Command::done(Plant::LayoutChange {
                id: bar_id,
                anchor: top.anchor,
                size: LayerSize::px(w, h),
            }),
            Command::done(Plant::ExclusiveZoneChange {
                id: bar_id,
                zone_size: Self::exclusive_px(&top, w, h),
            }),
        ];
        if top.local.floating {
            let m = top.local.margins;
            cmds.push(Command::done(Plant::MarginChange {
                id: bar_id,
                margin: (m.top, m.right, m.bottom, m.left),
            }));
        }
        Command::batch(cmds)
    }

    /// Re-apply every bar on `output` (resolution/scale changed geometry:
    /// `%` sizes now resolve to different px). Called on `OutputUpdated`.
    pub(crate) fn reapply_for_output(plots: &mut Plots, output: OutputId) -> Command<Plant> {
        let bars: Vec<window::Id> = plots
            .ids
            .iter()
            .filter_map(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == output => Some(*wid),
                _ => None,
            })
            .collect();
        Command::batch(
            bars.into_iter()
                .map(|wid| Self::apply_layout(plots, wid))
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn handle_release(
        plots: &mut Plots,
        id: window::Id,
        button: Button,
    ) -> Command<Plant> {
        if !matches!(plots.id_info(id), Some(PlotInfo::Top(_))) {
            return Command::none();
        }
        let start = plots.press_starts.remove(&id);
        match start {
            Some(t) if t.elapsed() >= Self::HOLD_THRESHOLD => {
                println!("top hold {button:?} on {id:?} after {:?}", t.elapsed());
            }
            Some(t) => {
                println!("top click {button:?} on {id:?} after {:?}", t.elapsed());
            }
            // Silent: the global Graft release safety net in update can clear
            // the press first when release lands on another window, and a
            // same-window release fires both PanelWindow and Graft paths.
            None => {}
        }
        Command::none()
    }

    /// Cursor bookkeeping for Top windows — press/release come from PanelWindow.
    pub(crate) fn handle_graft(
        plots: &mut Plots,
        id: window::Id,
        event: &iced::Event,
    ) -> Command<Plant> {
        if let iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) = event {
            return Self::handle_cursor_moved(plots, id, *position);
        }
        Command::none()
    }

    /// Cleanup sentinel tops (OutputId::MAX) created before outputs were known.
    /// Returns the window ids that were removed (caller should close them and clean last_cursor).
    pub(crate) fn cleanup_sentinels(
        tops: &mut HashMap<window::Id, Top>,
        ids: &mut HashMap<window::Id, PlotInfo>,
    ) -> Vec<window::Id> {
        let sentinel = OutputId(u32::MAX);
        let sentinel_ids: Vec<window::Id> = ids
            .iter()
            .filter_map(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == sentinel => Some(*wid),
                _ => None,
            })
            .collect();
        for id in &sentinel_ids {
            tops.remove(id);
            ids.remove(id);
        }
        sentinel_ids
    }

    /// Remove all tops for `output_id` (supports multiple per output).
    /// Returns the window ids that were removed.
    pub(crate) fn remove_for_output(
        tops: &mut HashMap<window::Id, Top>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        output_id: OutputId,
    ) -> Vec<window::Id> {
        let to_remove: Vec<window::Id> = ids
            .iter()
            .filter_map(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == output_id => Some(*wid),
                _ => None,
            })
            .collect();
        for wid in &to_remove {
            tops.remove(wid);
            ids.remove(wid);
        }
        to_remove
    }

    /// Handle `TopPlot(TopEvent::Sow)` – detect output and closest edge (Left/Right/Top/Bottom) where the
    /// context menu was opened and spawn a new bar there. Returns a `NewLayerShell` command.
    pub(crate) fn handle_add(
        tops: &mut HashMap<window::Id, Top>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        output_infos: &HashMap<OutputId, OutputInfo>,
        menu_pos: Option<Point>,
        menu_output: Option<OutputId>,
    ) -> Option<Command<Plant>> {
        // Geometry helpers live on Background so Top bars and Background
        // windows agree on output coordinates.
        use Background as Geo;
        fn closest_anchor(mp: Point, info: &OutputInfo) -> Anchor {
            let (sx, sy, sw, sh) = Geo::output_geometry(info);
            let left_dist = mp.x - sx;
            let right_dist = (sx + sw) - mp.x;
            let top_dist = mp.y - sy;
            let bottom_dist = (sy + sh) - mp.y;
            let mut best = (top_dist, Anchor::Top);
            if bottom_dist < best.0 {
                best = (bottom_dist, Anchor::Bottom);
            }
            if left_dist < best.0 {
                best = (left_dist, Anchor::Left);
            }
            if right_dist < best.0 {
                best = (right_dist, Anchor::Right);
            }
            best.1
        }
        fn anchor_name(a: Anchor) -> &'static str {
            if a == Anchor::Top {
                "Top"
            } else if a == Anchor::Bottom {
                "Bottom"
            } else if a == Anchor::Left {
                "Left"
            } else if a == Anchor::Right {
                "Right"
            } else {
                "Unknown"
            }
        }

        let (target_output, target_anchor): (Option<OutputId>, Anchor) = {
            if let Some(output) = menu_output {
                if let Some(info) = output_infos.get(&output) {
                    let anchor = menu_pos.map_or(Anchor::Top, |mp| closest_anchor(mp, info));
                    (Some(output), anchor)
                } else {
                    (Some(output), Anchor::Top)
                }
            } else if let Some(mp) = menu_pos {
                let mut found = output_infos.iter().find(|(_, info)| {
                    let (sx, sy, sw, sh) = Geo::output_geometry(info);
                    mp.x >= sx && mp.x < sx + sw && mp.y >= sy && mp.y < sy + sh
                });
                if found.is_none() && !output_infos.is_empty() {
                    let mut best: Option<(&OutputId, &OutputInfo, f32)> = None;
                    for (oid, info) in output_infos {
                        let (sx, sy, sw, sh) = Geo::output_geometry(info);
                        let cx = sx + sw / 2.0;
                        let cy = sy + sh / 2.0;
                        let dx = mp.x - cx;
                        let dy = mp.y - cy;
                        let dist2 = dx * dx + dy * dy;
                        if best.is_none() || dist2 < best.unwrap().2 {
                            best = Some((oid, info, dist2));
                        }
                    }
                    found = best.map(|(oid, info, _)| (oid, info));
                }
                if let Some((oid, info)) = found {
                    (Some(*oid), closest_anchor(mp, info))
                } else {
                    (None, Anchor::Top)
                }
            } else if let Some(oid) = output_infos.keys().next().copied() {
                (Some(oid), Anchor::Top)
            } else {
                (None, Anchor::Top)
            }
        };

        if let Some(output_id) = target_output {
            let anchor = target_anchor;
            let duplicate = ids.iter().any(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == output_id => {
                    tops.get(wid).is_some_and(|t| t.anchor() == anchor)
                }
                _ => false,
            });
            if duplicate {
                println!(
                    "Note: {} bar already exists for output {output_id:?}, spawning another at {:?}",
                    anchor_name(anchor),
                    anchor
                );
            }
            let top = Top::with_anchor(anchor);
            let (sw, sh) = Self::output_size(output_infos, output_id);
            let (w, h) = top.local.px_size(sw, sh);
            let (win_id, settings) = top.open(output_id.0, w, h);
            tops.insert(win_id, top);
            ids.insert(win_id, PlotInfo::Top(output_id));
            println!(
                "Added {} bar for output {output_id:?} window {win_id:?} (closest to {:?} @ {menu_pos:?} stored_output {menu_output:?}) — calling top.open() and spawning NewLayerShell",
                anchor_name(anchor),
                anchor
            );
            Some(Command::done(Plant::NewLayerShell {
                settings,
                id: win_id,
            }))
        } else {
            let top = Top::new();
            let (win_id, settings) = top.open_active();
            let sentinel = OutputId(u32::MAX);
            tops.insert(win_id, top);
            ids.insert(win_id, PlotInfo::Top(sentinel));
            println!(
                "Added sentinel Top window {win_id:?} (no output yet) — calling top.open_active()"
            );
            Some(Command::done(Plant::NewLayerShell {
                settings,
                id: win_id,
            }))
        }
    }
}
