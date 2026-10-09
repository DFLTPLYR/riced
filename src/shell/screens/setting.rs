use super::background::Background;
use super::top::{SlotAlign, Top, TopLocal};
use crate::app::ConfigEvent;
use crate::app::app::{PlotInfo, Plots};
use crate::app::layers::ContextMenu;
use crate::app::{BarEvent, Corner, Edge, Plant, StyleEvent, TopEvent};
use crate::components::display_map::{MapLayer, MapView, images_layer, outputs_layer};
use crate::components::panel_preview::Preview;
use crate::composables::spin_box::spin_box;
use crate::config::{AnimationSpeed, BackgroundImage, ConfigPatch};
use crate::theme;
use iced::widget::{
    Checkbox, Space, button, column, container, row, rule, scrollable, slider, stack, text,
    text_input,
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
                    .on_press(Plant::SettingPlot(crate::app::SettingEvent::Select(
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
            SettingPage::Panel => self.panel_content(id, plots),
            SettingPage::ContextMenu => self.context_menu_content(plots),
            SettingPage::Wallpaper => self.wallpaper_content(id, plots),
            SettingPage::Theme => self.theme_content(plots),
            SettingPage::Animation => self.animation_content(plots),
        }
    }

    fn context_menu_content(&self, plots: &Plots) -> Element<'_, Plant> {
        use crate::config::ComposableKind;
        column![
            section(
                "Menu surface",
                "Lua supplies sizing and appearance; Rust supplies the entries.",
                Self::composable_editor(plots, ComposableKind::ContextMenu)
            ),
            section(
                "Menu entries",
                "Each entry is rendered by this Lua app with label/action host props.",
                Self::composable_editor(plots, ComposableKind::ContextMenuItem)
            ),
        ]
        .spacing(16)
        .width(Length::Fill)
        .into()
    }

    fn composable_editor(
        plots: &Plots,
        kind: crate::config::ComposableKind,
    ) -> Element<'static, Plant> {
        use crate::config::{ComposableKind, PropValue};
        let config = plots.config.composable.get(kind);
        let host = match kind {
            ComposableKind::ContextMenu => {
                super::background::context_menu_host(&plots.config.composable.context_menu_item)
            }
            _ => serde_json::json!({ "label": "Preview", "action": "preview" }),
        };
        let rendered = plots.composable_runtime.borrow_mut().render(
            &format!("settings/{kind:?}"),
            config,
            host,
            &plots.config.theme,
            plots.components_mtime,
        );
        let mut props = config.props.clone();
        if let Some(rendered) = rendered
            && let Some(values) = rendered.props.as_object()
        {
            for (key, value) in values {
                if ["label", "action", "_item_revision"].contains(&key.as_str()) {
                    continue;
                }
                if let Ok(value) = serde_json::from_value::<PropValue>(value.clone()) {
                    props.entry(key.clone()).or_insert(value);
                }
            }
        }
        let source = text_input("component.lua", &config.src)
            .padding(6)
            .style(prop_input_style)
            .on_input(move |src| {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableSource(kind, src)))
            });
        let mut col = column![prop_row(
            "Source".into(),
            Some("Relative to components/; absolute paths also work.".into()),
            source.into(),
            None
        ),]
        .spacing(10);
        for (key, value) in props {
            let reset = config.props.contains_key(&key).then(|| {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableProp(
                    kind,
                    key.clone(),
                    None,
                )))
            });
            let key_for_control = key.clone();
            let patch = move |value| {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableProp(
                    kind,
                    key_for_control.clone(),
                    Some(value),
                )))
            };
            let control: Element<'static, Plant> = match value {
                PropValue::Bool(value) => Checkbox::new(value)
                    .on_toggle(move |v| patch(PropValue::Bool(v)))
                    .into(),
                PropValue::Number(value) => {
                    let minimum = if key == "width" || key == "height" {
                        1.0
                    } else {
                        0.0
                    };
                    spin_box(value, minimum..=1_000_000.0, 1.0, 2, move |v| {
                        patch(PropValue::Number(v))
                    })
                    .into()
                }
                PropValue::Text(value) => text_input("", &value)
                    .padding(6)
                    .style(prop_input_style)
                    .on_input(move |v| patch(PropValue::Text(v)))
                    .into(),
            };
            let subtitle = if key == "height" && matches!(kind, ComposableKind::ContextMenu) {
                "Fixed height when auto_sizing is off. Reset inherits the Lua default."
            } else {
                "Reset clears the override and inherits the Lua default."
            };
            col = col.push(prop_row(key, Some(subtitle.into()), control, reset));
        }
        col.width(Length::Fill).into()
    }

    /// Panel page: picker row of bars on top, controls for the picked bar
    /// below (defaults to the first). Length % + thickness px, floating
    /// (+ margins when floating) and per-corner rounding.
    fn panel_content(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
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
                    .on_press(Plant::SettingPlot(crate::app::SettingEvent::SelectBar(
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
                        .on_press(Plant::SettingPlot(crate::app::SettingEvent::SelectSlot(
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
                    Top::output_name(plots, wid),
                    top,
                    self.selected_placement.as_deref(),
                    plots,
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
        let current = &plots.config.theme;
        let mut list = column![
            section_heading(
                "Color mode",
                &format!(
                    "Current palette: {} · {}",
                    current.name,
                    if current.darkmode { "Dark" } else { "Light" }
                )
            ),
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
        .spacing(16);
        list = list.push(section_heading(
            "Choose a palette",
            "Swatches preview the primary, secondary, and tertiary colors.",
        ));
        for name in theme::available_themes() {
            let selected = name == current.name;
            let preview = theme::preview(&name, current.darkmode);
            list = list.push(
                button(
                    row![
                        text(name.clone()).size(13).color(preview.on_surface),
                        text(if selected { "Selected" } else { "" })
                            .size(11)
                            .color(preview.on_surface),
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
                .padding(14)
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
        let mut speeds = column![].spacing(10);
        for speed in AnimationSpeed::all() {
            let selected = speed == current;
            speeds = speeds.push(
                button(
                    column![
                        row![
                            text(speed.title())
                                .size(14)
                                .color(theme::text())
                                .width(Length::Fill),
                            text(format!(
                                "{} ms{}",
                                speed.duration().as_millis(),
                                if selected { " · Selected" } else { "" }
                            ))
                            .size(12)
                            .color(theme::text()),
                        ]
                        .spacing(8),
                        hint(match speed {
                            AnimationSpeed::Fast => "Quick and responsive",
                            AnimationSpeed::Medium => "Balanced everyday transitions",
                            AnimationSpeed::Slow => "Relaxed and more pronounced",
                        }),
                    ]
                    .spacing(5)
                    .width(Length::Fill),
                )
                .width(Length::Fill)
                .on_press(Plant::Config(ConfigEvent::Patch(
                    ConfigPatch::AnimationSpeed(speed),
                )))
                .padding(14)
                .style(theme::nav_button(selected)),
            );
        }
        column![
            section(
                "Transition speed",
                "Shorter durations feel snappier; longer durations make changes more gradual.",
                speeds.width(Length::Fill).into()
            ),
            hint("Some Lua listviews define their own durations and override this global setting."),
        ]
        .spacing(8)
        .width(Length::Fill)
        .into()
    }

    fn wallpaper_content(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        column![
            section_heading("Display layout", "Drag images to position them. Use the map to see how they overlap your displays."),
            row![
                button(text("Add wallpaper…").size(13).color(theme::button_text()))
                    .on_press(Plant::BackgroundPlot(
                        crate::app::BackgroundEvent::PickWallpaper
                    ))
                    .padding(8)
                    .style(theme::menu_button(theme::RADIUS)),
                Space::new().width(Length::Fill),
            ]
            .spacing(8),
            self.wallpaper_grid(id, plots),
            section("Image properties", "Choose an image below to adjust its placement. Reset restores that property's default.", self.wallpaper_images(id, plots)),
        ]
        .spacing(16)
        .into()
    }

    /// Image list below the map: picker row, remove, and per-property
    /// spinboxes (x/y/z/width/height/scale) each with a reset button.
    /// Commits go through `ConfigPatch` like every other panel edit, so
    /// coalesced save + regen arming apply unchanged.
    fn wallpaper_images(&self, id: window::Id, plots: &Plots) -> Element<'_, Plant> {
        let images = &plots.config.background.image;
        if images.is_empty() {
            return column![
                text("No images yet.").size(13).color(theme::text()),
                text("Add wallpaper… to place the first one.").size(11),
            ]
            .spacing(4)
            .into();
        }
        // Clamped pick (stale after external edits, like the bar picker).
        let sel = match self.selected_image {
            Some(s) if s < images.len() => s,
            _ => 0,
        };
        let mut picker = row![].spacing(8);
        for (i, img) in images.iter().enumerate() {
            let name = image_file_name(img);
            picker = picker.push(
                button(
                    text(format!("{} · {name}", i + 1))
                        .size(12)
                        .color(theme::text()),
                )
                .on_press(Plant::SettingPlot(crate::app::SettingEvent::SelectImage(
                    id, i,
                )))
                .padding(8)
                .style(theme::nav_button(i == sel)),
            );
        }
        let img = &images[sel];
        let name = image_file_name(img);
        let (ix, iy, iz, iw, ih, iscale) = (img.x, img.y, img.z, img.width, img.height, img.scale);
        // Actual file resolution for Width/Height reset (falls back to
        // 0 = native flag when the file is unreadable).
        let (fw, fh) = MapLayer::file_dimensions(img).unwrap_or((0.0, 0.0));
        let (shown_width, shown_height) = MapLayer::size_with_file(img, (fw, fh));
        // Match the renderer's effective scale, including wheel values up
        // to 10x and larger valid values loaded from config.toml.
        let shown_scale = iscale.max(0.01);
        let mut col = column![
            scrollable(picker).direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::default()
            ))
        ]
        .spacing(16);
        col = col.push(
            row![
                text(format!("Image {} · {name}", sel + 1))
                    .size(13)
                    .color(theme::text())
                    .width(Length::Fill),
                button(text("Remove image").size(12).color(theme::active().error))
                    .on_press(Plant::Config(ConfigEvent::Patch(
                        ConfigPatch::RemoveImage { index: sel }
                    )))
                    .padding(6)
                    .style(theme::menu_button_tinted(
                        theme::RADIUS,
                        theme::active().error
                    )),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );
        col = col.push(hint(format!("Source resolution: {fw:.0} × {fh:.0} px")));
        col = col.push(section_heading(
            "Position & stacking",
            "X and Y are desktop coordinates. Higher Z values place the image in front.",
        ));
        col = col.push(image_spin_row(
            "X (px)",
            ix as f64,
            -20000.0..=20000.0,
            1.0,
            1,
            move |v| {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveImage {
                    index: sel,
                    x: v as f32,
                    y: iy,
                }))
            },
            Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveImage {
                index: sel,
                x: 0.0,
                y: iy,
            })),
        ));
        col = col.push(image_spin_row(
            "Y (px)",
            iy as f64,
            -20000.0..=20000.0,
            1.0,
            1,
            move |v| {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveImage {
                    index: sel,
                    x: ix,
                    y: v as f32,
                }))
            },
            Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveImage {
                index: sel,
                x: ix,
                y: 0.0,
            })),
        ));
        col = col.push(image_spin_row(
            "Z (stack)",
            iz as f64,
            -100.0..=100.0,
            1.0,
            0,
            move |v| {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageZ {
                    index: sel,
                    z: v as i32,
                }))
            },
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageZ {
                index: sel,
                z: 0,
            })),
        ));
        col = col.push(section_heading(
            "Dimensions & scale",
            "Width and height define the base size; scale multiplies both dimensions.",
        ));
        col = col.push(image_spin_row(
            "Width (px)",
            shown_width as f64,
            0.0..=16000.0_f64.max(shown_width as f64),
            1.0,
            0,
            move |v| {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageSize {
                    index: sel,
                    width: v as f32,
                    height: ih,
                }))
            },
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageSize {
                index: sel,
                width: fw,
                height: ih,
            })),
        ));
        col = col.push(image_spin_row(
            "Height (px)",
            shown_height as f64,
            0.0..=16000.0_f64.max(shown_height as f64),
            1.0,
            0,
            move |v| {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageSize {
                    index: sel,
                    width: iw,
                    height: v as f32,
                }))
            },
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageSize {
                index: sel,
                width: iw,
                height: fh,
            })),
        ));
        col = col.push(image_spin_row(
            "Scale (×)",
            shown_scale as f64,
            0.01..=10.0_f64.max(shown_scale as f64),
            0.1,
            2,
            move |v| {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageScale {
                    index: sel,
                    scale: v as f32,
                }))
            },
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageScale {
                index: sel,
                scale: 1.0,
            })),
        ));
        col.spacing(8).width(Length::Fill).into()
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
        .height(Length::Fixed(360.0))
        .clip(true)
        .width(Length::Fill)
        .into()
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
        plots: &Plots,
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
        let commit = std::rc::Rc::new(move |patch: crate::app::PlacementProp| {
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
                .then(|| commit_interval(crate::app::PlacementProp::Interval(None)));
            let commit_value = commit.clone();
            col = col.push(prop_row(
                "Refresh interval".to_string(),
                placement
                    .interval
                    .is_none()
                    .then(|| format!("Widget default: {:.2}s", def.defaults.interval)),
                spin_box(effective as f64, 0.25..=3600.0, 0.25, 2, move |v| {
                    commit_value(crate::app::PlacementProp::Interval(Some(v as f32)))
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
                .then(|| commit_size(crate::app::PlacementProp::Size(None)));
            let commit_value = commit.clone();
            col = col.push(prop_row(
                "Text size".to_string(),
                placement
                    .size
                    .is_none()
                    .then(|| format!("Widget default: {:.1}px", def.defaults.size)),
                spin_box(effective as f64, 1.0..=128.0, 0.5, 1, move |v| {
                    commit_value(crate::app::PlacementProp::Size(Some(v as f32)))
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
        use crate::config::PropValue;
        let schema = def.schema.get(&key);
        let default = def.defaults.props.get(&key);
        let overridden = placement.props.get(&key);
        let value = overridden.or(default);
        let kind = schema
            .and_then(|s| s.prop_type.as_deref())
            .filter(|t| ["boolean", "number", "string"].contains(t))
            .unwrap_or_else(|| {
                default
                    .map(PropValue::kind)
                    .or_else(|| overridden.map(PropValue::kind))
                    .unwrap_or("string")
            });
        let label = schema
            .and_then(|s| s.label.clone())
            .unwrap_or_else(|| key.clone());
        // Reset clears the override so the key inherits again (absent
        // keys fall back to widget defaults at render).
        let reset = overridden
            .is_some()
            .then(|| patch_prop_msg(wid, pid.clone(), key.clone(), None));
        let inherited = default.map(|dflt| format!("Widget default: {}", prop_display(dflt)));
        let subtitle = schema.and_then(|s| s.description.clone()).or(inherited);
        match kind {
            "boolean" => {
                let checked = matches!(value, Some(PropValue::Bool(true)));
                let (wid_b, pid_b, key_b) = (wid, pid.clone(), key.clone());
                let control = Checkbox::new(checked)
                    .label(label.clone())
                    .on_toggle(move |b| {
                        patch_prop_msg(
                            wid_b,
                            pid_b.clone(),
                            key_b.clone(),
                            Some(PropValue::Bool(b)),
                        )
                    })
                    .width(Length::Fill)
                    .into();
                prop_row(label, subtitle, control, reset)
            }
            "number" => {
                let current = match value {
                    Some(PropValue::Number(n)) => *n,
                    _ => 0.0,
                };
                let min = schema.and_then(|s| s.min).unwrap_or(0.0);
                let max = schema.and_then(|s| s.max).unwrap_or(1_000_000.0);
                let (wid_n, pid_n, key_n) = (wid, pid.clone(), key.clone());
                let control = spin_box(current, min..=max, 1.0, 2, move |v| {
                    patch_prop_msg(
                        wid_n,
                        pid_n.clone(),
                        key_n.clone(),
                        Some(PropValue::Number(v)),
                    )
                })
                .width(Length::Fill)
                .into();
                prop_row(label, subtitle, control, reset)
            }
            _ => {
                let choices = schema.map(|s| s.choices.clone()).unwrap_or_default();
                if choices.is_empty() {
                    let current = match value {
                        Some(PropValue::Text(s)) => s.clone(),
                        _ => String::new(),
                    };
                    let (wid_t, pid_t, key_t) = (wid, pid.clone(), key.clone());
                    let control = text_input("", &current)
                        .size(12)
                        .width(Length::Fill)
                        .padding(6)
                        .style(prop_input_style)
                        .on_input(move |typed| {
                            patch_prop_msg(
                                wid_t,
                                pid_t.clone(),
                                key_t.clone(),
                                Some(PropValue::Text(typed)),
                            )
                        })
                        .into();
                    prop_row(label, subtitle, control, reset)
                } else {
                    let current = match value {
                        Some(PropValue::Text(s)) => s.clone(),
                        _ => String::new(),
                    };
                    let mut presets = row![].spacing(8);
                    for choice in choices {
                        let selected = *choice == current;
                        presets = presets.push(
                            button(text(choice.clone()).size(12).color(theme::text()))
                                .on_press(patch_prop_msg(
                                    wid,
                                    pid.clone(),
                                    key.clone(),
                                    Some(PropValue::Text(choice)),
                                ))
                                .padding(6)
                                .style(theme::nav_button(selected)),
                        );
                    }
                    prop_row(label, subtitle, presets.into(), reset)
                }
            }
        }
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
            plots.last_cursor.remove(&id);
            plots.press_targets.remove(&id);
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

/// Settings-surface text input: card background, hairline border, theme
/// text (mirrors the spin_box input).
use crate::ui::style::input_style as prop_input_style;

/// Build a [`BarEvent::WidgetProp`] patch message for one placement
/// property (shared by every editor control; keeps closures small).
fn patch_prop_msg(
    wid: window::Id,
    pid: String,
    key: String,
    value: Option<crate::config::PropValue>,
) -> Plant {
    Plant::TopPlot(TopEvent::Bar(BarEvent::WidgetProp {
        bar: wid,
        placement: pid,
        patch: crate::app::PlacementProp::Prop { key, value },
    }))
}

/// Human-readable property value for inherited-value captions.
fn prop_display(value: &crate::config::PropValue) -> String {
    use crate::config::PropValue;
    match value {
        PropValue::Bool(b) => b.to_string(),
        PropValue::Number(n) => {
            if n.fract() == 0.0 {
                format!("{n:.0}")
            } else {
                n.to_string()
            }
        }
        PropValue::Text(s) => s.clone(),
    }
}

/// One editor row: title, control, optional Reset (shown only for
/// overrides), optional inherited-value caption.
fn prop_row(
    title: String,
    inherited: Option<String>,
    control: Element<'static, Plant>,
    on_reset: Option<Plant>,
) -> Element<'static, Plant> {
    let mut header = row![
        text(title)
            .size(14)
            .color(theme::text())
            .width(Length::Fill)
    ];
    if let Some(reset) = on_reset {
        header = header.push(
            button(text("Reset").size(12).color(theme::button_text()))
                .on_press(reset)
                .padding(6)
                .style(theme::menu_button(theme::RADIUS)),
        );
    }
    let mut col = column![header.spacing(12), control].spacing(8);
    if let Some(caption) = inherited {
        col = col.push(hint(caption));
    }
    container(col).padding(12).width(Length::Fill).into()
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
            drag: Some(crate::components::display_map::MapDrag {
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
