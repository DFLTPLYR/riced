//! Lua-authored Animation page; Rust supplies snapshots and validates commands.
use crate::{
    config::{AnimationSpeed, ConfigPatch, ThemeConfig},
    shell::{ConfigEvent, Plant},
    ui::{action::Action, node::WidgetNode, settings::ComponentDefaults},
};
use iced::Element;
use serde_json::{Value, json};

/// One cache per Settings window. Lua runs only when its snapshot changes,
/// never on ordinary redraws; decoded trees retain no Lua references.
#[derive(Debug, Default)]
pub(super) struct Page {
    input: Option<Value>,
    node: Option<WidgetNode>,
    error: Option<String>,
}

fn speed_id(speed: AnimationSpeed) -> &'static str {
    match speed {
        AnimationSpeed::Fast => "fast",
        AnimationSpeed::Medium => "medium",
        AnimationSpeed::Slow => "slow",
    }
}

fn snapshot(current: AnimationSpeed) -> Value {
    json!({
        "selected": speed_id(current),
        "speeds": AnimationSpeed::all().map(|speed| json!({
            "id": speed_id(speed),
            "title": speed.title(),
            "duration_ms": speed.duration().as_millis() as u64,
            "description": match speed {
                AnimationSpeed::Fast => "Quick and responsive",
                AnimationSpeed::Medium => "Balanced everyday transitions",
                AnimationSpeed::Slow => "Relaxed and more pronounced",
            },
        })),
    })
}

fn action(action: Action) -> Plant {
    if !action.is_control_action() || action.value.is_some() {
        return Plant::Tend;
    }
    let speed = match action.key.as_str() {
        "animation:fast" => AnimationSpeed::Fast,
        "animation:medium" => AnimationSpeed::Medium,
        "animation:slow" => AnimationSpeed::Slow,
        _ => return Plant::Tend,
    };
    Plant::Config(ConfigEvent::Patch(ConfigPatch::AnimationSpeed(speed)))
}

impl Page {
    fn refresh(
        &mut self,
        current: AnimationSpeed,
        theme: &ThemeConfig,
        defaults: &ComponentDefaults,
    ) {
        let host = snapshot(current);
        let input = json!({"host":host, "source": super::sources::revision("animation.lua"), "palette":crate::theme::lua_palette(theme), "defaults":defaults});
        if self.input.as_ref() == Some(&input) {
            return;
        }
        let result = super::sources::render("animation.lua", &host, theme, defaults, &[]);
        match result {
            Ok(node) => {
                self.node = Some(node);
                self.error = None;
            }
            Err(error) => {
                crate::lua::error::report_once(
                    &mut self.error,
                    "Animation Settings: ",
                    error.to_string(),
                );
            }
        }
        self.input = Some(input);
    }

