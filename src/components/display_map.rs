use crate::app::ConfigEvent;
use crate::app::Plant;
use crate::config::{BackgroundImage, ConfigPatch};
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Stroke};
use iced::widget::image::Handle;
use iced::window;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};

/// Interactive map of the monitor layout on a grid (ports the Quickshell
/// Displays `Canvas` + per-screen `Display` delegate + `Flickable` pan/zoom
/// + wallpaper images).
///
/// Rendered as TWO stacked canvases, because `iced_wgpu` draws all quads
/// before all images *within* a layer no matter the `Frame` call order —
/// wallpaper pixels would always cover the output rects. `stack!` pushes a
/// new layer per child, so the outputs canvas paints over the images canvas.
/// Both programs are stateless views of the same [`MapView`], which lives on
/// `Setting` as the single source of truth (canvas `State` can't be shared
/// across the two widgets).
///
/// Owns plain data (global-logical output rects + wallpaper entries) so
/// `draw` needs no borrow of `Plots`: the caller snapshots at view time, and
/// monitor hot-plug / config edits arrive via the next redraw rebuilding the
/// programs.
///
/// Mapping is always uniform (`1:1` at zoom `1.0`), so outputs keep their real
/// relative dimensions. Starts auto-fit; scroll zooms to the cursor,
/// left-drag pans an empty spot or drags the topmost image under it.
#[derive(Debug, Clone)]
pub struct MapLayer {
    kind: LayerKind,
    outputs: Vec<(f32, f32, f32, f32)>,
    images: Vec<BackgroundImage>,
    /// Pre-warmed [`Handle`]s parallel to `images` (`None` = undecodable,
    /// skipped). Served from `Plots::wallpapers` so first paint already has
    /// pixels — file-backed handles would decode on a worker whose completion
    /// redraw the shell drops, leaving first paint blank until interaction.
    handles: Vec<Option<Handle>>,
    view: MapView,
    id: window::Id,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerKind {
    /// Grid + wallpaper pixels + image borders (bottom layer).
    Images,
    /// Output wash + overlap highlights + borders + labels (top layer).
    /// Owns all interaction; the images canvas is inert.
    Outputs,
}

/// Pan/zoom/drag view state, stored on `Setting` and snapshotted into both
/// canvas programs every redraw. `zoom: None` means auto-fit (initial view
/// and after hot-plug); the first interaction pins the fit as manual values.
#[derive(Debug, Clone, Copy, Default)]
pub struct MapView {
    pub zoom: Option<f32>,
    pub off_x: f32,
    pub off_y: f32,
    pub drag: Option<MapDrag>,
}

/// Ephemeral drag in progress. `image: None` means panning the map.
#[derive(Debug, Clone, Copy, Default)]
pub struct MapDrag {
    pub image: Option<usize>,
    pub last: Point,
    pub grab_dx: f32,
    pub grab_dy: f32,
}

/// Base grid step in logical px (QML used 10); multiplied until lines are at
/// least this far apart on screen so deep zoom-outs don't turn to mush.
const GRID_BASE: f32 = 10.0;
const GRID_MIN_PX: f32 = 8.0;
const ZOOM_MIN: f32 = 0.05;
const ZOOM_MAX: f32 = 5.0;

impl MapLayer {
    pub fn images(
        id: window::Id,
        outputs: Vec<(f32, f32, f32, f32)>,
        images: Vec<BackgroundImage>,
        handles: Vec<Option<Handle>>,
        view: MapView,
    ) -> Self {
        Self {
            kind: LayerKind::Images,
            outputs,
            images,
            handles,
            view,
            id,
        }
    }

    pub fn outputs(
        id: window::Id,
        outputs: Vec<(f32, f32, f32, f32)>,
        images: Vec<BackgroundImage>,
        handles: Vec<Option<Handle>>,
        view: MapView,
    ) -> Self {
        Self {
            kind: LayerKind::Outputs,
            outputs,
            images,
            handles,
            view,
            id,
        }
    }

