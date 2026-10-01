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
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Content inset (px) painted as a transparent gap inside the bar surface.
/// Implemented as widget padding (see `PanelWindow::padding`), so the bar
/// stays edge-pinned and keeps its exclusive zone while the backdrop shrinks.
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
    /// Index into `Config::bar` (`[[bar]]`) for config-spawned bars,
    /// `usize::MAX` for unpersisted temporaries (set on persist).
    pub bar_index: usize,
    pub local: TopLocal,
}

/// Local data for one bar: length %, thickness px, floating + margins, rounding.
/// Plain runtime state on `Top`, deliberately outside `Config` so bars stay
/// independent of the global config file and its hot-reload.
#[derive(Debug, Clone)]
pub struct TopLocal {
    pub length_pct: f32,
    pub thickness_px: f32,
    pub floating: bool,
    /// Inset of the bar backdrop inside its surface (view-live padding).
    pub margins: Margins,
    pub radius: CornerRadius,
}

impl Default for TopLocal {
    fn default() -> Self {
        Self {
            length_pct: 100.0,
            // ~50px, the old fixed thickness.
            thickness_px: 50.0,
            floating: false,
            margins: Margins::default(),
            radius: CornerRadius::default(),
        }
    }
}

impl TopLocal {
    pub(crate) fn px_size(&self, sw: f32, sh: f32, horizontal: bool) -> (u32, u32) {
        let length = self.length_pct.clamp(1.0, 100.0);
        let max_t = Self::max_thickness(sw, sh, horizontal).max(1.0);
        let thick = (self.thickness_px.clamp(1.0, max_t).round() as u32).max(1);
        if horizontal {
            let w = ((sw * length / 100.0).round() as u32).max(1);
            (w, thick)
        } else {
            let h = ((sh * length / 100.0).round() as u32).max(1);
            (thick, h)
        }
    }

    /// Max thickness in px for the given output size + orientation.
    pub(crate) fn max_thickness(sw: f32, sh: f32, horizontal: bool) -> f32 {
        if horizontal { sh } else { sw }
    }
}

impl From<&crate::config::TopConfig> for TopLocal {
    fn from(c: &crate::config::TopConfig) -> Self {
        Self {
            length_pct: c.length,
            thickness_px: c.thickness,
            floating: c.floating,
            margins: Margins {
                top: c.margin_top.max(0),
                right: c.margin_right.max(0),
                bottom: c.margin_bottom.max(0),
                left: c.margin_left.max(0),
            },
            radius: CornerRadius {
                top_left: c.radius_top_left.max(0.0),
                top_right: c.radius_top_right.max(0.0),
                bottom_left: c.radius_bottom_left.max(0.0),
                bottom_right: c.radius_bottom_right.max(0.0),
            },
        }
    }
}

impl Top {
    /// Hold threshold: press held >= this on release counts as hold.
    const HOLD_THRESHOLD: Duration = Duration::from_millis(500);

    pub fn new() -> Self {
        Self {
            anchor: Anchor::Top,
            bar_index: usize::MAX,
            local: TopLocal::default(),
        }
    }

    pub fn with_anchor(anchor: Anchor) -> Self {
        // Length defaults to full, thickness to ~50px, for any edge.
        Self {
            anchor,
            bar_index: usize::MAX,
            local: TopLocal::default(),
        }
    }

    /// Bar seeded from a `[[bar]]` config entry.
    pub fn with_config(bar_index: usize, anchor: Anchor, local: TopLocal) -> Self {
        Self {
            anchor,
            bar_index,
            local,
        }
    }

