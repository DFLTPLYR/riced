use super::background::Background;
use super::top::{Top, call_lua_text, lua_has_func, publish_system_tables, rich_text};
use crate::app::Plant;
use crate::app::app::{PlotInfo, Plots};
use crate::theme;
use iced::window;
use iced::{Element, Fill, Length, Point, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use iced_wayland_subscriber::OutputId;
use std::collections::HashMap;

/// QML-`Menu`-style popup for widget slots: a fullscreen transparent
/// overlay surface (`Layer::Overlay`, zero exclusive zone so nothing
/// reflows) with the menu box painted at a clamped position. Any click
/// anywhere dismisses it — layer-shell gives the overlay all pointer
/// input while open, which is exactly menu-grab semantics.
///
/// Opened by clicking a slot whose widget defines `popup()` in
/// `widget.lua`; the box renders that text (icons included). Positioning
/// prefers the open side of the bar edge and flips when the box would
/// overflow, clamped into the output.
#[derive(Debug, Clone)]
pub struct Popup {
    /// Own window id (set at open, used for direct dismiss).
    pub win_id: window::Id,
    /// Bar that spawned this popup (toggle handling).
    pub bar_id: window::Id,
    /// Slot index on that bar.
    pub slot: usize,
    /// Widget name whose `popup()` renders the body.
    pub widget: String,
    /// Last rendered `popup()` body.
    pub body: String,
    /// Text size, from the widget def at open time.
    pub size: f32,
    /// Box top-left in output coords.
    pub x: f32,
    pub y: f32,
    /// Box size in px.
    pub w: u32,
    pub h: u32,
}

impl Popup {
    /// Menu box width in px (height derives from the body line count).
    pub(crate) const WIDTH: f32 = 280.0;

    /// Box size for a body: fixed width, height from line count.
    pub(crate) fn size_for(sw: f32, body: &str) -> (u32, u32) {
        let w = sw.clamp(1.0, Self::WIDTH).round() as u32;
        let lines = body.lines().count().max(1) as f32;
        let h = (44.0 + lines * 22.0).clamp(80.0, 420.0).round() as u32;
        (w.max(1), h.max(1))
    }

    /// One axis of menu placement: `first` choice wins when it fits,
    /// otherwise the flipped `second` choice, otherwise `first`
    /// clamped into bounds.
    fn place(total: f32, first: f32, second: f32, size: f32) -> f32 {
        let fits = |p: f32| p >= 0.0 && p + size <= total;
        if fits(first) {
            first
        } else if fits(second) {
            second
        } else {
            first.clamp(0.0, (total - size).max(0.0))
        }
    }

    /// Menu box top-left in output coords. The bar-edge axis opens away
    /// from the bar (`first_side` = top/left edge) and flips when it
    /// would overflow; the cursor axis starts at the pointer and flips
    /// left/up instead. Everything clamps into the output.
    pub(crate) fn box_for(
        output: (f32, f32),
        bar: (f32, f32, f32, f32),
        horizontal: bool,
        first_side: bool,
        cursor: (f32, f32),
        size: (f32, f32),
    ) -> (f32, f32) {
        let (sw, sh) = output;
        let (bx, by, bw, bh) = bar;
        let (w, h) = size;
        let (x, y) = if horizontal {
            let y = if first_side {
                Self::place(sh, by + bh, by - h, h)
            } else {
                Self::place(sh, by - h, by + bh, h)
            };
            (Self::place(sw, cursor.0, cursor.0 - w, w), y)
        } else {
            let x = if first_side {
                Self::place(sw, bx + bw, bx - w, w)
            } else {
                Self::place(sw, bx - w, bx + bw, w)
            };
            (x, Self::place(sh, cursor.1, cursor.1 - h, h))
        };
        (x, y)
    }

    /// Bar origin in output coords. Full-length bars sit in the output
    /// corner for their anchor; partial-length bars are centered along
    /// the edge (common compositor behavior for single-anchor surfaces).
    pub(crate) fn bar_origin(
        anchor: Anchor,
        bw: f32,
        bh: f32,
        sw: f32,
        sh: f32,
        full_length: bool,
    ) -> (f32, f32) {
        let top = anchor == Anchor::Top;
        let bottom = anchor == Anchor::Bottom;
        let left = anchor == Anchor::Left;
        let _right = anchor == Anchor::Right;
        if top || bottom {
            let x = if full_length {
                0.0
            } else {
                ((sw - bw) / 2.0).max(0.0)
            };
            let y = if top { 0.0 } else { (sh - bh).max(0.0) };
            (x, y)
        } else if left {
            let y = if full_length {
                0.0
            } else {
                ((sh - bh) / 2.0).max(0.0)
            };
            (0.0, y)
        } else {
            let y = if full_length {
                0.0
            } else {
                ((sh - bh) / 2.0).max(0.0)
            };
            ((sw - bw).max(0.0), y)
        }
    }

    /// Slot index under a point, or `None` in gaps/outside. `ox/oy/ow/oh`
    /// is the content rect (bar minus floating margins), `n` the slot
    /// count, `gap` the inter-cell spacing.
    pub(crate) fn slot_at_point(
        content: (f32, f32, f32, f32),
        n: usize,
        gap: f32,
        horizontal: bool,
        p: Point,
    ) -> Option<usize> {
        let (ox, oy, ow, oh) = content;
        if n == 0 {
            return None;
        }
        let gap = gap.max(0.0);
        let (u, len) = if horizontal {
            (p.x - ox, ow)
        } else {
            (p.y - oy, oh)
        };
        if u < 0.0 || u >= len {
            return None;
        }
        let cell = (len - gap * (n as f32 - 1.0)) / n as f32;
        if cell <= 0.0 {
            return None;
        }
        let idx = (u / (cell + gap)).floor() as usize;
        if idx >= n {
            return None;
        }
        let within = u - idx as f32 * (cell + gap);
        if within <= cell { Some(idx) } else { None }
    }

    /// Center of one slot in output coords (cursor fallback when no
    /// cursor position is known). Takes the bar rect plus the floating
    /// content insets.
    pub(crate) fn slot_center(
        bar: (f32, f32, f32, f32),
        pads: (f32, f32, f32, f32),
        n: usize,
        gap: f32,
        horizontal: bool,
        pos: usize,
    ) -> Point {
        let (bx, by, bw, bh) = bar;
        let (pad_l, pad_t, pad_r, pad_b) = pads;
        if n == 0 {
            return Point::new(bx + bw / 2.0, by + bh / 2.0);
        }
        let gap = gap.max(0.0);
        let pos = pos.min(n - 1) as f32;
        if horizontal {
            let len = (bw - pad_l - pad_r).max(1.0);
            let cell = ((len - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
            Point::new(bx + pad_l + pos * (cell + gap) + cell / 2.0, by + bh / 2.0)
        } else {
            let len = (bh - pad_t - pad_b).max(1.0);
            let cell = ((len - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
            Point::new(bx + bw / 2.0, by + pad_t + pos * (cell + gap) + cell / 2.0)
        }
    }

    /// GPU busy % from DRM sysfs (AMD + Intel expose `gpu_busy_percent`
    /// per card; NVIDIA needs NVML and reads as unavailable). Busiest
    /// card wins on multi-GPU setups.
    pub(crate) fn gpu_usage_percent() -> Option<f32> {
        Self::gpu_usage_in(std::path::Path::new("/sys/class/drm"))
    }

    pub(crate) fn gpu_usage_in(drm: &std::path::Path) -> Option<f32> {
        std::fs::read_dir(drm)
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    name.strip_prefix("card").is_some_and(|rest| {
                        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                    })
                })
            })
            .filter_map(|entry| {
                std::fs::read_to_string(entry.path().join("device/gpu_busy_percent")).ok()
            })
            .filter_map(|text| text.trim().parse::<f32>().ok())
            .fold(None, |busiest: Option<f32>, usage| {
                Some(busiest.map_or(usage, |peak| peak.max(usage)))
            })
    }

    /// Open a popup for the first `popup()`-capable widget in a slot.
    /// `anchor` is the click point (or slot center) in output coords.
    /// Returns `None` when nothing in the slot can pop up.
    pub(crate) fn open_for(
        plots: &mut Plots,
        bar_id: window::Id,
        output: OutputId,
        pos: usize,
        anchor: Point,
    ) -> Option<Command<Plant>> {
        let top = plots.tops.get(&bar_id)?.clone();
        let names: Vec<String> = top.local.widgets_at(pos).to_vec();
        let name = names
            .iter()
            .find(|w| lua_has_func(&plots.widget_lua, w, "popup"))?
            .clone();
        let body = match plots.widget_lua.get(&name) {
            Some(lua) => match call_lua_text(lua, "popup") {
                Ok(body) => body,
                Err(e) => {
                    Top::note_widget_error(plots, &name, e);
                    return None;
                }
            },
            None => return None,
        };
        let (_, _, sw, sh) = Background::available_rect(output, &plots.output_infos)?;
        let horizontal = top.is_horizontal();
        let (bw, bh) = top.local.px_size(sw, sh, horizontal);
        let (bx, by) = Self::bar_origin(
            top.anchor(),
            bw as f32,
            bh as f32,
            sw,
            sh,
            top.local.length_pct >= 100.0,
        );
        let (w, h) = Self::size_for(sw, &body);
        let size = plots
            .widgets
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.size.max(1.0))
            .unwrap_or(13.0);
        let first_side = top.anchor() == Anchor::Top || top.anchor() == Anchor::Left;
        let (x, y) = Self::box_for(
            (sw, sh),
            (bx, by, bw as f32, bh as f32),
            horizontal,
            first_side,
            (anchor.x, anchor.y),
            (w as f32, h as f32),
        );
        let (win_id, settings) = Self::open_surface(output.0);
        plots.popups.insert(
            win_id,
            Popup {
                win_id,
                bar_id,
                slot: pos,
                widget: name,
                body,
                size,
                x,
                y,
                w,
                h,
            },
        );
        plots.ids.insert(win_id, PlotInfo::Popup(output));
        Some(Command::done(Plant::NewLayerShell {
            settings,
            id: win_id,
        }))
    }

    /// Open a fullscreen transparent overlay for `output`. The surface
    /// reserves nothing; only the menu box paints.
    pub fn open_surface(output: u32) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();
        let settings = NewLayerShellSettings {
            anchor: Anchor::all(),
            layer: Layer::Overlay,
            exclusive_zone: Some(0),
            size: LayerSize::FILL,
            output_option: OutputOption::GlobalName(output),
            margin: None,
            namespace: Some(format!("Riced - Popup {output}")),
            // Like bars: no blur, the box composites cleanly.
            blur_option: BlurOption::None,
            ..Default::default()
        };
        (id, settings)
    }

    pub fn view(&self) -> Element<'static, Plant> {
        use iced::widget::{container, mouse_area};
        let popup_box = container(rich_text(self.body.clone(), self.size, 4.0))
            .width(Length::Fixed(self.w as f32))
            .height(Length::Fixed(self.h as f32))
            .padding(12)
            .style(theme::menu_box);
        let content = container(popup_box)
            .width(Fill)
            .height(Fill)
            .padding(iced::Padding {
                top: self.y.max(0.0),
                left: self.x.max(0.0),
                ..iced::Padding::ZERO
            })
            .align_x(iced::Alignment::Start)
            .align_y(iced::Alignment::Start);
        // Any click anywhere dismisses (menu-grab semantics); the box
        // itself holds no interactive items yet.
        let win_id = self.win_id;
        mouse_area(content)
            .on_press(Plant::TopPlot(crate::app::TopEvent::PopupDismiss(win_id)))
            .on_right_press(Plant::TopPlot(crate::app::TopEvent::PopupDismiss(win_id)))
            .on_middle_press(Plant::TopPlot(crate::app::TopEvent::PopupDismiss(win_id)))
            .into()
    }

    /// Dismiss a popup: drop tracking + cursor state and close its window
    /// (idempotent, like every other layer close).
    pub(crate) fn handle_dismiss(plots: &mut Plots, id: window::Id) -> Command<Plant> {
        plots.last_cursor.remove(&id);
        plots.popups.remove(&id);
        plots.ids.remove(&id);
        iced_runtime::task::effect(Action::Window(WindowAction::Close(id)))
    }

    /// Re-render open popup bodies with live data (called when widget
    /// outputs move, so menus tick too). Bodies that break keep their
    /// last good text; popups whose runtime vanished are dismissed.
    /// Returns close commands for dismissals (possibly none).
    pub(crate) fn refresh_bodies(plots: &mut Plots) -> Command<Plant> {
        let open: Vec<(window::Id, String)> = plots
            .popups
            .iter()
            .map(|(id, popup)| (*id, popup.widget.clone()))
            .collect();
        if open.is_empty() {
            return Command::none();
        }
        plots.sysinfo.refresh_cpu_usage();
        plots.sysinfo.refresh_memory();
        let gpu = Self::gpu_usage_percent();
        let mut cmds = Vec::new();
        for (pid, name) in open {
            let body = match plots.widget_lua.get(&name) {
                Some(lua) => {
                    match publish_system_tables(lua, &plots.sysinfo, gpu)
                        .map_err(|e| e.to_string())
                        .and_then(|()| call_lua_text(lua, "popup"))
                    {
                        Ok(body) => Some(body),
                        Err(e) => {
                            Top::note_widget_error(plots, &name, e);
                            None
                        }
                    }
                }
                None => None,
            };
            match body {
                Some(text) => {
                    if plots.popups.get(&pid).is_some_and(|p| p.body != text)
                        && let Some(popup) = plots.popups.get_mut(&pid)
                    {
                        popup.body = text;
                    }
                }
                None => cmds.push(Self::handle_dismiss(plots, pid)),
            }
        }
        if cmds.is_empty() {
            Command::none()
        } else {
            Command::batch(cmds)
        }
    }

    /// Remove all popups for `output_id` (mirrors `Top::remove_for_output`).
    /// Returns the window ids that were removed.
    pub(crate) fn remove_for_output(
        popups: &mut HashMap<window::Id, Popup>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        output_id: OutputId,
    ) -> Vec<window::Id> {
        let to_remove: Vec<window::Id> = ids
            .iter()
            .filter_map(|(wid, info)| match info {
                PlotInfo::Popup(o) if *o == output_id => Some(*wid),
                _ => None,
            })
            .collect();
        for wid in &to_remove {
            popups.remove(wid);
            ids.remove(wid);
        }
        to_remove
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_at_point_maps_cells_and_gaps() {
        // 3 cells across 300px with 4px gaps: cells are ~97.33px.
        let at = |x: f32| {
            Popup::slot_at_point((0.0, 0.0, 300.0, 50.0), 3, 4.0, true, Point::new(x, 25.0))
        };
        assert_eq!(at(0.0), Some(0));
        assert_eq!(at(50.0), Some(0));
        // Gap between cell 0 and 1 starts at ~97.33.
        assert_eq!(at(97.0), Some(0));
        assert_eq!(at(98.0), None);
        assert_eq!(at(102.0), Some(1));
        assert_eq!(at(299.0), Some(2));
        assert_eq!(at(300.0), None);
        assert_eq!(at(-1.0), None);
        // Vertical bars read y instead.
        let at_y = |y: f32| {
            Popup::slot_at_point((0.0, 0.0, 50.0, 300.0), 3, 4.0, false, Point::new(25.0, y))
        };
        assert_eq!(at_y(10.0), Some(0));
        assert_eq!(at_y(150.0), Some(1));
        assert_eq!(at_y(290.0), Some(2));
        // Zero slots and zero cells never match.
        assert_eq!(
            Popup::slot_at_point(
                (0.0, 0.0, 300.0, 50.0),
                0,
                4.0,
                true,
                Point::new(10.0, 10.0)
            ),
            None
        );
    }

    #[test]
    fn bar_origin_pins_full_length_and_centers_partial() {
        // Full-length bars sit in the output corner for their anchor.
        assert_eq!(
            Popup::bar_origin(Anchor::Top, 1920.0, 50.0, 1920.0, 1080.0, true),
            (0.0, 0.0)
        );
        assert_eq!(
            Popup::bar_origin(Anchor::Bottom, 1920.0, 50.0, 1920.0, 1080.0, true),
            (0.0, 1030.0)
        );
        assert_eq!(
            Popup::bar_origin(Anchor::Left, 50.0, 1080.0, 1920.0, 1080.0, true),
            (0.0, 0.0)
        );
        assert_eq!(
            Popup::bar_origin(Anchor::Right, 50.0, 1080.0, 1920.0, 1080.0, true),
            (1870.0, 0.0)
        );
        // Partial-length bars center along the edge.
        assert_eq!(
            Popup::bar_origin(Anchor::Top, 960.0, 50.0, 1920.0, 1080.0, false),
            (480.0, 0.0)
        );
        assert_eq!(
            Popup::bar_origin(Anchor::Left, 50.0, 540.0, 1920.0, 1080.0, false),
            (0.0, 270.0)
        );
    }

    #[test]
    fn popup_box_prefers_open_side_then_flips_and_clamps() {
        // Top bar: opens below the cursor, shifts left at the right edge.
        assert_eq!(
            Popup::box_for(
                (1920.0, 1080.0),
                (0.0, 0.0, 1920.0, 50.0),
                true,
                true,
                (100.0, 20.0),
                (280.0, 180.0)
            ),
            (100.0, 50.0)
        );
        assert_eq!(
            Popup::box_for(
                (1920.0, 1080.0),
                (0.0, 0.0, 1920.0, 50.0),
                true,
                true,
                (1800.0, 20.0),
                (280.0, 180.0)
            ),
            (1520.0, 50.0)
        );
        // Bottom bar: opens above, flips below when no room.
        assert_eq!(
            Popup::box_for(
                (1920.0, 1080.0),
                (0.0, 1030.0, 1920.0, 50.0),
                true,
                false,
                (100.0, 1060.0),
                (280.0, 180.0)
            ),
            (100.0, 850.0)
        );
        // Left bar: opens to the right of the bar edge.
        assert_eq!(
            Popup::box_for(
                (1920.0, 1080.0),
                (0.0, 0.0, 50.0, 1080.0),
                false,
                true,
                (20.0, 100.0),
                (280.0, 180.0)
            ),
            (50.0, 100.0)
        );
        // Right bar: opens to the left.
        assert_eq!(
            Popup::box_for(
                (1920.0, 1080.0),
                (1870.0, 0.0, 50.0, 1080.0),
                false,
                false,
                (1900.0, 100.0),
                (280.0, 180.0)
            ),
            (1590.0, 100.0)
        );
    }

    #[test]
    fn popup_size_grows_with_body_lines() {
        assert_eq!(Popup::size_for(1920.0, "one"), (280, 80));
        assert_eq!(Popup::size_for(1920.0, "one\ntwo\nthree"), (280, 110));
        // Narrow outputs shrink the box, never below 1px.
        assert_eq!(Popup::size_for(200.0, "one"), (200, 80));
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
