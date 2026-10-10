//! Property-row presentation and messages shared by Settings editors.
use super::hint;
use crate::ui::{style::input_style, widgets::spin_box::spin_box};
use crate::{
    config::{PropValue, WidgetDefinition, WidgetPlacement},
    shell::{BarEvent, PlacementProp, Plant, TopEvent},
    theme,
};
use iced::{
    Element, Length,
    widget::{Checkbox, button, column, container, row, slider, text, text_input},
    window,
};
use std::ops::RangeInclusive;
use std::rc::Rc;

pub(super) fn plant_slider_row(
    label: String,
    value: f64,
    range: RangeInclusive<f64>,
    step: f64,
    msg: impl Fn(f64) -> Plant + 'static,
    release: Option<Plant>,
) -> Element<'static, Plant> {
    let bounds = format!("{:.0} – {:.0}", range.start(), range.end());
    let mut control = slider(range, value, msg).step(step).width(Length::Fill);
    if let Some(release) = release {
        control = control.on_release(release);
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
            control
        ]
        .spacing(10),
    )
    .padding(12)
    .width(Length::Fill)
    .into()
}

pub(super) fn image_spin_row(
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
                    .style(theme::menu_button(theme::RADIUS))
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center)
        ]
        .spacing(8),
    )
    .padding(10)
    .width(Length::Fill)
    .into()
}

pub(super) struct PropertyControl {
    pub label: String,
    pub kind: String,
    pub value: Option<PropValue>,
    pub min: f64,
    pub max: f64,
    pub choices: Vec<String>,
    pub widget_layout: bool,
}

pub(super) fn property_control(
    spec: PropertyControl,
    change: Rc<dyn Fn(PropValue) -> Plant>,
) -> Element<'static, Plant> {
    match spec.kind.as_str() {
        "boolean" => {
            let checked = matches!(spec.value, Some(PropValue::Bool(true)));
            let mut checkbox =
                Checkbox::new(checked).on_toggle(move |value| change(PropValue::Bool(value)));
            if spec.widget_layout {
                checkbox = checkbox.label(spec.label).width(Length::Fill);
            }
            checkbox.into()
        }
        "number" => {
            let current = match spec.value {
                Some(PropValue::Number(value)) => value,
                _ => 0.0,
            };
            let control = spin_box(current, spec.min..=spec.max, 1.0, 2, move |value| {
                change(PropValue::Number(value))
            });
            if spec.widget_layout {
                control.width(Length::Fill).into()
            } else {
                control.into()
            }
        }
        _ if spec.choices.is_empty() => {
            let current = match spec.value {
                Some(PropValue::Text(value)) => value,
                _ => String::new(),
            };
            let mut control = text_input("", &current)
                .padding(6)
                .style(input_style)
                .on_input(move |value| change(PropValue::Text(value)));
            if spec.widget_layout {
                control = control.size(12).width(Length::Fill);
            }
            control.into()
        }
        _ => {
            let current = match spec.value {
                Some(PropValue::Text(value)) => value,
                _ => String::new(),
            };
            let mut choices = row![].spacing(8);
            for choice in spec.choices {
                let selected = choice == current;
                choices = choices.push(
                    button(text(choice.clone()).size(12).color(theme::text()))
                        .on_press(change(PropValue::Text(choice)))
                        .padding(6)
                        .style(theme::nav_button(selected)),
                );
            }
            choices.into()
        }
    }
}

pub(super) fn widget_property(
    wid: window::Id,
    pid: String,
    key: String,
    placement: &WidgetPlacement,
    def: &WidgetDefinition,
) -> Element<'static, Plant> {
    let schema = def.schema.get(&key);
    let default = def.defaults.props.get(&key);
    let overridden = placement.props.get(&key);
    let value = overridden.or(default);
    let kind = schema
        .and_then(|schema| schema.prop_type.as_deref())
        .filter(|kind| ["boolean", "number", "string"].contains(kind))
        .unwrap_or_else(|| {
            default
                .or(overridden)
                .map(PropValue::kind)
                .unwrap_or("string")
        });
    let label = schema
        .and_then(|schema| schema.label.clone())
        .unwrap_or_else(|| key.clone());
    let reset = overridden
        .is_some()
        .then(|| patch_prop_msg(wid, pid.clone(), key.clone(), None));
    let subtitle = schema
        .and_then(|schema| schema.description.clone())
        .or_else(|| default.map(|value| format!("Widget default: {}", prop_display(value))));
    let control = property_control(
        PropertyControl {
            label: label.clone(),
            kind: kind.into(),
            value: value.cloned(),
            min: schema.and_then(|schema| schema.min).unwrap_or(0.0),
            max: schema.and_then(|schema| schema.max).unwrap_or(1_000_000.0),
            choices: schema
                .map(|schema| schema.choices.clone())
                .unwrap_or_default(),
            widget_layout: true,
        },
        Rc::new(move |value| patch_prop_msg(wid, pid.clone(), key.clone(), Some(value))),
    );
    prop_row(label, subtitle, control, reset)
}

pub(super) fn patch_prop_msg(
    wid: window::Id,
    pid: String,
    key: String,
    value: Option<PropValue>,
) -> Plant {
    Plant::TopPlot(TopEvent::Bar(BarEvent::WidgetProp {
        bar: wid,
        placement: pid,
        patch: PlacementProp::Prop { key, value },
    }))
}

pub(super) fn prop_display(value: &PropValue) -> String {
    match value {
        PropValue::Bool(value) => value.to_string(),
        PropValue::Number(value) if value.fract() == 0.0 => format!("{value:.0}"),
        PropValue::Number(value) => value.to_string(),
        PropValue::Text(value) => value.clone(),
    }
}

pub(super) fn prop_row(
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
