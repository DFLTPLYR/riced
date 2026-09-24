use crate::app::Plant;
use crate::app::app::{PlotInfo, Plots};
use iced::widget::{Space, container, stack, text};
use iced::window;
use iced::{Color, Element, Fill, Length, Point, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::top::Top;
use crate::components::contextmenu::contextmenu;

#[derive(Debug)]
pub struct Background;

/// Mirrors Quickshell QtObject selectionRect.
/// Stored in **global compositor coords** so a single drag can span outputs.
/// All translation goes through [`Background::available_rect`] (the Background
/// window's actual origin/size after Top exclusive zones), never the full
/// output geometry.
#[derive(Debug, Clone, Default)]
pub struct SelectionRect {
    pub start_point: Option<Point>, // global
    pub selecting: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl SelectionRect {
    pub fn reset(&mut self) {
        if self.selecting {
            // onSelectingChanged: if (!selecting) { width=0; height=0; startPoint=null }
            self.selecting = false;
            self.width = 0.0;
            self.height = 0.0;
            self.start_point = None;
            println!("select end -> reset rect (fade 150ms)");
        }
    }

    /// Expand from global start to global current. Returns true if changed >=0.5px.
    pub fn drag_update(&mut self, start: Point, current: Point) -> bool {
        let min_x = start.x.min(current.x);
        let min_y = start.y.min(current.y);
        let max_x = start.x.max(current.x);
        let max_y = start.y.max(current.y);
        if (self.x - min_x).abs() < 0.5
            && (self.y - min_y).abs() < 0.5
            && (self.width - (max_x - min_x)).abs() < 0.5
            && (self.height - (max_y - min_y)).abs() < 0.5
        {
            return false;
        }
        self.x = min_x;
        self.y = min_y;
        self.width = max_x - min_x;
        self.height = max_y - min_y;
        true
    }
}

#[derive(Debug, Clone, Default)]
pub struct ContextMenu {
    pub x: f32, // global
    pub y: f32, // global
    pub open: bool,
    pub output: Option<OutputId>,
}

pub(crate) static LAST_CURSOR_GLOBAL: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<iced::window::Id, Point>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

pub(crate) static SELECTING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

impl Background {
    pub fn open(&self, output: u32) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();
        let settings = NewLayerShellSettings {
            anchor: Anchor::all(),
            layer: Layer::Background,
            exclusive_zone: Some(0),
            size: LayerSize::FILL,
            output_option: OutputOption::GlobalName(output),
            namespace: Some("Riced - Background".to_string()),
            blur_option: BlurOption::None,
            ..Default::default()
        };
        (id, settings)
    }

    /// Full output geometry in global compositor coords (ignores bars).
    pub fn output_geometry(info: &OutputInfo) -> (f32, f32, f32, f32) {
        let (sx, sy) = info
            .logical_position
            .unwrap_or((info.location.0, info.location.1));
        let (sw, sh) = info.logical_size.unwrap_or_else(|| {
            info.modes
                .iter()
                .find(|m| m.current)
                .map(|m| m.dimensions)
                .unwrap_or((1920, 1080))
        });
        (sx as f32, sy as f32, sw as f32, sh as f32)
    }

    /// Background window geometry in global coords: full output minus Top
    /// exclusive zones on that output.
    ///
    /// This is the piece the old global code got wrong: it assumed the
    /// Background window origin == output origin with full height. With a Top
    /// bar on monitor one (exclusive 50), that window is actually at
    /// `(sx, sy + 50)` with height `sh - 50`, so local (0,0) maps to global
    /// `(sx, sy+50)`. Using full geometry shifts x/y and available height.
    /// All `to_global` / `intersects` / view math must use this rect.
    pub fn available_rect(
        output: OutputId,
        output_infos: &HashMap<OutputId, OutputInfo>,
        tops: &HashMap<window::Id, Top>,
        ids: &HashMap<window::Id, PlotInfo>,
    ) -> Option<(f32, f32, f32, f32)> {
        let info = output_infos.get(&output)?;
        let (sx, sy, sw, sh) = Self::output_geometry(info);
        let (left, right, top, bottom) = Top::insets_for_output(tops, ids, output);
        let ax = sx + left;
        let ay = sy + top;
        let aw = (sw - left - right).max(0.0);
        let ah = (sh - top - bottom).max(0.0);
        Some((ax, ay, aw, ah))
    }

    /// Fallback when output info is missing: full-HD at origin.
    fn available_rect_or_fallback(
        output: OutputId,
        output_infos: &HashMap<OutputId, OutputInfo>,
        tops: &HashMap<window::Id, Top>,
        ids: &HashMap<window::Id, PlotInfo>,
    ) -> (f32, f32, f32, f32) {
        Self::available_rect(output, output_infos, tops, ids).unwrap_or((0.0, 0.0, 1920.0, 1080.0))
    }

    /// mapToGlobal: local widget coords (window-relative) -> global compositor
    /// coords, using this Background window's actual origin.
    pub fn to_global(
        id: window::Id,
        local: Point,
        ids: &HashMap<window::Id, PlotInfo>,
        output_infos: &HashMap<OutputId, OutputInfo>,
        tops: &HashMap<window::Id, Top>,
    ) -> Point {
        if let Some(PlotInfo::Background(o)) = ids.get(&id).copied() {
            if let Some((ax, ay, _, _)) = Self::available_rect(o, output_infos, tops, ids) {
                return Point::new(local.x + ax, local.y + ay);
            }
            // output known but no avail (missing info) -> fall back to full geometry
            if let Some(info) = output_infos.get(&o) {
                let (sx, sy, _, _) = Self::output_geometry(info);
                return Point::new(local.x + sx, local.y + sy);
            }
        }
        // Top windows / daemon tiny window: no translation (not used for selection)
        local
    }

    /// Global -> local for a given Background output (for view positioning).
    #[allow(dead_code)]
    pub fn to_local(global: Point, avail: (f32, f32, f32, f32)) -> Point {
        Point::new(global.x - avail.0, global.y - avail.1)
    }

    /// Utils.intersects(a,b) for global rect vs this Background's available rect.
    pub fn intersects(rect: &SelectionRect, avail: (f32, f32, f32, f32)) -> bool {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return false;
        }
        let (sx, sy, sw, sh) = avail;
        let ax2 = rect.x + rect.width;
        let ay2 = rect.y + rect.height;
        let bx2 = sx + sw;
        let by2 = sy + sh;
        !(ax2 <= sx || rect.x >= bx2 || ay2 <= sy || rect.y >= by2)
    }

    /// Ensure a fullscreen Background exists for `output_id` (called on `LandEvent::OutputAdded`).
    /// Returns a `NewLayerShell` command if a new background was created.
    pub(crate) fn ensure_for_output(
        backgrounds: &mut HashMap<OutputId, Background>,
        background_ids: &mut HashMap<OutputId, window::Id>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        output_id: OutputId,
    ) -> Option<Command<Plant>> {
        if backgrounds.contains_key(&output_id) {
            return None;
        }
        let bg = Background;
        let (id, settings) = bg.open(output_id.0);
        backgrounds.insert(output_id, bg);
        background_ids.insert(output_id, id);
        ids.insert(id, PlotInfo::Background(output_id));
        Some(Command::done(Plant::NewLayerShell { settings, id }))
    }

    /// Remove the Background for `output_id` (called on `LandEvent::OutputRemoved`).
    /// Returns the closed window `Id` if one existed.
    pub(crate) fn remove_for_output(
        backgrounds: &mut HashMap<OutputId, Background>,
        background_ids: &mut HashMap<OutputId, window::Id>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        output_id: OutputId,
    ) -> Option<window::Id> {
        let id = background_ids.remove(&output_id)?;
        backgrounds.remove(&output_id);
        ids.remove(&id);
        Some(id)
    }

    // ------------------------------------------------------------------
    // Event handling — moved out of Plots::update so clicks live with the
    // Background they belong to (per `Plant::Graft(id, event)` window id).
    // Plots just delegates: `Background::handle_graft(plots, id, event)`.
    // ------------------------------------------------------------------

    fn last_local(plots: &Plots, id: window::Id) -> Point {
        LAST_CURSOR_GLOBAL
            .lock()
            .unwrap()
            .get(&id)
            .copied()
            .unwrap_or_else(|| {
                plots
                    .last_cursor
                    .get(&id)
                    .copied()
                    .unwrap_or(Point::new(0.0, 0.0))
            })
    }

    /// onPositionChanged equivalent — while selecting, expand the global rect.
    /// `position` is window-local; converted via this Background's available
    /// origin so monitor-one's Top bar offset doesn't leak into monitor two.
    pub(crate) fn handle_cursor_moved(
        plots: &mut Plots,
        id: window::Id,
        position: Point,
    ) -> Command<Plant> {
        plots.last_cursor.insert(id, position);
        if plots.selection_rect.selecting {
            let now = Instant::now();
            if let Some(last) = plots.last_selection_tick {
                if now.duration_since(last) < Duration::from_millis(16) {
                    return Command::none();
                }
            }
            plots.last_selection_tick = Some(now);
            if let Some(sp) = plots.selection_rect.start_point {
                let gp =
                    Self::to_global(id, position, &plots.ids, &plots.output_infos, &plots.tops);
                // skip tiny moves <1px to reduce choppy updates
                if plots.selection_rect.drag_update(sp, gp) {
                    // fall through to SelectionTick redraw
                } else {
                    return Command::none();
                }
            }
            // trigger All redraw via SelectionTick (Graft itself is None to avoid flood)
            return Command::done(Plant::SelectionTick);
        }
        Command::none()
    }

    fn menu_hit_test(plots: &Plots, gp: Point) -> bool {
        let cm = match &plots.context_menu {
            Some(cm) if cm.open => cm,
            _ => return false,
        };
        // find the Background available rect this menu is displayed in
        // (stored output first, else containing available rect)
        let menu_avail =
            cm.output
                .and_then(|o| {
                    Self::available_rect(o, &plots.output_infos, &plots.tops, &plots.ids)
                        .map(|a| (o, a))
                })
                .or_else(|| {
                    plots.output_infos.keys().find_map(|o| {
                        Self::available_rect(*o, &plots.output_infos, &plots.tops, &plots.ids)
                            .and_then(|a| {
                                // menu stored in global coords; check against *full* output
                                // geometry for containment, but clamp/render in available
                                let info = plots.output_infos.get(o)?;
                                let (sx, sy, sw, sh) = Self::output_geometry(info);
                                if cm.x >= sx && cm.x < sx + sw && cm.y >= sy && cm.y < sy + sh {
                                    Some((*o, a))
                                } else {
                                    None
                                }
                            })
                    })
                });
        let (menu_x, menu_y) = if let Some((_, (ax, ay, aw, ah))) = menu_avail {
            let lx = cm.x - ax;
            let ly = cm.y - ay;
            let clamped_lx = lx.clamp(0.0, (aw - plots.config.menu.width).max(0.0));
            let clamped_ly = ly.clamp(0.0, (ah - plots.config.menu.height).max(0.0));
            (ax + clamped_lx, ay + clamped_ly)
        } else {
            (cm.x, cm.y)
        };
        gp.x >= menu_x
            && gp.x <= menu_x + plots.config.menu.width
            && gp.y >= menu_y
            && gp.y <= menu_y + plots.config.menu.height
    }

    fn handle_right_press(plots: &mut Plots, id: window::Id) -> Command<Plant> {
        // only open context menu on Background (like QML Background MouseArea) – ignore Top layer
        if !matches!(plots.id_info(id), Some(PlotInfo::Background(_))) {
            return Command::none();
        }
        let pos = Self::last_local(plots, id);
        let gp = Self::to_global(id, pos, &plots.ids, &plots.output_infos, &plots.tops);
        let output = match plots.id_info(id) {
            Some(PlotInfo::Background(o)) => Some(o),
            _ => None,
        };
        plots.context_menu = Some(ContextMenu {
            x: gp.x,
            y: gp.y,
            open: true,
            output,
        });
        println!("right click context menu at {gp:?} (local {pos:?}) output {output:?}");
        Command::none()
    }

    fn handle_left_press(plots: &mut Plots, id: window::Id) -> Command<Plant> {
        // only start selection on Background (like QML Background MouseArea)
        if !matches!(plots.id_info(id), Some(PlotInfo::Background(_))) {
            return Command::none();
        }
        let pos = Self::last_local(plots, id);
        let gp = Self::to_global(id, pos, &plots.ids, &plots.output_infos, &plots.tops);

        if Self::menu_hit_test(plots, gp) {
            // click was on context menu — suppress selection drag
            return Command::none();
        }
        if let Some(cm) = &mut plots.context_menu {
            if cm.open {
                cm.open = false;
            }
        }

        plots.fade_rect = None;
        plots.fade_start = None;
        plots.selection_rect.selecting = true;
        SELECTING.store(true, std::sync::atomic::Ordering::Relaxed);
        plots.selection_rect.start_point = Some(gp);
        plots.selection_rect.x = gp.x;
        plots.selection_rect.y = gp.y;
        plots.selection_rect.width = 0.0;
        plots.selection_rect.height = 0.0;
        println!("select start {gp:?} (local {pos:?})");
        Command::none()
    }

    fn handle_left_release(plots: &mut Plots) -> Command<Plant> {
        if plots.selection_rect.selecting {
            plots.fade_rect = Some(plots.selection_rect.clone());
            plots.fade_start = Some(Instant::now());
        }
        plots.selection_rect.reset();
        SELECTING.store(false, std::sync::atomic::Ordering::Relaxed);
        Command::none()
    }

    /// Entry point for `Plant::Graft(id, event)` — call from `Plots::update`.
    /// Returns None-equivalent (`Command::none()`) for non-Background windows
    /// except cursor bookkeeping done in `handle_cursor_moved`.
    pub(crate) fn handle_graft(
        plots: &mut Plots,
        id: window::Id,
        event: &iced::Event,
    ) -> Command<Plant> {
        use iced::Event;
        use iced::mouse::Button;

        if let Event::Mouse(iced::mouse::Event::CursorMoved { position }) = event {
            return Self::handle_cursor_moved(plots, id, *position);
        }

        match event {
            Event::Mouse(iced::mouse::Event::ButtonPressed(Button::Right)) => {
                Self::handle_right_press(plots, id)
            }
            Event::Mouse(iced::mouse::Event::ButtonPressed(Button::Left)) => {
                Self::handle_left_press(plots, id)
            }
            Event::Mouse(iced::mouse::Event::ButtonPressed(Button::Middle)) => Command::none(),
            Event::Mouse(iced::mouse::Event::ButtonReleased(Button::Left)) => {
                Self::handle_left_release(plots)
            }
            Event::Mouse(iced::mouse::Event::ButtonReleased(Button::Right)) => Command::none(),
            Event::Mouse(iced::mouse::Event::ButtonReleased(_)) => Command::none(),
            _ => Command::none(),
        }
    }

    // ------------------------------------------------------------------
    // View — selection + context menu overlays, clipped to *this*
    // Background's available rect (not the full output).
    // Call from `Plots::view` via `Background::view_for_output(...)`
    // (which is `Background::open`'s window showing its own relevant part).
    // ------------------------------------------------------------------

    pub(crate) fn view(plots: &Plots, id: window::Id, output: OutputId) -> Element<'_, Plant> {
        let (ax, ay, aw, ah) =
            Self::available_rect_or_fallback(output, &plots.output_infos, &plots.tops, &plots.ids);

        // active rect is either selecting rect or fading rect (150ms InOutQuad)
        let (active_rect, opacity) = if plots.selection_rect.selecting {
            (Some(&plots.selection_rect), 1.0)
        } else if let (Some(fr), Some(start)) = (&plots.fade_rect, &plots.fade_start) {
            let elapsed = start.elapsed().as_millis() as f32;
            if elapsed >= 150.0 {
                (None, 0.0)
            } else {
                let p = elapsed / 150.0;
                let eased = if p < 0.5 {
                    2.0 * p * p
                } else {
                    -1.0 + (4.0 - 2.0 * p) * p
                };
                (Some(fr), 1.0 - eased)
            }
        } else {
            (None, 0.0)
        };

        // intersect global rect with THIS background's available rect,
        // then express in window-local coords for padding
        let (visible, clipped_w, clipped_h, local_x, local_y) = if let Some(ar) = active_rect {
            let avail = (ax, ay, aw, ah);
            let inter = Self::intersects(ar, avail);
            let vis = inter && opacity > 0.01;
            let cw = (ar.x + ar.width).min(ax + aw) - ar.x.max(ax);
            let ch = (ar.y + ar.height).min(ay + ah) - ar.y.max(ay);
            let lx = ar.x.max(ax) - ax;
            let ly = ar.y.max(ay) - ay;
            (vis, cw, ch, lx, ly)
        } else {
            (false, 0.0, 0.0, 0.0, 0.0)
        };

        let (cx, cy, cw, ch, op) = if visible && clipped_w > 0.0 && clipped_h > 0.0 {
            (local_x, local_y, clipped_w, clipped_h, opacity)
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        };
        let selection_overlay: Element<'_, Plant> = if cw > 1.0 && ch > 1.0 && op > 0.01 {
            let bg = Color::from_rgba(0.55, 0.65, 1.0, 0.5 * op);
            let border_col = Color::from_rgba(0.75, 0.8, 1.0, op);
            container(
                container(Space::new())
                    .width(Length::Fixed(cw))
                    .height(Length::Fixed(ch))
                    .style(move |_| container::Style {
                        background: Some(bg.into()),
                        border: iced::Border {
                            color: border_col,
                            width: 1.0,
                            radius: 0.0.into(),
                        },
                        ..Default::default()
                    }),
            )
            .width(Fill)
            .height(Fill)
            .padding(iced::Padding {
                top: cy,
                left: cx,
                right: 0.0,
                bottom: 0.0,
            })
            .into()
        } else {
            container(Space::new()).width(Fill).height(Fill).into()
        };

        let context_menu_overlay: Element<'_, Plant> = if let Some(cm) = &plots.context_menu {
            if cm.open {
                let lx = cm.x - ax;
                let ly = cm.y - ay;
                // only show on the Background whose available rect contains the click
                let in_screen = lx >= 0.0 && ly >= 0.0 && lx < aw && ly < ah;
                if in_screen {
                    let clamped_lx = lx.clamp(0.0, (aw - plots.config.menu.width).max(0.0));
                    let clamped_ly = ly.clamp(0.0, (ah - plots.config.menu.height).max(0.0));
                    contextmenu(plots.config.context_menu.width, clamped_lx, clamped_ly)
                } else {
                    Space::new().width(0).height(0).into()
                }
            } else {
                Space::new().width(0).height(0).into()
            }
        } else {
            Space::new().width(0).height(0).into()
        };

        let cursor = plots.last_cursor.get(&id).copied();
        let sr = &plots.selection_rect;
        let bg_label = if sr.selecting {
            format!(
                "selecting {}x{} at {:.0},{:.0} | avail {:.0},{:.0} {}x{}",
                sr.width as i32, sr.height as i32, sr.x, sr.y, ax, ay, aw as i32, ah as i32
            )
        } else if let Some(p) = cursor {
            format!(
                "BG click+drag  cursor local {p:?} global {:.0},{:.0}",
                ax + p.x,
                ay + p.y
            )
        } else {
            "BG click+drag  (move cursor)  | right click for menu".to_string()
        };
        let bg_view = container(
            text(bg_label)
                .size(13)
                .color(Color::from_rgba(1.0, 1.0, 1.0, 0.7)),
        )
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.10).into()),
            ..Default::default()
        })
        .into();

        stack(vec![bg_view, selection_overlay, context_menu_overlay]).into()
    }
}
