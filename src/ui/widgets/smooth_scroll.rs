//! Animate wheel targets while retaining native scrolling, hit testing and grabs.
use iced::advanced::{
    self, Layout, Widget, layout, renderer,
    widget::{self, Tree},
};
use iced::{Element, Event, Length, Rectangle, Size, Vector, mouse};
use std::time::{Duration, Instant};

const DURATION: Duration = Duration::from_millis(160);

#[derive(Default)]
struct State {
    motion: Option<Motion>,
}

struct Motion {
    from: Vector,
    target: Vector,
    last: Vector,
    started: Instant,
}

#[derive(Default)]
struct Offset {
    value: Vector,
    max: Vector,
    set: Option<Vector>,
    found: bool,
}

impl widget::Operation for Offset {
    fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn widget::Operation)) {
        if !self.found {
            visit(self);
        }
    }

    fn scrollable(
        &mut self,
        _: Option<&widget::Id>,
        bounds: Rectangle,
        content: Rectangle,
        translation: Vector,
        state: &mut dyn widget::operation::Scrollable,
    ) {
        if self.found {
            return;
        }
        self.found = true;
        self.value = translation;
        self.max = Vector::new(
            (content.width - bounds.width).max(0.0),
            (content.height - bounds.height).max(0.0),
        );
        if let Some(value) = self.set {
            state.scroll_to(iced::widget::scrollable::AbsoluteOffset {
                x: Some(value.x),
                y: Some(value.y),
            });
        }
    }
}

fn clamp(value: Vector, max: Vector) -> Vector {
    Vector::new(value.x.clamp(0.0, max.x), value.y.clamp(0.0, max.y))
}

pub(crate) fn wrap<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    Element::new(Smooth {
        content: content.into(),
    })
}

struct Smooth<'a, M> {
    content: Element<'a, M>,
}

impl<M> Smooth<'_, M> {
    fn offset(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        set: Option<Vector>,
    ) -> Offset {
        let mut offset = Offset {
            set,
            ..Default::default()
        };
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, &mut offset);
        offset
    }
}

