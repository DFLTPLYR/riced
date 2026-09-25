use crate::app::Plant;
use crate::app::app::PlotInfo;
use crate::app::layers::ContextMenu;
use iced::widget::{Space, container};
use iced::window;
use iced::{Color, Element, Length, Task as Command};
use iced_exwlshell::actions::IcedXdgWindowSettings;
use std::collections::HashMap;

/// Floating XDG toplevel for the Settings panel (spawned via `Plant::Sprout`).
/// Mirrors `Top` / `Background`: spawn + view live here, `Plots` just delegates.
#[derive(Debug, Default)]
pub struct Setting;

impl Setting {
    pub const TITLE: &'static str = "Riced Settings";

    pub fn title() -> String {
        String::from(Self::TITLE)
    }

    /// Fresh id + default XDG settings for a new Settings window.
    pub fn open(&self) -> (window::Id, IcedXdgWindowSettings) {
        (window::Id::unique(), IcedXdgWindowSettings::default())
    }

    /// Window translucency: `0.0` transparent, `1.0` opaque.
    /// iced 0.14 has no opacity widget, so this is applied as a
    /// translucent background (identical effect on an empty container).
    pub const OPACITY: f32 = 0.5;

    pub fn view(&self, _id: window::Id) -> Element<'_, Plant> {
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Color::from_rgba(0.0, 0.0, 0.0, Self::OPACITY).into()),
                ..Default::default()
            })
            .into()
    }

    /// Handle `Plant::Sprout`: close the context menu (like `TopEvent::Sow`),
    /// track the new window, and return the `NewBaseWindow` command.
    /// Equivalent to `Plant::base_window_open(IcedXdgWindowSettings::default())`
    /// (that helper is private to `events`, so the message is built directly).
    pub(crate) fn handle_add(
        settings: &mut HashMap<window::Id, Setting>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        context_menu: &mut Option<ContextMenu>,
    ) -> Command<Plant> {
        if let Some(cm) = context_menu {
            cm.open = false;
        }
        let setting = Setting;
        let (id, xdg_settings) = setting.open();
        settings.insert(id, setting);
        ids.insert(id, PlotInfo::Setting);
        Command::done(Plant::NewBaseWindow {
            settings: xdg_settings,
            id,
        })
    }

    /// Remove a Settings window from tracking. Returns true if one existed.
    /// Caller handles cursor/press cleanup + the idempotent Close effect.
    pub(crate) fn remove(
        settings: &mut HashMap<window::Id, Setting>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        id: window::Id,
    ) -> bool {
        ids.remove(&id);
        settings.remove(&id).is_some()
    }
}
