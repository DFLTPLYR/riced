use crate::app::Plant;
use crate::app::app::{PlotInfo, Plots};
use crate::app::layers::ContextMenu;
use iced::widget::{Space, container};
use iced::window;
use iced::{Color, Element, Length, Task as Command};
use iced_exwlshell::actions::IcedXdgWindowSettings;
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
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

    /// Fresh id + XDG settings for a new Settings window.
    /// Client-side decorations: no compositor titlebar/frame (which would stay
    /// opaque and unblurred) — the whole surface is ours to keep transparent.
    /// Tradeoff: no X button, close via the context-menu/CLI toggle instead.
    pub fn open(&self) -> (window::Id, IcedXdgWindowSettings) {
        (
            window::Id::unique(),
            IcedXdgWindowSettings {
                client_side_decorations: true,
                ..Default::default()
            },
        )
    }

    pub fn view(&self, _id: window::Id) -> Element<'_, Plant> {
        // Fully transparent root: with the daemon's transparent clear color
        // (see `main.rs:.style`), Hyprland blur/opacity windowrules can see
        // straight through this. Use `from_rgba(0,0,0,0.25)` for a frosted tint.
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Color::TRANSPARENT.into()),
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

    /// Remove all tracked Settings windows. Returns the closed ids so the
    /// caller can drop cursor/press state and emit idempotent Close effects.
    /// (The compositor's X button path via `close_events -> Uproot` also
    /// calls `remove`, so double-close is harmless.)
    pub(crate) fn close_all(
        settings: &mut HashMap<window::Id, Setting>,
        ids: &mut HashMap<window::Id, PlotInfo>,
    ) -> Vec<window::Id> {
        let to_close: Vec<window::Id> = settings.keys().copied().collect();
        for id in &to_close {
            Self::remove(settings, ids, *id);
        }
        to_close
    }

    /// Toggle `Plant::Sprout` / CLI `open-settings`: spawn when none is open,
    /// else close the current settings panel(s). Always closes the context
    /// menu first (like `TopEvent::Sow`).
    pub(crate) fn handle_toggle(plots: &mut Plots) -> Command<Plant> {
        if let Some(cm) = &mut plots.context_menu {
            cm.open = false;
        }
        if plots.settings.is_empty() {
            return Self::handle_add(&mut plots.settings, &mut plots.ids, &mut plots.context_menu);
        }
        let to_close = Self::close_all(&mut plots.settings, &mut plots.ids);
        let mut cmds = Vec::with_capacity(to_close.len());
        for id in to_close {
            plots.last_cursor.remove(&id);
            plots.press_starts.remove(&id);
            cmds.push(iced_runtime::task::effect(Action::Window(
                WindowAction::Close(id),
            )));
        }
        if cmds.is_empty() {
            Command::none()
        } else {
            Command::batch(cmds)
        }
    }
}
