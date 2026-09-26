use super::background::Background;
use crate::app::ConfigEvent;
use crate::app::Plant;
use crate::app::app::{PlotInfo, Plots};
use crate::app::layers::ContextMenu;
use crate::components::display_map::{MapView, images_layer, outputs_layer};
use crate::config::ConfigPatch;
use iced::widget::{Space, button, column, container, row, rule, slider, stack, text};
use iced::window;
use iced::{Color, Element, Length, Task as Command};
use iced_exwlshell::actions::IcedXdgWindowSettings;
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use std::collections::HashMap;
use std::ops::RangeInclusive;

/// Floating XDG toplevel for the Settings panel (spawned via `Plant::Sprout`).
/// Mirrors `Top` / `Background`: spawn + view live here, `Plots` just delegates.
#[derive(Debug, Default)]
pub struct Setting {
    page: SettingPage,
    /// Pan/zoom/drag view for the Background map. Single source of truth for
    /// both stacked canvas programs (canvas `State` can't be shared across
    /// the two widgets), updated via `SettingEvent::MapViewChanged`.
    map_view: MapView,
}

/// Master-detail pages: nav buttons on the left switch this, the right pane
/// renders the matching controls. Stored per-window so each panel keeps its
/// own selection (like `Top::thickness` lives on its own bar).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingPage {
    #[default]
    Menu,
    Panel,
    ContextMenu,
    Background,
}

impl SettingPage {
    fn all() -> [Self; 4] {
        [Self::Menu, Self::Panel, Self::ContextMenu, Self::Background]
    }

