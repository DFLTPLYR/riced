mod animation;
pub(crate) mod components;
mod context_menu;
mod desktop_page;
mod panel;
mod panel_bar;
mod placement;
mod shell_page;
mod sources;
mod theme_page;
mod wallpaper;
use crate::config::BackgroundImage;
use crate::shell::Plant;
use crate::shell::screens::ContextMenu;
use crate::shell::{state::Plots, windows::PlotInfo};
use crate::theme;
use crate::ui::widgets::display_map::MapView;
use iced::widget::{column, container, text};
use iced::window;
use iced::{Element, Length, Task as Command};
use iced_exwlshell::actions::IcedXdgWindowSettings;
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use std::collections::HashMap;

pub(crate) type IconPreviews = Vec<(String, Vec<Option<iced::widget::image::Handle>>)>;

/// Floating XDG toplevel for the Settings panel (spawned via `Plant::Sprout`).
/// Mirrors `Top` / `Background`: spawn + view live here, `Plots` just delegates.
#[derive(Debug, Default)]
pub struct Setting {
    animation_page: std::cell::RefCell<animation::Page>,
    theme_page: std::cell::RefCell<theme_page::Page>,
    placement_editor: std::cell::RefCell<placement::Editor>,
    panel_bar: std::cell::RefCell<panel_bar::Page>,
    components_page: std::cell::RefCell<components::Page>,
    desktop_page: std::cell::RefCell<desktop_page::Page>,
    wallpaper_page: std::cell::RefCell<wallpaper::Page>,
    context_menu_page: std::cell::RefCell<context_menu::Page>,
    shell_page: std::cell::RefCell<shell_page::Page>,
    icon_previews: Option<Result<IconPreviews, String>>,
    icon_previews_loading: bool,
    pub(crate) wallpaper_draft: Option<crate::wallpaper::draft::Draft>,
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
    /// Component picked in the Components page (`None` = first component).
    /// Stored per-window like `selected_bar`.
    pub(crate) selected_component: Option<usize>,
    /// Property picked in the Components page (`None` = first property).
    /// Stored per-window; follows the component pick.
    pub(crate) selected_property: Option<usize>,
    /// Draft Lua text for the Components page editor.
    /// `None` means the input mirrors the saved default.
    pub(crate) component_draft: Option<String>,
    pub(crate) component_error: Option<String>,
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
    Info,
    Desktop,
    Components,
    ContextMenu,
    Panel,
    Wallpaper,
    Theme,
    Animation,
    About,
}

