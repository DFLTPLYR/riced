//! Lua-authored Settings shell: navigation, page catalog, and page host.
//!
//! Rust validates the selected page id and realizes the page itself; every
//! layout decision (nav order, labels, descriptions, hints) lives in
//! `settings/shell.lua`. Action keys are `settings:page:<id>`.
use crate::{
    config::ThemeConfig,
    shell::{Plant, SettingEvent},
    ui::{action::Action, node::WidgetNode, settings::ComponentDefaults},
};
use serde_json::json;

use super::SettingPage;

/// One decoded shell tree per Settings window state.
#[derive(Debug, Default)]
pub(super) struct Page {
    input: Option<serde_json::Value>,
    node: Option<WidgetNode>,
    error: Option<String>,
}

/// Map a realized nav button to a validated page selection.
pub(super) fn action(action: Action, id: iced::window::Id) -> Plant {
    if !matches!(action.event, crate::ui::action::Event::Action) || action.value.is_some() {
        return Plant::Tend;
    }
    if let Some(key) = action.key.strip_prefix("settings:lua:") {
        return Plant::SettingPlot(SettingEvent::LuaPage {
            id,
            key: key.to_owned(),
        });
    }
    let Some(page) = action
        .key
        .strip_prefix("settings:page:")
        .and_then(SettingPage::from_id)
    else {
        return Plant::Tend;
    };
    Plant::SettingPlot(SettingEvent::Select(id, page))
}

impl Page {
    fn refresh(
        &mut self,
        selected: SettingPage,
        theme: &ThemeConfig,
        defaults: &ComponentDefaults,
    ) {
        let host = json!({"selected": selected.id()});
        let input = json!({
            "host": host,
            "source": super::sources::revision("shell.lua"),
            "palette": crate::theme::lua_palette(theme),
            "defaults": defaults,
        });
        if self.input.as_ref() == Some(&input) {
            return;
        }
        match super::sources::render("shell.lua", &host, theme, defaults, &[]) {
            Ok(node) => {
                self.node = Some(node);
                self.error = None;
            }
            Err(error) => {
                self.node = None;
                crate::lua::error::report_once(
                    &mut self.error,
                    "Settings shell: ",
                    error.to_string(),
                );
            }
        }
        self.input = Some(input);
    }

    /// Current shell tree, or the render error when it cannot be decoded.
    pub(super) fn node(
        &mut self,
        selected: SettingPage,
        theme: &ThemeConfig,
        defaults: &ComponentDefaults,
    ) -> Result<WidgetNode, String> {
        self.refresh(selected, theme, defaults);
        self.node.clone().ok_or_else(|| {
            self.error
                .clone()
                .unwrap_or_else(|| "Settings unavailable".into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::action::Event;

    fn nav(key: &str) -> Action {
        Action {
            key: key.into(),
            value: None,
            event: Event::Action,
        }
    }

    #[test]
    fn nav_actions_select_only_known_pages() {
        let id = iced::window::Id::unique();
        for page in [
            SettingPage::Desktop,
            SettingPage::Components,
            SettingPage::ContextMenu,
            SettingPage::Panel,
            SettingPage::Wallpaper,
            SettingPage::Theme,
            SettingPage::Animation,
        ] {
            let key = format!("settings:page:{}", page.id());
            match action(nav(&key), id) {
                Plant::SettingPlot(SettingEvent::Select(window, selected)) => {
                    assert_eq!(window, id);
                    assert_eq!(selected, page);
                }
                other => panic!("{key} mapped to {other:?}"),
            }
        }
        assert!(matches!(
            action(nav("settings:page:missing"), id),
            Plant::Tend
        ));
        assert!(matches!(action(nav("elsewhere"), id), Plant::Tend));
        assert!(matches!(
            action(
                Action {
                    value: Some(crate::ui::action::ActionValue::Boolean(true)),
                    ..nav("settings:page:theme")
                },
                id
            ),
            Plant::Tend
        ));
    }

    #[test]
    fn lua_page_actions_route_to_the_persistent_page() {
        let id = iced::window::Id::unique();
        match action(
            Action {
                key: "settings:lua:add:top".into(),
                value: None,
                event: Event::Action,
            },
            id,
        ) {
            Plant::SettingPlot(crate::shell::SettingEvent::LuaPage { id: got, key }) => {
                assert_eq!(got, id);
                assert_eq!(key, "add:top");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn bundled_panel_page_is_available() {
        assert!(super::super::sources::page_source("panel.lua").is_some());
    }

    #[test]
    fn bundled_shell_exposes_exactly_one_page_slot() {
        let node = super::super::sources::render(
            "shell.lua",
            &json!({"selected": "theme"}),
            &ThemeConfig::default(),
            &ComponentDefaults::default(),
            &[],
        )
        .unwrap();
        let mut slots = Vec::new();
        crate::ui::build::build_node_with_native(&node, 13.0, None, &mut |slot, _| {
            slots.push(slot.to_string());
            Ok(iced::widget::text("page").into())
        })
        .unwrap();
        assert_eq!(slots, vec!["settings-page".to_owned()]);
    }
}