    pub(super) fn view(
        &mut self,
        current: AnimationSpeed,
        theme: &ThemeConfig,
        defaults: &ComponentDefaults,
    ) -> Element<'static, Plant> {
        self.refresh(current, theme, defaults);
        let result = self
            .node
            .as_ref()
            .ok_or_else(|| "Animation page unavailable".to_owned())
            .and_then(|node| {
                crate::ui::build::build_node(node, 13.0, Some(std::sync::Arc::new(action)))
            });
        match result {
            Ok(element) => element,
            Err(error) => iced::widget::text(error).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_speed_renders_and_cached_views_follow_config_changes() {
        let mut page = Page::default();
        let theme = ThemeConfig::default();
        let defaults = ComponentDefaults::default();
        for speed in AnimationSpeed::all() {
            let _element = page.view(speed, &theme, &defaults);
            assert!(page.error.is_none(), "{:?}", page.error);
            assert!(page.node.is_some());
            assert_eq!(
                page.input.as_ref().unwrap()["host"]["selected"],
                speed_id(speed)
            );
            // A cache hit must not decode another tree.
            page.node = Some(WidgetNode::Spinner);
            page.refresh(speed, &theme, &defaults);
            assert!(matches!(page.node, Some(WidgetNode::Spinner)));
        }
    }

    #[test]
    fn page_actions_only_allow_known_speed_patches() {
        for speed in AnimationSpeed::all() {
            assert!(
                matches!(action(format!("animation:{}", speed_id(speed)).into()),
                Plant::Config(ConfigEvent::Patch(ConfigPatch::AnimationSpeed(actual))) if actual == speed)
            );
        }
        for key in ["animation:invalid", "animation:Fast", "theme:dynamic", ""] {
            assert!(matches!(action(key.into()), Plant::Tend));
        }
        assert!(matches!(action(Action::animation_frame()), Plant::Tend));
    }

    #[test]
    fn example_project_page_honors_the_animation_contract() {
        // The example commits one project-owned page; it must keep rendering
        // with the host snapshot and emitting only known action keys.
        let source = include_str!("../../../../examples/settings/animation.lua");
        fn walk(node: &WidgetNode, buttons: &mut Vec<(String, String)>, texts: &mut Vec<String>) {
            match node {
                WidgetNode::Button { label, action, .. } => {
                    buttons.push((label.clone(), action.clone()))
                }
                WidgetNode::Text { content, .. } => texts.push(content.clone()),
                WidgetNode::Column { children, .. } | WidgetNode::Row { children, .. } => {
                    for child in children {
                        walk(child, buttons, texts);
                    }
                }
                WidgetNode::Container { child, .. } => walk(child, buttons, texts),
                _ => {}
            }
        }
        for speed in AnimationSpeed::all() {
            let node = super::super::sources::render_with(
                "animation.lua",
                Some(source.to_owned()),
                &snapshot(speed),
                &ThemeConfig::default(),
                &ComponentDefaults::default(),
                &[],
            )
            .unwrap_or_else(|error| panic!("{error}"));
            let mut buttons = Vec::new();
            let mut texts = Vec::new();
            walk(&node, &mut buttons, &mut texts);
            let expected: Vec<String> = AnimationSpeed::all()
                .into_iter()
                .map(|speed| format!("animation:{}", speed_id(speed)))
                .collect();
            let keys: Vec<String> = buttons.iter().map(|(_, key)| key.clone()).collect();
            assert_eq!(keys, expected);
            assert!(texts.iter().any(|text| text.contains("Project page")));
        }
    }

    #[test]
    fn page_reads_only_props_the_host_provides() {
        let source = include_str!("../../../../scripts/settings/animation.lua");
        for speed in AnimationSpeed::all() {
            let host = snapshot(speed);
            let missing = super::super::sources::missing_props(source, &host, &[]);
            assert!(missing.is_empty(), "{missing:?} for {host}");
        }
    }

    #[test]
    fn lua_page_emits_the_native_speed_actions_and_selected_caption() {
        fn buttons(node: &WidgetNode, out: &mut Vec<(String, String)>) {
            match node {
                WidgetNode::Button { label, action, .. } => {
                    out.push((label.clone(), action.clone()))
                }
                WidgetNode::Column { children, .. } | WidgetNode::Row { children, .. } => {
                    for child in children {
                        buttons(child, out);
                    }
                }
                WidgetNode::Container { child, .. } => buttons(child, out),
                _ => {}
            }
        }
        let mut page = Page::default();
        for speed in AnimationSpeed::all() {
            page.refresh(
                speed,
                &ThemeConfig::default(),
                &ComponentDefaults::default(),
            );
            let mut choices = Vec::new();
            buttons(page.node.as_ref().unwrap(), &mut choices);
            assert_eq!(choices.len(), 3);
            for (label, key) in &choices {
                assert_eq!(
                    label.contains("Selected"),
                    *key == format!("animation:{}", speed_id(speed))
                );
                assert!(matches!(
                    action(key.clone().into()),
                    Plant::Config(ConfigEvent::Patch(ConfigPatch::AnimationSpeed(_)))
                ));
            }
        }
    }
}
