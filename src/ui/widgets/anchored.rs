//! Position an overlay after measuring its actual child layout.
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};
use std::sync::{Arc, Mutex};

pub(crate) type Bounds = Arc<Mutex<Option<Rectangle>>>;

pub(crate) fn anchored<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    content: Element<'a, M, T, R>,
    anchor: Point,
    output_origin: Point,
    bounds: Bounds,
) -> Element<'a, M, T, R> {
    Element::new(Anchored {
        content,
        anchor,
        output_origin,
        bounds,
        placement: None,
        margin: 0.0,
    })
}

pub(crate) fn placed<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    content: Element<'a, M, T, R>,
    placement: crate::ui::placement::Placement,
    margin: f32,
) -> Element<'a, M, T, R> {
    Element::new(Anchored {
        content,
        placement: Some(placement),
        margin,
        anchor: Point::ORIGIN,
        output_origin: Point::ORIGIN,
        bounds: Arc::new(Mutex::new(None)),
    })
}

struct Anchored<'a, M, T, R> {
    content: Element<'a, M, T, R>,
    anchor: Point,
    output_origin: Point,
    bounds: Bounds,
    placement: Option<crate::ui::placement::Placement>,
    margin: f32,
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Anchored<'_, M, T, R> {
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &R,
        limits: &layout::Limits,
    ) -> layout::Node {
        let mut size = limits.max();
        let mut child = self.content.as_widget_mut().layout(
            &mut tree.children[0],
            renderer,
            &layout::Limits::new(
                Size::ZERO,
                Size::new(
                    (size.width - 2.0 * self.margin).max(1.0),
                    (size.height - 2.0 * self.margin).max(1.0),
                ),
            ),
        );
        if !size.width.is_finite() {
            size.width = child.size().width + 2.0 * self.margin;
        }
        if !size.height.is_finite() {
            size.height = child.size().height + 2.0 * self.margin;
        }
        let position = if let Some(placement) = self.placement {
            placement.origin(size, child.size(), self.margin)
        } else {
            Point::new(
                self.anchor
                    .x
                    .clamp(0.0, (size.width - child.size().width).max(0.0)),
                self.anchor
                    .y
                    .clamp(0.0, (size.height - child.size().height).max(0.0)),
            )
        };
        child.move_to_mut(position);
        *self.bounds.lock().expect("menu layout bounds") = Some(Rectangle::new(
            Point::new(
                position.x + self.output_origin.x,
                position.y + self.output_origin.y,
            ),
            child.size(),
        ));
        layout::Node::with_children(size, vec![child])
    }
    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout.children().next().expect("anchored child"),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }
    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut R,
        theme: &T,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout.children().next().expect("anchored child"),
            cursor,
            viewport,
        );
    }
    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().expect("anchored child"),
            cursor,
            viewport,
            renderer,
        )
    }
    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &R,
        operation: &mut dyn widget::Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout.children().next().expect("anchored child"),
            renderer,
            operation,
        );
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        offset: Vector,
    ) -> Option<overlay::Element<'b, M, T, R>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout.children().next().expect("anchored child"),
            renderer,
            viewport,
            offset,
        )
    }
}
