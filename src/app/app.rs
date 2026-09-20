use iced::Color;
use iced::widget::{Space, container};
use iced::window::Id;
use iced::{Element, Event, Fill, Task as Command, event};
use iced_exwlshell::actions::{IcedNewMenuSettings, IcedXdgWindowSettings};
use iced_exwlshell::redraw::Scope;
use iced_runtime::window::Action as WindowAction;
use iced_runtime::{Action, task};
use std::collections::HashMap;

use iced_exwlshell::daemon;
use iced_exwlshell::reexport::{
    Anchor, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings, OutputOption, PixelSize,
};
use iced_exwlshell::settings::{LayerShellSettings, Settings, StartMode};
use iced_exwlshell::to_layer_message;
use iced_wayland_subscriber::shell::{ShellEvent, ShellReceiver};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use wayland_client::Connection;

use super::layers::{Background, Top};
use super::{Message, WayEvent};

#[derive(Debug)]
pub struct Monitor {
    value: i32,
    ids: HashMap<iced::window::Id, WindowInfo>,
    /// The bar opened for each output, so a retracted output closes its own.
    bars: HashMap<OutputId, iced::window::Id>,
    shell_events: ShellReceiver,

    top: Top,
    background: Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowInfo {
    Background,
    TopBar,
}

impl Monitor {
    pub fn style(&self, _theme: &iced::Theme) -> iced::theme::Style {
        iced::theme::Style {
            background_color: Color::TRANSPARENT,
            text_color: Color::WHITE,
        }
    }
    pub fn window_id(&self, info: &WindowInfo) -> Option<&iced::window::Id> {
        for (k, v) in self.ids.iter() {
            if info == v {
                return Some(k);
            }
        }
        None
    }
}

impl Monitor {
    pub fn new(shell_events: ShellReceiver) -> Self {
        Self {
            value: 0,
            ids: HashMap::new(),
            bars: HashMap::new(),
            shell_events,
            top: Top::new(),
            background: Background,
        }
    }

    pub fn id_info(&self, id: iced::window::Id) -> Option<WindowInfo> {
        self.ids.get(&id).cloned()
    }

    pub fn remove_id(&mut self, id: iced::window::Id) {
        self.ids.remove(&id);
        // A real unplug closes the surface compositor-side, so the bar can go
        // without an `OutputEvent::Removed` ever arriving.
        self.bars.retain(|_, bar| *bar != id);
    }

    pub fn namespace() -> String {
        String::from("Riced - Bottom")
    }

    pub fn subscription(&self) -> iced::Subscription<Message> {
        iced::Subscription::batch(vec![
            iced::window::close_events().map(Message::WindowClosed),
            self.shell_events.listen().filter_map(|event| match event {
                ShellEvent::OutputAdded(output) => {
                    Some(Message::Wayland(WayEvent::OutputInsert(output)))
                }
                ShellEvent::OutputRemoved(output) => Some(Message::Wayland(
                    WayEvent::OutputRemoved(OutputId::from(&output)),
                )),
                _ => None,
            }),
        ])
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        use iced::Event;
        use iced::keyboard;
        use iced::keyboard::key::Named;
        match message {
            Message::WindowClosed(id) => {
                self.remove_id(id);
                Command::none()
            }
            Message::IcedEvent(event) => {
                match event {
                    Event::Keyboard(keyboard::Event::KeyPressed {
                        key: keyboard::Key::Named(Named::Escape),
                        ..
                    }) => {}
                    Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Right)) => {
                    }
                    _ => {}
                }
                Command::none()
            }
            Message::Wayland(WayEvent::OutputRemoved(output)) => {
                let Some(id) = self.bars.remove(&output) else {
                    return Command::none();
                };
                iced_runtime::task::effect(Action::Window(WindowAction::Close(id)))
            }
            Message::Wayland(WayEvent::OutputInsert(output)) => {
                let output_id = OutputId::from(&output);

                let (id_top, top_settings) = self.top.open(output_id.0);
                let (id_background, background_settings) = self.background.open(output_id.0);

                self.ids.insert(id_top, WindowInfo::TopBar);
                self.ids.insert(id_background, WindowInfo::Background);

                Command::batch([
                    Command::done(Message::NewLayerShell {
                        settings: top_settings,
                        id: id_top,
                    }),
                    Command::done(Message::NewLayerShell {
                        settings: background_settings,
                        id: id_background,
                    }),
                ])
            }
            _ => unreachable!(),
        }
    }

    pub fn view(&self, id: iced::window::Id) -> Element<'_, Message> {
        if let Some(WindowInfo::TopBar) = self.id_info(id) {
            return self.top.view();
        }

        Space::new().into()
    }
}

pub fn redraw_scope(message: &Message) -> Scope {
    match message {
        _ => Scope::All,
    }
}
