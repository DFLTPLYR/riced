mod context_menu;
mod editors;
mod pages;
mod panel;
mod placement;
mod wallpaper;
use crate::config::BackgroundImage;
use crate::shell::Plant;
use crate::shell::screens::ContextMenu;
use crate::shell::{state::Plots, windows::PlotInfo};
use crate::theme;
use crate::ui::widgets::display_map::MapView;
use iced::widget::{Space, button, column, container, row, rule, scrollable, text};
use iced::window;
use iced::{Element, Length, Task as Command};
use iced_exwlshell::actions::IcedXdgWindowSettings;
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use std::collections::HashMap;

/// Floating XDG toplevel for the Settings panel (spawned via `Plant::Sprout`).
/// Mirrors `Top` / `Background`: spawn + view live here, `Plots` just delegates.
#[derive(Debug, Default)]
pub struct Setting {
    page: SettingPage,
    /// Pan/zoom/drag view for the Background map. Single source of truth for
    /// both stacked canvas programs (canvas `State` can't be shared across
    /// the two widgets), updated via `SettingEvent::MapViewChanged`.
    map_view: MapView,
    /// Bar picked in the Panel page picker row (`None` = first bar).
    /// Stored per-window like `page`, so each panel keeps its own pick.
    selected_bar: Option<window::Id>,
    /// Wallpaper image picked in the editor below the map (`None` = first
    /// image). Index into `[[background.image]]`; remapped on remove.
    selected_image: Option<usize>,
    /// Slot picked in the Panel page align row (`None` = first slot).
    /// Stored per-window like `selected_bar`.
    selected_slot: Option<usize>,
    /// Placement picked in the Panel preview (`None` = none selected).
    /// Stored per-window; cleared when its placement leaves the bar.
    selected_placement: Option<String>,
}

/// Master-detail pages: nav buttons on the left switch this, the right pane
/// renders the matching controls. Stored per-window so each panel keeps its
/// own selection (like `Top` config lives on its own bar).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingPage {
    #[default]
    ContextMenu,
    Panel,
    Wallpaper,
    Theme,
    Animation,
}

impl SettingPage {
    fn all() -> [Self; 5] {
        [
            Self::ContextMenu,
            Self::Panel,
            Self::Wallpaper,
            Self::Theme,
            Self::Animation,
        ]
    }

