use super::background::Background;
use crate::app::ConfigEvent;
use crate::app::app::{PlotInfo, Plots};
use crate::app::layers::ContextMenu;
use crate::app::{Plant, TopEvent};
use crate::components::display_map::{MapView, images_layer, outputs_layer};
use crate::composables::spin_box::spin_box;
use crate::config::{AnimationSpeed, ConfigPatch};
use crate::theme;
use iced::widget::{
    Checkbox, Space, button, column, container, row, rule, scrollable, slider, stack, text,
};
use iced::window;
use iced::{Element, Length, Task as Command};
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
    /// Bar picked in the Panel page picker row (`None` = first bar).
    /// Stored per-window like `page`, so each panel keeps its own pick.
    selected_bar: Option<window::Id>,
}

/// Master-detail pages: nav buttons on the left switch this, the right pane
/// renders the matching controls. Stored per-window so each panel keeps its
/// own selection (like `Top` config lives on its own bar).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingPage {
    #[default]
    Menu,
    Panel,
    ContextMenu,
    Wallpaper,
    Theme,
    Animation,
}

impl SettingPage {
    fn all() -> [Self; 6] {
        [
            Self::Menu,
            Self::Panel,
            Self::ContextMenu,
            Self::Wallpaper,
            Self::Theme,
            Self::Animation,
        ]
    }

