//! Lua source selection for Lua-owned context-menu presentation. The host
//! supplies the saved sources and validates every patch.
use crate::{
    config::{ComposableConfig, ComposableKind, ConfigPatch},
    shell::{ConfigEvent, Plant, state::Plots},
    ui::{
        action::{Action, ActionValue},
        node::WidgetNode,
        settings::ComponentDefaults,
    },
};
use iced::Element;
use serde_json::{Value, json};
use std::collections::HashMap;

/// The two editable composable sources, in page order.
const KINDS: [(&str, ComposableKind, &str, &str); 2] = [
    (
        "surface",
        ComposableKind::ContextMenu,
        "Menu surface",
        "Edit sizing and appearance in the Lua app.defaults table.",
    ),
    (
        "item",
        ComposableKind::ContextMenuItem,
        "Menu entries",
        "Edit padding and rounding in the Lua app.defaults table.",
    ),
];

fn rules() -> HashMap<String, ComposableKind> {
    KINDS
        .iter()
        .map(|(key, kind, ..)| ((*key).to_owned(), *kind))
        .collect()
}

fn dispatch(action: Action, rules: &HashMap<String, ComposableKind>) -> Plant {
    match (action.key.as_str(), action.value) {
        (key, Some(ActionValue::Text(source))) => match rules.get(key) {
            Some(kind) => Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableSource(
                *kind, source,
            ))),
            None => Plant::Tend,
        },
        _ => Plant::Tend,
    }
}

#[derive(Debug, Default)]
pub(super) struct Page {
    input: Option<Value>,
    node: Option<WidgetNode>,
    error: Option<String>,
}

impl Page {
    fn refresh(&mut self, config: &ComposableConfig, theme: &crate::config::ThemeConfig) {
        let mut host = serde_json::Map::new();
        for (key, kind, title, description) in KINDS {
            host.insert(key.to_owned(), json!(config.get(kind)));
            host.insert(format!("{key}_title"), json!(title));
            host.insert(format!("{key}_description"), json!(description));
        }
        let host = Value::Object(host);
        let input = json!({"host":host, "source": super::sources::revision("context_menu.lua"), "palette":crate::theme::lua_palette(theme)});
        if self.input.as_ref() == Some(&input) {
            return;
        }
        let result = super::sources::render(
            "context_menu.lua",
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
                    "Context Menu Settings: ",
                    error.to_string(),
                );
            }
        }
        self.input = Some(input);
    }

    pub(super) fn view(
        &mut self,
        plots: &Plots,
        theme: &crate::config::ThemeConfig,
    ) -> Element<'static, Plant> {
        self.refresh(&plots.config.composable, theme);
        let rules = rules();
        let sender = std::sync::Arc::new(move |action| dispatch(action, &rules));
        match self
            .node
            .as_ref()
            .ok_or_else(|| "Context Menu page unavailable".to_owned())
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

    fn config(surface: &str, item: &str) -> ComposableConfig {
        ComposableConfig {
            context_menu: surface.into(),
            context_menu_item: item.into(),
            ..Default::default()
        }
    }

    fn inputs<'a>(node: &'a WidgetNode, out: &mut Vec<(&'a str, &'a str)>) {
        match node {
            WidgetNode::TextInput {
                value, on_input, ..
            } => out.push((on_input.as_deref().unwrap_or_default(), value)),
            WidgetNode::Clickable { child, .. } | WidgetNode::Container { child, .. } => {
                inputs(child, out)
            }
            WidgetNode::Column { children, .. } | WidgetNode::Row { children, .. } => {
                for child in children {
                    inputs(child, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn both_sources_render_with_their_saved_values() {
        let theme = crate::config::ThemeConfig::default();
        let mut page = Page::default();
        let _element = page.view(
            &Plots::new(iced_wayland_subscriber::shell::channel().1),
            &theme,
        );
        // The default config still renders both editors.
        assert!(page.error.is_none(), "{:?}", page.error);
        let mut page = Page::default();
        page.refresh(&config("surface.lua", "item.lua"), &theme);
        assert!(page.error.is_none(), "{:?}", page.error);
        let mut found = Vec::new();
        inputs(page.node.as_ref().unwrap(), &mut found);
        assert!(found.contains(&("surface", "surface.lua")), "{found:?}");
        assert!(found.contains(&("item", "item.lua")), "{found:?}");
        crate::ui::build::build_node(page.node.as_ref().unwrap(), 13.0, None).unwrap();
    }

    #[test]
    fn typing_maps_to_the_matching_composable_kind() {
        let rules = rules();
        assert!(matches!(
            dispatch(Action::change("surface".into(), ActionValue::Text("a.lua".into())), &rules),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableSource(ComposableKind::ContextMenu, source))) if source == "a.lua"
        ));
        assert!(matches!(
            dispatch(Action::change("item".into(), ActionValue::Text("b.lua".into())), &rules),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableSource(ComposableKind::ContextMenuItem, source))) if source == "b.lua"
        ));
        assert!(matches!(
            dispatch(
                Action::change("missing".into(), ActionValue::Text("c.lua".into())),
                &rules
            ),
            Plant::Tend
        ));
        assert!(matches!(dispatch("surface".into(), &rules), Plant::Tend));
        assert!(matches!(
            dispatch(Action::animation_frame(), &rules),
            Plant::Tend
        ));
    }

    #[test]
    fn edited_sources_replace_the_cached_tree() {
        let theme = crate::config::ThemeConfig::default();
        let mut page = Page::default();
        page.refresh(&config("surface.lua", "item.lua"), &theme);
        page.node = Some(WidgetNode::Spinner);
        page.refresh(&config("surface.lua", "item.lua"), &theme);
        assert!(matches!(page.node, Some(WidgetNode::Spinner)));
        page.refresh(&config("other.lua", "item.lua"), &theme);
        assert!(!matches!(page.node, Some(WidgetNode::Spinner)));
    }
}
