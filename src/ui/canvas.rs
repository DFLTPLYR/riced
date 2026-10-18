//! Owned canvas drawings shared by widgets, shell chrome, and the Lua host.
//!
//! A drawing is a plain value (lines + rectangles in canvas-local logical
//! pixels); [`Shape`] is the inert `canvas::Program` that realizes it. Lua
//! authors the geometry, Rust owns painting, matching the rest of the UI IR.
use iced::{
    Color, Point, Size, Theme, mouse,
    widget::canvas::{self, Frame, Geometry, Path, Stroke},
};

/// One stroked polyline in canvas-local logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CanvasLine {
    pub points: Vec<(f32, f32)>,
    pub color: Color,
    pub width: f32,
}

/// One filled axis-aligned rectangle in canvas-local logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CanvasRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Color,
}

/// Owned drawing ops. Rectangles paint first (backgrounds, grids, bands),
/// then lines on top.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CanvasDrawing {
    pub rects: Vec<CanvasRect>,
    pub lines: Vec<CanvasLine>,
}

/// Inert [`canvas::Program`]: draws the owned ops and never captures input,
/// so embedded canvases (bar, popup, desktop) stay click-through to their
/// host shells.
pub(crate) struct Shape {
    pub drawing: CanvasDrawing,
}

impl<Message> canvas::Program<Message> for Shape {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        for rect in &self.drawing.rects {
            if rect.width <= 0.0 || rect.height <= 0.0 {
                continue;
            }
            frame.fill_rectangle(
                Point::new(rect.x, rect.y),
                Size::new(rect.width, rect.height),
                rect.color,
            );
        }
        for line in &self.drawing.lines {
            if line.points.len() < 2 {
                continue;
            }
            let path = Path::new(|builder| {
                let mut points = line.points.iter();
                if let Some(&(x, y)) = points.next() {
                    builder.move_to(Point::new(x, y));
                }
                for &(x, y) in points {
                    builder.line_to(Point::new(x, y));
                }
            });
            frame.stroke(
                &path,
                Stroke::default()
                    .with_color(line.color)
                    .with_width(line.width.max(0.0)),
            );
        }
        vec![frame.into_geometry()]
    }
}
