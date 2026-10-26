//! Lua placement editor; native code owns metadata, bounds and target identity.
use crate::{
    config::{PropValue, ThemeConfig, WidgetDefinition, WidgetPlacement},
    shell::{BarEvent, PlacementProp, Plant, TopEvent, screens::Top},
    ui::{
        action::{Action, ActionValue, Event},
        node::WidgetNode,
        settings::ComponentDefaults,
    },
};
use iced::{Element, window};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Clone)]
enum Target {
    Interval,
    Size,
    Property(String),
}

impl Target {
    fn patch(&self, value: Option<PropValue>) -> PlacementProp {
        match self {
            Self::Interval => PlacementProp::Interval(value.and_then(|v| match v {
                PropValue::Number(n) => Some(n as f32),
                _ => None,
            })),
            Self::Size => PlacementProp::Size(value.and_then(|v| match v {
                PropValue::Number(n) => Some(n as f32),
                _ => None,
            })),
            Self::Property(key) => PlacementProp::Prop {
                key: key.clone(),
                value,
            },
        }
    }
}

#[derive(Debug, Clone)]
enum Rule {
    Number(Target, f64, f64),
    Boolean(Target),
    Text(Target),
    Fixed(Target, Option<PropValue>),
}

fn patch(action: Action, rules: &HashMap<String, Rule>) -> Option<PlacementProp> {
    if !matches!(action.event, Event::Action) {
        return None;
    }
    match (rules.get(&action.key)?, action.value) {
        (Rule::Number(target, min, max), Some(ActionValue::Number(value))) if value.is_finite() => {
            Some(target.patch(Some(PropValue::Number(f64::from(value).clamp(*min, *max)))))
        }
        (Rule::Boolean(target), Some(ActionValue::Boolean(value))) => {
            Some(target.patch(Some(PropValue::Bool(value))))
        }
        (Rule::Text(target), Some(ActionValue::Text(value))) => {
            Some(target.patch(Some(PropValue::Text(value))))
        }
        (Rule::Fixed(target, value), None) => Some(target.patch(value.clone())),
        _ => None,
    }
}

fn display(value: &PropValue) -> String {
    match value {
        PropValue::Bool(v) => v.to_string(),
        PropValue::Number(v) => v.to_string(),
        PropValue::Text(v) => v.clone(),
    }
}

struct Snapshot {
    host: Value,
    rules: HashMap<String, Rule>,
}