    /// Parse a config `anchor` value (`top`/`bottom`/`left`/`right`,
    /// case-insensitive). `None` logs nothing — the caller warns.
    pub fn parse_anchor(s: &str) -> Option<Anchor> {
        match s.trim().to_lowercase().as_str() {
            "top" => Some(Anchor::Top),
            "bottom" => Some(Anchor::Bottom),
            "left" => Some(Anchor::Left),
            "right" => Some(Anchor::Right),
            _ => None,
        }
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
            // Visual margins are widget padding (see `view`); the compositor
            // surface stays edge-pinned with no layer offset.
            margin: None,
            namespace: Some(format!("Riced - {} {}", self.anchor_label(), output)),
            blur_option: BlurOption::FullRegion,
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
            // Visual margins are widget padding (see `view`); no layer offset.
            margin: None,
            blur_option: BlurOption::FullRegion,
            ..Default::default()
        };

        (id, settings)
    }

    pub fn view(&self, id: window::Id) -> Element<'_, Plant> {
        let content: Element<'_, Plant> = if self.is_horizontal() {
            row![].width(Fill).height(Fill).into()
        } else {
            column![].width(Fill).height(Fill).into()
        };
        let radius = self.local.radius;
        // Margins inset the backdrop inside the edge-pinned surface
        // (transparent gap, exclusive zone untouched). Gated on floating so
        // an un-floating bar goes back to a full-bleed strip.
        let m = self.local.margins;
        let padding = if self.local.floating {
            iced::Padding {
                top: m.top.max(0) as f32,
                right: m.right.max(0) as f32,
                bottom: m.bottom.max(0) as f32,
                left: m.left.max(0) as f32,
            }
        } else {
            iced::Padding::ZERO
        };
        top_window(id)
            .padding(padding)
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
    /// sliders stay smooth (radius is view-live and needs nothing). Every
    /// edit also stages a coalesced write to the bar's `[[bar]]`
    /// entry, so bars survive restarts.
    pub(crate) fn handle_set_length(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.length_pct = value.clamp(1.0, 100.0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_thickness(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        // Thickness is px, bound to 1..=thin output axis.
        let max = plots
            .ids
            .get(&id)
            .copied()
            .and_then(|info| match info {
                PlotInfo::Top(o) => plots.output_infos.get(&o),
                _ => None,
            })
            .map(|info| {
                let (_, _, sw, sh) = Background::output_geometry(info);
                let horizontal = plots.tops.get(&id).map_or(true, |t| t.is_horizontal());
                TopLocal::max_thickness(sw, sh, horizontal).max(1.0)
            })
            .unwrap_or(1080.0);
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.thickness_px = value.clamp(1.0, max);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_floating(
        plots: &mut Plots,
        id: window::Id,
        value: bool,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.floating = value;
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_margin_top(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.top = value.max(0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_margin_right(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.right = value.max(0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_margin_bottom(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.bottom = value.max(0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_margin_left(
        plots: &mut Plots,
        id: window::Id,
        value: i32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.margins.left = value.max(0);
        }
        Command::batch(vec![
            Self::apply_layout(plots, id),
            Self::persist_bar(plots, id),
        ])
    }

    pub(crate) fn handle_set_radius_tl(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.top_left = value.max(0.0);
        }
        Self::persist_bar(plots, id)
    }

    pub(crate) fn handle_set_radius_tr(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.top_right = value.max(0.0);
        }
        Self::persist_bar(plots, id)
    }

    pub(crate) fn handle_set_radius_bl(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.bottom_left = value.max(0.0);
        }
        Self::persist_bar(plots, id)
    }

    pub(crate) fn handle_set_radius_br(
        plots: &mut Plots,
        id: window::Id,
        value: f32,
    ) -> Command<Plant> {
        if let Some(top) = plots.tops.get_mut(&id) {
            top.local.radius.bottom_right = value.max(0.0);
        }
        Self::persist_bar(plots, id)
    }

    /// Connector name for a bar's output (`""` when unknown/sentinel).
    pub(crate) fn output_name(plots: &Plots, bar_id: window::Id) -> String {
        plots
            .ids
            .get(&bar_id)
            .copied()
            .and_then(|info| match info {
                PlotInfo::Top(o) => plots.output_infos.get(&o),
                _ => None,
            })
            .and_then(|info| info.name.clone())
            .unwrap_or_default()
    }

    /// Write the bar's current state back to its `[[bar]]` entry and
    /// arm a coalesced config save (same idle-write as `ConfigPatch`
    /// drags). Entries are matched by index; out-of-range indices push.
    pub(crate) fn persist_bar(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        let (index, anchor, local) = match plots.tops.get(&bar_id) {
            Some(t) => (
                t.bar_index,
                t.anchor_label().to_lowercase(),
                t.local.clone(),
            ),
            None => return Command::none(),
        };
        let output = Self::output_name(plots, bar_id);
        let entry = crate::config::TopConfig {
            anchor,
            output,
            length: local.length_pct,
            thickness: local.thickness_px,
            floating: local.floating,
            margin_top: local.margins.top,
            margin_right: local.margins.right,
            margin_bottom: local.margins.bottom,
            margin_left: local.margins.left,
            radius_top_left: local.radius.top_left,
            radius_top_right: local.radius.top_right,
            radius_bottom_left: local.radius.bottom_left,
            radius_bottom_right: local.radius.bottom_right,
        };
        if index == usize::MAX || index >= plots.config.bar.len() {
            plots.config.bar.push(entry);
            if let Some(top) = plots.tops.get_mut(&bar_id) {
                top.bar_index = plots.config.bar.len() - 1;
            }
        } else {
            plots.config.bar[index] = entry;
        }
        plots.arm_config_save()
    }

    /// Remove a bar (`TopEvent::Remove`): drop tracking + cursor state,
    /// close its window, and delete its `[[bar]]` entry (persisted
    /// immediately) so it stays gone after restart.
    pub(crate) fn handle_remove(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        plots.last_cursor.remove(&bar_id);
        plots.press_starts.remove(&bar_id);
        if let Some(top) = plots.tops.remove(&bar_id) {
            plots.ids.remove(&bar_id);
            if top.bar_index < plots.config.bar.len() {
                plots.config.bar.remove(top.bar_index);
                // Indices after the hole shift down by one.
                for other in plots.tops.values_mut() {
                    if other.bar_index > top.bar_index {
                        other.bar_index -= 1;
                    }
                }
            }
            // flush_config_save only writes when dirty — mark it first
            // (same for persist_new below).
            plots.config_dirty = true;
            plots.flush_config_save();
        } else {
            plots.ids.remove(&bar_id);
        }
        iced_runtime::task::effect(Action::Window(WindowAction::Close(bar_id)))
    }

    /// Push the bar's current size/exclusive/margins to its live window.
    /// Same window id throughout — no close/reopen flicker. Skips sentinel
    /// windows (fixed fallback until outputs arrive and replace them).
    pub(crate) fn apply_layout(plots: &mut Plots, bar_id: window::Id) -> Command<Plant> {
        // Clamp stale values: length 1–100%, thickness 1..=thin output axis.
        if let Some(top) = plots.tops.get_mut(&bar_id) {
            top.local.length_pct = top.local.length_pct.clamp(1.0, 100.0);
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
        let mut top = top;
        let max = TopLocal::max_thickness(sw, sh, top.is_horizontal()).max(1.0);
        top.local.thickness_px = top.local.thickness_px.clamp(1.0, max);
        if let Some(stored) = plots.tops.get_mut(&bar_id) {
            stored.local.thickness_px = top.local.thickness_px;
            stored.local.length_pct = top.local.length_pct;
        }
        let horizontal = top.is_horizontal();
        let (w, h) = top.local.px_size(sw, sh, horizontal);
        // Margins are widget padding (see `view`): the surface stays
        // edge-pinned with its full exclusive zone. Always clear the
        // compositor-side margins so no stale layer offset lingers.
        let cmds = vec![
            Command::done(Plant::LayoutChange {
                id: bar_id,
                anchor: top.anchor,
                size: LayerSize::px(w, h),
            }),
            Command::done(Plant::ExclusiveZoneChange {
                id: bar_id,
                zone_size: Self::exclusive_px(&top, w, h),
            }),
            Command::done(Plant::MarginChange {
                id: bar_id,
                margin: (0, 0, 0, 0),
            }),
        ];
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
        plots: &mut Plots,
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
        /// Persist a fresh bar as a `[[bar]]` entry right away (a
        /// click, not a drag — no coalescing needed) and record its index.
        fn persist_new(plots: &mut Plots, top: &mut Top, anchor: Anchor, output: String) {
            let l = top.local.clone();
            plots.config.bar.push(crate::config::TopConfig {
                anchor: anchor_name(anchor).to_lowercase(),
                output,
                length: l.length_pct,
                thickness: l.thickness_px,
                floating: l.floating,
                margin_top: l.margins.top,
                margin_right: l.margins.right,
                margin_bottom: l.margins.bottom,
                margin_left: l.margins.left,
                radius_top_left: l.radius.top_left,
                radius_top_right: l.radius.top_right,
                radius_bottom_left: l.radius.bottom_left,
                radius_bottom_right: l.radius.bottom_right,
            });
            top.bar_index = plots.config.bar.len() - 1;
            plots.config_dirty = true;
            plots.flush_config_save();
        }

        let (target_output, target_anchor): (Option<OutputId>, Anchor) = {
            if let Some(output) = menu_output {
                if let Some(info) = plots.output_infos.get(&output) {
                    let anchor = menu_pos.map_or(Anchor::Top, |mp| closest_anchor(mp, info));
                    (Some(output), anchor)
                } else {
                    (Some(output), Anchor::Top)
                }
            } else if let Some(mp) = menu_pos {
                let mut found = plots.output_infos.iter().find(|(_, info)| {
                    let (sx, sy, sw, sh) = Geo::output_geometry(info);
                    mp.x >= sx && mp.x < sx + sw && mp.y >= sy && mp.y < sy + sh
                });
                if found.is_none() && !plots.output_infos.is_empty() {
                    let mut best: Option<(&OutputId, &OutputInfo, f32)> = None;
                    for (oid, info) in &plots.output_infos {
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
            } else if let Some(oid) = plots.output_infos.keys().next().copied() {
                (Some(oid), Anchor::Top)
            } else {
                (None, Anchor::Top)
            }
        };

        if let Some(output_id) = target_output {
            let anchor = target_anchor;
            let duplicate = plots.ids.iter().any(|(wid, info)| match info {
                PlotInfo::Top(o) if *o == output_id => {
                    plots.tops.get(wid).is_some_and(|t| t.anchor() == anchor)
                }
                _ => false,
            });
            if duplicate {
                println!(
                    "Note: {} bar already exists for output {output_id:?}, not spawning another",
                    anchor_name(anchor),
                );
                return None;
            }
            let mut top = Top::with_anchor(anchor);
            let (sw, sh) = Self::output_size(&plots.output_infos, output_id);
            let (w, h) = top.local.px_size(sw, sh, top.is_horizontal());
            let (win_id, settings) = top.open(output_id.0, w, h);
            let output = plots
                .output_infos
                .get(&output_id)
                .and_then(|info| info.name.clone())
                .unwrap_or_default();
            persist_new(plots, &mut top, anchor, output);
            plots.tops.insert(win_id, top);
            plots.ids.insert(win_id, PlotInfo::Top(output_id));
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
            let sentinel = OutputId(u32::MAX);
            let duplicate = plots
                .ids
                .iter()
                .any(|(_, info)| matches!(info, PlotInfo::Top(o) if *o == sentinel));
            if duplicate {
                println!("Note: sentinel bar already exists (no output yet), not spawning another");
                return None;
            }
            let mut top = Top::new();
            let (win_id, settings) = top.open_active();
            persist_new(plots, &mut top, Anchor::Top, String::new());
            plots.tops.insert(win_id, top);
            plots.ids.insert(win_id, PlotInfo::Top(sentinel));
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