    /// Global bounding box of all outputs. `None` when there are no outputs
    /// yet (settings opened before any `OutputAdded`).
    fn bbox(&self) -> Option<(f32, f32, f32, f32)> {
        let mut it = self.outputs.iter();
        let &(x0, y0, w0, h0) = it.next()?;
        let (mut min_x, mut min_y) = (x0, y0);
        let (mut max_x, mut max_y) = (x0 + w0, y0 + h0);
        for &(x, y, w, h) in it {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + w);
            max_y = max_y.max(y + h);
        }
        Some((
            min_x,
            min_y,
            (max_x - min_x).max(1.0),
            (max_y - min_y).max(1.0),
        ))
    }

    /// Fit-scale + centering for `size` (mirrors QML `contentX/Y` centering).
    fn fit(size: Size, bbox: (f32, f32, f32, f32)) -> (f32, f32, f32) {
        let (bx, by, bw, bh) = bbox;
        if size.width <= 0.0 || size.height <= 0.0 {
            return (1.0, 0.0, 0.0);
        }
        let zoom = ((size.width / bw).min(size.height / bh) * 0.9).max(0.001);
        let off_x = size.width / 2.0 - (bx + bw / 2.0) * zoom;
        let off_y = size.height / 2.0 - (by + bh / 2.0) * zoom;
        (zoom, off_x, off_y)
    }

    /// Effective `(zoom, off_x, off_y)`: manual values once the user has
    /// interacted, otherwise auto-fit.
    fn effective(
        size: Size,
        bbox: Option<(f32, f32, f32, f32)>,
        view: &MapView,
    ) -> (f32, f32, f32) {
        match view.zoom {
            Some(zoom) => (zoom, view.off_x, view.off_y),
            None => bbox
                .map(|bbox| Self::fit(size, bbox))
                .unwrap_or((1.0, 0.0, 0.0)),
        }
    }

    /// Pin the current auto-fit as manual values so interaction has a base.
    fn pinned(&self, size: Size, view: MapView) -> MapView {
        if view.zoom.is_some() {
            return view;
        }
        let (zoom, off_x, off_y) = Self::effective(size, self.bbox(), &view);
        MapView {
            zoom: Some(zoom),
            off_x,
            off_y,
            drag: view.drag,
        }
    }

    /// Native size: stored `width`/`height`, or a header-only lookup when
    /// both are `0` (mirrors the QML `width = sourceSize.width` default).
    /// `None` for empty/unreadable paths or degenerate sizes.
    pub(crate) fn native_size(img: &BackgroundImage) -> Option<(f32, f32)> {
        if img.width > 0.0 && img.height > 0.0 {
            return Some((img.width, img.height));
        }
        let path = img.local_path();
        if path.as_os_str().is_empty() {
            return None;
        }
        let (w, h) = image::image_dimensions(&path).ok()?;
        (w > 0 && h > 0).then_some((w as f32, h as f32))
    }

    /// Concrete global rect for an image: native size × `scale` at `(x, y)`.
    /// `None` for empty/unreadable paths or degenerate sizes (skipped in draw
    /// + hits). Shared with the Background layer so map preview and applied
    /// wallpaper agree on placement.
    pub(crate) fn resolved(img: &BackgroundImage) -> Option<(f32, f32, f32, f32)> {
        let path = img.local_path();
        if path.as_os_str().is_empty() {
            return None;
        }
        let (w, h) = Self::native_size(img)?;
        let s = img.scale.max(0.01);
        Some((img.x, img.y, w * s, h * s))
    }

    /// Overlap of two global rects, or `None` when disjoint. Used to preview
    /// each image's intersection with every output (same shape as
    /// `Background::intersects`, but returning the overlap instead of a bool),
    /// and by the Background layer to crop wallpapers per output.
    pub(crate) fn overlap(
        a: (f32, f32, f32, f32),
        b: (f32, f32, f32, f32),
    ) -> Option<(f32, f32, f32, f32)> {
        let x = a.0.max(b.0);
        let y = a.1.max(b.1);
        let x2 = (a.0 + a.2).min(b.0 + b.2);
        let y2 = (a.1 + a.3).min(b.1 + b.3);
        (x2 > x && y2 > y).then_some((x, y, x2 - x, y2 - y))
    }

