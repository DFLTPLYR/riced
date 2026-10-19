//! Placement-local pointer and keyboard events, after child controls.
use crate::ui::action::{Action, Event as InputEvent, Sender};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
use std::time::{Duration, Instant};

const HOLD: Duration = Duration::from_millis(500);

pub(crate) fn input<M: Clone + 'static>(
    content: Element<'static, M>,
    sender: Sender<M>,
) -> Element<'static, M> {
    with_hold(content, sender, HOLD)
}

pub(crate) fn with_hold<M: Clone + 'static>(
    content: Element<'static, M>,
    sender: Sender<M>,
    hold: Duration,
) -> Element<'static, M> {
    Element::new(Input {
        content,
        sender,
        hold,
    })
}

struct Input<M> {
    content: Element<'static, M>,
    sender: Sender<M>,
    hold: Duration,
}

#[derive(Default)]
struct State {
    pressed: Option<(mouse::Button, Instant, iced::Point)>,
    held: bool,
    inside: bool,
}

impl<M: Clone + 'static> Widget<M, iced::Theme, iced::Renderer> for Input<M> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }
    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut widget::Tree) {
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
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
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
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
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
        let state = tree.state.downcast_mut::<State>();
        let inside = cursor.is_over(layout.bounds()) && cursor.is_over(*viewport);
        let position = cursor.position().map(|point| {
            iced::Point::new(point.x - layout.bounds().x, point.y - layout.bounds().y)
        });
        let pointer =
            |kind: &str, button: Option<mouse::Button>, origin: Option<iced::Point>| Action {
                key: String::new(),
                value: None,
                event: InputEvent::Mouse {
                    state: kind.into(),
                    button: button.map(|button| match button {
                        mouse::Button::Left => "left".into(),
                        mouse::Button::Right => "right".into(),
                        mouse::Button::Middle => "middle".into(),
                        other => format!("{other:?}").to_lowercase(),
                    }),
                    position,
                    origin,
                    bounds: Some(layout.bounds()),
                },
            };
        if inside != state.inside {
            shell.publish((self.sender)(pointer(
                if inside { "enter" } else { "exit" },
                None,
                None,
            )));
            state.inside = inside;
            if !inside {
                state.pressed = None;
            }
        }
        if shell.is_event_captured() {
            state.pressed = None;
            return;
        }
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) if inside => {
                shell.publish((self.sender)(pointer("move", None, None)))
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) if inside => {
                if state
                    .pressed
                    .is_some_and(|(pressed, _, _)| pressed != *button)
                {
                    shell.publish((self.sender)(pointer("press", Some(*button), position)));
                    shell.capture_event();
                    return;
                }
                let now = Instant::now();
                state.pressed = Some((*button, now, position.unwrap_or(iced::Point::ORIGIN)));
                state.held = false;
                shell.request_redraw_at(now + self.hold);
                shell.publish((self.sender)(pointer("press", Some(*button), position)));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                if inside {
                    shell.publish((self.sender)(pointer(
                        "release",
                        Some(*button),
                        state.pressed.map(|(_, _, origin)| origin),
                    )));
                }
                if let Some((pressed, time, origin)) = state.pressed.take()
                    && inside
                    && pressed == *button
                {
                    if !state.held && time.elapsed() < self.hold {
                        shell.publish((self.sender)(pointer("click", Some(*button), Some(origin))));
                    }
                    shell.capture_event();
                }
            }
            Event::Window(iced::window::Event::RedrawRequested(_)) if inside && !state.held => {
                if let Some((button, started, origin)) = state.pressed {
                    let deadline = started + self.hold;
                    if Instant::now() >= deadline {
                        state.held = true;
                        shell.publish((self.sender)(pointer("hold", Some(button), Some(origin))));
                    } else {
                        shell.request_redraw_at(deadline);
                    }
                }
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) if inside => {
                shell.publish((self.sender)(key_event(key, "pressed", *modifiers)))
            }
            Event::Keyboard(iced::keyboard::Event::KeyReleased { key, modifiers, .. })
                if inside =>
            {
                shell.publish((self.sender)(key_event(key, "released", *modifiers)))
            }
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
        offset: Vector,
    ) -> Option<overlay::Element<'a, M, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            offset,
        )
    }
}

