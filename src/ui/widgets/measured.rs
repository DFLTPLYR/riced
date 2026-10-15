//! Native layout measurements shared by scene hosts; no fixed card dimensions.
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Marker {
    Interactive,
    Item(String),
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Measurement {
    pub regions: Vec<Rectangle>,
    pub items: BTreeMap<String, Rectangle>,
}
#[derive(Debug, Default)]
pub(crate) struct Frame {
    pub last: Option<Measurement>,
    reported: Option<(u64, Measurement)>,
}
pub(crate) type SharedFrame = Arc<Mutex<Frame>>;
struct Collector {
    data: Measurement,
    offset: Vector,
    clip: Rectangle,
    pending: Option<(Vector, Rectangle)>,
}
impl widget::Operation for Collector {
    fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn widget::Operation)) {
        let before = (self.offset, self.clip);
        if let Some(next) = self.pending.take() {
            (self.offset, self.clip) = next;
        }
        visit(self);
        (self.offset, self.clip) = before;
    }
    fn scrollable(
        &mut self,
        _: Option<&widget::Id>,
        bounds: Rectangle,
        _: Rectangle,
        translation: Vector,
        _: &mut dyn widget::operation::Scrollable,
    ) {
        let clip = (bounds + self.offset)
            .intersection(&self.clip)
            .unwrap_or(Rectangle::with_size(Size::ZERO));
        self.pending = Some((self.offset - translation, clip));
    }
    fn custom(&mut self, _: Option<&widget::Id>, bounds: Rectangle, state: &mut dyn Any) {
        let Some(marker) = state.downcast_ref::<Marker>() else {
            return;
        };
        let bounds = bounds + self.offset;
        match marker {
            Marker::Interactive => {
                if let Some(rect) = bounds.intersection(&self.clip)
                    && rect.width > 0.0
                    && rect.height > 0.0
                {
                    self.data.regions.push(rect);
                }
            }
            Marker::Item(key) => {
                self.data.items.insert(key.clone(), bounds);
            }
        }
    }
}
pub(crate) fn marked<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    content: Element<'a, M, T, R>,
    marker: Marker,
) -> Element<'a, M, T, R> {
    Element::new(Observed {
        content,
        marker: Some(marker),
        frame: None,
        revision: 0,
        report: None,
    })
}
pub(crate) fn root<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    content: Element<'a, M, T, R>,
    frame: SharedFrame,
    revision: u64,
    report: Arc<dyn Fn(Measurement) -> M + Send + Sync>,
) -> Element<'a, M, T, R> {
    Element::new(Observed {
        content,
        marker: None,
        frame: Some(frame),
        revision,
        report: Some(report),
    })
}
struct Observed<'a, M, T, R> {
    content: Element<'a, M, T, R>,
    marker: Option<Marker>,
    frame: Option<SharedFrame>,
    revision: u64,
    report: Option<Arc<dyn Fn(Measurement) -> M + Send + Sync>>,
}
impl<M, T, R: renderer::Renderer> Observed<'_, M, T, R> {
    fn measure(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &R,
        layout: Layout<'_>,
        viewport: Rectangle,
    ) -> Measurement {
        let mut collector = Collector {
            data: Measurement::default(),
            offset: Vector::ZERO,
            clip: viewport,
            pending: None,
        };
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            renderer,
            &mut collector,
        );
        collector.data
    }
}
impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Observed<'_, M, T, R> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<Option<Marker>>()
    }
    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(self.marker.clone())
    }
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut widget::Tree) {
        *tree.state.downcast_mut::<Option<Marker>>() = self.marker.clone();
        tree.diff_children(std::slice::from_ref(&self.content));
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
        renderer: &R,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        if let Some(frame) = self.frame.clone() {
            let data = self.measure(
                tree,
                renderer,
                Layout::new(&child),
                Rectangle::with_size(child.size()),
            );
            if let Ok(mut frame) = frame.lock() {
                frame.last = Some(data);
            }
        }
        layout::Node::with_children(child.size(), vec![child])
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
            layout.child(0),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        if let (Some(frame), Some(report)) = (self.frame.clone(), self.report.clone()) {
            let data = self.measure(tree, renderer, layout.child(0), *viewport);
            if let Ok(mut frame) = frame.lock() {
                frame.last = Some(data.clone());
                if frame.reported.as_ref() != Some(&(self.revision, data.clone())) {
                    frame.reported = Some((self.revision, data.clone()));
                    shell.publish(report(data));
                }
            }
        }
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
            layout.child(0),
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
            layout.child(0),
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
        if let Some(marker) = tree.state.downcast_mut::<Option<Marker>>() {
            operation.custom(None, layout.bounds(), marker);
        }
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout.child(0),
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
            layout.child(0),
            renderer,
            viewport,
            offset,
        )
    }
}
/// Preserve focus and scroll operations while transforming measured bounds.
pub(crate) struct Translated<'a> {
    pub operation: &'a mut dyn widget::Operation,
    pub offset: Vector,
}
impl widget::Operation for Translated<'_> {
    fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn widget::Operation)) {
        let offset = self.offset;
        self.operation
            .traverse(&mut |operation| visit(&mut Translated { operation, offset }));
    }
    fn container(&mut self, id: Option<&widget::Id>, bounds: Rectangle) {
        self.operation.container(id, bounds + self.offset);
    }
    fn custom(&mut self, id: Option<&widget::Id>, bounds: Rectangle, state: &mut dyn Any) {
        self.operation.custom(id, bounds + self.offset, state);
    }
    fn text(&mut self, id: Option<&widget::Id>, bounds: Rectangle, text: &str) {
        self.operation.text(id, bounds + self.offset, text);
    }
    fn text_input(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn widget::operation::TextInput,
    ) {
        self.operation.text_input(id, bounds + self.offset, state);
    }
    fn focusable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn widget::operation::Focusable,
    ) {
        self.operation.focusable(id, bounds + self.offset, state);
    }
    fn scrollable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        content: Rectangle,
        translation: Vector,
        state: &mut dyn widget::operation::Scrollable,
    ) {
        self.operation.scrollable(
            id,
            bounds + self.offset,
            content + self.offset,
            translation,
            state,
        );
    }
}