fn snapshot(
    placement: &WidgetPlacement,
    definition: &WidgetDefinition,
    slot: usize,
    output: &str,
) -> Snapshot {
    let location = if output.is_empty() {
        format!("Slot {}", slot + 1)
    } else {
        format!("Slot {} · {output}", slot + 1)
    };
    let mut fields = Vec::new();
    let mut rules = HashMap::new();
    for (key, label, value, overridden, default, min, max, step, decimals, target) in [
        (
            "interval",
            "Refresh interval",
            placement.effective_interval(&definition.defaults),
            placement.interval.is_some(),
            format!("Widget default: {:.2}s", definition.defaults.interval),
            0.25,
            3600.0,
            0.25,
            2,
            Target::Interval,
        ),
        (
            "size",
            "Text size",
            placement.effective_size(&definition.defaults),
            placement.size.is_some(),
            format!("Widget default: {:.1}px", definition.defaults.size),
            1.0,
            128.0,
            0.5,
            1,
            Target::Size,
        ),
    ] {
        rules.insert(key.into(), Rule::Number(target.clone(), min, max));
        let reset = overridden.then(|| format!("reset:{key}"));
        if let Some(reset) = &reset {
            rules.insert(reset.clone(), Rule::Fixed(target, None));
        }
        fields.push(
            json!({"label":label, "kind":"number", "value":value, "min":min, "max":max,
            "step":step, "decimals":decimals, "action":key, "reset":reset,
            "subtitle":(!overridden).then_some(default), "choices":[]}),
        );
    }
    let mut keys: Vec<_> = definition
        .defaults
        .props
        .keys()
        .chain(placement.props.keys())
        .collect();
    keys.sort();
    keys.dedup();
    let has_properties = !keys.is_empty();
    for (index, key) in keys.into_iter().enumerate() {
        let schema = definition.schema.get(key);
        let default = definition.defaults.props.get(key);
        let overridden = placement.props.get(key);
        let value = overridden.or(default);
        let kind = schema
            .and_then(|s| s.prop_type.as_deref())
            .filter(|kind| ["boolean", "number", "string"].contains(kind))
            .unwrap_or_else(|| {
                default
                    .or(overridden)
                    .map(PropValue::kind)
                    .unwrap_or("string")
            });
        let target = Target::Property(key.clone());
        let action = format!("property:{index}");
        let reset = overridden.is_some().then(|| format!("reset:{index}"));
        if let Some(reset) = &reset {
            rules.insert(reset.clone(), Rule::Fixed(target.clone(), None));
        }
        let min = schema.and_then(|s| s.min).unwrap_or(0.0);
        let max = schema.and_then(|s| s.max).unwrap_or(1_000_000.0);
        let read_only = kind == "number"
            && (!min.is_finite()
                || !max.is_finite()
                || min >= max
                || !((max - min) as f32).is_finite()
                || !(min as f32).is_finite()
                || !(max as f32).is_finite()
                || min as f32 >= max as f32);
        let mut choices = Vec::new();
        if !read_only {
            match kind {
                "boolean" => {
                    rules.insert(action.clone(), Rule::Boolean(target));
                }
                "number" => {
                    rules.insert(action.clone(), Rule::Number(target, min, max));
                }
                _ => {
                    let options = schema.map(|s| s.choices.as_slice()).unwrap_or_default();
                    if options.is_empty() {
                        rules.insert(action.clone(), Rule::Text(target));
                    } else {
                        for (i, choice) in options.iter().enumerate() {
                            let action = format!("choice:{index}:{i}");
                            rules.insert(
                                action.clone(),
                                Rule::Fixed(target.clone(), Some(PropValue::Text(choice.clone()))),
                            );
                            choices.push(json!({"value":choice, "action":action}));
                        }
                    }
                }
            }
        }
        fields.push(json!({"property_key":key, "label":schema.and_then(|s| s.label.as_ref()).unwrap_or(key), "kind":kind,
            "value":value, "action":action, "reset":reset, "min":min, "max":max, "read_only":read_only,
            "step":1, "decimals":2, "choices":choices,
            "subtitle":schema.and_then(|s| s.description.clone()).or_else(|| default.map(|v| format!("Widget default: {}", display(v))))}));
    }
    Snapshot {
        host: json!({"title":format!("{} · {location}", placement.name), "fields":fields, "has_properties":has_properties}),
        rules,
    }
}

#[derive(Debug, Default)]
pub(super) struct Editor {
    input: Option<Value>,
    node: Option<WidgetNode>,
    rules: HashMap<String, Rule>,
    error: Option<String>,
}

pub(super) struct Context<'a> {
    pub bar: window::Id,
    pub output: &'a str,
    pub top: &'a Top,
    pub selected: Option<&'a str>,
    pub catalog: &'a [WidgetDefinition],
    pub theme: &'a ThemeConfig,
    pub defaults: &'a ComponentDefaults,
}

impl Editor {
    fn render(
        &mut self,
        snapshot: Snapshot,
        identity: &str,
        theme: &ThemeConfig,
        defaults: &ComponentDefaults,
    ) {
        let input = json!({"host":snapshot.host, "identity":identity, "source": super::sources::revision("placement.lua"), "palette":crate::theme::lua_palette(theme), "defaults":defaults});
        if self.input.as_ref() == Some(&input) {
            return;
        }
        let result = super::sources::render("placement.lua", &snapshot.host, theme, defaults, &[]);
        match result {
            Ok(node) => {
                self.node = Some(node);
                self.rules = snapshot.rules;
                self.error = None;
            }
            Err(error) => {
                // Never reuse another placement's controls after a failed render.
                self.node = None;
                self.rules.clear();
                crate::lua::error::report_once(
                    &mut self.error,
                    "Placement Settings: ",
                    error.to_string(),
                );
            }
        }
        self.input = Some(input);
    }

