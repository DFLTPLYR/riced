//! Lua Theme page. Native work is restricted to snapshots, images and actions.
use crate::{
    config::{ConfigPatch, ThemeConfig},
    shell::{ConfigEvent, Plant},
    theme,
    theme::lua::hex,
    ui::{action::Action, node::WidgetNode, settings::ComponentDefaults},
};
use iced::{Element, widget::image::Handle};
use serde_json::{Value, json};
use std::collections::HashMap;

const PREVIEW_APPS: [(&str, &[&str]); 6] = [
    (
        "Firefox",
        &["firefox", "org.mozilla.firefox", "web-browser"],
    ),
    (
        "Files",
        &["org.gnome.Nautilus", "system-file-manager", "folder"],
    ),
    (
        "Terminal",
        &["org.gnome.Terminal", "utilities-terminal", "terminal"],
    ),
    (
        "Editor",
        &["code", "visual-studio-code", "accessories-text-editor"],
    ),
    (
        "VLC",
        &["vlc", "org.videolan.VLC", "multimedia-video-player"],
    ),
    (
        "Steam",
        &["steam", "com.valvesoftware.Steam", "applications-games"],
    ),
];

pub(super) fn load_icon_previews(current: String) -> super::IconPreviews {
    let mut themes = crate::icons::available_themes();
    if !themes.contains(&current) {
        themes.push(current);
    }
    let mut resolver = crate::icons::Resolver::for_previews();
    let mut decoded = HashMap::new();
    themes
        .into_iter()
        .map(|theme| {
            let icons = PREVIEW_APPS
                .iter()
                .map(|(_, names)| {
                    names.iter().find_map(|name| {
                        let path = resolver.resolve_in_theme(&theme, name, 48, &[])?;
                        decoded
                            .entry(path.clone())
                            .or_insert_with(|| {
                                crate::notify::images::image_file(&path.to_string_lossy())
                            })
                            .clone()
                    })
                })
                .collect();
            (theme, icons)
        })
        .collect()
}

#[derive(Debug, Default)]
pub(super) struct Page {
    input: Option<Value>,
    node: Option<WidgetNode>,
    actions: HashMap<String, ConfigPatch>,
    error: Option<String>,
}

struct Snapshot {
    host: Value,
    images: Vec<(String, Handle)>,
    actions: HashMap<String, ConfigPatch>,
}

fn snapshot(
    current: &ThemeConfig,
    previews: Option<&Result<super::IconPreviews, String>>,
) -> Snapshot {
    let mut actions = HashMap::from([
        ("mode:dark".into(), ConfigPatch::ThemeDarkmode(true)),
        ("mode:light".into(), ConfigPatch::ThemeDarkmode(false)),
    ]);
    let palettes: Vec<_> = theme::available_themes().into_iter().map(|name| {
        let preview = theme::preview(&name, current.darkmode);
        actions.insert(format!("palette:{name}"), ConfigPatch::ThemeName(name.clone()));
        json!({"name":name, "surface":hex(preview.surface), "on_surface":hex(preview.on_surface),
            "primary":hex(preview.primary), "secondary":hex(preview.secondary),
            "tertiary":hex(preview.tertiary), "outline":hex(preview.outline)})
    }).collect();
    let mut images = Vec::new();
    let mut icons = Vec::new();
    let (status, message) = match previews {
        None => ("loading", "Loading icon previews…".to_owned()),
        Some(Err(error)) => ("error", format!("Could not load icon previews: {error}")),
        Some(Ok(previews)) => {
            for (index, (name, handles)) in previews.iter().enumerate() {
                actions.insert(format!("icon:{name}"), ConfigPatch::IconTheme(name.clone()));
                let samples: Vec<_> = PREVIEW_APPS
                    .iter()
                    .enumerate()
                    .map(|(i, (label, _))| {
                        let key = handles.get(i).and_then(Option::as_ref).map(|handle| {
                            let key = format!("{index}:{i}");
                            images.push((key.clone(), handle.clone()));
                            key
                        });
                        json!({"label":label, "image":key})
                    })
                    .collect();
                icons.push(json!({"name":name, "samples":samples}));
            }
            ("ready", String::new())
        }
    };
    Snapshot {
        host: json!({"name":current.name, "darkmode":current.darkmode, "icon_theme":current.icon_theme,
            "palettes":palettes, "icon_themes":icons, "preview_status":status, "preview_message":message}),
        images,
        actions,
    }
}

