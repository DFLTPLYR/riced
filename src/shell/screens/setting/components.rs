//! Per-component settable defaults editor (`component_defaults.<component>`).
//! Explicit Lua setters override these per property; Reset removes the
//! saved override so the property inherits again.
use super::section_heading;
use crate::config::{ConfigPatch, parse_lua_value as parse_config_value, to_lua_literal};
use crate::shell::{ConfigEvent, Plant, SettingComponentEdit, SettingEvent, state::Plots};
use crate::theme;
use crate::ui::settings::{ComponentDefaults, properties};
use iced::widget::{button, column, container, pick_list, row, text, text_input};
use iced::{Element, Length, window};

pub(super) fn component_names() -> Vec<String> {
    crate::ui::components::COMPONENTS
        .iter()
        .map(|component| component.name.to_owned())
        .collect()
}

fn component_at(index: usize) -> String {
    component_names()
        .get(index)
        .cloned()
        .unwrap_or_else(|| "button".into())
}

fn property_at(component: &str, index: usize) -> String {
    properties(component)
        .get(index)
        .map(|property| (*property).to_owned())
        .unwrap_or_else(|| "margin".into())
}

fn saved_value(defaults: &ComponentDefaults, component: &str, property: &str) -> String {
    defaults
        .0
        .get(component)
        .and_then(|fields| fields.get(property))
        .map(to_lua_literal)
        .unwrap_or_default()
}

/// Parse one Lua literal from the editor (`nil`/empty/`none` clears the
/// property). Scalars, `{ ... }` arrays, and `{ key = ... }` tables all
/// parse; functions and non-data values are rejected by the evaluator.
fn parse_lua_value(raw: &str) -> Result<Option<serde_json::Value>, String> {
    parse_config_value(raw).map_err(|error| format!("not a Lua value: {error}"))
}

fn apply(
    defaults: &ComponentDefaults,
    component: &str,
    property: &str,
    value: Option<serde_json::Value>,
) -> Result<ComponentDefaults, String> {
    let mut next = defaults.clone();
    next.set(component.into(), property.into(), value);
    next.validate()?;
    Ok(next)
}

fn editor_state(
    selected_component: Option<usize>,
    selected_property: Option<usize>,
    draft: Option<&str>,
    defaults: &ComponentDefaults,
) -> (String, String, String) {
    let names = component_names();
    let component = selected_component
        .filter(|index| *index < names.len())
        .map(|index| names[index].clone())
        .unwrap_or_else(|| "button".into());
    let fields = properties(&component);
    let property = selected_property
        .filter(|index| *index < fields.len())
        .map(|index| fields[index].to_owned())
        .unwrap_or_else(|| "margin".into());
    let draft = draft
        .map(str::to_owned)
        .unwrap_or_else(|| saved_value(defaults, &component, &property));
    (component, property, draft)
}

pub(super) fn view(
    id: window::Id,
    setting: &crate::shell::screens::setting::Setting,
    defaults: &ComponentDefaults,
) -> Element<'static, Plant> {
    let (component, property, draft) = editor_state(
        setting.selected_component,
        setting.selected_property,
        setting.component_draft.as_deref(),
        defaults,
    );
    let names = component_names();
    let fields: Vec<String> = properties(&component)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let submit = Plant::SettingPlot(SettingEvent::ComponentsEdit(
        id,
        SettingComponentEdit::Submit,
    ));
    let placeholder = match property.as_str() {
        "padding" | "margin" => "Number or { top = 0, right = 0, bottom = 0, left = 0 }",
        "radius" => "Number or { top_left = 0, top_right = 0, bottom_right = 0, bottom_left = 0 }",
        _ => "Lua value, e.g. 0, true, \"center\", or \"#ffffff\"",
    };
    let mut body = column![
        super::hint("Explicit Lua setters override these values individually. Reset removes the saved default."),
        row![
            pick_list(names, Some(component.clone()), move |name| {
                Plant::SettingPlot(SettingEvent::ComponentsEdit(
                    id,
                    SettingComponentEdit::Component(name),
                ))
            })
            .placeholder("Component")
            .width(Length::Fill),
            pick_list(fields, Some(property.clone()), move |name| {
                Plant::SettingPlot(SettingEvent::ComponentsEdit(
                    id,
                    SettingComponentEdit::Property(name),
                ))
            })
            .placeholder("Property")
            .width(Length::Fill),
        ]
        .spacing(12),
        text_input(placeholder, &draft)
        .on_input(move |input| Plant::SettingPlot(SettingEvent::ComponentsEdit(
            id,
            SettingComponentEdit::Draft(input),
        )))
        .on_submit(submit)
        .padding(8),
        row![
            button(text("Apply default").size(13))
                .on_press(Plant::SettingPlot(SettingEvent::ComponentsEdit(
                    id,
                    SettingComponentEdit::Submit,
                )))
                .padding(8)
                .style(theme::menu_button(theme::RADIUS)),
            button(text("Reset property").size(13))
                .on_press(Plant::SettingPlot(SettingEvent::ComponentsEdit(
                    id,
                    SettingComponentEdit::Reset,
                )))
                .padding(8)
                .style(theme::menu_button(theme::RADIUS)),
        ]
        .spacing(12),
        super::hint("Numbers and booleans are unquoted. Strings (including colors, sizes and action keys) need double quotes. Arrays and tables use { ... }. Empty, `nil`, or `none` clears the property."),
        section_heading("Saved defaults", "Persisted to config.lua under component_defaults.<component>."),
        saved_list(defaults),
    ]
    .spacing(12)
    .width(Length::Fill);
    if let Some(error) = &setting.component_error {
        body = body.push(text(error.clone()).color(theme::active().error));
    }
    container(body).width(Length::Fill).into()
}