    pub(super) fn view(&mut self, context: Context<'_>) -> Element<'static, Plant> {
        let Context {
            bar: wid,
            output,
            top,
            selected,
            catalog,
            theme,
            defaults,
        } = context;
        let found = selected.and_then(|id| {
            top.local
                .widgets
                .iter()
                .enumerate()
                .find_map(|(slot, entries)| entries.iter().find(|p| p.id == id).map(|p| (slot, p)))
        });
        let definition = found.and_then(|(_, p)| catalog.iter().find(|d| d.name == p.name));
        let snapshot = match (selected, found, definition) {
            (None, _, _) => Snapshot {
                host: json!({"message":"Click a widget chip above to edit its size, refresh rate, and custom properties."}),
                rules: HashMap::new(),
            },
            (_, None, _) => Snapshot {
                host: json!({"message":"That widget left the bar — pick another chip to edit its settings."}),
                rules: HashMap::new(),
            },
            (_, Some((_, p)), None) => Snapshot {
                host: json!({"message":format!("{} references a widget with no definition file. Add widgets/<name>.lua, or drag its chip to the pool to remove it.", p.name)}),
                rules: HashMap::new(),
            },
            (_, Some((slot, p)), Some(d)) => snapshot(p, d, slot, output),
        };
        let pid = selected.unwrap_or_default().to_owned();
        self.render(snapshot, &format!("{wid:?}:{pid}"), theme, defaults);
        let rules = self.rules.clone();
        let sender = std::sync::Arc::new(move |action| match patch(action, &rules) {
            Some(patch) => Plant::TopPlot(TopEvent::Bar(BarEvent::WidgetProp {
                bar: wid,
                placement: pid.clone(),
                patch,
            })),
            None => Plant::Tend,
        });
        match self
            .node
            .as_ref()
            .ok_or_else(|| "Placement editor unavailable".to_owned())
            .and_then(|node| crate::ui::build::build_node(node, 13.0, Some(sender)))
        {
            Ok(element) => element,
            Err(error) => iced::widget::text(error).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PropSchema;

    fn fixture() -> (WidgetPlacement, WidgetDefinition) {
        let mut definition = WidgetDefinition::fallback("fixture", "fixture.lua".into());
        definition.defaults.props = HashMap::from([
            ("count".into(), PropValue::Number(4.0)),
            ("enabled".into(), PropValue::Bool(true)),
            ("mode".into(), PropValue::Text("auto".into())),
            ("title".into(), PropValue::Text("Hello".into())),
        ]);
        definition.schema.insert(
            "count".into(),
            PropSchema {
                min: Some(1.0),
                max: Some(10.0),
                ..Default::default()
            },
        );
        definition.schema.insert(
            "mode".into(),
            PropSchema {
                choices: vec!["auto".into(), "custom".into()],
                ..Default::default()
            },
        );
        let placement = WidgetPlacement {
            id: "one".into(),
            name: "fixture".into(),
            interval: Some(2.0),
            size: Some(14.0),
            props: HashMap::from([("count".into(), PropValue::Number(8.0))]),
            ..Default::default()
        };
        (placement, definition)
    }

    #[test]
    fn typed_lua_controls_render_inherited_and_overridden_values() {
        let (placement, definition) = fixture();
        let snapshot = snapshot(&placement, &definition, 1, "DP-1");
        assert_eq!(snapshot.host["title"], "fixture · Slot 2 · DP-1");
        assert_eq!(snapshot.host["fields"][2]["value"], 8.0);
        assert_eq!(snapshot.host["fields"][3]["value"], true);
        assert_eq!(
            snapshot.host["fields"][4]["choices"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let mut editor = Editor::default();
        editor.render(
            snapshot,
            "one",
            &ThemeConfig::default(),
            &ComponentDefaults::default(),
        );
        assert!(editor.error.is_none(), "{:?}", editor.error);
        crate::ui::build::build_node(editor.node.as_ref().unwrap(), 13.0, None).unwrap();
    }

    #[test]
    fn page_reads_only_props_the_host_provides() {
        let source = include_str!("../../../../scripts/settings/placement.lua");
        let (placement, definition) = fixture();
        for output in ["", "DP-1"] {
            let host = snapshot(&placement, &definition, 0, output).host;
            let missing = super::super::sources::missing_props(source, &host, &[]);
            assert!(missing.is_empty(), "{missing:?} for {host}");
        }
    }

    #[test]
    fn numeric_changes_clamp_and_wrong_types_or_unknown_actions_are_rejected() {
        let (placement, definition) = fixture();
        let rules = snapshot(&placement, &definition, 0, "").rules;
        assert!(matches!(
            patch(
                Action::change("interval".into(), ActionValue::Number(0.0)),
                &rules
            ),
            Some(PlacementProp::Interval(Some(0.25)))
        ));
        assert!(matches!(
            patch(
                Action::change("size".into(), ActionValue::Number(500.0)),
                &rules
            ),
            Some(PlacementProp::Size(Some(128.0)))
        ));
        assert!(
            matches!(patch(Action::change("property:0".into(), ActionValue::Number(50.0)), &rules), Some(PlacementProp::Prop { key, value:Some(PropValue::Number(10.0)) }) if key == "count")
        );
        assert!(
            patch(
                Action::change("interval".into(), ActionValue::Number(f32::NAN)),
                &rules
            )
            .is_none()
        );
        assert!(
            patch(
                Action::change("interval".into(), ActionValue::Text("5".into())),
                &rules
            )
            .is_none()
        );
        assert!(
            patch(
                Action::change("missing".into(), ActionValue::Number(5.0)),
                &rules
            )
            .is_none()
        );
        assert!(patch(Action::animation_frame(), &rules).is_none());
    }

    #[test]
    fn booleans_strings_choices_and_resets_use_original_property_keys() {
        let (mut placement, definition) = fixture();
        let rules = snapshot(&placement, &definition, 0, "").rules;
        for action in [
            Action::change("property:1".into(), ActionValue::Boolean(false)),
            Action::change("property:3".into(), ActionValue::Text("Edited".into())),
            "choice:2:1".into(),
            "reset:0".into(),
            "reset:interval".into(),
            "reset:size".into(),
        ] {
            Top::apply_prop_patch(&mut placement, &patch(action, &rules).unwrap());
        }
        assert_eq!(
            placement.props.get("enabled"),
            Some(&PropValue::Bool(false))
        );
        assert_eq!(
            placement.props.get("title"),
            Some(&PropValue::Text("Edited".into()))
        );
        assert_eq!(
            placement.props.get("mode"),
            Some(&PropValue::Text("custom".into()))
        );
        assert!(!placement.props.contains_key("count"));
        assert_eq!(
            placement.resolve_props(&definition.defaults.props)["count"],
            PropValue::Number(4.0)
        );
        assert!(placement.interval.is_none() && placement.size.is_none());
        assert!(
            patch(
                Action::change(
                    "property:2".into(),
                    ActionValue::Text("not a choice".into())
                ),
                &rules
            )
            .is_none()
        );
        assert!(
            patch("reset:1".into(), &rules).is_none(),
            "inherited properties have no reset action"
        );
    }

    #[test]
    fn invalid_schema_bounds_render_read_only_instead_of_panicking() {
        let (placement, mut definition) = fixture();
        for (min, max) in [
            (10.0, 1.0),
            (1.0, 1.0),
            (f64::NAN, 10.0),
            (0.0, f64::INFINITY),
        ] {
            let schema = definition.schema.get_mut("count").unwrap();
            schema.min = Some(min);
            schema.max = Some(max);
            let snapshot = snapshot(&placement, &definition, 0, "");
            assert!(!snapshot.rules.contains_key("property:0"));
            let mut editor = Editor::default();
            editor.render(
                snapshot,
                "one",
                &ThemeConfig::default(),
                &ComponentDefaults::default(),
            );
            assert!(editor.error.is_none(), "{:?}", editor.error);
        }
    }

    #[test]
    fn selection_changes_missing_definitions_and_removed_placements_drop_old_rules() {
        let (placement, definition) = fixture();
        let mut top = Top::new();
        top.local.widgets = vec![vec![placement]];
        let bar = window::Id::unique();
        let theme = ThemeConfig::default();
        let defaults = ComponentDefaults::default();
        let mut editor = Editor::default();
        for (selected, catalog, expected_rules) in [
            (Some("one"), std::slice::from_ref(&definition), true),
            (None, std::slice::from_ref(&definition), false),
            (Some("missing"), std::slice::from_ref(&definition), false),
            (Some("one"), &[][..], false),
        ] {
            let _element = editor.view(Context {
                bar,
                output: "DP-1",
                top: &top,
                selected,
                catalog,
                theme: &theme,
                defaults: &defaults,
            });
            assert!(editor.error.is_none(), "{:?}", editor.error);
            assert_eq!(!editor.rules.is_empty(), expected_rules);
        }
    }

    #[test]
    fn identical_labels_do_not_reuse_routes_after_property_renames() {
        let placement = WidgetPlacement {
            id: "one".into(),
            name: "fixture".into(),
            ..Default::default()
        };
        let mut editor = Editor::default();
        for key in ["old:key", "new:key"] {
            let mut definition = WidgetDefinition::fallback("fixture", "fixture.lua".into());
            definition
                .defaults
                .props
                .insert(key.into(), PropValue::Text("same value".into()));
            definition.schema.insert(
                key.into(),
                PropSchema {
                    label: Some("Same label".into()),
                    ..Default::default()
                },
            );
            editor.render(
                snapshot(&placement, &definition, 0, "DP-1"),
                "one",
                &ThemeConfig::default(),
                &ComponentDefaults::default(),
            );
            assert!(
                matches!(patch(Action::change("property:0".into(), ActionValue::Text("edited".into())), &editor.rules), Some(PlacementProp::Prop { key:actual, .. }) if actual == key)
            );
        }
    }
}
