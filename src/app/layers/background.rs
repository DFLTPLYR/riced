use crate::app::app::{PlotInfo, Plots};
use crate::app::{BackgroundEvent, ConfigEvent, Plant, TopEvent};
use iced::mouse::Button;
use iced::widget::image::Image;
use iced::widget::{Space, button, column, container, stack, text};
use iced::window;
use iced::{Element, Fill, Length, Point, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;
use std::time::Instant;

use crate::components::contextmenu::contextmenu;
use crate::composables::panel::panel;
use crate::composables::panel_window::background_window;
use crate::theme;

#[derive(Debug)]
pub struct Background;

/// Mirrors Quickshell QtObject selectionRect.
/// Stored in **global compositor coords** so a single drag can span outputs.
/// All translation goes through [`Background::available_rect`] (always the
/// full output — the window ignores exclusive zones), never ad-hoc math.
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
            println!("select end -> reset rect (fade starts)");
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
            exclusive_zone: Some(-1),
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

    /// Background window geometry in global coords: always the full output.
    ///
    /// The window ignores exclusive zones (`-1`, Quickshell
    /// `ExclusionMode.Ignore`), so Top bars never move or shrink it: local
    /// (0,0) is always the output origin and no bar math is needed here.
    /// All `to_global` / `intersects` / view math uses this rect.
    pub fn available_rect(
        output: OutputId,
        output_infos: &HashMap<OutputId, OutputInfo>,
    ) -> Option<(f32, f32, f32, f32)> {
        let info = output_infos.get(&output)?;
        Some(Self::output_geometry(info))
    }

    /// Fallback when output info is missing: full-HD at origin.
    fn available_rect_or_fallback(
        output: OutputId,
        output_infos: &HashMap<OutputId, OutputInfo>,
    ) -> (f32, f32, f32, f32) {
        Self::available_rect(output, output_infos).unwrap_or((0.0, 0.0, 1920.0, 1080.0))
    }

    /// mapToGlobal: local widget coords (window-relative) -> global compositor
    /// coords. The window is fullscreen at the output origin, so this is a
    /// plain output-offset translation.
    pub fn to_global(
        id: window::Id,
        local: Point,
        ids: &HashMap<window::Id, PlotInfo>,
        output_infos: &HashMap<OutputId, OutputInfo>,
    ) -> Point {
        if let Some(PlotInfo::Background(o)) = ids.get(&id).copied() {
            if let Some((ax, ay, _, _)) = Self::available_rect(o, output_infos) {
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

    /// Delayed-heal tick (see `Plots::repaint_burst`): the surface, its
    /// configure, or an image upload wasn't ready when the creation
    /// message's redraw ran, so a timer re-requests a full redraw. Bumps
    /// `repaint_seq` so the heal is a real state transition rather than a
    /// silent no-op; the redraw itself comes from `redraw_scope => Scope::All`.
    pub(crate) fn repaint(plots: &mut Plots) -> Command<Plant> {
        plots.repaint_seq += 1;
        Command::none()
    }

    /// Native "open image" dialog for a new wallpaper. Async via `rfd`
    /// (never blocks the shell); the result returns as
    /// `BackgroundEvent::WallpaperPicked`.
    pub(crate) fn handle_pick_wallpaper() -> Command<Plant> {
        Command::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .set_title("Add wallpaper")
                    .add_filter(
                        "Images",
                        &[
                            "png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "tif", "svg",
                        ],
                    )
                    .pick_file()
                    .await
                    .map(|h| h.path().to_string_lossy().into_owned())
            },
            |picked| Plant::BackgroundPlot(BackgroundEvent::WallpaperPicked(picked)),
        )
    }

    /// Dialog result: accept appends a `[[background.image]]` entry (top of
    /// the `z` stack, reusing the `AddImage` patch path so sync/regen/save
    /// behave like every other image edit); cancel (`None`) is a no-op.
    pub(crate) fn handle_wallpaper_picked(
        plots: &mut Plots,
        picked: Option<String>,
    ) -> Command<Plant> {
        let Some(path) = picked else {
            return Command::none();
        };
        let z = plots
            .config
            .background
            .image
            .iter()
            .map(|img| img.z)
            .max()
            .unwrap_or(-1)
            + 1;
        // Origin placement at native size (`width`/`height` 0): the user
        // drags it into place on the wallpaper map.
        Command::done(Plant::Config(ConfigEvent::Patch(
            crate::config::ConfigPatch::AddImage(crate::config::BackgroundImage {
                path,
                z,
                ..Default::default()
            }),
        )))
    }

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
            plots.last_selection_tick = Some(now);
            if let Some(sp) = plots.selection_rect.start_point {
                let gp = Self::to_global(id, position, &plots.ids, &plots.output_infos);
                // skip tiny moves <1px to reduce choppy updates
                if plots.selection_rect.drag_update(sp, gp) {
                    // fall through to BackgroundPlot::SelectionTick redraw
                } else {
                    return Command::none();
                }
            }
            // trigger All redraw via BackgroundPlot::SelectionTick (Graft itself is None to avoid flood)
            return Command::done(Plant::BackgroundPlot(BackgroundEvent::SelectionTick));
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
        let menu_avail = cm
            .output
            .and_then(|o| Self::available_rect(o, &plots.output_infos).map(|a| (o, a)))
            .or_else(|| {
                plots.output_infos.keys().find_map(|o| {
                    Self::available_rect(*o, &plots.output_infos).and_then(|a| {
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
            let clamped_lx = lx.clamp(0.0, (aw - plots.config.composable.menu.width).max(0.0));
            let clamped_ly = ly.clamp(0.0, (ah - plots.config.composable.menu.height).max(0.0));
            (ax + clamped_lx, ay + clamped_ly)
        } else {
            (cm.x, cm.y)
        };
        gp.x >= menu_x
            && gp.x <= menu_x + plots.config.composable.menu.width
            && gp.y >= menu_y
            && gp.y <= menu_y + plots.config.composable.menu.height
    }

    pub(crate) fn handle_right_press(plots: &mut Plots, id: window::Id) -> Command<Plant> {
        // only open context menu on Background (like QML Background MouseArea) – ignore Top layer
        if !matches!(plots.id_info(id), Some(PlotInfo::Background(_))) {
            return Command::none();
        }
        let pos = Self::last_local(plots, id);
        let gp = Self::to_global(id, pos, &plots.ids, &plots.output_infos);
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

    pub(crate) fn handle_left_press(plots: &mut Plots, id: window::Id) -> Command<Plant> {
        // only start selection on Background (like QML Background MouseArea)
        if !matches!(plots.id_info(id), Some(PlotInfo::Background(_))) {
            return Command::none();
        }
        let pos = Self::last_local(plots, id);
        let gp = Self::to_global(id, pos, &plots.ids, &plots.output_infos);

        if Self::menu_hit_test(plots, gp) {
            // click was on context menu — suppress selection drag
            return Command::none();
        }
        if let Some(cm) = &mut plots.context_menu
            && cm.open
        {
            cm.open = false;
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

    pub(crate) fn handle_left_release(plots: &mut Plots) -> Command<Plant> {
        if plots.selection_rect.selecting {
            plots.fade_rect = Some(plots.selection_rect.clone());
            plots.fade_start = Some(Instant::now());
        }
        plots.selection_rect.reset();
        SELECTING.store(false, std::sync::atomic::Ordering::Relaxed);
        Command::none()
    }

    /// Cursor bookkeeping for Background windows — press/release now arrive
    /// via PanelWindow (mouse_area) as BackgroundEvent::Pressed/Released.
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

    /// Dispatch a PanelWindow button event to the matching press/release handler.
    pub(crate) fn handle_panel_button(
        plots: &mut Plots,
        id: window::Id,
        button: Button,
        pressed: bool,
    ) -> Command<Plant> {
        match (button, pressed) {
            (Button::Right, true) => Self::handle_right_press(plots, id),
            (Button::Left, true) => Self::handle_left_press(plots, id),
            (Button::Left, false) => Self::handle_left_release(plots),
            _ => Command::none(),
        }
    }

    // ------------------------------------------------------------------
    // View — selection + context menu overlays, clipped to *this*
    // Background's available rect (not the full output).
    // Call from `Plots::view` via `Background::view_for_output(...)`
    // (which is `Background::open`'s window showing its own relevant part).
    // ------------------------------------------------------------------

    /// Selection/fade box for *this* Background window, unclamped: full global
    /// size at its window-local offset, surface-clipped by the compositor.
    /// Owns the rect math + styling; `view` just positions it in the stack.
    fn selection_overlay(plots: &Plots, avail: (f32, f32, f32, f32)) -> Element<'_, Plant> {
        let (ax, ay, aw, ah) = avail;
        let fade_ms = plots.config.animation.speed.duration().as_millis() as f32;
        // active rect is either selecting rect or fading rect (speed-scaled InOutQuad)
        let (active_rect, opacity) = if plots.selection_rect.selecting {
            (Some(&plots.selection_rect), 1.0)
        } else if let (Some(fr), Some(start)) = (&plots.fade_rect, &plots.fade_start) {
            let elapsed = start.elapsed().as_millis() as f32;
            if elapsed >= fade_ms {
                (None, 0.0)
            } else {
                let p = elapsed / fade_ms;
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

        // global rect expressed in window-local coords for padding.
        // NOTE: intentionally NOT clamped to this monitor's avail rect —
        // each Background window draws the full rect at its local offset and
        // the surface clips what falls outside, so a drag spanning outputs
        // shows its true size on every monitor.
        let (visible, clipped_w, clipped_h, local_x, local_y) = if let Some(ar) = active_rect {
            let vis =
                opacity > 0.01 && ar.width > 0.0 && ar.height > 0.0 && Self::intersects(ar, avail);
            (vis, ar.width, ar.height, ar.x - ax, ar.y - ay)
        } else {
            (false, 0.0, 0.0, 0.0, 0.0)
        };

        let (cx, cy, cw, ch, op) = if visible && clipped_w > 0.0 && clipped_h > 0.0 {
            (local_x, local_y, clipped_w, clipped_h, opacity)
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        };
        if cw > 1.0 && ch > 1.0 && op > 0.01 {
            // Round only the corners that land inside this output's surface.
            // Corners past the edge would be hard-clipped mid-radius by the
            // compositor (the window can't paint outside itself), which reads
            // as a sliced/broken corner — those go square instead.
            let gx = cx + ax;
            let gy = cy + ay;
            let inside = |px: f32, py: f32| px >= ax && px <= ax + aw && py >= ay && py <= ay + ah;
            let r = 0.0;
            let radius = iced::border::Radius {
                top_left: if inside(gx, gy) { r } else { 0.0 },
                top_right: if inside(gx + cw, gy) { r } else { 0.0 },
                bottom_right: if inside(gx + cw, gy + ch) { r } else { 0.0 },
                bottom_left: if inside(gx, gy + ch) { r } else { 0.0 },
            };
            panel()
                .content(
                    container(Space::new())
                        .width(Length::Fixed(cw))
                        .height(Length::Fixed(ch))
                        .style(theme::selection_box(op, radius)),
                )
                .padding(iced::Padding {
                    top: cy,
                    left: cx,
                    right: 0.0,
                    bottom: 0.0,
                })
                .into()
        } else {
            panel().content(Space::new()).into()
        }
    }

    /// "Add Top" menu for *this* Background's available rect.
    /// Owns clamping + content; `view` just positions it in the stack.
    fn context_menu_overlay(plots: &Plots, avail: (f32, f32, f32, f32)) -> Element<'_, Plant> {
        let (ax, ay, aw, ah) = avail;
        let cm = match &plots.context_menu {
            Some(cm) if cm.open => cm,
            _ => return Space::new().width(0).height(0).into(),
        };
        let lx = cm.x - ax;
        let ly = cm.y - ay;
        // only show on the Background whose available rect contains the click
        let in_screen = lx >= 0.0 && ly >= 0.0 && lx < aw && ly < ah;
        if !in_screen {
            return Space::new().width(0).height(0).into();
        }
        let clamped_lx = lx.clamp(0.0, (aw - plots.config.composable.menu.width).max(0.0));
        let clamped_ly = ly.clamp(0.0, (ah - plots.config.composable.menu.height).max(0.0));
        contextmenu(
            plots.config.composable.context_menu.width,
            clamped_lx,
            clamped_ly,
        )
        .padding(plots.config.composable.context_menu.padding)
        .content(
            column![
                button(text("Add Top").size(12).color(theme::button_text()))
                    .on_press(Plant::TopPlot(TopEvent::Sow))
                    .padding(plots.config.composable.context_menu_item.padding)
                    .style(theme::menu_button(
                        plots.config.composable.context_menu_item.rounding
                    ))
                    .width(Fill),
                button(text("Open Settings").size(12).color(theme::button_text()))
                    .on_press(Plant::Sprout)
                    .padding(plots.config.composable.context_menu_item.padding)
                    .style(theme::menu_button(
                        plots.config.composable.context_menu_item.rounding
                    ))
                    .width(Fill)
            ]
            .spacing(plots.config.composable.context_menu.spacing)
            .width(Fill),
        )
        .into()
    }

    pub(crate) fn view(plots: &Plots, id: window::Id, output: OutputId) -> Element<'_, Plant> {
        let avail = Self::available_rect_or_fallback(output, &plots.output_infos);

        // Bottom of the stack is wallpaper images (QML `Background`), then
        // the debug label, selection, and menu overlays on top.
        let mut layers = Self::wallpaper_views(plots, avail);
        layers.push(Self::selection_overlay(plots, avail));
        layers.push(Self::context_menu_overlay(plots, avail));

        // Content lives in the helpers above; the panel owns Fill + events.
        background_window(id).content(stack(layers)).into()
    }

    /// Wallpaper images cropped to their overlap with THIS output's available
    /// rect, ascending `z` (later paints on top). Each crop is positioned at
    /// its overlap-local offset — always ≥ 0 inside this output — so no
    /// negative-padding tricks are needed and the surface clips the rest.
    fn wallpaper_views(plots: &Plots, avail: (f32, f32, f32, f32)) -> Vec<Element<'_, Plant>> {
        use crate::components::display_map::MapLayer;

        let (ax, ay, _, _) = avail;
        let mut order: Vec<usize> = (0..plots.config.background.image.len()).collect();
        order.sort_by_key(|&i| plots.config.background.image[i].z);
        let mut views = Vec::new();
        for i in order {
            let img = &plots.config.background.image[i];
            // Pre-warmed Handle: file-backed handles decode on a worker whose
            // completion redraw the shell drops, leaving first paint blank.
            let handle = match plots.wallpaper_handle(img) {
                Some(handle) => handle,
                None => continue,
            };
            let (ix, iy, iw, ih) = match MapLayer::resolved(img) {
                Some(rect) => rect,
                None => continue,
            };
            let native = match MapLayer::native_size(img) {
                Some(size) => size,
                None => continue,
            };
            let (ox, oy, ow, oh) = match MapLayer::overlap((ix, iy, iw, ih), avail) {
                Some(overlap) => overlap,
                None => continue,
            };
            let crop = match MapLayer::crop_for((ix, iy, iw, ih), native, (ox, oy, ow, oh)) {
                Some(crop) => crop,
                None => continue,
            };
            views.push(
                container(
                    Image::new(handle)
                        .crop(crop)
                        .width(Length::Fixed(ow))
                        .height(Length::Fixed(oh)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(iced::Padding {
                    top: oy - ay,
                    left: ox - ax,
                    right: 0.0,
                    bottom: 0.0,
                })
                .into(),
            );
        }
        views
    }
}
