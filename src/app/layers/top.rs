use super::background::Background;
use crate::app::Plant;
use crate::app::app::{PlotInfo, Plots};
use crate::composables::panel_window::top_window;
use iced::mouse::Button;
use iced::widget::{container, text};
use iced::window;
use iced::{Color, Element, Fill, Point, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Top {
    thickness: u32,
    anchor: Anchor,
}

impl Top {
    /// Hold threshold: press held >= this on release counts as hold.
    const HOLD_THRESHOLD: Duration = Duration::from_millis(500);

    pub fn new() -> Self {
        Self {
            thickness: 50,
            anchor: Anchor::Top,
        }
    }

    pub fn with_anchor(anchor: Anchor) -> Self {
        Self {
            thickness: 50,
            anchor,
        }
    }

    pub fn anchor(&self) -> Anchor {
        self.anchor
    }

    pub fn thickness(&self) -> u32 {
        self.thickness
    }

    /// Total reserved insets for `output` from all Top bars on that output.
    /// Returns (left, right, top, bottom) in logical px.
    /// The compositor shrinks Anchor::all() Background windows by these
    /// exclusive zones, so the Background window origin != output origin.
    pub(crate) fn insets_for_output(
        tops: &HashMap<window::Id, Top>,
        ids: &HashMap<window::Id, crate::app::app::PlotInfo>,
        output: OutputId,
    ) -> (f32, f32, f32, f32) {
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        let mut top = 0.0f32;
        let mut bottom = 0.0f32;
        for (wid, info) in ids.iter() {
            match info {
                crate::app::app::PlotInfo::Top(o) if *o == output => {
                    if let Some(t) = tops.get(wid) {
                        let th = t.thickness() as f32;
                        if t.anchor == Anchor::Left {
                            left += th;
                        } else if t.anchor == Anchor::Right {
                            right += th;
                        } else if t.anchor == Anchor::Bottom {
                            bottom += th;
                        } else {
                            // Anchor::Top and any other/combined anchor reserves top
                            // (Top::layer_size only distinguishes Left/Right vs rest)
                            top += th;
                        }
                    }
                }
                _ => {}
            }
        }
        (left, right, top, bottom)
    }

    fn layer_size(&self) -> LayerSize {
        // Top/Bottom span width (fill_width), Left/Right span height (fill_height)
        if self.anchor == Anchor::Left || self.anchor == Anchor::Right {
            LayerSize::fill_height(self.thickness)
        } else {
            LayerSize::fill_width(self.thickness)
        }
    }

    fn anchor_label(&self) -> &'static str {
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

    /// Open a Top bar for a specific output (GlobalName).
    /// Called on `WayEvent::OutputInsert` which fires at startup for each
    /// active output when `StartMode::AllScreens`.
    pub fn open(&self, output: u32) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();

        let settings = NewLayerShellSettings {
            anchor: self.anchor,
            layer: Layer::Top,
            exclusive_zone: Some(self.thickness as i32),
            size: self.layer_size(),
            output_option: OutputOption::GlobalName(output),
            namespace: Some(format!("Riced - {} {}", self.anchor_label(), output)),
            blur_option: BlurOption::FullRegion,
            ..Default::default()
        };

        (id, settings)
    }

    /// Fallback for startup when no OutputId is known yet (uses Active output)
    pub fn open_active(&self) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();

        let settings = NewLayerShellSettings {
            anchor: self.anchor,
            layer: Layer::Top,
            exclusive_zone: Some(self.thickness as i32),
            size: self.layer_size(),
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
        top_window(id)
            .content(
                container(text(format!("{} BAR TEST", self.anchor_label())).size(30))
                    .width(Fill)
                    .height(Fill)
                    .center_x(Fill)
                    .center_y(Fill)
                    .style(|_| container::Style {
                        background: Some(Color::from_rgb(0.10, 0.10, 0.12).into()),
                        ..Default::default()
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

    pub(crate) fn handle_press(plots: &mut Plots, id: window::Id, button: Button) -> Command<Plant> {
        if !matches!(plots.id_info(id), Some(PlotInfo::Top(_))) {
            return Command::none();
        }
        plots.press_starts.insert(id, Instant::now());
        println!("top press {button:?} on {id:?}");
        Command::none()
    }

    pub(crate) fn handle_release(plots: &mut Plots, id: window::Id, button: Button) -> Command<Plant> {
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
            let (win_id, settings) = top.open(output_id.0);
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
