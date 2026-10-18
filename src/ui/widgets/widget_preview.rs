//! Inert native widget content fitted into a picker, with inverse-mapped grabs.
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, renderer, widget};
use iced::{Element, Event, Length, Point, Rectangle, Size, Transformation};
use std::sync::Arc;

pub(crate) fn preview<M: Clone + 'static>(
    content: Element<'static, M>,
    native_limit: Size,
    pick: Arc<dyn Fn(Point) -> M + Send + Sync>,
) -> Element<'static, M> {
    Element::new(Preview {
        content,
        native_limit,
        pick,
    })
}

struct Preview<M> {
    content: Element<'static, M>,
    native_limit: Size,
    pick: Arc<dyn Fn(Point) -> M + Send + Sync>,
}

struct State<M> {
    pressed: Option<(M, Size)>,
}

fn fitted(canvas: Rectangle, native: Size) -> (f32, Rectangle) {
    let scale = (canvas.width / native.width.max(1.0))
        .min(canvas.height / native.height.max(1.0))
        .min(1.0);
    let size = Size::new(native.width * scale, native.height * scale);
    (
        scale,
        Rectangle::new(
            Point::new(
                canvas.x + (canvas.width - size.width) / 2.0,
                canvas.y + (canvas.height - size.height) / 2.0,
            ),
            size,
        ),
    )
}

fn grab(
    canvas: Rectangle,
    native: Size,
    cursor: mouse::Cursor,
    viewport: Rectangle,
) -> Option<Point> {
    if !native.width.is_finite()
        || !native.height.is_finite()
        || native.width <= 0.0
        || native.height <= 0.0
    {
        return None;
    }
    let (scale, shown) = fitted(canvas, native);
    if scale <= 0.0 || !cursor.is_over(viewport) {
        return None;
    }
    cursor
        .position_in(shown)
        .map(|point| Point::new(point.x / scale, point.y / scale))
}

impl<M: Clone + 'static> Widget<M, iced::Theme, iced::Renderer> for Preview<M> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State<M>>()
    }
    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::<M> { pressed: None })
    }
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(320.0), Length::Fixed(180.0))
    }
    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child = self.content.as_widget_mut().layout(
            &mut tree.children[0],
            renderer,
            &layout::Limits::new(Size::ZERO, self.native_limit),
        );
        let size = limits.resolve(Length::Fixed(320.0), Length::Fixed(180.0), Size::ZERO);
        layout::Node::with_children(size, vec![child])
    }
    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        // Never forward events to embedded controls or Lua action callbacks.
        if matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            // Native spinners and control visual caches still receive frames.
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout.child(0),
                mouse::Cursor::Unavailable,
                renderer,
                clipboard,
                shell,
                &layout.child(0).bounds(),
            );
        }
        let native = layout.child(0).bounds().size();
        let point = grab(layout.bounds(), native, cursor, *viewport);
        let state = tree.state.downcast_mut::<State<M>>();
        if point.is_none() {
            state.pressed = None;
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                state.pressed = point.map(|point| ((self.pick)(point), native));
                if point.is_some() {
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let Some((message, size)) = state.pressed.take()
                    && point.is_some()
                    && native == size
                {
                    shell.publish(message);
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorLeft) => state.pressed = None,
            _ => {}
        }
    }
    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let child = layout.child(0);
        let (scale, shown) = fitted(layout.bounds(), child.bounds().size());
        let Some(clip) = shown.intersection(viewport).filter(|_| scale > 0.0) else {
            return;
        };
        let transform = Transformation::translate(shown.x, shown.y)
            * Transformation::scale(scale)
            * Transformation::translate(-child.bounds().x, -child.bounds().y);
        renderer.with_layer(clip, |renderer| {
            renderer.with_transformation(transform, |renderer| {
                self.content.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    child,
                    mouse::Cursor::Unavailable,
                    &child.bounds(),
                );
            });
        });
    }
    fn mouse_interaction(
        &self,
        _: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        _: &iced::Renderer,
    ) -> mouse::Interaction {
        if grab(
            layout.bounds(),
            layout.child(0).bounds().size(),
            cursor,
            *viewport,
        )
        .is_some()
        {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::renderer::Headless;

    #[test]
    fn scaled_picker_uses_press_point_and_never_activates_embedded_controls() {
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let content = || {
            iced::widget::button(iced::widget::Space::new())
                .width(640)
                .height(360)
                .on_press(Point::new(-1.0, -1.0))
                .into()
        };
        let mut element = preview(
            content(),
            Size::new(1920.0, 1080.0),
            Arc::new(|point| point),
        );
        let mut tree = widget::Tree::new(&element);
        let limits = layout::Limits::new(Size::ZERO, Size::new(320.0, 180.0));
        let node = element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &limits)
            .move_to(Point::new(40.0, 20.0));
        assert_eq!(
            Layout::new(&node).child(0).bounds().size(),
            Size::new(640.0, 360.0)
        );
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Layout::new(&node),
            mouse::Cursor::Available(Point::new(110.0, 45.0)),
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::new(Point::ORIGIN, Size::new(800.0, 600.0)),
        );
        assert!(messages.is_empty());
        // A rebuilt preview must deliver the original pick token, not the new
        // callback's token. Hosts can then reject source/config revisions.
        let mut element = preview(
            content(),
            Size::new(1920.0, 1080.0),
            Arc::new(|_| Point::ORIGIN),
        );
        tree.diff(element.as_widget());
        let node = element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &limits)
            .move_to(Point::new(40.0, 20.0));
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            Layout::new(&node),
            mouse::Cursor::Available(Point::new(140.0, 55.0)),
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::new(Point::ORIGIN, Size::new(800.0, 600.0)),
        );
        assert_eq!(messages, [Point::new(140.0, 50.0)]);
    }

    #[test]
    fn intrinsic_previews_exclude_letterboxing_and_scroll_clipping() {
        let canvas = Rectangle::new(Point::new(40.0, 20.0), Size::new(320.0, 180.0));
        let native = Size::new(100.0, 40.0);
        let viewport = Rectangle::new(Point::ORIGIN, Size::new(800.0, 600.0));
        assert_eq!(
            grab(
                canvas,
                native,
                mouse::Cursor::Available(Point::new(160.0, 100.0)),
                viewport
            ),
            Some(Point::new(10.0, 10.0))
        );
        assert!(
            grab(
                canvas,
                native,
                mouse::Cursor::Available(Point::new(50.0, 25.0)),
                viewport
            )
            .is_none()
        );
        let clipped = Rectangle::new(Point::new(40.0, 120.0), Size::new(320.0, 80.0));
        assert!(
            grab(
                canvas,
                native,
                mouse::Cursor::Available(Point::new(160.0, 100.0)),
                clipped
            )
            .is_none()
        );
    }
}