fn action(action: Action, allowed: &HashMap<String, ConfigPatch>) -> Plant {
    if !matches!(action.event, crate::ui::action::Event::Action) || action.value.is_some() {
        return Plant::Tend;
    }
    allowed
        .get(&action.key)
        .map(|patch| Plant::Config(ConfigEvent::Patch(patch.clone())))
        .unwrap_or(Plant::Tend)
}

impl Page {
    fn refresh(
        &mut self,
        current: &ThemeConfig,
        previews: Option<&Result<super::IconPreviews, String>>,
        defaults: &ComponentDefaults,
    ) {
        let snapshot = snapshot(current, previews);
        let image_ids: Vec<_> = snapshot
            .images
            .iter()
            .map(|(key, image)| (key, format!("{:?}", image.id())))
            .collect();
        let input = json!({"host":snapshot.host, "image_ids":image_ids, "source": super::sources::revision("theme.lua"), "palette":theme::lua_palette(current), "defaults":defaults});
        if self.input.as_ref() == Some(&input) {
            return;
        }
        let result = super::sources::render(
            "theme.lua",
            &snapshot.host,
            current,
            defaults,
            &snapshot.images,
        );
        match result {
            Ok(node) => {
                self.node = Some(node);
                self.actions = snapshot.actions;
                self.error = None;
            }
            Err(error) => crate::lua::error::report_once(
                &mut self.error,
                "Theme Settings: ",
                error.to_string(),
            ),
        }
        self.input = Some(input);
    }