    /// Source-pixel crop for an image's overlap with one output: maps the
    /// global overlap rect back through placement (`x`, `y`, `scale`) into
    /// native pixels, so the Background layer renders exactly the visible
    /// part. `None` when the image misses the output entirely.
    /// Pure math over the already-resolved rect — unit-tested below.
    pub(crate) fn crop_for(
        img_rect: (f32, f32, f32, f32),
        native: (f32, f32),
        overlap: (f32, f32, f32, f32),
    ) -> Option<iced::Rectangle<u32>> {
        let (ix, iy, iw, ih) = img_rect;
        let (nw, nh) = native;
        if iw <= 0.0 || ih <= 0.0 || nw <= 0.0 || nh <= 0.0 {
            return None;
        }
        let (ox, oy, ow, oh) = overlap;
        let sx = ((ox - ix) / (iw) * nw).max(0.0);
        let sy = ((oy - iy) / (ih) * nh).max(0.0);
        let sw = (ow / iw * nw).max(1.0);
        let sh = (oh / ih * nh).max(1.0);
        Some(iced::Rectangle {
            x: sx as u32,
            y: sy as u32,
            width: sw as u32,
            height: sh as u32,
        })
    }
    /// Config index of the topmost image under global point `g`, or `None`.
    /// Paint order is ascending `z`, so hits resolve in reverse. Outputs never
    /// participate: presses pass through output rects to whatever is beneath.
    fn hit(&self, g: (f32, f32)) -> Option<usize> {
        let mut order: Vec<usize> = (0..self.images.len()).collect();
        order.sort_by_key(|&i| self.images[i].z);
        order.into_iter().rev().find_map(|i| {
            let (x, y, w, h) = Self::resolved(&self.images[i])?;
            (g.0 >= x && g.0 < x + w && g.1 >= y && g.1 < y + h).then_some(i)
        })
    }
}

