//! Measure intrinsic popup height with the live renderer, independently of the surface.
use crate::{
    shell::{Plant, TopEvent, WidgetEvent},
    ui::node::{NodeLength, WidgetNode},
};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector, window};
use std::{
    collections::VecDeque,
    fmt,
    hash::{Hash, Hasher},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MeasurementKey {
    content: u64,
    width: u32,
    text_size: u32,
    max_height: u32,
}

pub(super) fn measurement_key(
    text: &str,
    items: &[super::PopupItem],
    tree: Option<&WidgetNode>,
    width: u32,
    size: f32,
    max_height: f32,
) -> Option<MeasurementKey> {
    // Natural image dimensions can change on disk without changing the tree.
    if tree.is_some_and(crate::ui::node::contains_images) {
        return None;
    }
    struct Writer<'a>(&'a mut std::collections::hash_map::DefaultHasher);
    impl fmt::Write for Writer<'_> {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            self.0.write(value.as_bytes());
            Ok(())
        }
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hash);
    // Hash the owned description without retaining it or allocating a Debug string.
    fmt::write(&mut Writer(&mut hash), format_args!("{items:?}{tree:?}")).ok()?;
    Some(MeasurementKey {
        content: hash.finish(),
        width,
        text_size: size.to_bits(),
        max_height: max_height.to_bits(),
    })
}

#[derive(Debug, Default)]
pub(crate) struct MeasurementCache(VecDeque<(MeasurementKey, u32)>);

impl MeasurementCache {
    pub(super) fn get(&mut self, key: Option<MeasurementKey>) -> Option<u32> {
        let key = key?;
        let index = self.0.iter().position(|(cached, _)| *cached == key)?;
        let entry = self.0.remove(index)?;
        self.0.push_back(entry);
        Some(entry.1)
    }

    pub(super) fn remember(&mut self, key: Option<MeasurementKey>, height: u32) {
        let Some(key) = key else {
            return;
        };
        if let Some(index) = self.0.iter().position(|(cached, _)| *cached == key) {
            self.0.remove(index);
        }
        self.0.push_back((key, height));
        while self.0.len() > 32 {
            self.0.pop_front();
        }
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn portion_heights_are_intrinsic_for_popup_measurement_but_widths_are_preserved() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let value = lua.load("return iced.column({iced.text('body'):height('fill:3'):on_press('body')}):width('fill:4'):height('fill:2')").eval().unwrap();
        let node = crate::ui::decode::parse_node(&value).unwrap();
        let measured = intrinsic_height(&node);
        let WidgetNode::Column {
            width,
            height,
            children,
            ..
        } = &measured
        else {
            panic!("column");
        };
        assert_eq!(*width, NodeLength::FillPortion(4));
        assert_eq!(*height, NodeLength::Native);
        let WidgetNode::Clickable { child, .. } = &children[0] else {
            panic!("clickable");
        };
        assert!(matches!(
            child.as_ref(),
            WidgetNode::Text { height: None, .. }
        ));
        assert_eq!(
            crate::ui::build::estimate_height(&node, 13.0),
            crate::ui::build::estimate_height(&measured, 13.0)
        );
    }

    #[test]
    fn popup_measurements_are_bounded_and_invalidate_on_layout_changes() {
        let mut cache = MeasurementCache::default();
        let key = measurement_key("Session", &[], None, 220, 13.0, 1080.0);
        cache.remember(key, 183);
        assert_eq!(cache.get(key), Some(183));
        assert_eq!(
            cache.get(measurement_key("Changed", &[], None, 220, 13.0, 1080.0)),
            None
        );
        assert_eq!(
            cache.get(measurement_key("Session", &[], None, 300, 13.0, 1080.0)),
            None
        );
        assert_eq!(
            cache.get(measurement_key("Session", &[], None, 220, 18.0, 1080.0)),
            None
        );
        assert_eq!(
            cache.get(measurement_key("Session", &[], None, 220, 13.0, 300.0)),
            None
        );
        let image = WidgetNode::Image {
            path: "changing.png".into(),
            width: NodeLength::Native,
            height: NodeLength::Native,
        };
        assert!(measurement_key("Session", &[], Some(&image), 220, 13.0, 1080.0).is_none());
        for i in 0..33 {
            cache.remember(
                measurement_key(&i.to_string(), &[], None, 220, 13.0, 1080.0),
                i + 1,
            );
        }
        assert_eq!(cache.0.len(), 32);
        assert_eq!(cache.get(key), None);
    }
}

/// Fill describes space within a window, not the window's intrinsic height.
/// Keep fixed heights and all widths (including text wrapping) for measurement.
pub(super) fn intrinsic_height(node: &WidgetNode) -> WidgetNode {
    let mut node = node.clone();
    naturalize_height(&mut node);
    node
}

fn naturalize_height(node: &mut WidgetNode) {
    fn natural(height: &mut NodeLength) {
        if matches!(height, NodeLength::Fill | NodeLength::FillPortion(_)) {
            *height = NodeLength::Native;
        }
    }
    match node {
        WidgetNode::Anchored { child, .. } | WidgetNode::Clickable { child, .. } => {
            naturalize_height(child)
        }
        WidgetNode::Container { child, height, .. }
        | WidgetNode::Scrollable { child, height, .. } => {
            natural(height);
            naturalize_height(child);
        }
        WidgetNode::Row {
            children, height, ..
        }
        | WidgetNode::Column {
            children, height, ..
        } => {
            natural(height);
            for child in children {
                naturalize_height(child);
            }
        }
        WidgetNode::ListView { items, height, .. } => {
            natural(height);
            for (_, child) in items {
                naturalize_height(child);
            }
        }
        WidgetNode::Text { height, .. }
        | WidgetNode::Button { height, .. }
        | WidgetNode::Progress { height, .. } => {
            if matches!(height, Some(NodeLength::Fill | NodeLength::FillPortion(_))) {
                *height = None;
            }
        }
        WidgetNode::Space { height, .. }
        | WidgetNode::Image { height, .. }
        | WidgetNode::Raster { height, .. } => natural(height),
        _ => {}
    }
}

pub(super) struct MeasuredPopup {
    pub content: Element<'static, Plant>,
    pub intrinsic: Element<'static, Plant>,
    pub id: window::Id,
    pub revision: u64,
    pub confirmed: bool,
    pub height: u32,
    pub max_height: f32,
}

#[derive(Default)]
struct State {
    height: u32,
    reported: Option<(u64, u32)>,
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
            let state = tree.state.downcast_mut::<State>();
            let height = state.height;
            if (height != self.height || !self.confirmed)
                && state.reported != Some((self.revision, height))
            {
                state.reported = Some((self.revision, height));
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