    pub(super) fn view(
        &mut self,
        current: &ThemeConfig,
        previews: Option<&Result<super::IconPreviews, String>>,
        defaults: &ComponentDefaults,
    ) -> Element<'static, Plant> {
        self.refresh(current, previews, defaults);
        let allowed = self.actions.clone();
        let result = self
            .node
            .as_ref()
            .ok_or_else(|| "Theme page unavailable".to_owned())
            .and_then(|node| {
                crate::ui::build::build_node(
                    node,
                    13.0,
                    Some(std::sync::Arc::new(move |message| {
                        action(message, &allowed)
                    })),
                )
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

    fn inspect<'a>(
        node: &'a WidgetNode,
        images: &mut usize,
        keys: &mut Vec<&'a str>,
        labels: &mut Vec<&'a str>,
    ) {
        match node {
            WidgetNode::Raster { .. } => *images += 1,
            WidgetNode::Text { content, .. } => labels.push(content),
            WidgetNode::Button { action, .. } => keys.push(action),
            WidgetNode::Clickable { action, child } => {
                keys.push(action);
                inspect(child, images, keys, labels);
            }
            WidgetNode::Container { child, .. } | WidgetNode::Aligned { child, .. } => {
                inspect(child, images, keys, labels)
            }
            WidgetNode::Column { children, .. }
            | WidgetNode::Row { children, .. }
            | WidgetNode::Masonry { children, .. } => {
                for child in children {
                    inspect(child, images, keys, labels);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn page_reads_only_props_the_host_provides() {
        let source = include_str!("../../../../scripts/settings/theme.lua");
        let previews = Ok(vec![(
            "fixture".into(),
            vec![Some(Handle::from_rgba(1, 1, vec![255; 4]))],
        )]);
        let failed = Err("fixture failure".into());
        for darkmode in [true, false] {
            let theme = ThemeConfig {
                darkmode,
                ..Default::default()
            };
            for status in [None, Some(&failed), Some(&previews)] {
                let host = snapshot(&theme, status).host;
                // Icon samples carry their image token through `props.images`.
                let missing = super::super::sources::missing_props(source, &host, &["images"]);
                assert!(missing.is_empty(), "{missing:?} for {host}");
            }
        }
    }

    #[test]
    fn loading_errors_and_ready_icons_render_in_both_modes() {
        let previews = Ok(vec![(
            "fixture".into(),
            vec![Some(Handle::from_rgba(1, 1, vec![255; 4]))],
        )]);
        let failed = Err("fixture failure".into());
        let mut page = Page::default();
        for darkmode in [true, false] {
            let theme = ThemeConfig {
                darkmode,
                ..Default::default()
            };
            for status in [None, Some(&failed), Some(&previews)] {
                let _element = page.view(&theme, status, &ComponentDefaults::default());
                assert!(page.error.is_none(), "{:?}", page.error);
                let mut images = 0;
                let mut keys = Vec::new();
                let mut labels = Vec::new();
                inspect(
                    page.node.as_ref().unwrap(),
                    &mut images,
                    &mut keys,
                    &mut labels,
                );
                assert!(keys.contains(&"mode:dark") && keys.contains(&"mode:light"));
                assert!(keys.iter().any(|key| key.starts_with("palette:")));
                if status.is_some_and(Result::is_ok) {
                    assert_eq!(images, 1);
                    assert!(keys.contains(&"icon:fixture"));
                    for label in PREVIEW_APPS.map(|(label, _)| label) {
                        assert!(labels.contains(&label));
                    }
                } else {
                    assert_eq!(images, 0);
                    assert!(labels.iter().any(
                        |label| label.contains("Loading") || label.contains("fixture failure")
                    ));
                }
            }
        }
    }

    #[test]
    fn theme_actions_are_allowlisted_and_keep_names_opaque() {
        let allowed = HashMap::from([
            ("mode:dark".into(), ConfigPatch::ThemeDarkmode(true)),
            (
                "palette:gruvbox".into(),
                ConfigPatch::ThemeName("gruvbox".into()),
            ),
            (
                "icon:name:with:colons".into(),
                ConfigPatch::IconTheme("name:with:colons".into()),
            ),
        ]);
        assert!(matches!(
            action("mode:dark".into(), &allowed),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::ThemeDarkmode(true)))
        ));
        assert!(
            matches!(action("palette:gruvbox".into(), &allowed), Plant::Config(ConfigEvent::Patch(ConfigPatch::ThemeName(name))) if name == "gruvbox")
        );
        assert!(
            matches!(action("icon:name:with:colons".into(), &allowed), Plant::Config(ConfigEvent::Patch(ConfigPatch::IconTheme(name))) if name == "name:with:colons")
        );
        for key in ["palette:missing", "icon:missing", "mode:invalid"] {
            assert!(matches!(action(key.into(), &allowed), Plant::Tend));
        }
        assert!(matches!(
            action(Action::animation_frame(), &allowed),
            Plant::Tend
        ));
    }

    #[test]
    fn cache_reuses_nodes_but_replaces_changed_image_handles() {
        let theme = ThemeConfig::default();
        let defaults = ComponentDefaults::default();
        let first = Ok(vec![(
            "fixture".into(),
            vec![Some(Handle::from_rgba(1, 1, vec![255; 4]))],
        )]);
        let second = Ok(vec![(
            "fixture".into(),
            vec![Some(Handle::from_rgba(1, 1, vec![0; 4]))],
        )]);
        let mut page = Page::default();
        page.refresh(&theme, Some(&first), &defaults);
        assert!(page.error.is_none(), "{:?}", page.error);
        page.node = Some(WidgetNode::Spinner);
        page.refresh(&theme, Some(&first), &defaults);
        assert!(matches!(page.node, Some(WidgetNode::Spinner)));
        page.refresh(&theme, Some(&second), &defaults);
        assert!(!matches!(page.node, Some(WidgetNode::Spinner)));
        assert!(page.error.is_none(), "{:?}", page.error);
    }
}