    fn title(self) -> &'static str {
        match self {
            Self::Panel => "Panel",
            Self::ContextMenu => "Context Menu",
            Self::Wallpaper => "Wallpaper",
            Self::Theme => "Theme",
            Self::Animation => "Animation",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Panel => "Choose a bar, arrange its widgets, and fine-tune its appearance.",
            Self::ContextMenu => "Customize the menu opened by right-clicking the wallpaper.",
            Self::Wallpaper => "Arrange images across your displays and adjust their placement.",
            Self::Theme => "Choose a palette and preview its colors in light or dark mode.",
            Self::Animation => "Set the pace of animated color and property changes.",
        }
    }
}

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

    pub fn view(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        container(
            row![
                scrollable(self.nav(id))
                    .width(Length::Fixed(168.0))
                    .height(Length::Fill),
                scrollable(
                    column![
                        page_heading(self.page.title(), self.page.description()),
                        self.content(id, plots),
                        hint("Changes apply live and are saved automatically."),
                    ]
                    .spacing(20)
                    .width(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .spacing(8),
            ]
            .spacing(24)
            .padding(24),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::bar)
        .into()
    }

    /// Left nav pane (30%): one button per page, highlighted when selected.
    fn nav(&self, id: window::Id) -> Element<'_, Plant> {
        let mut col = column![
            text("RICED").size(20).color(theme::text()),
            hint("Make it yours"),
            rule::horizontal(1)
        ]
        .spacing(12);
        for page in SettingPage::all() {
            let selected = page == self.page;
            col = col.push(
                button(text(page.title()).size(14).color(theme::text()))
                    .width(Length::Fill)
                    .on_press(Plant::SettingPlot(crate::shell::SettingEvent::Select(
                        id, page,
                    )))
                    .padding(12)
                    .style(theme::nav_button(selected)),
            );
        }
        col.spacing(10).width(Length::Fill).into()
    }

    /// Right content pane (70%): live controls for the selected page. Each
    /// slider reads the single `plots.config` source of truth and writes back
    /// via `ConfigEvent::Patch`, so every layer updates on the next redraw.
    fn content(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        match self.page {
            SettingPage::Panel => self.panel_content(id, panel::PanelContext::from(plots)),
            SettingPage::ContextMenu => self.context_menu_content(plots),
            SettingPage::Wallpaper => self.wallpaper_content(id, plots),
            SettingPage::Theme => self.theme_content(plots),
            SettingPage::Animation => self.animation_content(plots),
        }
    }

    fn context_menu_content(&self, plots: &Plots) -> Element<'_, Plant> {
        context_menu::view(context_menu::Context::from(plots))
    }

    /// Panel page: picker row of bars on top, controls for the picked bar
    /// below (defaults to the first). Length % + thickness px, floating
    /// (+ margins when floating) and per-corner rounding.
    fn panel_content(&self, id: window::Id, plots: panel::PanelContext<'_>) -> Element<'_, Plant> {
        panel::view(self, id, plots)
    }

    /// Theme picker: dark/light toggle plus one button per
    /// `~/.config/riced/theme/*.json` (same files as reshell). Writes back
    /// via `ConfigEvent::Patch`, so the daemon re-themes on the next redraw
    /// and persists the choice to `config.toml`.
    fn theme_content(&self, plots: &Plots) -> Element<'_, Plant> {
        pages::theme_page(&plots.config.theme)
    }

    /// Animation speed picker: one global speed for every animated
    /// transition (selection fade and any interpolated color/property
    /// change). Writes back via `ConfigEvent::Patch`, so the daemon
    /// re-times on the next redraw and persists the choice to `config.toml`.
    fn animation_content(&self, plots: &Plots) -> Element<'_, Plant> {
        pages::animation_page(plots.config.animation.speed)
    }

    fn wallpaper_content(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        wallpaper::view(self, id, &wallpaper::WallpaperContext::from(plots))
    }

    /// Replace the map pan/zoom/drag view (`MapViewChanged`). Field stays
    /// private; `Plots` writes through here like cursor maps elsewhere.
    pub(crate) fn set_map_view(&mut self, view: MapView) {
        if let Some(index) = view.selected {
            self.selected_image = Some(index);
        }
        self.map_view = view;
    }

    /// Current map view (for drop detection: an image drag ending means
    /// wallpapers moved, even though per-move patches never dirty the theme).
    pub(crate) fn map_view(&self) -> MapView {
        self.map_view
    }

    /// Switch the selected page on `SettingEvent::Select`. Opening the
    /// Wallpaper page builds the map canvases fresh, whose first frames can
    /// go out empty like new Background surfaces — schedule the same heals.
    pub(crate) fn handle_select(
        plots: &mut Plots,
        id: window::Id,
        page: SettingPage,
    ) -> Command<Plant> {
        if let Some(setting) = plots.windows.settings.get_mut(&id) {
            setting.page = page;
        }
        if page == SettingPage::Wallpaper {
            return Plots::repaint_after(80);
        }
        Command::none()
    }

    /// Pick the bar the Panel page edits (`SettingEvent::SelectBar`).
    /// Clears the placement pick (ids resolve per bar).
    pub(crate) fn handle_select_bar(
        plots: &mut Plots,
        id: window::Id,
        bar: window::Id,
    ) -> Command<Plant> {
        if let Some(setting) = plots.windows.settings.get_mut(&id) {
            setting.selected_bar = Some(bar);
            setting.selected_placement = None;
        }
        Command::none()
    }

    /// Pick the slot the Panel page aligns (`SettingEvent::SelectSlot`).
    /// Clears the placement pick so the editor follows chip clicks.
    pub(crate) fn handle_select_slot(
        plots: &mut Plots,
        id: window::Id,
        pos: usize,
    ) -> Command<Plant> {
        if let Some(setting) = plots.windows.settings.get_mut(&id) {
            setting.selected_slot = Some(pos);
            setting.selected_placement = None;
        }
        Command::none()
    }

    /// Pick the placement the Panel page edits
    /// (`SettingEvent::SelectPlacement`). Also selects its slot so the
    /// align picker follows chip clicks. Unknown ids clear the editor
    /// (stale click after a drag or reload).
    pub(crate) fn handle_select_placement(
        plots: &mut Plots,
        id: window::Id,
        bar: window::Id,
        placement: String,
    ) -> Command<Plant> {
        let slot = plots.windows.tops.get(&bar).and_then(|top| {
            top.local
                .widgets
                .iter()
                .position(|slot| slot.iter().any(|p| p.id == placement))
        });
        if let Some(setting) = plots.windows.settings.get_mut(&id) {
            setting.selected_slot = slot.or(setting.selected_slot);
            setting.selected_placement = slot.map(|_| placement);
        }
        Command::none()
    }

    /// Pick the wallpaper image the editor below the map edits
    /// (`SettingEvent::SelectImage`).
    pub(crate) fn handle_select_image(
        plots: &mut Plots,
        id: window::Id,
        index: usize,
    ) -> Command<Plant> {
        if let Some(setting) = plots.windows.settings.get_mut(&id) {
            setting.selected_image = Some(index);
            setting.map_view.selected = Some(index);
        }
        Command::none()
    }

    /// Remap the picked image after `RemoveImage{index}` (later entries
    /// shift down by one; a removed pick clears to first).
    pub(crate) fn image_removed(&mut self, index: usize) {
        match self.selected_image {
            Some(s) if s == index => self.selected_image = None,
            Some(s) if s > index => self.selected_image = Some(s - 1),
            _ => {}
        }
        match self.map_view.selected {
            Some(s) if s == index => self.map_view.selected = None,
            Some(s) if s > index => self.map_view.selected = Some(s - 1),
            _ => {}
        }
        if let Some(drag) = &mut self.map_view.drag {
            match drag.image {
                Some(s) if s == index => self.map_view.drag = None,
                Some(s) if s > index => drag.image = Some(s - 1),
                _ => {}
            }
        }
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
        let setting = Setting::default();
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
        if let Some(cm) = &mut plots.desktop.context_menu {
            cm.open = false;
        }
        if plots.windows.settings.is_empty() {
            return Self::handle_add(
                &mut plots.windows.settings,
                &mut plots.windows.ids,
                &mut plots.desktop.context_menu,
            );
        }
        let to_close = Self::close_all(&mut plots.windows.settings, &mut plots.windows.ids);
        // Panel edits are done: persist staged config now instead of waiting
        // out the coalescing save timer (Uproot flushes again, harmless).
        plots.flush_config_save();
        let mut cmds = Vec::with_capacity(to_close.len());
        for id in to_close {
            plots.input.cursors.remove(&id);
            plots.input.presses.remove(&id);
            cmds.push(iced_runtime::task::effect(Action::Window(
                WindowAction::Close(id),
            )));
        }
        // Toggle-closed with pending wallpaper edits: the user is done —
        // regen now instead of waiting out the countdown.
        if plots.theme_jobs.dirty {
            cmds.push(plots.fire_regen_theme());
        }
        if cmds.is_empty() {
            Command::none()
        } else {
            Command::batch(cmds)
        }
    }
}