    fn title(self) -> &'static str {
        match self {
            Self::Menu => "Menu",
            Self::Panel => "Panel",
            Self::ContextMenu => "Context Menu",
            Self::Wallpaper => "Wallpaper",
            Self::Theme => "Theme",
            Self::Animation => "Animation",
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
                    .width(Length::FillPortion(2))
                    .height(Length::Fill),
                rule::vertical(2),
                scrollable(self.content(id, plots))
                    .width(Length::FillPortion(8))
                    .height(Length::Fill),
            ]
            .spacing(12)
            .padding(16),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::bar)
        .into()
    }

    /// Left nav pane (30%): one button per page, highlighted when selected.
    fn nav(&self, id: window::Id) -> Element<'_, Plant> {
        let mut col = column![text("Settings").size(16)];
        for page in SettingPage::all() {
            let selected = page == self.page;
            col = col.push(
                button(text(page.title()).size(13).color(theme::text()))
                    .width(Length::Fill)
                    .on_press(Plant::SettingPlot(crate::app::SettingEvent::Select(
                        id, page,
                    )))
                    .padding(8)
                    .style(theme::nav_button(selected)),
            );
        }
        col.spacing(8).width(Length::Fill).into()
    }

    /// Right content pane (70%): live controls for the selected page. Each
    /// slider reads the single `plots.config` source of truth and writes back
    /// via `ConfigEvent::Patch`, so every layer updates on the next redraw.
    fn content(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        match self.page {
            SettingPage::Menu => self.menu_content(plots),
            SettingPage::Panel => self.panel_content(id, plots),
            SettingPage::ContextMenu => self.context_menu_content(plots),
            SettingPage::Wallpaper => self.wallpaper_content(id, plots),
            SettingPage::Theme => self.theme_content(plots),
            SettingPage::Animation => self.animation_content(plots),
        }
    }

    fn menu_content(&self, plots: &Plots) -> Element<'_, Plant> {
        let c = &plots.config.composable;
        column![
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
        .width(Length::Fill)
        .into()
    }

    fn context_menu_content(&self, plots: &Plots) -> Element<'_, Plant> {
        let c = &plots.config.composable;
        column![
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
            rule::horizontal(2),
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
        .width(Length::Fill)
        .into()
    }

    /// Panel page: picker row of bars on top, controls for the picked bar
    /// below (defaults to the first). Length % + thickness px, floating
    /// (+ margins when floating) and per-corner rounding.
    fn panel_content(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        let mut col = column![
            text("Panel").size(16),
            text("Bars").size(16),
            text("Size, floating and margins apply live to the bar.").size(11),
        ]
        .spacing(8);
        let mut bars: Vec<_> = plots
            .ids
            .iter()
            .filter_map(|(wid, info)| match info {
                PlotInfo::Top(o) => Some((*wid, *o)),
                _ => None,
            })
            .collect();
        bars.sort_by_key(|(wid, o)| (o.0, format!("{wid:?}")));
        if bars.is_empty() {
            return col
                .push(
                    column![
                        text("No Bar Available").size(13).color(theme::text()),
                        text("Right-click the wallpaper, then Add Top.").size(11),
                    ]
                    .spacing(4)
                    .width(Length::Fill)
                    .align_x(iced::Alignment::Center),
                )
                .spacing(8)
                .width(Length::Fill)
                .into();
        }
        // Picked bar, falling back to the first when unset or stale
        // (e.g. its output was unplugged).
        let selected = match self.selected_bar {
            Some(s) if bars.iter().any(|(wid, _)| *wid == s) => s,
            _ => bars[0].0,
        };
        let mut picker = row![].spacing(8);
        for (wid, output) in &bars {
            let label = plots
                .tops
                .get(wid)
                .map(|t| format!("{} {output:?}", t.anchor_label()))
                .unwrap_or_else(|| format!("BAR {output:?}"));
            picker = picker.push(
                button(text(label).size(13).color(theme::text()))
                    .on_press(Plant::SettingPlot(crate::app::SettingEvent::SelectBar(
                        id, *wid,
                    )))
                    .padding(8)
                    .style(theme::nav_button(*wid == selected)),
            );
        }
        col = col.push(picker);
        let remove_label = plots
            .tops
            .get(&selected)
            .map(|t| format!("Remove {} bar", t.anchor_label()))
            .unwrap_or_else(|| String::from("Remove bar"));
        col = col.push(
            button(text(remove_label).size(13).color(theme::button_text()))
                .on_press(Plant::TopPlot(TopEvent::Remove(selected)))
                .padding(8)
                .style(theme::menu_button(theme::RADIUS))
                .width(Length::Fill),
        );
        let output = bars
            .iter()
            .find_map(|(wid, o)| (*wid == selected).then_some(*o));
        let top = plots.tops.get(&selected);
        let (Some(output), Some(top)) = (output, top) else {
            return col
                .push(
                    column![text("No Bar Available").size(13).color(theme::text()),]
                        .width(Length::Fill)
                        .align_x(iced::Alignment::Center),
                )
                .spacing(8)
                .width(Length::Fill)
                .into();
        };
        {
            let wid = selected;
            // Length is % of the long axis (1–100); thickness is px bound to
            // 1..=thin output axis (horizontal: sh, vertical: sw).
            let horizontal = top.is_horizontal();
            let thick_max: f64 = plots
                .output_infos
                .get(&output)
                .map(Background::output_geometry)
                .map(
                    |(_, _, sw, sh)| {
                        if horizontal { sh as f64 } else { sw as f64 }
                    },
                )
                .unwrap_or(if horizontal { 1080.0 } else { 1920.0 })
                .max(1.0);
            let length = top.local.length_pct.clamp(1.0, 100.0);
            let thickness = top.local.thickness_px.clamp(1.0, thick_max as f32);
            col = col.push(plant_slider_row(
                format!("Length {:.0}%", length),
                length as f64,
                1.0..=100.0,
                1.0,
                move |v| Plant::TopPlot(TopEvent::SetLength(wid, v as f32)),
                None,
            ));
            col = col.push(plant_slider_row(
                format!("Thickness {:.0}px", thickness),
                thickness as f64,
                1.0..=thick_max,
                1.0,
                move |v| Plant::TopPlot(TopEvent::SetThickness(wid, v as f32)),
                None,
            ));
            col = col.push(rule::horizontal(2));
            col = col.push(
                Checkbox::new(top.local.floating)
                    .label("Floating look (inset content, reserved space kept)")
                    .on_toggle(move |v| Plant::TopPlot(TopEvent::SetFloating(wid, v))),
            );

            if top.local.floating {
                let top_margin = row![
                    text("Margin top (px)").width(Length::FillPortion(7)),
                    spin_box(
                        top.local.margins.top as f64,
                        0.0..=256.0,
                        1.0,
                        0,
                        move |v| Plant::TopPlot(TopEvent::SetMarginTop(wid, v as i32)),
                    )
                    .width(Length::Fixed(120.0)),
                ]
                .spacing(8);
                let right_margin = row![
                    text("Margin right (px)").width(Length::FillPortion(7)),
                    spin_box(
                        top.local.margins.right as f64,
                        0.0..=256.0,
                        1.0,
                        0,
                        move |v| Plant::TopPlot(TopEvent::SetMarginRight(wid, v as i32)),
                    )
                    .width(Length::Fixed(120.0)),
                ]
                .spacing(8);
                let left_margin = row![
                    text("Margin left (px)").width(Length::FillPortion(7)),
                    spin_box(
                        top.local.margins.left as f64,
                        0.0..=256.0,
                        1.0,
                        0,
                        move |v| Plant::TopPlot(TopEvent::SetMarginLeft(wid, v as i32)),
                    )
                    .width(Length::Fixed(120.0)),
                ]
                .spacing(8);
                let bottom_margin = row![
                    text("Margin bottom (px)").width(Length::FillPortion(7)),
                    spin_box(
                        top.local.margins.bottom as f64,
                        0.0..=256.0,
                        1.0,
                        0,
                        move |v| Plant::TopPlot(TopEvent::SetMarginBottom(wid, v as i32)),
                    )
                    .width(Length::Fixed(120.0)),
                ]
                .spacing(8);
                col = col.push(top_margin);
                col = col.push(bottom_margin);
                col = col.push(right_margin);
                col = col.push(left_margin);
            } else {
                col = col.push(text("Enable Floating to adjust margins.").size(11));
            }
            col = col.push(rule::horizontal(2));

            let top_radius_group = row![
                plant_slider_row(
                    format!("Round top-left {:.0}", top.local.radius.top_left),
                    top.local.radius.top_left as f64,
                    0.0..=32.0,
                    1.0,
                    move |v| Plant::TopPlot(TopEvent::SetRadiusTl(wid, v as f32)),
                    None,
                ),
                plant_slider_row(
                    format!("Round top-right {:.0}", top.local.radius.top_right),
                    top.local.radius.top_right as f64,
                    0.0..=32.0,
                    1.0,
                    move |v| Plant::TopPlot(TopEvent::SetRadiusTr(wid, v as f32)),
                    None,
                )
            ]
            .spacing(8);
            let bottom_radius_group = row![
                plant_slider_row(
                    format!("Round bottom-left {:.0}", top.local.radius.bottom_left),
                    top.local.radius.bottom_left as f64,
                    0.0..=32.0,
                    1.0,
                    move |v| Plant::TopPlot(TopEvent::SetRadiusBl(wid, v as f32)),
                    None,
                ),
                plant_slider_row(
                    format!("Round bottom-right {:.0}", top.local.radius.bottom_right),
                    top.local.radius.bottom_right as f64,
                    0.0..=32.0,
                    1.0,
                    move |v| Plant::TopPlot(TopEvent::SetRadiusBr(wid, v as f32)),
                    None,
                )
            ]
            .spacing(8);
            col = col.push(top_radius_group);
            col = col.push(bottom_radius_group);
        }
        col.spacing(8).width(Length::Fill).into()
    }

    /// Theme picker: dark/light toggle plus one button per
    /// `~/.config/riced/theme/*.json` (same files as reshell). Writes back
    /// via `ConfigEvent::Patch`, so the daemon re-themes on the next redraw
    /// and persists the choice to `config.toml`.
    fn theme_content(&self, plots: &Plots) -> Element<'_, Plant> {
        let current = &plots.config.theme;
        let mut list = column![
            text("Theme").size(16),
            row![
                button(text("Dark").size(13).color(theme::text()))
                    .width(Length::Fill)
                    .on_press(Plant::Config(ConfigEvent::Patch(
                        ConfigPatch::ThemeDarkmode(true)
                    )))
                    .padding(8)
                    .style(theme::nav_button(current.darkmode)),
                button(text("Light").size(13).color(theme::text()))
                    .width(Length::Fill)
                    .on_press(Plant::Config(ConfigEvent::Patch(
                        ConfigPatch::ThemeDarkmode(false)
                    )))
                    .padding(8)
                    .style(theme::nav_button(!current.darkmode)),
            ]
            .spacing(8),
        ]
        .spacing(8);
        for name in theme::available_themes() {
            let selected = name == current.name;
            let preview = theme::preview(&name, current.darkmode);
            list = list.push(
                button(
                    row![
                        text(name.clone()).size(13).color(preview.on_surface),
                        Space::new().width(Length::Fill),
                        row![
                            swatch(preview.primary, preview.outline),
                            swatch(preview.secondary, preview.outline),
                            swatch(preview.tertiary, preview.outline),
                        ]
                        .spacing(4),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center)
                    .width(Length::Fill),
                )
                .width(Length::Fill)
                .on_press(Plant::Config(ConfigEvent::Patch(ConfigPatch::ThemeName(
                    name,
                ))))
                .padding(8)
                .style(theme::preview_button(preview, selected)),
            );
        }
        list.width(Length::Fill).into()
    }

    /// Animation speed picker: one global speed for every animated
    /// transition (selection fade and any interpolated color/property
    /// change). Writes back via `ConfigEvent::Patch`, so the daemon
    /// re-times on the next redraw and persists the choice to `config.toml`.
    fn animation_content(&self, plots: &Plots) -> Element<'_, Plant> {
        let current = plots.config.animation.speed;
        let mut speeds = row![].spacing(8);
        for speed in AnimationSpeed::all() {
            let selected = speed == current;
            speeds = speeds.push(
                button(
                    text(format!(
                        "{} ({}ms)",
                        speed.title(),
                        speed.duration().as_millis()
                    ))
                    .size(13)
                    .color(theme::text()),
                )
                .width(Length::Fill)
                .on_press(Plant::Config(ConfigEvent::Patch(
                    ConfigPatch::AnimationSpeed(speed),
                )))
                .padding(8)
                .style(theme::nav_button(selected)),
            );
        }
        column![
            text("Animation").size(16),
            text("Global speed for animated color and property changes.").size(11),
            speeds.width(Length::Fill),
        ]
        .spacing(8)
        .width(Length::Fill)
        .into()
    }

    fn wallpaper_content(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        column![
            self.wallpaper_grid(id, plots),
            container(Space::new().height(Length::Fill).width(Length::Fill)),
        ]
        .into()
    }

    fn wallpaper_grid(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
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
        container(stack![
            images_layer(id, outputs.clone(), images.clone(), handles.clone(), view),
            outputs_layer(id, outputs, images, handles, view),
        ])
        .style(theme::menu_box)
        .height(Length::Fixed(600.0))
        .clip(true)
        .width(Length::Fill)
        .into()
    }

    /// Replace the map pan/zoom/drag view (`MapViewChanged`). Field stays
    /// private; `Plots` writes through here like cursor maps elsewhere.
    pub(crate) fn set_map_view(&mut self, view: MapView) {
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
        if let Some(setting) = plots.settings.get_mut(&id) {
            setting.page = page;
        }
        if page == SettingPage::Wallpaper {
            return Plots::repaint_after(80);
        }
        Command::none()
    }

    /// Pick the bar the Panel page edits (`SettingEvent::SelectBar`).
    pub(crate) fn handle_select_bar(
        plots: &mut Plots,
        id: window::Id,
        bar: window::Id,
    ) -> Command<Plant> {
        if let Some(setting) = plots.settings.get_mut(&id) {
            setting.selected_bar = Some(bar);
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
        // Panel edits are done: persist staged config now instead of waiting
        // out the coalescing save timer (Uproot flushes again, harmless).
        plots.flush_config_save();
        let mut cmds = Vec::with_capacity(to_close.len());
        for id in to_close {
            plots.last_cursor.remove(&id);
            plots.press_starts.remove(&id);
            cmds.push(iced_runtime::task::effect(Action::Window(
                WindowAction::Close(id),
            )));
        }
        // Toggle-closed with pending wallpaper edits: the user is done —
        // regen now instead of waiting out the countdown.
        if plots.theme_regen_dirty {
            cmds.push(plots.fire_regen_theme());
        }
        if cmds.is_empty() {
            Command::none()
        } else {
            Command::batch(cmds)
        }
    }
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
        text(label).size(13).color(theme::text()),
        slider(range, value as f64, move |v| {
            Plant::Config(ConfigEvent::Patch(ctor(v as f32)))
        })
        // Drags preview in live memory; release persists to the config file.
        .on_release(Plant::Config(ConfigEvent::SaveNow))
        .width(Length::Fill),
    ]
    .spacing(4)
    .into()
}

/// Label + slider emitting a [`Plant`] directly (per-bar Top controls).
/// Unlike [`slider_row`], the message is built by closure so any event fits.
/// `release` fires once when the drag ends (currently unused by bars,
/// which apply live — kept for future release-gated controls).
fn plant_slider_row(
    label: String,
    value: f64,
    range: RangeInclusive<f64>,
    step: f64,
    msg: impl Fn(f64) -> Plant + 'static,
    release: Option<Plant>,
) -> Element<'static, Plant> {
    let mut sl = slider(range, value, msg).step(step).width(Length::Fill);
    if let Some(on_release) = release {
        sl = sl.on_release(on_release);
    }
    column![text(label).size(13).color(theme::text()), sl,]
        .spacing(4)
        .into()
}
