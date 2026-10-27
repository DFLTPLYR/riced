//! Per-component settable defaults editor (`component_defaults.<component>`).
//! The page layout is Lua; parsing, validation, and persistence stay native.
//! Explicit Lua setters override these per property; Reset removes the
//! saved override so the property inherits again.
use crate::config::{ConfigPatch, parse_lua_value as parse_config_value, to_lua_literal};
use crate::shell::{ConfigEvent, Plant, SettingComponentEdit, SettingEvent, state::Plots};
use crate::ui::settings::{ComponentDefaults, properties};
use crate::ui::{action::Action, node::WidgetNode};
use iced::{Element, window};
use serde_json::{Value, json};

pub(super) fn component_names() -> Vec<String> {
    crate::ui::components::COMPONENTS
        .iter()
        // `native` is a host-filled slot, not a user-configurable component.
        .filter(|component| component.name != "plugin")
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

/// Editor selection state read from the owning Settings window.
pub(super) struct Selection<'a> {
    pub component: Option<usize>,
    pub property: Option<usize>,
    pub draft: Option<&'a str>,
    pub error: Option<&'a str>,
}

/// One cache per Settings window. Lua runs only when the editor snapshot
/// changes; decoded trees retain no Lua references.
#[derive(Debug, Default)]
pub(super) struct Page {
    input: Option<Value>,
    node: Option<WidgetNode>,
    error: Option<String>,
}

fn placeholder(property: &str) -> &'static str {
    match property {
        "padding" | "margin" => "Number or { top = 0, right = 0, bottom = 0, left = 0 }",
        "radius" => "Number or { top_left = 0, top_right = 0, bottom_right = 0, bottom_left = 0 }",
        _ => "Lua value, e.g. 0, true, \"center\", or \"#ffffff\"",
    }
}

fn saved_lines(defaults: &ComponentDefaults) -> Vec<String> {
    defaults
        .0
        .iter()
        .flat_map(|(component, fields)| {
            fields.iter().map(move |(property, value)| {
                format!("{component}.{property} = {}", to_lua_literal(value))
            })
        })
        .collect()
}

fn dispatch(action: Action, id: window::Id) -> Plant {
    let edit = match (action.key.as_str(), action.value) {
        ("component", Some(crate::ui::action::ActionValue::Text(name))) => {
            SettingComponentEdit::Component(name)
        }
        ("property", Some(crate::ui::action::ActionValue::Text(name))) => {
            SettingComponentEdit::Property(name)
        }
        ("draft", Some(crate::ui::action::ActionValue::Text(input))) => {
            SettingComponentEdit::Draft(input)
        }
        ("submit", None) => SettingComponentEdit::Submit,
        ("reset", None) => SettingComponentEdit::Reset,
        _ => return Plant::Tend,
    };
    Plant::SettingPlot(SettingEvent::ComponentsEdit(id, edit))
}

impl Page {
    fn refresh(
        &mut self,
        id: window::Id,
        selection: &Selection<'_>,
        defaults: &ComponentDefaults,
        theme: &crate::config::ThemeConfig,
    ) {
        let (component, property, draft) = editor_state(
            selection.component,
            selection.property,
            selection.draft,
            defaults,
        );
        let fields: Vec<String> = properties(&component)
            .into_iter()
            .map(str::to_owned)
            .collect();
        // JSON `null` becomes a truthy light userdata in Lua, so absent
        // fields are omitted rather than sent as null.
        let mut host = serde_json::Map::new();
        host.insert("components".into(), json!(component_names()));
        host.insert("component".into(), json!(component));
        host.insert("properties".into(), json!(fields));
        host.insert("property".into(), json!(property));
        host.insert("draft".into(), json!(draft));
        host.insert("placeholder".into(), json!(placeholder(&property)));
        host.insert("saved".into(), json!(saved_lines(defaults)));
        if let Some(error) = selection.error {
            host.insert("error".into(), json!(error));
        }
        let host = Value::Object(host);
        let input = json!({"host":host, "source": super::sources::revision("components.lua"), "palette":crate::theme::lua_palette(theme), "id":format!("{id:?}")});
        if self.input.as_ref() == Some(&input) {
            return;
        }
        // Deliberately empty defaults: the editor must not restyle itself as
        // the user edits the very defaults that apply to its own widgets.
        let result = super::sources::render(
            "components.lua",
            &host,
            theme,
            &ComponentDefaults::default(),
            &[],
        );
        match result {
            Ok(node) => {
                self.node = Some(node);
                self.error = None;
            }
            Err(error) => {
                self.node = None;
                crate::lua::error::report_once(
                    &mut self.error,
                    "Components Settings: ",
                    error.to_string(),
                );
            }
        }
        self.input = Some(input);
    }

