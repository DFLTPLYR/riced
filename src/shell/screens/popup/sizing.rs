//! Measure both popup axes with the live renderer, independently of the surface.
use crate::{
    shell::{Plant, TopEvent, WidgetEvent},
    ui::node::{NodeLength, WidgetNode},
};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector, window};
use std::{collections::VecDeque, fmt, hash::Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MeasurementKey {
    content: u64,
    width: Option<u32>,
    height: Option<u32>,
    text_size: u32,
    max_width: u32,
    max_height: u32,
}

pub(super) fn measurement_key(
    tree: Option<&WidgetNode>,
    width: Option<f32>,
    height: Option<f32>,
    size: f32,
    max_width: f32,
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
    // Hash the owned description without retaining it or allocating a Debug string.
    fmt::write(&mut Writer(&mut hash), format_args!("{tree:?}")).ok()?;
    Some(MeasurementKey {
        content: hash.finish(),
        width: width.map(f32::to_bits),
        height: height.map(f32::to_bits),
        text_size: size.to_bits(),
        max_width: max_width.to_bits(),
        max_height: max_height.to_bits(),
    })
}

#[derive(Debug, Default)]
pub(crate) struct MeasurementCache(VecDeque<(MeasurementKey, (u32, u32))>);

impl MeasurementCache {
    pub(super) fn get(&mut self, key: Option<MeasurementKey>) -> Option<(u32, u32)> {
        let key = key?;
        let index = self.0.iter().position(|(cached, _)| *cached == key)?;
        let entry = self.0.remove(index)?;
        self.0.push_back(entry);
        Some(entry.1)
    }

    pub(super) fn remember(&mut self, key: Option<MeasurementKey>, size: (u32, u32)) {
        let Some(key) = key else {
            return;
        };
        if let Some(index) = self.0.iter().position(|(cached, _)| *cached == key) {
            self.0.remove(index);
        }
        self.0.push_back((key, size));
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
        let key = measurement_key(None, Some(220.0), None, 13.0, 1920.0, 1080.0);
        cache.remember(key, (220, 183));
        assert_eq!(cache.get(key), Some((220, 183)));
        assert_eq!(
            cache.get(measurement_key(
                None,
                Some(220.0),
                Some(50.0),
                13.0,
                1920.0,
                1080.0
            )),
            None
        );
        assert_eq!(
            cache.get(measurement_key(
                None,
                Some(300.0),
                None,
                13.0,
                1920.0,
                1080.0
            )),
            None
        );
        assert_eq!(
            cache.get(measurement_key(
                None,
                Some(220.0),
                None,
                18.0,
                1920.0,
                1080.0
            )),
            None
        );
        assert_eq!(
            cache.get(measurement_key(
                None,
                Some(220.0),
                None,
                13.0,
                1920.0,
                300.0
            )),
            None
        );
        let image = WidgetNode::Image {
            path: "changing.png".into(),
            width: NodeLength::Native,
            height: NodeLength::Native,
        };
        assert!(measurement_key(Some(&image), Some(220.0), None, 13.0, 1920.0, 1080.0).is_none());
        for i in 0..33 {
            cache.remember(
                measurement_key(None, Some(i as f32), None, 13.0, 1920.0, 1080.0),
                (220, i + 1),
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

pub(super) fn intrinsic_size(node: &WidgetNode, width: bool, height: bool) -> WidgetNode {
    let mut node = if height {
        intrinsic_height(node)
    } else {
        node.clone()
    };
    if width {
        naturalize_width(&mut node);
    }
    node
}

fn naturalize_width(node: &mut WidgetNode) {
    fn natural(width: &mut NodeLength) {
        if matches!(width, NodeLength::Fill | NodeLength::FillPortion(_)) {
            *width = NodeLength::Shrink;
        }
    }
    match node {
        WidgetNode::Anchored { child, .. }
        | WidgetNode::Clickable { child, .. }
        | WidgetNode::Aligned { child, .. } => naturalize_width(child),
        WidgetNode::Container { child, width, .. }
        | WidgetNode::Popup { child, width, .. }
        | WidgetNode::Scrollable { child, width, .. } => {
            natural(width);
            naturalize_width(child);
        }
        WidgetNode::Row {
            children, width, ..
        }
        | WidgetNode::Column {
            children, width, ..
        }
        | WidgetNode::Stack {
            children, width, ..
        } => {
            natural(width);
            for child in children {
                naturalize_width(child);
            }
        }
        WidgetNode::ListView { items, width, .. } => {
            natural(width);
            for (_, child) in items {
                naturalize_width(child);
            }
        }
        WidgetNode::Text { width, .. }
        | WidgetNode::Button { width, .. }
        | WidgetNode::Checkbox { width, .. }
        | WidgetNode::Toggler { width, .. }
        | WidgetNode::Slider { width, .. }
        | WidgetNode::TextInput { width, .. }
        | WidgetNode::PickList { width, .. }
        | WidgetNode::Radio { width, .. } => {
            if let Some(width) = width {
                natural(width);
            }
        }
        WidgetNode::Space { width, .. }
        | WidgetNode::Image { width, .. }
        | WidgetNode::Raster { width, .. }
        | WidgetNode::Progress { width, .. }
        | WidgetNode::Canvas { width, .. }
        | WidgetNode::Shader { width, .. } => natural(width),
        _ => {}
    }
}

fn naturalize_height(node: &mut WidgetNode) {
    fn natural(height: &mut NodeLength) {
        if matches!(height, NodeLength::Fill | NodeLength::FillPortion(_)) {
            *height = NodeLength::Native;
        }
    }
    match node {
        WidgetNode::Anchored { child, .. }
        | WidgetNode::Clickable { child, .. }
        | WidgetNode::Aligned { child, .. } => naturalize_height(child),
        WidgetNode::Container { child, height, .. }
        | WidgetNode::Popup { child, height, .. }
        | WidgetNode::Scrollable { child, height, .. } => {
            natural(height);
            naturalize_height(child);
        }
        WidgetNode::Row {
            children, height, ..
        }
        | WidgetNode::Column {
            children, height, ..
        }
        | WidgetNode::Stack {
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
    pub size: Size<u32>,
    pub max: Size,
    pub auto_width: bool,
    pub auto_height: bool,
}

#[derive(Default)]
struct State {
    size: Size<u32>,
    reported: Option<(u64, Size<u32>)>,
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
            &layout::Limits::new(
                Size::ZERO,
                Size::new(
                    if self.auto_width {
                        self.max.width
                    } else {
                        self.size.width as f32
                    },
                    self.max.height,
                ),
            ),
        );
        let width = if self.auto_width {
            measured.size().width.ceil().clamp(1.0, self.max.width) as u32
        } else {
            self.size.width
        };
        // Final wrapping is measured at the chosen outer width, independent of
        // the provisional native surface size.
        let measured = self.intrinsic.as_widget_mut().layout(
            &mut tree.children[1],
            renderer,
            &layout::Limits::new(Size::ZERO, Size::new(width as f32, self.max.height)),
        );
        let height = if self.auto_height {
            measured.size().height.ceil().clamp(1.0, self.max.height) as u32
        } else {
            self.size.height
        };
        tree.state.downcast_mut::<State>().size = Size::new(width, height);
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
            let size = state.size;
            if (size != self.size || !self.confirmed)
                && state.reported != Some((self.revision, size))
            {
                state.reported = Some((self.revision, size));
                shell.publish(Plant::TopPlot(TopEvent::Widget(
                    WidgetEvent::PopupMeasured(self.id, self.revision, size),
                )));
            }
        }
        if !self.confirmed {
            return;
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
        if !self.confirmed {
            return;
        }
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
