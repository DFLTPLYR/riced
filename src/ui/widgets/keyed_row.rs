//! Horizontal counterpart to iced's keyed column.
use iced::advanced::{
    self, Layout, Widget, layout, renderer,
    widget::{self, Tree},
};
use iced::{Element, Event, Length, Rectangle, Size, Vector, mouse};

pub(crate) fn wrap<'a, K: Clone + Eq + 'static, M: 'a>(
    keys: Vec<K>,
    row: iced::widget::Row<'a, M>,
) -> Element<'a, M> {
    Element::new(KeyedRow {
        keys,
        row: row.into(),
    })
}

struct KeyedRow<'a, K, M> {
    keys: Vec<K>,
    row: Element<'a, M>,
}

impl<K: Clone + Eq + 'static, M> Widget<M, iced::Theme, iced::Renderer> for KeyedRow<'_, K, M> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<Vec<K>>()
    }
    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(self.keys.clone())
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.row)]
    }
    fn diff(&self, tree: &mut Tree) {
        let old = tree.state.downcast_mut::<Vec<K>>();
        let children = std::mem::take(&mut tree.children[0].children);
        let mut previous: Vec<_> = old.iter().cloned().zip(children).map(Some).collect();
        tree.children[0].children = self
            .keys
            .iter()
            .map(|key| {
                previous
                    .iter_mut()
                    .find(|entry| entry.as_ref().is_some_and(|(old, _)| old == key))
                    .and_then(Option::take)
                    .map_or_else(Tree::empty, |(_, tree)| tree)
            })
            .collect();
        old.clone_from(&self.keys);
        tree.children[0].diff(&self.row);
    }
    fn size(&self) -> Size<Length> {
        self.row.as_widget().size()
    }
    fn size_hint(&self) -> Size<Length> {
        self.row.as_widget().size_hint()
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.row
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn advanced::Clipboard,
        shell: &mut advanced::Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.row.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.row.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.row.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.row
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }
    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<advanced::overlay::Element<'a, M, iced::Theme, iced::Renderer>> {
        self.row.as_widget_mut().overlay(
            &mut tree.children[0],
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
    struct Stateful;
    impl Widget<(), iced::Theme, iced::Renderer> for Stateful {
        fn tag(&self) -> widget::tree::Tag {
            widget::tree::Tag::of::<usize>()
        }
        fn state(&self) -> widget::tree::State {
            widget::tree::State::new(0usize)
        }
        fn size(&self) -> Size<Length> {
            Size::new(Length::Shrink, Length::Shrink)
        }
        fn layout(&mut self, _: &mut Tree, _: &iced::Renderer, _: &layout::Limits) -> layout::Node {
            layout::Node::new(Size::new(10.0, 10.0))
        }
        fn draw(
            &self,
            _: &Tree,
            _: &mut iced::Renderer,
            _: &iced::Theme,
            _: &renderer::Style,
            _: Layout<'_>,
            _: mouse::Cursor,
            _: &Rectangle,
        ) {
        }
    }
    #[test]
    fn removing_and_reordering_rows_preserves_survivor_state() {
        let row = |keys: Vec<&'static str>| {
            let content =
                iced::widget::Row::with_children(keys.iter().map(|_| Element::new(Stateful)));
            wrap(keys, content)
        };
        let original = row(vec!["a", "b", "c"]);
        let mut tree = Tree::new(&original);
        for (index, child) in tree.children[0].children.iter_mut().enumerate() {
            *child.state.downcast_mut::<usize>() = index + 1;
        }
        let updated = row(vec!["c", "b", "d"]);
        tree.diff(&updated);
        let states: Vec<_> = tree.children[0]
            .children
            .iter()
            .map(|child| *child.state.downcast_ref::<usize>())
            .collect();
        assert_eq!(states, [3, 2, 0]);
    }
}
