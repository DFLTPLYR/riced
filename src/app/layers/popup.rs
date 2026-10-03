use super::background::Background;
use super::top::{Top, TopLocal, call_lua_text, lua_has_func, publish_system_tables, rich_text};
use crate::app::Plant;
use crate::app::app::{PlotInfo, Plots};
use crate::theme;
use iced::window;
use iced::{Element, Length, Point, Task as Command};
use iced_exwlshell::actions::IcedNewPopupSettings;
use iced_exwlshell::reexport::{
    Anchor, PixelSize, PopupAnchor, PopupConstraintAdjustment, PopupGravity,
};
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use iced_wayland_subscriber::OutputId;
use std::collections::HashMap;

/// QML-`Menu`-style popup for widget slots, backed by a native
/// `xdg_popup` (`Plant::NewPopUp`): an exactly-sized surface anchored
/// to the clicked slot, with the compositor flipping/sliding it into
/// view on overflow. No fullscreen overlay, no cursor math.
///
/// Opened by clicking a slot whose widget defines `popup()` in
/// `widget.lua`; the box renders that text (icons included). Clicking
/// the same slot toggles it; the compositor dismisses on outside
/// clicks (menu-grab semantics).
#[derive(Debug, Clone)]
pub struct Popup {
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

    /// Slot cell rect in bar-local px: `content` is the bar rect minus
    /// floating margins, `n` the slot count, `gap` the inter-cell
    /// spacing. Used as the popup anchor rect.
    pub(crate) fn slot_rect(
        content: (f32, f32, f32, f32),
        n: usize,
        gap: f32,
        horizontal: bool,
        pos: usize,
    ) -> (i32, i32, u32, u32) {
        let (ox, oy, ow, oh) = content;
        if n == 0 {
            return (ox as i32, oy as i32, 1, 1);
        }
        let gap = gap.max(0.0);
        let pos = pos.min(n - 1) as f32;
        if horizontal {
            let cell = ((ow - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
            (
                (ox + pos * (cell + gap)).round() as i32,
                oy.round() as i32,
                cell.round().max(1.0) as u32,
                oh.round().max(1.0) as u32,
            )
        } else {
            let cell = ((oh - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
            (
                ox.round() as i32,
                (oy + pos * (cell + gap)).round() as i32,
                ow.round().max(1.0) as u32,
                cell.round().max(1.0) as u32,
            )
        }
    }

    /// Slot index under a point, or `None` in gaps/outside. `content`
    /// is the bar rect minus floating margins, `n` the slot count,
    /// `gap` the inter-cell spacing. The point is bar-local (cursor
    /// positions already are).
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

    /// Open a popup for the first `popup()`-capable widget in a slot,
    /// anchored to the slot rect with the menu growing off the bar edge
    /// (flipped/slid into view by the compositor on overflow).
    /// Returns `None` when nothing in the slot can pop up.
    pub(crate) fn open_for(
        plots: &mut Plots,
        bar_id: window::Id,
        output: OutputId,
        pos: usize,
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
        let (pl, pt, pr, pb) = if top.local.floating {
            let m = top.local.margins;
            (
                m.left.max(0) as f32,
                m.top.max(0) as f32,
                m.right.max(0) as f32,
                m.bottom.max(0) as f32,
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        let gap = top.local.slot_spacing.clamp(0.0, TopLocal::MAX_SLOT_GAP);
        let n = top.local.slots.clamp(1, TopLocal::MAX_SLOTS) as usize;
        let (rx, ry, rw, rh) = Self::slot_rect(
            (pl, pt, bw as f32 - pl - pr, bh as f32 - pt - pb),
            n,
            gap,
            horizontal,
            pos,
        );
        let (w, h) = Self::size_for(sw, &body);
        let size = PixelSize::try_px(w, h)?;
        let anchor_size = PixelSize::try_px(rw, rh)?;
        let first_side = top.anchor() == Anchor::Top || top.anchor() == Anchor::Left;
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
        let settings = IcedNewPopupSettings::new(bar_id, size, (rx, ry), anchor_size)
            .anchor(anchor)
            .gravity(gravity)
            .constraint_adjustment(
                PopupConstraintAdjustment::FlipX
                    | PopupConstraintAdjustment::FlipY
                    | PopupConstraintAdjustment::SlideX
                    | PopupConstraintAdjustment::SlideY,
            );
        let win_id = window::Id::unique();
        let size_text = plots
            .widgets
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.size.max(1.0))
            .unwrap_or(13.0);
        plots.popups.insert(
            win_id,
            Popup {
                bar_id,
                slot: pos,
                widget: name,
                body,
                size: size_text,
                w,
                h,
            },
        );
        plots.ids.insert(win_id, PlotInfo::Popup(output));
        Some(Command::done(Plant::NewPopUp {
            settings,
            id: win_id,
        }))
    }

    pub fn view(&self) -> Element<'static, Plant> {
        use iced::widget::container;
        container(rich_text(self.body.clone(), self.size, 4.0))
            .width(Length::Fixed(self.w as f32))
            .height(Length::Fixed(self.h as f32))
            .padding(12)
            .style(theme::menu_box)
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
        // Zero slots never match.
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
    fn slot_rect_matches_slot_at_point() {
        // Anchor rects tile the content exactly like hit-testing.
        let content = (0.0, 0.0, 300.0, 50.0);
        for pos in 0..3 {
            let (x, y, w, h) = Popup::slot_rect(content, 3, 4.0, true, pos);
            assert_eq!((y, h), (0, 50));
            let center = Point::new(x as f32 + w as f32 / 2.0, 25.0);
            assert_eq!(
                Popup::slot_at_point(content, 3, 4.0, true, center),
                Some(pos)
            );
        }
        // First cell starts at the content origin with the even share.
        assert_eq!(
            Popup::slot_rect((8.0, 6.0, 300.0, 50.0), 3, 4.0, true, 0),
            (8, 6, 97, 50)
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
