//! Responsive, measured shortest-column packing without regrouping widget state.
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

pub(crate) fn masonry<'a, M: 'a>(
    children: Vec<Element<'a, M>>,
    column_width: f32,
    spacing: f32,
) -> Element<'a, M> {
    Element::new(Masonry {
        children,
        column_width: column_width.max(1.0),
        spacing: spacing.max(0.0),
    })
}

struct Masonry<'a, M> {
    children: Vec<Element<'a, M>>,
    column_width: f32,
    spacing: f32,
}

impl<M> Widget<M, iced::Theme, iced::Renderer> for Masonry<'_, M> {
    fn children(&self) -> Vec<widget::Tree> {
        self.children.iter().map(widget::Tree::new).collect()
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        if self.children.is_empty() {
            return layout::Node::new(limits.resolve(Length::Fill, Length::Shrink, Size::ZERO));
        }
        let available = limits.max().width;
        let width = if available.is_finite() {
            available
        } else {
            self.column_width
        };
        let columns = (((width + self.spacing) / (self.column_width + self.spacing)).floor()
            as usize)
            .clamp(1, self.children.len());
        let column_width =
            ((width - self.spacing * (columns - 1) as f32) / columns as f32).max(0.0);
        let child_limits = layout::Limits::new(
            Size::new(column_width, 0.0),
            Size::new(column_width, f32::INFINITY),
        );
        let mut bottoms = vec![0.0_f32; columns];
        let mut nodes = Vec::with_capacity(self.children.len());
        for (child, tree) in self.children.iter_mut().zip(&mut tree.children) {
            let mut node = child.as_widget_mut().layout(tree, renderer, &child_limits);
            let column = bottoms
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.total_cmp(b))
                .map_or(0, |(index, _)| index);
            node.move_to_mut(Point::new(
                column as f32 * (column_width + self.spacing),
                bottoms[column],
            ));
            bottoms[column] += node.size().height + self.spacing;
            nodes.push(node);
        }
        let height = (bottoms.into_iter().fold(0.0_f32, f32::max) - self.spacing).max(0.0);
        layout::Node::with_children(
            limits.resolve(Length::Fill, Length::Shrink, Size::new(width, height)),
            nodes,
        )
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
        for ((child, tree), layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                tree, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((child, tree), layout) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, layout)| layout.bounds().intersects(viewport))
        {
            child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, tree), layout)| {
                child
                    .as_widget()
                    .mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((child, tree), layout) in self
                .children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
            {
                child
                    .as_widget_mut()
                    .operate(tree, layout, renderer, operation);
            }
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, iced::Theme, iced::Renderer>> {
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::renderer::Headless;
    use iced::widget::{Space, button};

    fn renderer() -> iced::Renderer {
        iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap()
    }

    #[test]
    fn measured_cards_fill_shortest_columns_and_reflow_without_reordering() {
        let renderer = renderer();
        let cards: Vec<Element<'_, ()>> = [100, 40, 30, 80]
            .into_iter()
            .map(|height| Space::new().width(Length::Fill).height(height).into())
            .collect();
        let mut gallery = masonry(cards, 100.0, 10.0);
        let mut tree = widget::Tree::new(&gallery);
        for (width, height, expected) in [
            (
                210.0,
                170.0,
                [(0.0, 0.0), (110.0, 0.0), (110.0, 50.0), (110.0, 90.0)],
            ),
            (
                80.0,
                280.0,
                [(0.0, 0.0), (0.0, 110.0), (0.0, 160.0), (0.0, 200.0)],
            ),
            (
                320.0,
                120.0,
                [(0.0, 0.0), (110.0, 0.0), (220.0, 0.0), (220.0, 40.0)],
            ),
        ] {
            let node = gallery.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY)),
            );
            assert_eq!(node.size(), Size::new(width, height));
            for ((child, (x, y)), expected_height) in node
                .children()
                .iter()
                .zip(expected)
                .zip([100.0, 40.0, 30.0, 80.0])
            {
                assert_eq!(child.bounds().position(), Point::new(x, y));
                assert_eq!(child.size().height, expected_height);
                assert!(child.bounds().x + child.size().width <= width);
            }
        }
    }

    #[test]
    fn translated_cards_forward_input_to_the_correct_child() {
        let renderer = renderer();
        let mut gallery = masonry(
            vec![
                Space::new().width(Length::Fill).height(100).into(),
                button("Pick")
                    .width(Length::Fill)
                    .height(40)
                    .on_press("picked")
                    .into(),
            ],
            100.0,
            10.0,
        );
        let mut tree = widget::Tree::new(&gallery);
        let node = gallery
            .as_widget_mut()
            .layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, Size::new(210.0, f32::INFINITY)),
            )
            .move_to(Point::new(25.0, 40.0));
        let cursor = mouse::Cursor::Available(Layout::new(&node).child(1).bounds().center());
        let mut messages = Vec::new();
        for event in [
            mouse::Event::ButtonPressed(mouse::Button::Left),
            mouse::Event::ButtonReleased(mouse::Button::Left),
        ] {
            gallery.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(event),
                Layout::new(&node),
                cursor,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(Size::new(400.0, 400.0)),
            );
        }
        assert_eq!(messages, ["picked"]);
    }
}
