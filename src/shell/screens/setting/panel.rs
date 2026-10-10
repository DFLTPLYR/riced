//! Read-only inputs required by the Panel page and placement editor.
use super::editors::plant_slider_row;
use super::{Setting, hint, section, section_heading};
use crate::{
    config::WidgetDefinition,
    shell::{screens::Top, state::Plots, windows::PlotInfo},
};
use crate::{
    shell::{
        BarEvent, Corner, Edge, Plant, SettingEvent, StyleEvent, TopEvent,
        screens::top::{SlotAlign, TopLocal},
    },
    theme,
    ui::widgets::{panel_preview::Preview, spin_box::spin_box},
};
use iced::window;
use iced::{
    Element, Length,
    widget::{Checkbox, Space, button, column, row, rule, scrollable, text},
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;

pub(super) struct PanelContext<'a> {
    pub ids: &'a HashMap<window::Id, PlotInfo>,
    pub tops: &'a HashMap<window::Id, Top>,
    pub output_infos: &'a HashMap<OutputId, OutputInfo>,
    pub widgets: &'a [WidgetDefinition],
}

impl<'a> From<&'a Plots> for PanelContext<'a> {
    fn from(plots: &'a Plots) -> Self {
        Self {
            ids: &plots.windows.ids,
            tops: &plots.windows.tops,
            output_infos: &plots.windows.output_infos,
            widgets: &plots.catalog.definitions,
        }
    }
}

impl PanelContext<'_> {
    pub fn output_name(&self, bar: window::Id) -> String {
        let Some(PlotInfo::Top(output)) = self.ids.get(&bar) else {
            return String::new();
        };
        self.output_infos
            .get(output)
            .and_then(|info| info.name.clone())
            .unwrap_or_default()
    }
}

