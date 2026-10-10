//! Per-placement configuration editor, driven by a catalog snapshot.
use super::{
    editors::{prop_row, widget_property},
    hint, section, section_heading,
};
use crate::{
    config::WidgetDefinition,
    shell::{BarEvent, PlacementProp, Plant, TopEvent, screens::Top},
    ui::widgets::spin_box::spin_box,
};
use iced::{Element, Length, widget::column, window};
use std::rc::Rc;

pub(super) fn view(
    wid: window::Id,
    output: String,
    top: &Top,
    selected: Option<&str>,
    catalog: &[WidgetDefinition],
) -> Element<'static, Plant> {
    let Some(id) = selected else {
        return hint(
            "Click a widget chip above to edit its size, refresh rate, and custom properties.",
        );
    };
    let found = top
        .local
        .widgets
        .iter()
        .enumerate()
        .find_map(|(slot, entries)| {
            entries
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| (slot, entry.clone()))
        });
    let Some((slot, placement)) = found else {
        return hint("That widget left the bar — pick another chip to edit its settings.");
    };
    let pid = placement.id.clone();
    let message_id = pid.clone();
    let commit = Rc::new(move |patch| {
        Plant::TopPlot(TopEvent::Bar(BarEvent::WidgetProp {
            bar: wid,
            placement: message_id.clone(),
            patch,
        }))
    });
    let location = if output.is_empty() {
        format!("Slot {}", slot + 1)
    } else {
        format!("Slot {} · {output}", slot + 1)
    };
    let Some(definition) = catalog
        .iter()
        .find(|definition| definition.name == placement.name)
    else {
        return section(
            &format!("{} · {location}", placement.name),
            "This placement references a widget with no definition file.",
            hint("Add widgets/<name>.lua, or drag its chip to the pool to remove it."),
        );
    };
    let definition = definition.clone();
    let mut col = column![section_heading(
        &format!("{} · {location}", placement.name),
        "Overrides apply to this instance only. Reset returns to the widget default."
    )]
    .spacing(8);
    let reset_commit = commit.clone();
    let reset = placement
        .interval
        .is_some()
        .then(|| reset_commit(PlacementProp::Interval(None)));
    let change = commit.clone();
    col = col.push(prop_row(
        "Refresh interval".into(),
        placement
            .interval
            .is_none()
            .then(|| format!("Widget default: {:.2}s", definition.defaults.interval)),
        spin_box(
            placement.effective_interval(&definition.defaults) as f64,
            0.25..=3600.0,
            0.25,
            2,
            move |value| change(PlacementProp::Interval(Some(value as f32))),
        )
        .width(Length::Fill)
        .into(),
        reset,
    ));
    let reset_commit = commit.clone();
    let reset = placement
        .size
        .is_some()
        .then(|| reset_commit(PlacementProp::Size(None)));
    let change = commit.clone();
    col = col.push(prop_row(
        "Text size".into(),
        placement
            .size
            .is_none()
            .then(|| format!("Widget default: {:.1}px", definition.defaults.size)),
        spin_box(
            placement.effective_size(&definition.defaults) as f64,
            1.0..=128.0,
            0.5,
            1,
            move |value| change(PlacementProp::Size(Some(value as f32))),
        )
        .width(Length::Fill)
        .into(),
        reset,
    ));
    let mut keys: Vec<_> = definition
        .defaults
        .props
        .keys()
        .chain(placement.props.keys())
        .collect();
    keys.sort();
    keys.dedup();
    let has_props = !keys.is_empty();
    for key in keys {
        col = col.push(widget_property(
            wid,
            pid.clone(),
            key.clone(),
            &placement,
            &definition,
        ));
    }
    if !has_props {
        col = col.push(hint(
            "This widget declares no custom properties — interval and size above are the knobs.",
        ));
    }
    col.spacing(10).width(Length::Fill).into()
}
