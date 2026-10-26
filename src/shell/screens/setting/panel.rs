//! Read-only inputs required by the Panel page, its Lua controls, and the
//! native arrangement preview.
use super::{Setting, hint, section, section_heading};
use crate::{
    config::WidgetDefinition,
    shell::{
        Plant, SettingEvent, screens::Top, screens::top::TopLocal, state::Plots, windows::PlotInfo,
    },
    theme,
    ui::widgets::panel_preview::Preview,
};
use iced::window;
use iced::{
    Element, Length,
    widget::{Space, button, column, row, scrollable, text},
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;

pub(super) struct PanelContext<'a> {
    pub theme: &'a crate::config::ThemeConfig,
    pub defaults: &'a crate::ui::settings::ComponentDefaults,
    pub ids: &'a HashMap<window::Id, PlotInfo>,
    pub tops: &'a HashMap<window::Id, Top>,
    pub output_infos: &'a HashMap<OutputId, OutputInfo>,
    pub widgets: &'a [WidgetDefinition],
}

impl<'a> From<&'a Plots> for PanelContext<'a> {
    fn from(plots: &'a Plots) -> Self {
        Self {
            theme: &plots.config.theme,
            defaults: &plots.config.component_defaults,
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
    let count = top
        .local
        .slots
        .clamp(1, crate::shell::screens::top::TopLocal::MAX_SLOTS) as usize;
    let slot = setting
        .selected_slot
        .filter(|slot| *slot < count)
        .unwrap_or(0);
    col = col.push(setting.panel_bar.borrow_mut().view(
        super::panel_bar::Section::Layout,
        super::panel_bar::Context {
            setting: id,
            bar: wid,
            top,
            thick_max,
            selected_slot: slot,
            theme: context.theme,
            defaults: context.defaults,
        },
    ));
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
    col = col.push(
        setting
            .placement_editor
            .borrow_mut()
            .view(super::placement::Context {
                bar: wid,
                output: &context.output_name(wid),
                top,
                selected: setting.selected_placement.as_deref(),
                catalog: context.widgets,
                theme: context.theme,
                defaults: context.defaults,
            }),
    );
    col = col.push(setting.panel_bar.borrow_mut().view(
        super::panel_bar::Section::Appearance,
        super::panel_bar::Context {
            setting: id,
            bar: wid,
            top,
            thick_max,
            selected_slot: slot,
            theme: context.theme,
            defaults: context.defaults,
        },
    ));
    col.spacing(16).width(Length::Fill).into()
}