impl canvas::Program<Plant, Theme, Renderer> for MapLayer {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Plant>> {
        // Only the top (outputs) layer interacts; the images canvas is inert.
        if self.kind != LayerKind::Outputs {
            return None;
        }
        let select = |view: MapView| {
            Plant::SettingPlot(crate::app::SettingEvent::MapViewChanged { id: self.id, view })
        };
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let pos = cursor.position_in(bounds)?;
                let mut view = self.pinned(bounds.size(), self.view);
                let zoom = view.zoom.unwrap_or(1.0);
                let g = ((pos.x - view.off_x) / zoom, (pos.y - view.off_y) / zoom);
                view.drag = Some(if let Some(i) = self.hit(g) {
                    // Grab the image with its offset so the drag doesn't snap.
                    let (x, y, _, _) = Self::resolved(&self.images[i])?;
                    MapDrag {
                        image: Some(i),
                        last: pos,
                        grab_dx: g.0 - x,
                        grab_dy: g.1 - y,
                    }
                } else {
                    MapDrag {
                        image: None,
                        last: pos,
                        grab_dx: 0.0,
                        grab_dy: 0.0,
                    }
                });
                Some(Action::publish(select(view)).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if self.view.drag.is_none() {
                    return None;
                }
                let mut view = self.view;
                view.drag = None;
                Some(Action::publish(select(view)).and_capture())
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                // Release outside the canvas never reaches us; don't jump on
                // re-entry with a stale drag anchor.
                if self.view.drag.is_none() {
                    return None;
                }
                let mut view = self.view;
                view.drag = None;
                Some(Action::publish(select(view)))
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let mut view = self.view;
                let mut drag = view.drag?;
                let pos = cursor.position_in(bounds)?;
                if let Some(i) = drag.image {
                    // Drag the image: publish absolute global coords, which
                    // persist via the normal Patch → save → redraw broadcast.
                    let zoom = view.zoom.unwrap_or(1.0);
                    let x = (pos.x - view.off_x) / zoom - drag.grab_dx;
                    let y = (pos.y - view.off_y) / zoom - drag.grab_dy;
                    return Some(
                        Action::publish(Plant::Config(ConfigEvent::Patch(
                            ConfigPatch::MoveImage { index: i, x, y },
                        )))
                        .and_capture(),
                    );
                }
                // Canvas-local delta pans 1:1 with the pointer.
                view.off_x += pos.x - drag.last.x;
                view.off_y += pos.y - drag.last.y;
                drag.last = pos;
                view.drag = Some(drag);
                Some(Action::publish(select(view)).and_capture())
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let pos = cursor.position_in(bounds)?;
                let mut view = self.pinned(bounds.size(), self.view);
                let zoom = view.zoom.unwrap_or(1.0);
                let lines = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => y * 24.0,
                    mouse::ScrollDelta::Pixels { y, .. } => y,
                };
                let factor = (1.0 + lines * 0.002).clamp(0.2, 5.0);
                let next = (zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
                // Zoom to cursor: keep the global point under it fixed.
                let global = (pos.x - view.off_x) / zoom;
                view.off_x = pos.x - global * next;
                let global_y = (pos.y - view.off_y) / zoom;
                view.off_y = pos.y - global_y * next;
                view.zoom = Some(next);
                Some(Action::publish(select(view)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());

        if self.bbox().is_none() {
            return vec![frame.into_geometry()];
        }
        let (zoom, off_x, off_y) = Self::effective(bounds.size(), self.bbox(), &self.view);
        let local = |x: f32, y: f32| Point::new(x * zoom + off_x, y * zoom + off_y);

        match self.kind {
            LayerKind::Images => {
                // Full-span gridlines in content space (like the QML grid):
                // every line crossing the canvas is drawn edge to edge, so
                // they read as a real grid at any pan/zoom. Step grows until
                // lines are ≥8px apart.
                let mut step = GRID_BASE;
                while step * zoom < GRID_MIN_PX {
                    step *= 10.0;
                }
                let grid = Color::from_rgba(1.0, 1.0, 1.0, 0.35);
                let k_min_x = ((0.0 - off_x) / zoom / step).floor() as i32;
                let k_max_x = ((bounds.width - off_x) / zoom / step).ceil() as i32;
                for k in k_min_x..=k_max_x {
                    let x = (k as f32 * step * zoom + off_x).round() + 0.5;
                    frame.fill_rectangle(Point::new(x, 0.0), Size::new(1.0, bounds.height), grid);
                }
                let k_min_y = ((0.0 - off_y) / zoom / step).floor() as i32;
                let k_max_y = ((bounds.height - off_y) / zoom / step).ceil() as i32;
                for k in k_min_y..=k_max_y {
                    let y = (k as f32 * step * zoom + off_y).round() + 0.5;
                    frame.fill_rectangle(Point::new(0.0, y), Size::new(bounds.width, 1.0), grid);
                }

                // Wallpaper pixels, ascending `z`, each with a thin border.
                let mut order: Vec<usize> = (0..self.images.len()).collect();
                order.sort_by_key(|&i| self.images[i].z);
                for i in order {
                    let img = &self.images[i];
                    let Some((x, y, w, h)) = Self::resolved(img) else {
                        continue;
                    };
                    let Some(handle) = self.handles.get(i).and_then(|h| h.clone()) else {
                        continue;
                    };
                    let top_left = local(x, y);
                    let size = Size::new(w * zoom, h * zoom);
                    frame.draw_image(Rectangle::new(top_left, size), &handle);
                    frame.stroke_rectangle(
                        top_left,
                        size,
                        Stroke::default()
                            .with_width(1.0)
                            .with_color(Color::from_rgb(0.5, 0.5, 0.55)),
                    );
                }
            }
            LayerKind::Outputs => {
                // Output overlay: faint wash, then each image's intersection
                // with every output highlighted, then crisp borders + labels.
                for (x, y, w, h) in &self.outputs {
                    let top_left = local(*x, *y);
                    let size = Size::new((*w * zoom).max(2.0), (*h * zoom).max(2.0));
                    frame.fill_rectangle(top_left, size, Color::from_rgba(0.55, 0.65, 1.0, 0.06));
                    for img in &self.images {
                        if let Some((ix, iy, iw, ih)) = Self::resolved(img) {
                            for output in &self.outputs {
                                if let Some((ox, oy, ow, oh)) =
                                    Self::overlap((ix, iy, iw, ih), *output)
                                {
                                    let otl = local(ox, oy);
                                    let osize = Size::new(ow * zoom, oh * zoom);
                                    frame.fill_rectangle(
                                        otl,
                                        osize,
                                        Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                                    );
                                }
                            }
                        }
                    }
                    frame.stroke_rectangle(
                        top_left,
                        size,
                        Stroke::default()
                            .with_width(1.5)
                            .with_color(Color::from_rgb(0.75, 0.8, 1.0)),
                    );
                    frame.fill_text(canvas::Text {
                        content: format!("{}x{}", *w as i32, *h as i32),
                        position: Point::new(top_left.x + 6.0, top_left.y + 4.0),
                        color: Color::from_rgba(1.0, 1.0, 1.0, 0.8),
                        size: 12.0.into(),
                        max_width: (size.width - 12.0).max(0.0),
                        ..Default::default()
                    });
                }
            }
        }

        vec![frame.into_geometry()]
    }
}

/// Bottom canvas: grid + wallpaper pixels. Inert (no `update` handling).
pub fn images_layer(
    id: window::Id,
    outputs: Vec<(f32, f32, f32, f32)>,
    images: Vec<BackgroundImage>,
    handles: Vec<Option<Handle>>,
    view: MapView,
) -> MapLayer {
    MapLayer::images(id, outputs, images, handles, view)
}

/// Top canvas: output overlay + all map interaction.
pub fn outputs_layer(
    id: window::Id,
    outputs: Vec<(f32, f32, f32, f32)>,
    images: Vec<BackgroundImage>,
    handles: Vec<Option<Handle>>,
    view: MapView,
) -> MapLayer {
    MapLayer::outputs(id, outputs, images, handles, view)
}

impl From<MapLayer> for Element<'_, Plant> {
    fn from(layer: MapLayer) -> Self {
        iced::widget::Canvas::new(layer)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs() -> Vec<(f32, f32, f32, f32)> {
        vec![(0.0, 0.0, 1920.0, 1080.0), (1920.0, -100.0, 1280.0, 1024.0)]
    }

    fn layer() -> MapLayer {
        MapLayer::outputs(
            window::Id::unique(),
            outputs(),
            vec![],
            vec![],
            MapView::default(),
        )
    }

    #[test]
    fn empty_has_no_bbox() {
        let map = MapLayer::outputs(
            window::Id::unique(),
            vec![],
            vec![],
            vec![],
            MapView::default(),
        );
        assert!(map.bbox().is_none());
    }

    #[test]
    fn bbox_spans_all_outputs() {
        assert_eq!(layer().bbox(), Some((0.0, -100.0, 3200.0, 1180.0)));
    }

    #[test]
    fn fit_centers_bbox_uniformly() {
        // Two 1080p monitors side by side in an 800x600 canvas: width-limited.
        let map = MapLayer::outputs(
            window::Id::unique(),
            vec![(0.0, 0.0, 1920.0, 1080.0), (1920.0, 0.0, 1920.0, 1080.0)],
            vec![],
            vec![],
            MapView::default(),
        );
        let bbox = map.bbox().unwrap();
        let view = MapView::default();
        let (zoom, off_x, off_y) = MapLayer::effective(Size::new(800.0, 600.0), Some(bbox), &view);
        assert!((zoom - 800.0 / 3840.0 * 0.9).abs() < 1e-6);
        // Same zoom on both axes (1:1 dimensions) and bbox center on canvas center.
        assert!((1920.0 * zoom + off_x - 400.0).abs() < 1e-4);
        assert!((540.0 * zoom + off_y - 300.0).abs() < 1e-4);
    }

    #[test]
    fn manual_view_overrides_fit() {
        let map = layer();
        let size = Size::new(800.0, 600.0);
        let view = MapView {
            zoom: Some(1.0),
            off_x: 10.0,
            off_y: 20.0,
            drag: None,
        };
        assert_eq!(
            MapLayer::effective(size, map.bbox(), &view),
            (1.0, 10.0, 20.0)
        );
        // Untouched view falls back to fit.
        let (zoom, _, _) = MapLayer::effective(size, map.bbox(), &MapView::default());
        assert!((zoom - 800.0 / 3200.0 * 0.9).abs() < 1e-6);
    }

    #[test]
    fn overlap_returns_intersection() {
        assert_eq!(
            MapLayer::overlap((0.0, 0.0, 100.0, 100.0), (50.0, 50.0, 100.0, 100.0)),
            Some((50.0, 50.0, 50.0, 50.0))
        );
        assert_eq!(
            MapLayer::overlap((0.0, 0.0, 100.0, 100.0), (200.0, 200.0, 50.0, 50.0)),
            None
        );
        // touching edges don't overlap
        assert_eq!(
            MapLayer::overlap((0.0, 0.0, 100.0, 100.0), (100.0, 0.0, 50.0, 50.0)),
            None
        );
    }

    #[test]
    fn hit_resolves_topmost_by_z() {
        let map = MapLayer::outputs(
            window::Id::unique(),
            vec![],
            vec![
                BackgroundImage {
                    path: "/a.png".into(),
                    x: 0.0,
                    y: 0.0,
                    z: 0,
                    width: 100.0,
                    height: 100.0,
                    ..Default::default()
                },
                BackgroundImage {
                    path: "/b.png".into(),
                    x: 50.0,
                    y: 50.0,
                    z: 1,
                    width: 100.0,
                    height: 100.0,
                    ..Default::default()
                },
            ],
            vec![],
            MapView::default(),
        );
        // shared area → higher z wins; exclusive area → its owner
        assert_eq!(map.hit((75.0, 75.0)), Some(1));
        assert_eq!(map.hit((10.0, 10.0)), Some(0));
        assert_eq!(map.hit((500.0, 500.0)), None);
    }

    #[test]
    fn crop_maps_overlap_to_source_pixels() {
        // 100x100 native image at (0,0) scale 1; output covers right half.
        let crop = MapLayer::crop_for(
            (0.0, 0.0, 100.0, 100.0),
            (100.0, 100.0),
            (50.0, 0.0, 50.0, 100.0),
        )
        .unwrap();
        assert_eq!((crop.x, crop.y, crop.width, crop.height), (50, 0, 50, 100));

        // 2x scale: global rect is 200x200 of the same 100px source.
        let crop = MapLayer::crop_for(
            (0.0, 0.0, 200.0, 200.0),
            (100.0, 100.0),
            (0.0, 0.0, 200.0, 200.0),
        )
        .unwrap();
        assert_eq!((crop.x, crop.y, crop.width, crop.height), (0, 0, 100, 100));

        // degenerate inputs → None, never panics
        assert!(
            MapLayer::crop_for(
                (0.0, 0.0, 0.0, 100.0),
                (100.0, 100.0),
                (0.0, 0.0, 10.0, 10.0)
            )
            .is_none()
        );
    }

    #[test]
    fn resolved_skips_empty_paths() {
        assert!(
            MapLayer::resolved(&BackgroundImage {
                path: String::new(),
                ..Default::default()
            })
            .is_none()
        );
    }
}