    pub(super) fn view(
        &mut self,
        id: window::Id,
        selection: Selection<'_>,
        defaults: &ComponentDefaults,
        theme: &crate::config::ThemeConfig,
    ) -> Element<'static, Plant> {
        self.refresh(id, &selection, defaults, theme);
        let sender = std::sync::Arc::new(move |action| dispatch(action, id));
        match self
            .node
            .as_ref()
            .ok_or_else(|| "Components editor unavailable".to_owned())
            .and_then(|node| crate::ui::build::build_node(node, 13.0, Some(sender)))
        {
            Ok(element) => element,
            Err(error) => iced::widget::text(error).into(),
        }
    }
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

#[cfg(test)]
mod page_tests {
    use super::*;
    use crate::config::ThemeConfig;

    fn ids<'a>(node: &'a WidgetNode, out: &mut Vec<(&'a str, Option<&'a str>)>) {
        match node {
            WidgetNode::PickList { on_select, .. } => out.push(("pick", on_select.as_deref())),
            WidgetNode::TextInput {
                on_input,
                on_submit,
                ..
            } => {
                out.push(("input", on_input.as_deref()));
                out.push(("submit", on_submit.as_deref()));
            }
            WidgetNode::Button { action, .. } => out.push(("button", Some(action))),
            WidgetNode::Clickable { child, .. } | WidgetNode::Container { child, .. } => {
                ids(child, out)
            }
            WidgetNode::Column { children, .. } | WidgetNode::Row { children, .. } => {
                for child in children {
                    ids(child, out);
                }
            }
            _ => {}
        }
    }

    fn defaults(source: &str) -> ComponentDefaults {
        serde_json::from_str(source).unwrap()
    }

    #[test]
    fn editor_renders_pickers_input_actions_and_saved_lines() {
        let defaults = defaults(r#"{"button":{"padding":8},"column":{"align_x":"end"}}"#);
        let theme = ThemeConfig::default();
        let mut page = Page::default();
        let _element = page.view(
            window::Id::unique(),
            Selection {
                component: None,
                property: None,
                draft: None,
                error: None,
            },
            &defaults,
            &theme,
        );
        assert!(page.error.is_none(), "{:?}", page.error);
        crate::ui::build::build_node(page.node.as_ref().unwrap(), 13.0, None).unwrap();
        let mut keys = Vec::new();
        ids(page.node.as_ref().unwrap(), &mut keys);
        assert!(keys.contains(&("pick", Some("component"))));
        assert!(keys.contains(&("pick", Some("property"))));
        assert!(keys.contains(&("input", Some("draft"))));
        assert!(keys.contains(&("submit", Some("submit"))));
        assert!(keys.contains(&("button", Some("submit"))));
        assert!(keys.contains(&("button", Some("reset"))));
        let labels: Vec<String> = saved_lines(&defaults);
        assert_eq!(labels, ["button.padding = 8", "column.align_x = \"end\""]);
    }

    #[test]
    fn every_component_and_property_selection_renders() {
        let theme = ThemeConfig::default();
        let mut page = Page::default();
        for (index, name) in component_names().into_iter().enumerate() {
            let fields = properties(&name);
            assert!(!fields.is_empty(), "{name} exposes no properties");
            for property in 0..fields.len() {
                page.view(
                    window::Id::unique(),
                    Selection {
                        component: Some(index),
                        property: Some(property),
                        draft: Some("0"),
                        error: None,
                    },
                    &ComponentDefaults::default(),
                    &theme,
                );
                assert!(page.error.is_none(), "{name}: {:?}", page.error);
            }
        }
    }

    #[test]
    fn actions_map_to_component_edits_and_reject_unknown_keys() {
        let id = window::Id::unique();
        assert!(
            matches!(dispatch(Action::change("component".into(), crate::ui::action::ActionValue::Text("text".into())), id), Plant::SettingPlot(SettingEvent::ComponentsEdit(_, SettingComponentEdit::Component(name))) if name == "text")
        );
        assert!(
            matches!(dispatch(Action::change("property".into(), crate::ui::action::ActionValue::Text("size".into())), id), Plant::SettingPlot(SettingEvent::ComponentsEdit(_, SettingComponentEdit::Property(name))) if name == "size")
        );
        assert!(
            matches!(dispatch(Action::change("draft".into(), crate::ui::action::ActionValue::Text("4".into())), id), Plant::SettingPlot(SettingEvent::ComponentsEdit(_, SettingComponentEdit::Draft(input))) if input == "4")
        );
        assert!(matches!(
            dispatch("submit".into(), id),
            Plant::SettingPlot(SettingEvent::ComponentsEdit(
                _,
                SettingComponentEdit::Submit
            ))
        ));
        assert!(matches!(
            dispatch("reset".into(), id),
            Plant::SettingPlot(SettingEvent::ComponentsEdit(_, SettingComponentEdit::Reset))
        ));
        assert!(matches!(
            dispatch(
                Action::change(
                    "component".into(),
                    crate::ui::action::ActionValue::Number(1.0)
                ),
                id
            ),
            Plant::Tend
        ));
        assert!(matches!(
            dispatch(
                Action::change(
                    "submit".into(),
                    crate::ui::action::ActionValue::Text("x".into())
                ),
                id
            ),
            Plant::Tend
        ));
        assert!(matches!(dispatch("missing".into(), id), Plant::Tend));
        assert!(matches!(
            dispatch(Action::animation_frame(), id),
            Plant::Tend
        ));
    }

    #[test]
    fn drafts_errors_and_selection_changes_reach_the_editor() {
        let theme = ThemeConfig::default();
        let mut page = Page::default();
        let id = window::Id::unique();
        page.view(
            id,
            Selection {
                component: None,
                property: None,
                draft: Some("9"),
                error: Some("bad value"),
            },
            &ComponentDefaults::default(),
            &theme,
        );
        assert!(page.error.is_none(), "{:?}", page.error);
        let mut text = Vec::new();
        collect_text(page.node.as_ref().unwrap(), &mut text);
        assert!(text.contains(&"9".to_owned()), "{text:?}");
        assert!(text.contains(&"bad value".to_owned()), "{text:?}");
        // Unchanged input reuses the cached tree; a new draft replaces it.
        page.node = Some(WidgetNode::Spinner);
        page.view(
            id,
            Selection {
                component: None,
                property: None,
                draft: Some("9"),
                error: Some("bad value"),
            },
            &ComponentDefaults::default(),
            &theme,
        );
        assert!(matches!(page.node, Some(WidgetNode::Spinner)));
        page.view(
            id,
            Selection {
                component: None,
                property: None,
                draft: Some("10"),
                error: None,
            },
            &ComponentDefaults::default(),
            &theme,
        );
        assert!(!matches!(page.node, Some(WidgetNode::Spinner)));
    }

    fn collect_text(node: &WidgetNode, out: &mut Vec<String>) {
        match node {
            WidgetNode::Text { content, .. } => out.push(content.clone()),
            WidgetNode::TextInput {
                value, placeholder, ..
            } => {
                out.push(value.clone());
                out.push(placeholder.clone());
            }
            WidgetNode::Clickable { child, .. } | WidgetNode::Container { child, .. } => {
                collect_text(child, out)
            }
            WidgetNode::Column { children, .. } | WidgetNode::Row { children, .. } => {
                for child in children {
                    collect_text(child, out);
                }
            }
            _ => {}
        }
    }
}
