mod context_menu;
mod editors;
mod pages;
mod panel;
mod wallpaper;
use super::background::Background;
use super::top::{SlotAlign, Top, TopLocal};
use crate::config::BackgroundImage;
use crate::shell::screens::ContextMenu;
use crate::shell::state::{PlotInfo, Plots};
use crate::shell::{BarEvent, Corner, Edge, Plant, StyleEvent, TopEvent};
use crate::theme;
use crate::ui::widgets::display_map::MapView;
use crate::ui::widgets::panel_preview::Preview;
use crate::ui::widgets::spin_box::spin_box;
use editors::prop_row;
use iced::widget::{
    Checkbox, Space, button, column, container, row, rule, scrollable, slider, text,
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
        let mut col = column![section_heading(
            "Choose a bar",
            "Each display can have its own bar configuration."
        )]
        .spacing(16);
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
            let connector = plots
                .output_infos
                .get(output)
                .and_then(|info| info.name.clone())
                .unwrap_or_else(|| "Unknown display".to_string());
            let label = plots
                .tops
                .get(wid)
                .map(|t| format!("{connector} · {}", t.anchor_label()))
                .unwrap_or(connector);
            picker = picker.push(
                button(text(label).size(13).color(theme::text()))
                    .on_press(Plant::SettingPlot(crate::shell::SettingEvent::SelectBar(
                        id, *wid,
                    )))
                    .padding(8)
                    .style(theme::nav_button(*wid == selected)),
            );
        }
        col = col.push(
            scrollable(picker).direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::default(),
            )),
        );
        let remove_label = plots
            .tops
            .get(&selected)
            .map(|t| format!("Remove {} bar", t.anchor_label()))
            .unwrap_or_else(|| String::from("Remove bar"));
        let remove_button = button(text(remove_label).size(13).color(theme::active().error))
            .on_press(Plant::TopPlot(TopEvent::Remove(selected)))
            .padding(8)
            .style(theme::menu_button_tinted(
                theme::RADIUS,
                theme::active().error,
            ))
            .width(Length::Shrink);
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
            col = col.push(section_heading(
                "Size & appearance",
                "Length is a percentage of the display edge; thickness is measured in pixels.",
            ));
            col = col.push(plant_slider_row(
                format!("Length {:.0}%", length),
                length as f64,
                1.0..=100.0,
                1.0,
                move |v| Plant::TopPlot(TopEvent::Bar(BarEvent::Length(wid, v as f32))),
                None,
            ));
            col = col.push(plant_slider_row(
                format!("Thickness {:.0}px", thickness),
                thickness as f64,
                1.0..=thick_max,
                1.0,
                move |v| Plant::TopPlot(TopEvent::Bar(BarEvent::Thickness(wid, v as f32))),
                None,
            ));
            // Opacity presets (0/25/50/75/100): one commit per press, no
            // drag stream. The view bakes the stepped alpha into a fresh
            // RGBA fill.
            {
                let current = TopLocal::snap_opacity(top.local.opacity);
                let mut presets = row![text("Opacity").width(Length::Fill)].spacing(8);
                for step in TopLocal::OPACITY_STEPS {
                    let label = format!("{:.0}%", step * 100.0);
                    presets = presets.push(
                        button(text(label).size(12).color(theme::text()))
                            .on_press(Plant::TopPlot(TopEvent::Style(StyleEvent::Opacity(
                                wid, step,
                            ))))
                            .padding(6)
                            .style(theme::nav_button(current == step)),
                    );
                }
                col = col.push(presets);
            }

            col = col.push(section_heading("Slots & widgets", "Slots divide the bar into regions. Select a slot to edit its alignment and widgets."));
            // Grid cells along the long axis. Named `Slots` (not
            // columns/rows) so the label stays correct when the anchor
            // flips between horizontal (top/bottom) and vertical
            // (left/right).
            let slots_label = format!("Slots ({})", top.local.slots);
            col = col.push(
                row![
                    text(slots_label).width(Length::FillPortion(7)),
                    spin_box(
                        top.local.slots as f64,
                        1.0..=TopLocal::MAX_SLOTS as f64,
                        1.0,
                        0,
                        move |v| Plant::TopPlot(TopEvent::Bar(BarEvent::Slots(wid, v as u32))),
                    )
                    .width(Length::Fixed(120.0)),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
            // Cell gaps: inset inside every slot around its content, plus
            // the gap between cells (also used between icon/text runs).
            let slot_padding = top.local.slot_padding.clamp(0.0, TopLocal::MAX_SLOT_GAP);
            let slot_spacing = top.local.slot_spacing.clamp(0.0, TopLocal::MAX_SLOT_GAP);
            col = col.push(plant_slider_row(
                format!("Slot padding {:.0}px", slot_padding),
                slot_padding as f64,
                0.0..=32.0,
                1.0,
                move |v| Plant::TopPlot(TopEvent::Bar(BarEvent::SlotPadding(wid, v as f32))),
                None,
            ));
            col = col.push(plant_slider_row(
                format!("Slot spacing {:.0}px", slot_spacing),
                slot_spacing as f64,
                0.0..=32.0,
                1.0,
                move |v| Plant::TopPlot(TopEvent::Bar(BarEvent::SlotSpacing(wid, v as f32))),
                None,
            ));
            // Per-slot child alignment: picker row of slots, then
            // Start/Center/End presets for the picked one (one commit
            // per press, like opacity).
            {
                let n = top.local.slots.clamp(1, TopLocal::MAX_SLOTS) as usize;
                let sel = match self.selected_slot {
                    Some(s) if s < n => s,
                    _ => 0,
                };
                let mut picker = row![].spacing(8);
                for pos in 0..n {
                    picker = picker.push(
                        button(
                            text(format!("Slot {}", pos + 1))
                                .size(12)
                                .color(theme::text()),
                        )
                        .on_press(Plant::SettingPlot(crate::shell::SettingEvent::SelectSlot(
                            id, pos,
                        )))
                        .padding(6)
                        .style(theme::nav_button(pos == sel)),
                    );
                }
                col = col.push(
                    scrollable(picker).direction(scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::default(),
                    )),
                );
                let current = top.local.align_at(sel);
                let mut presets = row![].spacing(8);
                for align in [SlotAlign::Start, SlotAlign::Center, SlotAlign::End] {
                    presets = presets.push(
                        button(text(align.as_str()).size(12).color(theme::text()))
                            .on_press(Plant::TopPlot(TopEvent::Bar(BarEvent::SlotAlign(
                                wid, sel, align,
                            ))))
                            .padding(6)
                            .style(theme::nav_button(current == align)),
                    );
                }
                col = col.push(presets);
                let slots = top.local.widgets.clone();
                let names: Vec<String> = plots
                    .widgets
                    .iter()
                    .filter(|def| !TopLocal::is_empty_widget(&def.name))
                    .map(|def| def.name.clone())
                    .collect();
                let preview = iced::widget::responsive(move |size| {
                    Preview::new(
                        id,
                        wid,
                        slots.clone(),
                        names.clone(),
                        horizontal,
                        sel,
                        self.selected_placement.clone(),
                        size.width,
                    )
                    .element()
                })
                .width(Length::Fill)
                .height(Length::Shrink);
                col = col.push(Space::new().height(Length::Fixed(8.0)));
                col = col.push(section(
                    "Arrange your panel",
                    "Drag onto a widget to swap, or onto free slot space to move. Click a widget to edit its settings below. Escape cancels a drag.",
                    preview.into(),
                ));
                if plots.widgets.is_empty() {
                    col = col.push(hint(
                        "Add .lua files to your widgets directory to fill the available-widget pool.",
                    ));
                }
                col = col.push(Self::placement_editor(
                    wid,
                    plots.output_name(wid),
                    top,
                    self.selected_placement.as_deref(),
                    &plots,
                ));
            }

            col = col.push(section_heading(
                "Floating & corners",
                "Inset the bar from the display edge and soften its corners.",
            ));
            col = col.push(
                Checkbox::new(top.local.floating)
                    .label("Floating bar")
                    .on_toggle(move |v| {
                        Plant::TopPlot(TopEvent::Style(StyleEvent::Floating(wid, v)))
                    }),
            );
            col = col.push(hint(
                "Floating adds space around the bar; the reserved desktop area stays unchanged.",
            ));

            if top.local.floating {
                let top_margin = row![
                    text("Margin top (px)").width(Length::FillPortion(7)),
                    spin_box(
                        top.local.margins.top as f64,
                        0.0..=256.0,
                        1.0,
                        0,
                        move |v| Plant::TopPlot(TopEvent::Style(StyleEvent::Margin(
                            wid,
                            Edge::Top,
                            v as i32
                        ))),
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
                        move |v| Plant::TopPlot(TopEvent::Style(StyleEvent::Margin(
                            wid,
                            Edge::Right,
                            v as i32
                        ))),
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
                        move |v| Plant::TopPlot(TopEvent::Style(StyleEvent::Margin(
                            wid,
                            Edge::Left,
                            v as i32
                        ))),
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
                        move |v| Plant::TopPlot(TopEvent::Style(StyleEvent::Margin(
                            wid,
                            Edge::Bottom,
                            v as i32
                        ))),
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
                    move |v| Plant::TopPlot(TopEvent::Style(StyleEvent::Radius(
                        wid,
                        Corner::TopLeft,
                        v as f32
                    ))),
                    None,
                ),
                plant_slider_row(
                    format!("Round top-right {:.0}", top.local.radius.top_right),
                    top.local.radius.top_right as f64,
                    0.0..=32.0,
                    1.0,
                    move |v| Plant::TopPlot(TopEvent::Style(StyleEvent::Radius(
                        wid,
                        Corner::TopRight,
                        v as f32
                    ))),
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
                    move |v| Plant::TopPlot(TopEvent::Style(StyleEvent::Radius(
                        wid,
                        Corner::BottomLeft,
                        v as f32
                    ))),
                    None,
                ),
                plant_slider_row(
                    format!("Round bottom-right {:.0}", top.local.radius.bottom_right),
                    top.local.radius.bottom_right as f64,
                    0.0..=32.0,
                    1.0,
                    move |v| Plant::TopPlot(TopEvent::Style(StyleEvent::Radius(
                        wid,
                        Corner::BottomRight,
                        v as f32
                    ))),
                    None,
                )
            ]
            .spacing(8);
            col = col.push(top_radius_group);
            col = col.push(bottom_radius_group);
        }
        col.push(section(
            "Remove this bar",
            "Remove the selected bar from the display and saved configuration.",
            remove_button.into(),
        ))
        .spacing(16)
        .width(Length::Fill)
        .into()
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
        if let Some(setting) = plots.settings.get_mut(&id) {
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
        if let Some(setting) = plots.settings.get_mut(&id) {
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
        if let Some(setting) = plots.settings.get_mut(&id) {
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
        let slot = plots.tops.get(&bar).and_then(|top| {
            top.local
                .widgets
                .iter()
                .position(|slot| slot.iter().any(|p| p.id == placement))
        });
        if let Some(setting) = plots.settings.get_mut(&id) {
            setting.selected_slot = slot.or(setting.selected_slot);
            setting.selected_placement = slot.map(|_| placement);
        }
        Command::none()
    }

    /// Per-placement settings editor below the preview: refresh
    /// interval, text size, and custom properties, each showing the
    /// inherited default with a Reset that clears the override.
    /// Unknown ids (or missing definitions) render guidance instead.
    fn placement_editor(
        wid: window::Id,
        output: String,
        top: &Top,
        selected: Option<&str>,
        plots: &panel::PanelContext<'_>,
    ) -> Element<'static, Plant> {
        let Some(sel_id) = selected else {
            return hint(
                "Click a widget chip above to edit its size, refresh rate, and custom properties.",
            );
        };
        let found = top
            .local
            .widgets
            .iter()
            .enumerate()
            .find_map(|(i, slot)| slot.iter().find(|p| p.id == sel_id).map(|p| (i, p.clone())));
        let Some((slot_idx, placement)) = found else {
            return hint("That widget left the bar — pick another chip to edit its settings.");
        };
        let pid = placement.id.clone();
        let pid_msg = pid.clone();
        let commit = std::rc::Rc::new(move |patch: crate::shell::PlacementProp| {
            Plant::TopPlot(TopEvent::Bar(BarEvent::WidgetProp {
                bar: wid,
                placement: pid_msg.clone(),
                patch,
            }))
        });
        let where_at = if output.is_empty() {
            format!("Slot {}", slot_idx + 1)
        } else {
            format!("Slot {} · {output}", slot_idx + 1)
        };
        let Some(def) = plots.widgets.iter().find(|d| d.name == placement.name) else {
            return section(
                &format!("{} · {where_at}", placement.name),
                "This placement references a widget with no definition file.",
                hint("Add widgets/<name>.lua, or drag its chip to the pool to remove it."),
            );
        };
        let def = def.clone();
        let mut col = column![section_heading(
            &format!("{} · {where_at}", placement.name),
            "Overrides apply to this instance only. Reset returns to the widget default."
        )]
        .spacing(8);
        // Refresh interval.
        {
            let effective = placement.effective_interval(&def.defaults);
            let commit_interval = commit.clone();
            let reset = placement
                .interval
                .is_some()
                .then(|| commit_interval(crate::shell::PlacementProp::Interval(None)));
            let commit_value = commit.clone();
            col = col.push(prop_row(
                "Refresh interval".to_string(),
                placement
                    .interval
                    .is_none()
                    .then(|| format!("Widget default: {:.2}s", def.defaults.interval)),
                spin_box(effective as f64, 0.25..=3600.0, 0.25, 2, move |v| {
                    commit_value(crate::shell::PlacementProp::Interval(Some(v as f32)))
                })
                .width(Length::Fill)
                .into(),
                reset,
            ));
        }
        // Text size.
        {
            let effective = placement.effective_size(&def.defaults);
            let commit_size = commit.clone();
            let reset = placement
                .size
                .is_some()
                .then(|| commit_size(crate::shell::PlacementProp::Size(None)));
            let commit_value = commit.clone();
            col = col.push(prop_row(
                "Text size".to_string(),
                placement
                    .size
                    .is_none()
                    .then(|| format!("Widget default: {:.1}px", def.defaults.size)),
                spin_box(effective as f64, 1.0..=128.0, 0.5, 1, move |v| {
                    commit_value(crate::shell::PlacementProp::Size(Some(v as f32)))
                })
                .width(Length::Fill)
                .into(),
                reset,
            ));
        }
        // Custom properties: union of default and override keys, sorted.
        let mut keys: Vec<&String> = def
            .defaults
            .props
            .keys()
            .chain(placement.props.keys())
            .collect();
        keys.sort();
        keys.dedup();
        let has_props = !keys.is_empty();
        for key in keys {
            col = col.push(Self::prop_editor_row(
                wid,
                pid.clone(),
                key.clone(),
                &placement,
                &def,
            ));
        }
        if !has_props {
            col = col.push(hint(
                "This widget declares no custom properties — interval and size above are the knobs.",
            ));
        }
        col.spacing(10).width(Length::Fill).into()
    }

    /// One custom-property row: schema-driven control (checkbox, spin,
    /// text, or choice presets) with an inherited-value caption and a
    /// Reset that clears the override.
    fn prop_editor_row(
        wid: window::Id,
        pid: String,
        key: String,
        placement: &crate::config::WidgetPlacement,
        def: &crate::config::WidgetDefinition,
    ) -> Element<'static, Plant> {
        editors::widget_property(wid, pid, key, placement, def)
    }

    /// Pick the wallpaper image the editor below the map edits
    /// (`SettingEvent::SelectImage`).
    pub(crate) fn handle_select_image(
        plots: &mut Plots,
        id: window::Id,
        index: usize,
    ) -> Command<Plant> {
        if let Some(setting) = plots.settings.get_mut(&id) {
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

/// Label + slider emitting a [`Plant`] directly (per-bar Top controls).
/// The message is built by closure so any event fits.
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
    let bounds = format!("{:.0} – {:.0}", range.start(), range.end());
    let mut sl = slider(range, value, msg).step(step).width(Length::Fill);
    if let Some(on_release) = release {
        sl = sl.on_release(on_release);
    }
    container(
        column![
            row![
                text(label)
                    .size(14)
                    .color(theme::text())
                    .width(Length::Fill),
                hint(bounds)
            ]
            .spacing(12),
            sl,
        ]
        .spacing(10),
    )
    .padding(12)
    .width(Length::Fill)
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

/// Label + [`spin_box`] + Reset button for one wallpaper image property.
/// Commits go through `ConfigPatch`; reset restores the entry default.
fn image_spin_row(
    label: impl Into<String>,
    value: f64,
    range: RangeInclusive<f64>,
    step: f64,
    decimals: usize,
    on_commit: impl Fn(f64) -> Plant + Clone + 'static,
    on_reset: Plant,
) -> Element<'static, Plant> {
    container(
        column![
            text(label.into())
                .size(14)
                .color(theme::text())
                .width(Length::Fill),
            row![
                spin_box(value, range, step, decimals, on_commit).width(Length::Fill),
                button(text("Reset default").size(12).color(theme::button_text()))
                    .on_press(on_reset)
                    .padding(6)
                    .style(theme::menu_button(theme::RADIUS)),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(8),
    )
    .padding(10)
    .width(Length::Fill)
    .into()
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