fn key_event(
    key: &iced::keyboard::Key,
    state: &str,
    modifiers: iced::keyboard::Modifiers,
) -> Action {
    Action {
        key: match key {
            iced::keyboard::Key::Character(value) => value.to_string(),
            iced::keyboard::Key::Named(value) => format!("{value:?}"),
            iced::keyboard::Key::Unidentified => "Unidentified".into(),
        },
        value: None,
        event: InputEvent::Key {
            state: state.into(),
            modifiers,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::renderer::Headless;

    #[test]
    fn pointer_events_are_local_and_delivered_keys_keep_modifiers() {
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element = input(
            iced::widget::Space::new().width(100).height(50).into(),
            std::sync::Arc::new(|events| events),
        );
        let mut tree = widget::Tree::new(&element);
        let size = Size::new(300.0, 200.0);
        let layout = element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size))
            .move_to(iced::Point::new(80.0, 60.0));
        let mut messages = Vec::new();
        for event in [
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Right)),
            Event::Keyboard(iced::keyboard::Event::KeyReleased {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                physical_key: iced::keyboard::key::Physical::Code(
                    iced::keyboard::key::Code::Escape,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: iced::keyboard::Modifiers::SHIFT,
            }),
        ] {
            element.as_widget_mut().update(
                &mut tree,
                &event,
                Layout::new(&layout),
                mouse::Cursor::Available(iced::Point::new(90.0, 75.0)),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
        }
        assert!(messages.iter().any(|action| matches!(&action.event,
            InputEvent::Mouse { state, bounds: Some(bounds), .. }
            if state == "click" && *bounds == Rectangle { x: 80.0, y: 60.0, width: 100.0, height: 50.0 }
        )));
        let lua = mlua::Lua::new();
        let tables: Vec<_> = messages
            .iter()
            .map(|events| {
                events
                    .arguments(&lua)
                    .unwrap()
                    .pop_front()
                    .unwrap()
                    .as_table()
                    .unwrap()
                    .clone()
            })
            .collect();
        let click = tables
            .iter()
            .find(|event| event.get::<String>("type").unwrap() == "click")
            .unwrap();
        assert_eq!(click.get::<String>("button").unwrap(), "right");
        assert_eq!(
            (
                click.get::<f32>("x").unwrap(),
                click.get::<f32>("y").unwrap()
            ),
            (10.0, 15.0)
        );
        let key = tables
            .iter()
            .find(|event| event.get::<String>("type").unwrap() == "key")
            .unwrap();
        assert_eq!(key.get::<String>("key").unwrap(), "Escape");
        assert_eq!(key.get::<String>("state").unwrap(), "released");
        assert!(
            key.get::<mlua::Table>("modifiers")
                .unwrap()
                .get::<bool>("shift")
                .unwrap()
        );
    }

    #[test]
    fn held_gesture_fires_once_and_consumed_buttons_do_not_emit_bare_clicks() {
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let hold = Duration::from_secs(5);
        let mut element = with_hold(
            iced::widget::Space::new().width(100).height(50).into(),
            std::sync::Arc::new(|events| events),
            hold,
        );
        let mut tree = widget::Tree::new(&element);
        let size = Size::new(100.0, 50.0);
        let layout = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        tree.state.downcast_mut::<State>().pressed = Some((
            mouse::Button::Left,
            Instant::now() - HOLD,
            iced::Point::new(7.0, 8.0),
        ));
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            &mut tree,
            &Event::Window(iced::window::Event::RedrawRequested(Instant::now())),
            Layout::new(&layout),
            mouse::Cursor::Available(iced::Point::new(10.0, 10.0)),
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::with_size(size),
        );
        assert!(
            !messages.iter().any(
                |event| matches!(&event.event, InputEvent::Mouse {state,..} if state == "hold")
            )
        );
        tree.state
            .downcast_mut::<State>()
            .pressed
            .as_mut()
            .unwrap()
            .1 = Instant::now() - hold;
        for event in [
            Event::Window(iced::window::Event::RedrawRequested(Instant::now())),
            Event::Window(iced::window::Event::RedrawRequested(Instant::now())),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ] {
            element.as_widget_mut().update(
                &mut tree,
                &event,
                Layout::new(&layout),
                mouse::Cursor::Available(iced::Point::new(10.0, 10.0)),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
        }
        let gestures: Vec<_> = messages
            .iter()
            .filter_map(|events| match &events.event {
                InputEvent::Mouse { state, .. } if state == "click" || state == "hold" => {
                    Some(state.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(gestures, ["hold"]);
        assert!(messages.iter().any(|event| matches!(&event.event,
            InputEvent::Mouse { state, origin: Some(origin), position: Some(position), .. }
            if state == "hold" && *origin == iced::Point::new(7.0, 8.0) && *position == iced::Point::new(10.0, 10.0)
        )));

        // A delayed redraw must not begin a drag after the button is already up.
        messages.clear();
        let state = tree.state.downcast_mut::<State>();
        state.held = false;
        state.pressed = Some((
            mouse::Button::Left,
            Instant::now() - hold,
            iced::Point::new(7.0, 8.0),
        ));
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            Layout::new(&layout),
            mouse::Cursor::Available(iced::Point::new(10.0, 10.0)),
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::with_size(size),
        );
        assert!(!messages.iter().any(|event| matches!(&event.event, InputEvent::Mouse {state,..} if state == "hold" || state == "click")));

        let button = iced::widget::button(iced::widget::Space::new().width(80).height(30))
            .on_press(Action::from("owned"));
        let mut element = input(button.into(), std::sync::Arc::new(|events| events));
        let mut tree = widget::Tree::new(&element);
        let layout = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        messages.clear();
        for event in [
            mouse::Event::ButtonPressed(mouse::Button::Left),
            mouse::Event::ButtonReleased(mouse::Button::Left),
        ] {
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(event),
                Layout::new(&layout),
                mouse::Cursor::Available(iced::Point::new(10.0, 10.0)),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
        }
        assert_eq!(
            messages
                .iter()
                .filter(|events| events.key == "owned")
                .count(),
            1
        );
        assert!(!messages.iter().any(|events| matches!(&events.event, InputEvent::Mouse{state,..} if state == "click" || state == "hold")));
    }
}