pub(super) fn view<'a>(
    setting: &'a Setting,
    id: window::Id,
    context: PanelContext<'_>,
) -> Element<'a, Plant> {
    let mut col = column![section_heading(
        "Choose a bar",
        "Each display can have its own bar configuration."
    )]
    .spacing(16);
    let mut bars: Vec<_> = context
        .ids
        .iter()
        .filter_map(|(wid, info)| match info {
            PlotInfo::Top(output) => Some((*wid, *output)),
            _ => None,
        })
        .collect();
    bars.sort_by_key(|(wid, output)| (output.0, format!("{wid:?}")));
    if bars.is_empty() {
        return col
            .push(
                column![
                    text("No Bar Available").size(13).color(theme::text()),
                    text("Right-click the wallpaper, then Add Top.").size(11)
                ]
                .spacing(4)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .spacing(8)
            .width(Length::Fill)
            .into();
    }
    let selected = setting
        .selected_bar
        .filter(|wid| bars.iter().any(|(bar, _)| bar == wid))
        .unwrap_or(bars[0].0);
    let mut picker = row![].spacing(8);
    for (wid, output) in &bars {
        let connector = context
            .output_infos
            .get(output)
            .and_then(|info| info.name.clone())
            .unwrap_or_else(|| "Unknown display".into());
        let label = context
            .tops
            .get(wid)
            .map(|top| format!("{connector} · {}", top.anchor_label()))
            .unwrap_or(connector);
        picker = picker.push(
            button(text(label).size(13).color(theme::text()))
                .on_press(Plant::SettingPlot(SettingEvent::SelectBar(id, *wid)))
                .padding(8)
                .style(theme::nav_button(*wid == selected)),
        );
    }
    col = col.push(
        scrollable(picker).direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::default(),
        )),
    );
    let remove_label = context
        .tops
        .get(&selected)
        .map(|top| format!("Remove {} bar", top.anchor_label()))
        .unwrap_or_else(|| "Remove bar".into());
    let remove = button(text(remove_label).size(13).color(theme::active().error))
        .on_press(Plant::TopPlot(TopEvent::Remove(selected)))
        .padding(8)
        .style(theme::menu_button_tinted(
            theme::RADIUS,
            theme::active().error,
        ))
        .width(Length::Shrink);
    let output = bars
        .iter()
        .find_map(|(wid, output)| (*wid == selected).then_some(*output));
    let (Some(output), Some(top)) = (output, context.tops.get(&selected)) else {
        return col
            .push(
                column![text("No Bar Available").size(13).color(theme::text())]
                    .width(Length::Fill)
                    .align_x(iced::Alignment::Center),
            )
            .spacing(8)
            .width(Length::Fill)
            .into();
    };
    let wid = selected;
    let horizontal = top.is_horizontal();
    let thick_max = context
        .output_infos
        .get(&output)
        .map(crate::shared::geometry::output_geometry)
        .map(|(_, _, width, height)| {
            if horizontal {
                height as f64
            } else {
                width as f64
            }
        })
        .unwrap_or(if horizontal { 1080.0 } else { 1920.0 })
        .max(1.0);
    let length = top.local.length_pct.clamp(1.0, 100.0);
    let thickness = top.local.thickness_px.clamp(1.0, thick_max as f32);
    col = col.push(section_heading(
        "Size & appearance",
        "Length is a percentage of the display edge; thickness is measured in pixels.",
    ));
    col = col.push(plant_slider_row(
        format!("Length {length:.0}%"),
        length as f64,
        1.0..=100.0,
        1.0,
        move |value| Plant::TopPlot(TopEvent::Bar(BarEvent::Length(wid, value as f32))),
        None,
    ));
    col = col.push(plant_slider_row(
        format!("Thickness {thickness:.0}px"),
        thickness as f64,
        1.0..=thick_max,
        1.0,
        move |value| Plant::TopPlot(TopEvent::Bar(BarEvent::Thickness(wid, value as f32))),
        None,
    ));
    let current = TopLocal::snap_opacity(top.local.opacity);
    let mut opacity = row![text("Opacity").width(Length::Fill)].spacing(8);
    for step in TopLocal::OPACITY_STEPS {
        opacity = opacity.push(
            button(
                text(format!("{:.0}%", step * 100.0))
                    .size(12)
                    .color(theme::text()),
            )
            .on_press(Plant::TopPlot(TopEvent::Style(StyleEvent::Opacity(
                wid, step,
            ))))
            .padding(6)
            .style(theme::nav_button(current == step)),
        );
    }
    col = col.push(opacity);
    col = col.push(section_heading(
        "Slots & widgets",
        "Slots divide the bar into regions. Select a slot to edit its alignment and widgets.",
    ));
    col = col.push(
        row![
            text(format!("Slots ({})", top.local.slots)).width(Length::FillPortion(7)),
            spin_box(
                top.local.slots as f64,
                1.0..=TopLocal::MAX_SLOTS as f64,
                1.0,
                0,
                move |value| Plant::TopPlot(TopEvent::Bar(BarEvent::Slots(wid, value as u32)))
            )
            .width(Length::Fixed(120.0))
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    );
    let padding = top.local.slot_padding.clamp(0.0, TopLocal::MAX_SLOT_GAP);
    let spacing = top.local.slot_spacing.clamp(0.0, TopLocal::MAX_SLOT_GAP);
    col = col.push(plant_slider_row(
        format!("Slot padding {padding:.0}px"),
        padding as f64,
        0.0..=32.0,
        1.0,
        move |value| Plant::TopPlot(TopEvent::Bar(BarEvent::SlotPadding(wid, value as f32))),
        None,
    ));
    col = col.push(plant_slider_row(
        format!("Slot spacing {spacing:.0}px"),
        spacing as f64,
        0.0..=32.0,
        1.0,
        move |value| Plant::TopPlot(TopEvent::Bar(BarEvent::SlotSpacing(wid, value as f32))),
        None,
    ));
    let count = top.local.slots.clamp(1, TopLocal::MAX_SLOTS) as usize;
    let slot = setting
        .selected_slot
        .filter(|slot| *slot < count)
        .unwrap_or(0);
    let mut slots_picker = row![].spacing(8);
    for position in 0..count {
        slots_picker = slots_picker.push(
            button(
                text(format!("Slot {}", position + 1))
                    .size(12)
                    .color(theme::text()),
            )
            .on_press(Plant::SettingPlot(SettingEvent::SelectSlot(id, position)))
            .padding(6)
            .style(theme::nav_button(position == slot)),
        );
    }
    col = col.push(
        scrollable(slots_picker).direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::default(),
        )),
    );
    let current = top.local.align_at(slot);
    let mut alignments = row![].spacing(8);
    for align in [SlotAlign::Start, SlotAlign::Center, SlotAlign::End] {
        alignments = alignments.push(
            button(text(align.as_str()).size(12).color(theme::text()))
                .on_press(Plant::TopPlot(TopEvent::Bar(BarEvent::SlotAlign(
                    wid, slot, align,
                ))))
                .padding(6)
                .style(theme::nav_button(current == align)),
        );
    }
    col = col.push(alignments);
    let slots = top.local.widgets.clone();
    let names: Vec<_> = context
        .widgets
        .iter()
        .filter(|definition| !TopLocal::is_empty_widget(&definition.name))
        .map(|definition| definition.name.clone())
        .collect();
    let preview = iced::widget::responsive(move |size| {
        Preview::new(
            id,
            wid,
            slots.clone(),
            names.clone(),
            horizontal,
            slot,
            setting.selected_placement.clone(),
            size.width,
        )
        .element()
    })
    .width(Length::Fill)
    .height(Length::Shrink);
    col = col.push(Space::new().height(Length::Fixed(8.0)));
    col = col.push(section("Arrange your panel", "Drag onto a widget to swap, or onto free slot space to move. Click a widget to edit its settings below. Escape cancels a drag.", preview.into()));
    if context.widgets.is_empty() {
        col = col.push(hint(
            "Add .lua files to your widgets directory to fill the available-widget pool.",
        ));
    }
    col = col.push(super::placement::view(
        wid,
        context.output_name(wid),
        top,
        setting.selected_placement.as_deref(),
        context.widgets,
    ));
    col = col.push(section_heading(
        "Floating & corners",
        "Inset the bar from the display edge and soften its corners.",
    ));
    col = col.push(
        Checkbox::new(top.local.floating)
            .label("Floating bar")
            .on_toggle(move |value| {
                Plant::TopPlot(TopEvent::Style(StyleEvent::Floating(wid, value)))
            }),
    );
    col = col.push(hint(
        "Floating adds space around the bar; the reserved desktop area stays unchanged.",
    ));
    if top.local.floating {
        for (label, edge, value) in [
            ("Margin top (px)", Edge::Top, top.local.margins.top),
            ("Margin bottom (px)", Edge::Bottom, top.local.margins.bottom),
            ("Margin right (px)", Edge::Right, top.local.margins.right),
            ("Margin left (px)", Edge::Left, top.local.margins.left),
        ] {
            col = col.push(
                row![
                    text(label).width(Length::FillPortion(7)),
                    spin_box(value as f64, 0.0..=256.0, 1.0, 0, move |value| {
                        Plant::TopPlot(TopEvent::Style(StyleEvent::Margin(wid, edge, value as i32)))
                    })
                    .width(Length::Fixed(120.0))
                ]
                .spacing(8),
            );
        }
    } else {
        col = col.push(text("Enable Floating to adjust margins.").size(11));
    }
    col = col.push(rule::horizontal(2));
    for corners in [
        [
            ("Round top-left", Corner::TopLeft, top.local.radius.top_left),
            (
                "Round top-right",
                Corner::TopRight,
                top.local.radius.top_right,
            ),
        ],
        [
            (
                "Round bottom-left",
                Corner::BottomLeft,
                top.local.radius.bottom_left,
            ),
            (
                "Round bottom-right",
                Corner::BottomRight,
                top.local.radius.bottom_right,
            ),
        ],
    ] {
        let mut group = row![].spacing(8);
        for (label, corner, value) in corners {
            group = group.push(plant_slider_row(
                format!("{label} {value:.0}"),
                value as f64,
                0.0..=32.0,
                1.0,
                move |value| {
                    Plant::TopPlot(TopEvent::Style(StyleEvent::Radius(
                        wid,
                        corner,
                        value as f32,
                    )))
                },
                None,
            ));
        }
        col = col.push(group);
    }
    col.push(section(
        "Remove this bar",
        "Remove the selected bar from the display and saved configuration.",
        remove.into(),
    ))
    .spacing(16)
    .width(Length::Fill)
    .into()
}