impl SettingPage {
    /// Stable id used by `settings:page:<id>` actions and the shell snapshot.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Desktop => "desktop",
            Self::Components => "components",
            Self::ContextMenu => "context_menu",
            Self::Panel => "panel",
            Self::Wallpaper => "wallpaper",
            Self::Theme => "theme",
            Self::Animation => "animation",
            Self::About => "about",
        }
    }

    /// Validate one page id from Lua. Unknown ids stay inert.
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        match id {
            "info" => Some(Self::Info),
            "desktop" => Some(Self::Desktop),
            "components" => Some(Self::Components),
            "context_menu" => Some(Self::ContextMenu),
            "panel" => Some(Self::Panel),
            "wallpaper" => Some(Self::Wallpaper),
            "theme" => Some(Self::Theme),
            "animation" => Some(Self::Animation),
            "about" => Some(Self::About),
            _ => None,
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

    pub fn view(&self, id: window::Id, plots: &Plots) -> Element<'static, Plant> {
        let sender = std::sync::Arc::new(move |action| shell_page::action(action, id));
        let result = self
            .shell_page
            .borrow_mut()
            .node(
                self.page,
                &plots.config.theme,
                &plots.config.component_defaults,
            )
            .and_then(|node| {
                crate::ui::build::build_node_with_native(&node, 13.0, Some(sender), &mut |slot| {
                    if slot != "settings-page" {
                        return Err(format!("unknown native slot {slot:?}"));
                    }
                    Ok(self.content(id, plots))
                })
            });
        match result {
            Ok(element) => element,
            Err(error) => iced::widget::text(error).into(),
        }
    }

    /// Most pages edit live config; Wallpaper owns an isolated draft and
    /// publishes a desktop snapshot only after Apply completes.
    fn content(&self, id: window::Id, plots: &Plots) -> Element<'static, Plant> {
        match self.page {
            SettingPage::Info => column![].spacing(0).width(Length::Fill).into(),
            SettingPage::Desktop => {
                self.desktop_page
                    .borrow_mut()
                    .view(id, plots, &plots.config.theme)
            }
            SettingPage::Components => self.components_page.borrow_mut().view(
                id,
                components::Selection {
                    component: self.selected_component,
                    property: self.selected_property,
                    draft: self.component_draft.as_deref(),
                    error: self.component_error.as_deref(),
                },
                &plots.config.component_defaults,
                &plots.config.theme,
            ),
            SettingPage::Panel => self.panel_content(id, panel::PanelContext::from(plots)),
            SettingPage::ContextMenu => self.context_menu_content(plots),
            SettingPage::Wallpaper => self.wallpaper_content(id, plots),
            SettingPage::Theme => self.theme_content(plots),
            SettingPage::Animation => self.animation_content(plots),
            SettingPage::About => column![].spacing(0).width(Length::Fill).into(),
        }
    }

    fn context_menu_content(&self, plots: &Plots) -> Element<'static, Plant> {
        self.context_menu_page
            .borrow_mut()
            .view(plots, &plots.config.theme)
    }

    /// Panel page: picker row of bars on top, controls for the picked bar
    /// below (defaults to the first). Length % + thickness px, floating
    /// (+ margins when floating) and per-corner rounding.
    fn panel_content(
        &self,
        id: window::Id,
        plots: panel::PanelContext<'_>,
    ) -> Element<'static, Plant> {
        panel::view(self, id, plots)
    }

    /// Theme picker: dark/light toggle plus one button per
    /// `~/.config/riced/theme/*.json` (same files as reshell). Writes back
    /// via `ConfigEvent::Patch`, so the daemon re-themes on the next redraw
    /// and persists the choice to `config.lua`.
    fn theme_content(&self, plots: &Plots) -> Element<'static, Plant> {
        self.theme_page.borrow_mut().view(
            &plots.config.theme,
            self.icon_previews.as_ref(),
            &plots.config.component_defaults,
        )
    }

    /// Animation speed picker: one global speed for every animated
    /// transition (selection fade and any interpolated color/property
    /// change). Writes back via `ConfigEvent::Patch`, so the daemon
    /// re-times on the next redraw and persists the choice to `config.lua`.
    fn animation_content(&self, plots: &Plots) -> Element<'static, Plant> {
        self.animation_page.borrow_mut().view(
            plots.config.animation.speed,
            &plots.config.theme,
            &plots.config.component_defaults,
        )
    }

    fn wallpaper_content(&self, id: window::Id, plots: &Plots) -> Element<'static, Plant> {
        self.wallpaper_page.borrow_mut().view(
            wallpaper::Context {
                id,
                plots,
                draft: self
                    .wallpaper_draft
                    .as_ref()
                    .expect("wallpaper page requires a draft"),
                map_view: self.map_view,
                selected_image: self.selected_image,
            },
            &plots.config.theme,
        )
    }

    /// Replace the map pan/zoom/drag view (`MapViewChanged`). Field stays
    /// private; `Plots` writes through here like cursor maps elsewhere.
    pub(crate) fn set_map_view(&mut self, view: MapView) {
        if let Some(index) = view.selected {
            self.selected_image = Some(index);
        }
        self.map_view = view;
    }

    pub(crate) fn cancel_wallpaper_drag(&mut self) {
        self.map_view.drag = None;
        self.map_view.selected = None;
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
            if page == SettingPage::Theme
                && !setting.icon_previews_loading
                && setting.icon_previews.is_none()
            {
                setting.icon_previews_loading = true;
                let current = plots.config.theme.icon_theme.clone();
                return Command::perform(
                    async move {
                        tokio::task::spawn_blocking(move || theme_page::load_icon_previews(current))
                            .await
                            .map_err(|error| error.to_string())
                    },
                    move |result| {
                        Plant::SettingPlot(crate::shell::SettingEvent::IconPreviewsReady {
                            id,
                            result,
                        })
                    },
                );
            }
        }
        if page == SettingPage::Wallpaper {
            if let Some(setting) = plots.windows.settings.get_mut(&id)
                && setting.wallpaper_draft.is_none()
            {
                setting.wallpaper_draft = Some(crate::wallpaper::draft::Draft::new(
                    &plots.config.background,
                ));
            }
            return plots.load_wallpaper_preview(id);
        }
        Command::none()
    }

    pub(crate) fn icon_previews_ready(&mut self, result: Result<IconPreviews, String>) {
        self.icon_previews_loading = false;
        self.icon_previews = Some(result);
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
        // Compute before close_all drops the drafts: only image-owning panels
        // need the full-redraw reclaim (others stay host-only).
        let owned_images = plots.windows.settings.values().any(|setting| {
            setting
                .wallpaper_draft
                .as_ref()
                .is_some_and(|draft| !draft.sources.is_empty())
        });
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
        cmds.push(plots.reclaim_image_memory(owned_images));
        // Consume only requests originating from an accepted explicit Apply.
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