fn hint(message: impl Into<String>) -> Element<'static, Plant> {
    text(message.into())
        .size(12)
        .color(theme::text_dim())
        .into()
}

fn page_heading(title: &str, description: &str) -> Element<'static, Plant> {
    column![
        text(title.to_owned()).size(26).color(theme::text()),
        hint(description),
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

fn section_heading(title: &str, description: &str) -> Element<'static, Plant> {
    column![
        text(title.to_owned()).size(16).color(theme::text()),
        hint(description),
    ]
    .spacing(5)
    .width(Length::Fill)
    .into()
}

fn section<'a>(title: &str, description: &str, body: Element<'a, Plant>) -> Element<'a, Plant> {
    container(column![section_heading(title, description), body].spacing(16))
        .padding(18)
        .width(Length::Fill)
        .style(theme::menu_box)
        .into()
}

/// Single palette swatch box for theme preview rows: fixed-size tile
/// painted with the previewed theme's own color.
fn swatch(color: iced::Color, border: iced::Color) -> Element<'static, Plant> {
    container(
        Space::new()
            .width(Length::Fixed(18.0))
            .height(Length::Fixed(18.0)),
    )
    .style(theme::swatch(color, border))
    .into()
}

/// Short file name for an image entry (`"(empty path)"` for sparse entries).
fn image_file_name(img: &BackgroundImage) -> String {
    const EMPTY: &str = "(empty path)";
    let name = img
        .local_path()
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.is_empty() {
        EMPTY.to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_selection_updates_wallpaper_input_selection() {
        let mut setting = Setting::default();
        setting.set_map_view(MapView {
            selected: Some(2),
            ..Default::default()
        });
        assert_eq!(setting.selected_image, Some(2));
        // Panning empty space should retain the last inspected image.
        setting.set_map_view(MapView::default());
        assert_eq!(setting.selected_image, Some(2));
    }

    #[test]
    fn removing_wallpaper_remaps_editor_and_map_selection() {
        let mut setting = Setting::default();
        setting.set_map_view(MapView {
            selected: Some(2),
            drag: Some(crate::ui::widgets::display_map::MapDrag {
                image: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        });
        setting.image_removed(0);
        assert_eq!(setting.selected_image, Some(1));
        assert_eq!(setting.map_view.selected, Some(1));
        assert_eq!(setting.map_view.drag.unwrap().image, Some(1));
        setting.image_removed(1);
        assert_eq!(setting.selected_image, None);
        assert_eq!(setting.map_view.selected, None);
        assert!(setting.map_view.drag.is_none());
    }
}