    fn title(self) -> &'static str {
        match self {
            Self::Menu => "Menu",
            Self::Panel => "Panel",
            Self::ContextMenu => "Context Menu",
            Self::Background => "Background",
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
        // Master-detail: 30% nav buttons left, 70% page content right.
        // (`row` is a macro — `row![a, b]` — not a function, and
        // `keyed_column!` only keys children for diffing; it can't select.)
        // Fully transparent root: with the daemon's transparent clear color
        // (see `main.rs:.style`), Hyprland blur/opacity windowrules can see
        // straight through this. Use `from_rgba(0,0,0,0.25)` for a frosted tint.
        container(
            row![self.nav(id), rule::vertical(2), self.content(id, plots)]
                .spacing(12)
                .padding(16),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(Color::TRANSPARENT.into()),
            ..Default::default()
        })
        .into()
    }

    /// Left nav pane (30%): one button per page, highlighted when selected.
    fn nav(&self, id: window::Id) -> Element<'_, Plant> {
        let mut col = column![text("Settings").size(16)];
        for page in SettingPage::all() {
            let selected = page == self.page;
            col = col.push(
                button(text(page.title()).size(13).color(Color::WHITE))
                    .width(Length::Fill)
                    .on_press(Plant::SettingPlot(crate::app::SettingEvent::Select(
                        id, page,
                    )))
                    .padding(8)
                    .style(move |_, _| button::Style {
                        background: Some(
                            if selected {
                                Color::from_rgb(0.35, 0.35, 0.40)
                            } else {
                                Color::from_rgb(0.25, 0.25, 0.28)
                            }
                            .into(),
                        ),
                        text_color: Color::WHITE,
                        border: iced::Border {
                            color: Color::from_rgb(0.5, 0.5, 0.55),
                            width: 1.0,
                            radius: 6.0.into(),
                        },
                        ..Default::default()
                    }),
            );
        }
        col.spacing(8).width(Length::FillPortion(2)).into()
    }

    /// Right content pane (70%): live controls for the selected page. Each
    /// slider reads the single `plots.config` source of truth and writes back
    /// via `ConfigEvent::Patch`, so every layer updates on the next redraw.
    fn content(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        let c = &plots.config.composable;
        match self.page {
            SettingPage::Menu => column![
                text("Menu").size(16),
                slider_row(
                    format!("Width {:.0}", c.menu.width),
                    c.menu.width,
                    80.0..=400.0,
                    ConfigPatch::MenuWidth
                ),
                slider_row(
                    format!("Height {:.0}", c.menu.height),
                    c.menu.height,
                    40.0..=200.0,
                    ConfigPatch::MenuHeight
                ),
                slider_row(
                    format!("Padding {:.0}", c.menu.padding),
                    c.menu.padding,
                    0.0..=32.0,
                    ConfigPatch::MenuPadding
                ),
                slider_row(
                    format!("Spacing {:.0}", c.menu.spacing),
                    c.menu.spacing,
                    0.0..=32.0,
                    ConfigPatch::MenuSpacing
                ),
                slider_row(
                    format!("Rounding {:.0}", c.menu.rounding),
                    c.menu.rounding,
                    0.0..=20.0,
                    ConfigPatch::MenuRounding
                ),
            ]
            .spacing(8)
            .width(Length::FillPortion(8))
            .into(),
            SettingPage::Panel => column![
                text("Panel").size(16),
                slider_row(
                    format!("Padding {:.0}", c.panel.padding),
                    c.panel.padding,
                    0.0..=32.0,
                    ConfigPatch::PanelPadding
                ),
                slider_row(
                    format!("Spacing {:.0}", c.panel.spacing),
                    c.panel.spacing,
                    0.0..=32.0,
                    ConfigPatch::PanelSpacing
                ),
                slider_row(
                    format!("Rounding {:.0}", c.panel.rounding),
                    c.panel.rounding,
                    0.0..=20.0,
                    ConfigPatch::PanelRounding
                ),
            ]
            .spacing(8)
            .width(Length::FillPortion(8))
            .into(),
            SettingPage::ContextMenu => column![
                text("Context Menu").size(16),
                slider_row(
                    format!("Width {:.0}", c.context_menu.width),
                    c.context_menu.width,
                    80.0..=400.0,
                    ConfigPatch::ContextMenuWidth
                ),
                slider_row(
                    format!("Padding {:.0}", c.context_menu.padding),
                    c.context_menu.padding,
                    0.0..=32.0,
                    ConfigPatch::ContextMenuPadding
                ),
                slider_row(
                    format!("Spacing {:.0}", c.context_menu.spacing),
                    c.context_menu.spacing,
                    0.0..=32.0,
                    ConfigPatch::ContextMenuSpacing
                ),
                slider_row(
                    format!("Rounding {:.0}", c.context_menu.rounding),
                    c.context_menu.rounding,
                    0.0..=20.0,
                    ConfigPatch::ContextMenuRounding
                ),
                slider_row(
                    format!("Item padding {:.0}", c.context_menu_item.padding),
                    c.context_menu_item.padding,
                    0.0..=32.0,
                    ConfigPatch::ContextMenuItemPadding
                ),
                slider_row(
                    format!("Item rounding {:.0}", c.context_menu_item.rounding),
                    c.context_menu_item.rounding,
                    0.0..=20.0,
                    ConfigPatch::ContextMenuItemRounding
                ),
            ]
            .spacing(8)
            .width(Length::FillPortion(8))
            .into(),
            SettingPage::Background => self.background_content(id, plots),
        }
    }

    fn background_content(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        // Snapshot output geometry (global logical coords) and wallpaper
        // entries into the map programs, outputs sorted for stable numbering.
        // Hot-plug / config edits arrive via the next redraw rebuilding them.
        // Two stacked canvases: `stack!` pushes a new render layer per child,
        // so (and only so) the output overlay paints over wallpaper pixels —
        // `iced_wgpu` draws all quads before all images within one layer.
        let mut outputs: Vec<(f32, f32, f32, f32)> = plots
            .output_infos
            .values()
            .map(Background::output_geometry)
            .collect();
        outputs.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
        let images = plots.config.background.image.clone();
        let handles: Vec<Option<iced::widget::image::Handle>> = images
            .iter()
            .map(|img| plots.wallpaper_handle(img))
            .collect();
        let view = self.map_view;
        column![
            container(stack![
                images_layer(id, outputs.clone(), images.clone(), handles.clone(), view),
                outputs_layer(id, outputs, images, handles, view),
            ])
            .style(|_| container::Style {
                background: Some(Color::from_rgb(0.15, 0.15, 0.18).into()),
                border: iced::Border {
                    color: Color::from_rgb(0.5, 0.5, 0.55),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..Default::default()
            })
            .height(Length::FillPortion(6))
            .width(Length::Fill),
            container(Space::new())
                .style(|_| container::Style {
                    background: Some(Color::from_rgb(0.15, 0.15, 0.18).into()),
                    border: iced::Border {
                        color: Color::from_rgb(0.5, 0.5, 0.55),
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                })
                .height(Length::FillPortion(4))
                .width(Length::Fill)
        ]
        .spacing(8)
        .height(Length::FillPortion(6))
        .width(Length::FillPortion(8))
        .into()
    }

    /// Replace the map pan/zoom/drag view (`MapViewChanged`). Field stays
    /// private; `Plots` writes through here like cursor maps elsewhere.
    pub(crate) fn set_map_view(&mut self, view: MapView) {
        self.map_view = view;
    }

    /// Switch the selected page on `SettingEvent::Select`.
    pub(crate) fn handle_select(
        plots: &mut Plots,
        id: window::Id,
        page: SettingPage,
    ) -> Command<Plant> {
        if let Some(setting) = plots.settings.get_mut(&id) {
            setting.page = page;
        }
        Command::none()
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

/// Label + slider bound to one config leaf: reads the live value, writes back
/// via `ConfigEvent::Patch`. Fully owned element, so pages compose freely.
/// The variant constructor doubles as the patch fn (`ConfigPatch::MenuWidth`
/// is `fn(f32) -> ConfigPatch`).
fn slider_row(
    label: String,
    value: f32,
    range: RangeInclusive<f64>,
    ctor: fn(f32) -> ConfigPatch,
) -> Element<'static, Plant> {
    column![
        text(label).size(13).color(Color::WHITE),
        slider(range, value as f64, move |v| {
            Plant::Config(ConfigEvent::Patch(ctor(v as f32)))
        })
        .width(Length::Fill),
    ]
    .spacing(4)
    .into()
}
