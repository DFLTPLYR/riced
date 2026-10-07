//! Draw-time translation, consistent hit testing, and non-layout exit delegates.
//! Opacity is applied by primitive builders, not by inherited text color.

use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget};
use iced::{Element, Event, Length, Rectangle, Size, Transformation, Vector};

pub fn shifted<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    content: impl Into<Element<'a, M, T, R>>,
    x: f32,
    y: f32,
    clickable: bool,
) -> Element<'a, M, T, R> {
    Element::new(Motion {
        content: content.into(),
        offset: Vector::new(x, y),
        clickable,
        detached: false,
    })
}

pub fn ghost<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    content: Element<'a, M, T, R>,
    x: f32,
    y: f32,
) -> Element<'a, M, T, R> {
    Element::new(Motion {
        content,
        offset: Vector::new(x, y),
        clickable: false,
        detached: true,
    })
}

struct Motion<'a, M, T, R> {
    content: Element<'a, M, T, R>,
    offset: Vector,
    clickable: bool,
    detached: bool,
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Motion<'_, M, T, R> {
    fn tag(&self) -> widget::tree::Tag {
        self.content.as_widget().tag()
    }
    fn state(&self) -> widget::tree::State {
        self.content.as_widget().state()
    }
    fn children(&self) -> Vec<widget::Tree> {
        self.content.as_widget().children()
    }
    fn diff(&self, tree: &mut widget::Tree) {
        self.content.as_widget().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        if self.detached {
            Size::new(Length::Fixed(0.0), Length::Fixed(0.0))
        } else {
            self.content.as_widget().size()
        }
    }
    fn size_hint(&self) -> Size<Length> {
        self.size()
    }
    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &R,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child = self.content.as_widget_mut().layout(tree, renderer, limits);
        if self.detached {
            layout::Node::with_children(Size::ZERO, vec![child])
        } else {
            child
        }
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
        if !self.clickable {
            return;
        }
        self.content.as_widget_mut().update(
            tree,
            event,
            layout,
            cursor * Transformation::translate(-self.offset.x, -self.offset.y),
            renderer,
            clipboard,
            shell,
            &(*viewport - self.offset),
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
        let child = if self.detached {
            layout.children().next().expect("ghost child")
        } else {
            layout
        };
        renderer.with_translation(self.offset, |renderer| {
            self.content.as_widget().draw(
                tree,
                renderer,
                theme,
                style,
                child,
                cursor * Transformation::translate(-self.offset.x, -self.offset.y),
                &(*viewport - self.offset),
            );
        });
    }
    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        if !self.clickable {
            return mouse::Interaction::None;
        }
        self.content.as_widget().mouse_interaction(
            tree,
            layout,
            cursor * Transformation::translate(-self.offset.x, -self.offset.y),
            &(*viewport - self.offset),
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
        if self.clickable {
            self.content
                .as_widget_mut()
                .operate(tree, layout, renderer, operation);
        }
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        offset: Vector,
    ) -> Option<overlay::Element<'b, M, T, R>> {
        if !self.clickable {
            return None;
        }
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, offset + self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_delegate_does_not_reserve_layout_space() {
        let content: Element<'static, (), (), ()> =
            iced::widget::Space::new().width(80).height(100).into();
        let mut element = ghost(content, -200.0, 108.0);
        let mut tree = widget::Tree::new(&element);
        let layout = element.as_widget_mut().layout(
            &mut tree,
            &(),
            &layout::Limits::new(Size::ZERO, Size::new(320.0, 800.0)),
        );
        assert_eq!(layout.size(), Size::ZERO);
        assert_eq!(layout.children()[0].size(), Size::new(80.0, 100.0));
    }

    #[test]
    fn shifted_click_uses_visible_coordinates_and_ghost_is_inert() {
        for clickable in [true, false] {
            let content: Element<'static, (), (), ()> =
                iced::widget::mouse_area(iced::widget::Space::new().width(50).height(50))
                    .on_press(())
                    .into();
            let mut element = shifted(content, 100.0, 0.0, clickable);
            let mut tree = widget::Tree::new(&element);
            let node = element.as_widget_mut().layout(
                &mut tree,
                &(),
                &layout::Limits::new(Size::ZERO, Size::new(320.0, 800.0)),
            );
            let mut messages = Vec::new();
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
                Layout::new(&node),
                mouse::Cursor::Available(iced::Point::new(110.0, 10.0)),
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(Size::new(320.0, 800.0)),
            );
            assert_eq!(messages.len(), usize::from(clickable));
        }
    }
}
