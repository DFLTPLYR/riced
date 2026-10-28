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
        coalesce_motion: false,
        press: None,
    })
}

/// Layer apps rebuild Lua views on input, so keep only the latest move per frame.
pub(crate) fn frame_input<M: Clone + 'static>(
    content: Element<'static, M>,
    sender: Sender<M>,
) -> Element<'static, M> {
    Element::new(Input {
        content,
        sender,
        hold: HOLD,
        coalesce_motion: true,
        press: None,
    })
}

/// A keyed press retains its own measured bounds for popup anchoring.
pub(crate) fn on_press<M: Clone + 'static>(
    content: Element<'static, M>,
    key: String,
    sender: Sender<M>,
) -> Element<'static, M> {
    Element::new(Input {
        content,
        sender,
        hold: HOLD,
        coalesce_motion: false,
        press: Some(key),
    })
}

struct Input<M> {
    content: Element<'static, M>,
    sender: Sender<M>,
    hold: Duration,
    coalesce_motion: bool,
    press: Option<String>,
}

#[derive(Default)]
struct State {
    pressed: Option<(mouse::Button, Instant, iced::Point)>,
    held: bool,
    inside: bool,
    pending_motion: Option<Action>,
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
        // Flush before child controls can consume a release, and before exit.
        let state = tree.state.downcast_mut::<State>();
        let dragging = self.coalesce_motion && state.pressed.is_some();
        if (matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_) | iced::window::Event::Unfocused)
                | Event::Mouse(
                    mouse::Event::ButtonPressed(_)
                        | mouse::Event::ButtonReleased(_)
                        | mouse::Event::CursorLeft
                )
        ) || (!dragging && (!cursor.is_over(layout.bounds()) || !cursor.is_over(*viewport))))
            && let Some(action) = state.pending_motion.take()
        {
            shell.publish((self.sender)(action));
        }
        if !dragging
            || !matches!(
                event,
                Event::Mouse(
                    mouse::Event::CursorMoved { .. }
                        | mouse::Event::ButtonPressed(_)
                        | mouse::Event::ButtonReleased(_)
                )
            )
        {
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
        let state = tree.state.downcast_mut::<State>();
        let inside = cursor.is_over(layout.bounds()) && cursor.is_over(*viewport);
        let position = cursor.position().map(|point| {
            iced::Point::new(point.x - layout.bounds().x, point.y - layout.bounds().y)
        });
        if let Some(key) = &self.press {
            if !shell.is_event_captured()
                && inside
                && matches!(
                    event,
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                )
            {
                shell.publish((self.sender)(Action {
                    key: key.clone(),
                    value: None,
                    event: InputEvent::Mouse {
                        state: "action".into(),
                        button: Some("left".into()),
                        position,
                        origin: position,
                        bounds: Some(layout.bounds()),
                    },
                }));
                shell.capture_event();
            }
            return;
        }
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
            if !inside && !dragging {
                state.pressed = None;
            }
        }
        if shell.is_event_captured() {
            state.pressed = None;
            return;
        }
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) if inside || dragging => {
                let action = pointer("move", None, state.pressed.map(|(_, _, origin)| origin));
                if self.coalesce_motion {
                    state.pending_motion = Some(action);
                    shell.request_redraw();
                } else {
                    shell.publish((self.sender)(action));
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) if inside || dragging => {
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
                if inside || dragging {
                    shell.publish((self.sender)(pointer(
                        "release",
                        Some(*button),
                        state.pressed.map(|(_, _, origin)| origin),
                    )));
                }
                if state
                    .pressed
                    .is_some_and(|(pressed, _, _)| pressed == *button)
                    && let Some((_, time, origin)) = state.pressed.take()
                    && (inside || dragging)
                {
                    if inside && !state.held && time.elapsed() < self.hold {
                        shell.publish((self.sender)(pointer("click", Some(*button), Some(origin))));
                    }
                    shell.capture_event();
                }
            }
            Event::Window(iced::window::Event::Unfocused) if dragging => {
                state.pressed = None;
                state.pending_motion = None;
                shell.publish((self.sender)(pointer("cancel", None, None)));
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
        if self.press.is_some() && cursor.is_over(layout.bounds()) && cursor.is_over(*viewport) {
            return mouse::Interaction::Pointer;
        }
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
    fn keyed_presses_report_the_selected_icon_bounds_and_keep_lua_action_events() {
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let icon = |key: &str| {
            on_press(
                iced::widget::Space::new().width(20).height(20).into(),
                key.into(),
                std::sync::Arc::new(|action| action),
            )
        };
        let mut element: Element<'static, Action> =
            iced::widget::row![icon("first"), icon("second")]
                .spacing(8)
                .into();
        let mut tree = widget::Tree::new(&element);
        let node = element
            .as_widget_mut()
            .layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, Size::new(300.0, 100.0)),
            )
            .move_to(iced::Point::new(100.0, 20.0));
        let lua = mlua::Lua::new();
        for (key, x) in [("first", 100.0), ("second", 128.0)] {
            let mut messages = Vec::new();
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Layout::new(&node),
                mouse::Cursor::Available(iced::Point::new(x + 10.0, 30.0)),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(Size::new(300.0, 100.0)),
            );
            assert_eq!(messages.len(), 1);
            let action = &messages[0];
            assert_eq!(action.key, key);
            assert!(
                matches!(&action.event, InputEvent::Mouse { bounds: Some(bounds), .. }
                if *bounds == Rectangle::new(iced::Point::new(x, 20.0), Size::new(20.0, 20.0)))
            );
            let args = action.arguments(&lua).unwrap();
            let mlua::Value::Table(events) = &args[0] else {
                panic!("events table")
            };
            assert_eq!(events.get::<String>("type").unwrap(), "action");
            assert_eq!(events.get::<String>("key").unwrap(), key);
        }
    }

    #[test]
    fn frame_drag_keeps_implicit_grab_outside_surface_until_release_or_cancel() {
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        for ending in [
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            Event::Window(iced::window::Event::Unfocused),
        ] {
            let mut element = frame_input(
                iced::widget::Space::new().width(100).height(50).into(),
                std::sync::Arc::new(|event| event),
            );
            let mut tree = widget::Tree::new(&element);
            let size = Size::new(100.0, 50.0);
            let layout = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
            let mut messages = Vec::new();
            for (event, cursor) in [
                (
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                    mouse::Cursor::Available(iced::Point::new(20.0, 10.0)),
                ),
                (
                    Event::Mouse(mouse::Event::CursorLeft),
                    mouse::Cursor::Unavailable,
                ),
                (
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Right)),
                    mouse::Cursor::Available(iced::Point::new(150.0, -10.0)),
                ),
                (
                    Event::Mouse(mouse::Event::CursorMoved {
                        position: iced::Point::new(150.0, -10.0),
                    }),
                    mouse::Cursor::Available(iced::Point::new(150.0, -10.0)),
                ),
                (
                    ending.clone(),
                    mouse::Cursor::Available(iced::Point::new(150.0, -10.0)),
                ),
            ] {
                element.as_widget_mut().update(
                    &mut tree,
                    &event,
                    Layout::new(&layout),
                    cursor,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut Shell::new(&mut messages),
                    &Rectangle::with_size(size),
                );
            }
            assert!(tree.state.downcast_ref::<State>().pressed.is_none());
            assert!(messages.iter().any(|action| matches!(&action.event, InputEvent::Mouse {state,position:Some(position),..} if state=="move" && *position==iced::Point::new(150.0,-10.0))));
            let expected = if matches!(ending, Event::Window(_)) {
                "cancel"
            } else {
                "release"
            };
            assert!(messages.iter().any(
                |action| matches!(&action.event, InputEvent::Mouse {state,..} if state==expected)
            ));
            assert!(!messages.iter().any(
                |action| matches!(&action.event, InputEvent::Mouse {state,..} if state=="click")
            ));
        }
    }

    #[test]
    fn frame_motion_is_latest_wins_and_flushes_before_release_or_exit() {
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        for ending in [
            Event::Window(iced::window::Event::RedrawRequested(Instant::now())),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            Event::Mouse(mouse::Event::CursorLeft),
        ] {
            let mut element = frame_input(
                iced::widget::Space::new().width(100).height(50).into(),
                std::sync::Arc::new(|event| event),
            );
            let mut tree = widget::Tree::new(&element);
            let size = Size::new(100.0, 50.0);
            let layout = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
            tree.state.downcast_mut::<State>().inside = true;
            let mut messages = Vec::new();
            for x in 1..=100 {
                let position = iced::Point::new(x as f32 / 2.0, 10.0);
                let mut shell = Shell::new(&mut messages);
                element.as_widget_mut().update(
                    &mut tree,
                    &Event::Mouse(mouse::Event::CursorMoved { position }),
                    Layout::new(&layout),
                    mouse::Cursor::Available(position),
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(size),
                );
                assert_eq!(
                    shell.redraw_request(),
                    iced::window::RedrawRequest::NextFrame
                );
            }
            assert!(messages.is_empty());
            // A normal host view rebuild must preserve the queued sample.
            tree.diff(&element);
            let cursor = if matches!(ending, Event::Mouse(mouse::Event::CursorLeft)) {
                mouse::Cursor::Unavailable
            } else {
                mouse::Cursor::Available(iced::Point::new(50.0, 10.0))
            };
            element.as_widget_mut().update(
                &mut tree,
                &ending,
                Layout::new(&layout),
                cursor,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
            assert!(
                matches!(&messages[0].event, InputEvent::Mouse { state, position: Some(position), .. } if state == "move" && *position == iced::Point::new(50.0, 10.0))
            );
            assert_eq!(messages.iter().filter(|action| matches!(&action.event, InputEvent::Mouse {state,..} if state == "move")).count(), 1);
            assert!(tree.state.downcast_ref::<State>().pending_motion.is_none());
            if matches!(ending, Event::Mouse(mouse::Event::ButtonReleased(_))) {
                assert!(
                    matches!(&messages[1].event, InputEvent::Mouse {state,..} if state == "release")
                );
            }
            if matches!(ending, Event::Mouse(mouse::Event::CursorLeft)) {
                assert!(
                    matches!(&messages[1].event, InputEvent::Mouse {state,..} if state == "exit")
                );
            }
        }
    }

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
        let mut element = frame_input(button.into(), std::sync::Arc::new(|events| events));
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
        assert!(!messages.iter().any(|events| matches!(&events.event, InputEvent::Mouse{state,..} if state == "press" || state == "click" || state == "hold")));
    }
}