impl<M> Widget<M, iced::Theme, iced::Renderer> for Smooth<'_, M> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }
    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut Tree) {
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
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
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
        let before = self.offset(tree, layout, renderer, None);
        let mut motion = tree.state.downcast_mut::<State>().motion.take();
        let reduced = crate::ui::animations::reduced();
        // External scroll operations and content shrinkage take precedence over a pending target.
        if motion
            .as_ref()
            .is_some_and(|motion| motion.last != before.value)
        {
            motion = None;
        }
        if let Event::Window(iced::window::Event::RedrawRequested(now)) = event
            && let Some(active) = &mut motion
        {
            active.target = clamp(active.target, before.max);
            let t = if reduced {
                1.0
            } else {
                (now.saturating_duration_since(active.started).as_secs_f32()
                    / DURATION.as_secs_f32())
                .min(1.0)
            };
            let value = clamp(
                active.from + (active.target - active.from) * (1.0 - (1.0 - t).powi(3)),
                before.max,
            );
            self.offset(tree, layout, renderer, Some(value));
            // Native scroll translations are pixel-rounded; retain the
            // applied value so rounding is not mistaken for an external jump.
            active.last = self.offset(tree, layout, renderer, None).value;
            if t >= 1.0 {
                motion = None;
            } else {
                shell.request_redraw();
            }
        }
        // Touchpad pixels are already continuous. Grabs/touch must track the pointer directly.
        if reduced
            || matches!(
                event,
                Event::Mouse(
                    mouse::Event::ButtonPressed(_)
                        | mouse::Event::WheelScrolled {
                            delta: mouse::ScrollDelta::Pixels { .. }
                        }
                ) | Event::Touch(_)
            )
        {
            motion = None;
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
        if !reduced
            && matches!(
                event,
                Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Lines { .. }
                })
            )
        {
            let after = self.offset(tree, layout, renderer, None);
            let delta = after.value - before.value;
            if delta != Vector::ZERO {
                let target = clamp(
                    motion.as_ref().map_or(before.value, |motion| motion.target) + delta,
                    after.max,
                );
                self.offset(tree, layout, renderer, Some(before.value));
                motion = Some(Motion {
                    from: before.value,
                    target,
                    last: before.value,
                    started: Instant::now(),
                });
                shell.request_redraw();
            } else if cursor.is_over(layout.bounds()) {
                // A reverse wheel at the current boundary must stop an old
                // target even when the native scrollable cannot move further.
                motion = None;
            }
        }
        tree.state.downcast_mut::<State>().motion = motion;
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
        tree: &Tree,
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
        tree: &mut Tree,
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
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<advanced::overlay::Element<'a, M, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
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
    use iced::advanced::renderer::Headless;

    fn renderer() -> iced::Renderer {
        iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap()
    }
    fn scroll(height: f32) -> Smooth<'static, ()> {
        Smooth {
            content: iced::widget::scrollable(iced::widget::Space::new().width(100).height(height))
                .width(120)
                .height(100)
                .into(),
        }
    }
    fn event(
        widget: &mut Smooth<'_, ()>,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        layout: &layout::Node,
        event: Event,
    ) {
        widget.update(
            tree,
            &event,
            Layout::new(layout),
            mouse::Cursor::Available(iced::Point::new(50.0, 50.0)),
            renderer,
            &mut advanced::clipboard::Null,
            &mut advanced::Shell::new(&mut Vec::new()),
            &Rectangle::with_size(Size::new(120.0, 100.0)),
        );
    }
    fn wheel(y: f32) -> Event {
        Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y },
        })
    }
    #[test]
    fn wheel_retargets_and_survives_rebuild_without_jumping() {
        let renderer = renderer();
        let mut widget = scroll(1000.0);
        let mut tree = Tree::new(&widget as &dyn Widget<(), iced::Theme, iced::Renderer>);
        let limits = layout::Limits::new(Size::ZERO, Size::new(120.0, 100.0));
        let layout = widget.layout(&mut tree, &renderer, &limits);
        event(&mut widget, &mut tree, &renderer, &layout, wheel(-1.0));
        let start = tree
            .state
            .downcast_ref::<State>()
            .motion
            .as_ref()
            .unwrap()
            .started;
        assert_eq!(
            widget
                .offset(&mut tree, Layout::new(&layout), &renderer, None)
                .value
                .y,
            0.0
        );
        event(
            &mut widget,
            &mut tree,
            &renderer,
            &layout,
            Event::Window(iced::window::Event::RedrawRequested(
                start + Duration::from_millis(40),
            )),
        );
        let partial = widget
            .offset(&mut tree, Layout::new(&layout), &renderer, None)
            .value
            .y;
        assert!(partial > 0.0 && partial < 60.0);
        event(&mut widget, &mut tree, &renderer, &layout, wheel(-1.0));
        assert_eq!(
            widget
                .offset(&mut tree, Layout::new(&layout), &renderer, None)
                .value
                .y,
            partial
        );
        let start = tree
            .state
            .downcast_ref::<State>()
            .motion
            .as_ref()
            .unwrap()
            .started;
        let mut rebuilt = scroll(1000.0);
        rebuilt.diff(&mut tree);
        let layout = rebuilt.layout(&mut tree, &renderer, &limits);
        event(
            &mut rebuilt,
            &mut tree,
            &renderer,
            &layout,
            Event::Window(iced::window::Event::RedrawRequested(start + DURATION)),
        );
        assert_eq!(
            rebuilt
                .offset(&mut tree, Layout::new(&layout), &renderer, None)
                .value
                .y,
            120.0
        );
        assert!(tree.state.downcast_ref::<State>().motion.is_none());
    }
    #[test]
    fn reverse_boundary_input_and_direct_input_cancel_pending_motion() {
        let renderer = renderer();
        let mut widget = scroll(1000.0);
        let mut tree = Tree::new(&widget as &dyn Widget<(), iced::Theme, iced::Renderer>);
        let layout = widget.layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(120.0, 100.0)),
        );
        event(&mut widget, &mut tree, &renderer, &layout, wheel(-1.0));
        event(&mut widget, &mut tree, &renderer, &layout, wheel(1.0));
        assert!(tree.state.downcast_ref::<State>().motion.is_none());
        event(&mut widget, &mut tree, &renderer, &layout, wheel(-1.0));
        event(
            &mut widget,
            &mut tree,
            &renderer,
            &layout,
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Pixels { x: 0.0, y: -15.0 },
            }),
        );
        assert_eq!(
            widget
                .offset(&mut tree, Layout::new(&layout), &renderer, None)
                .value
                .y,
            15.0
        );
        assert!(tree.state.downcast_ref::<State>().motion.is_none());
        event(&mut widget, &mut tree, &renderer, &layout, wheel(-1.0));
        event(
            &mut widget,
            &mut tree,
            &renderer,
            &layout,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        );
        assert!(tree.state.downcast_ref::<State>().motion.is_none());
    }
}