fn saved_list(defaults: &ComponentDefaults) -> Element<'static, Plant> {
    if defaults.0.is_empty() {
        return super::hint("No component defaults saved yet.");
    }
    let mut list = column![].spacing(4);
    for (component, fields) in &defaults.0 {
        for (property, value) in fields {
            list = list.push(
                text(format!(
                    "{component}.{property} = {}",
                    to_lua_literal(value)
                ))
                .size(13),
            );
        }
    }
    list.into()
}

pub(crate) fn handle_edit(
    plots: &mut Plots,
    id: window::Id,
    edit: SettingComponentEdit,
) -> iced::Task<Plant> {
    let names = component_names();
    let Some(setting) = plots.windows.settings.get_mut(&id) else {
        return iced::Task::none();
    };
    match edit {
        SettingComponentEdit::Component(name) => {
            if let Some(index) = names.iter().position(|known| *known == name) {
                setting.selected_component = Some(index);
                setting.selected_property = None;
                setting.component_draft = None;
                setting.component_error = None;
            }
            iced::Task::none()
        }
        SettingComponentEdit::Property(name) => {
            let component = component_at(
                setting
                    .selected_component
                    .filter(|index| *index < names.len())
                    .unwrap_or(usize::MAX),
            );
            if properties(&component).contains(&name.as_str()) {
                setting.selected_property = properties(&component)
                    .iter()
                    .position(|known| *known == name.as_str());
                setting.component_draft = None;
                setting.component_error = None;
            }
            iced::Task::none()
        }
        SettingComponentEdit::Draft(input) => {
            setting.component_draft = Some(input);
            setting.component_error = None;
            iced::Task::none()
        }
        SettingComponentEdit::Submit => {
            let (_, _, raw) = editor_state(
                setting.selected_component,
                setting.selected_property,
                setting.component_draft.as_deref(),
                &plots.config.component_defaults,
            );
            commit(plots, id, parse_lua_value(&raw))
        }
        SettingComponentEdit::Reset => commit(plots, id, Ok(None)),
    }
}

fn commit(
    plots: &mut Plots,
    id: window::Id,
    value: Result<Option<serde_json::Value>, String>,
) -> iced::Task<Plant> {
    let names = component_names();
    let value = match value {
        Ok(value) => value,
        Err(error) => {
            if let Some(setting) = plots.windows.settings.get_mut(&id) {
                setting.component_error = Some(error);
            }
            return iced::Task::none();
        }
    };
    let Some(setting) = plots.windows.settings.get_mut(&id) else {
        return iced::Task::none();
    };
    let component = component_at(
        setting
            .selected_component
            .filter(|index| *index < names.len())
            .unwrap_or(usize::MAX),
    );
    let property = property_at(
        &component,
        setting
            .selected_property
            .filter(|index| *index < properties(&component).len())
            .unwrap_or(usize::MAX),
    );
    match apply(
        &plots.config.component_defaults,
        &component,
        &property,
        value,
    ) {
        Ok(defaults) => {
            setting.component_draft = None;
            setting.component_error = None;
            iced::Task::done(Plant::Config(ConfigEvent::Patch(
                ConfigPatch::ComponentDefaults(defaults),
            )))
        }
        Err(error) => {
            setting.component_error = Some(error);
            iced::Task::none()
        }
    }
}
