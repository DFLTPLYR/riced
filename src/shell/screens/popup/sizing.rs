//! Measure intrinsic popup height with the live renderer, independently of the surface.
use crate::{
    shell::{Plant, TopEvent, WidgetEvent},
    ui::node::{NodeLength, WidgetNode},
};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector, window};

/// Fill describes space within a window, not the window's intrinsic height.
/// Keep fixed heights and all widths (including text wrapping) for measurement.
pub(super) fn intrinsic_height(node: &WidgetNode) -> WidgetNode {
    let mut node = node.clone();
    fn natural(height: &mut NodeLength) {
        if *height == NodeLength::Fill {
            *height = NodeLength::Native;
        }
    }
    match &mut node {
        WidgetNode::Container { child, height, .. }
        | WidgetNode::Scrollable { child, height, .. } => {
            natural(height);
            **child = intrinsic_height(child);
        }
        WidgetNode::Row {
            children, height, ..
        }
        | WidgetNode::Column {
            children, height, ..
        } => {
            natural(height);
            for child in children {
                *child = intrinsic_height(child);
            }
        }
        WidgetNode::ListView { items, height, .. } => {
            natural(height);
            for (_, child) in items {
                *child = intrinsic_height(child);
            }
        }
        WidgetNode::Text { height, .. }
        | WidgetNode::Button { height, .. }
        | WidgetNode::Progress { height, .. } => {
            if *height == Some(NodeLength::Fill) {
                *height = None;
            }
        }
        WidgetNode::Space { height, .. } | WidgetNode::Image { height, .. } => natural(height),
        _ => {}
    }
    node
}

pub(super) struct MeasuredPopup {
    pub content: Element<'static, Plant>,
    pub intrinsic: Element<'static, Plant>,
    pub id: window::Id,
    pub revision: u64,
    pub height: u32,
    pub max_height: f32,
}

#[derive(Default)]
struct State {
    height: u32,
}

impl Widget<Plant, iced::Theme, iced::Renderer> for MeasuredPopup {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }
    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }
    fn children(&self) -> Vec<widget::Tree> {
        vec![
            widget::Tree::new(&self.content),
            widget::Tree::new(&self.intrinsic),
        ]
    }
    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&[&self.content, &self.intrinsic]);
    }
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }
    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let measured = self.intrinsic.as_widget_mut().layout(
            &mut tree.children[1],
            renderer,
            &layout::Limits::new(Size::ZERO, Size::new(limits.max().width, self.max_height)),
        );
        tree.state.downcast_mut::<State>().height =
            measured.size().height.ceil().clamp(1.0, self.max_height) as u32;
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Plant>,
        viewport: &Rectangle,
    ) {
        if matches!(event, Event::Window(window::Event::RedrawRequested(_))) {
            let height = tree.state.downcast_ref::<State>().height;
            if height != self.height {
                shell.publish(Plant::TopPlot(TopEvent::Widget(
                    WidgetEvent::PopupMeasured(self.id, self.revision, height),
                )));
            }
        }
        self.content.as_widget_mut().update(
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
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
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
            layout,
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
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }
    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut widget::Tree,
        layout: Layout<'a>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Plant, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}
