//! Settings page sources. A project copy under `settings/` overrides the
//! bundled page; a broken copy falls back to the bundled one so a bad edit
//! cannot take Settings down. File stamps drive hot reload.
use crate::{
    config::ThemeConfig,
    lua::LuaRuntime,
    ui::{node::WidgetNode, settings::ComponentDefaults},
};
use serde_json::Value;
use std::time::SystemTime;

/// Bundled page sources, embedded so Settings works without a project copy.
const BUNDLED: &[(&str, &str)] = &[
    (
        "shell.lua",
        include_str!("../../../../scripts/settings/shell.lua"),
    ),
    (
        "animation.lua",
        include_str!("../../../../scripts/settings/animation.lua"),
    ),
    (
        "theme.lua",
        include_str!("../../../../scripts/settings/theme.lua"),
    ),
    (
        "panel_bar.lua",
        include_str!("../../../../scripts/settings/panel_bar.lua"),
    ),
    (
        "placement.lua",
        include_str!("../../../../scripts/settings/placement.lua"),
    ),
    (
        "components.lua",
        include_str!("../../../../scripts/settings/components.lua"),
    ),
    (
        "desktop.lua",
        include_str!("../../../../scripts/settings/desktop.lua"),
    ),
    (
        "context_menu.lua",
        include_str!("../../../../scripts/settings/context_menu.lua"),
    ),
    (
        "wallpaper.lua",
        include_str!("../../../../scripts/settings/wallpaper.lua"),
    ),
];

fn bundled(name: &str) -> Option<&'static str> {
    BUNDLED
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, text)| *text)
}

fn project_path(name: &str) -> std::path::PathBuf {
    crate::config::settings_dir().join(name)
}

/// Modification stamp of the project copy, or `None` while it uses the
/// bundled source.
fn stamp(name: &str) -> Option<SystemTime> {
    std::fs::metadata(project_path(name))
        .ok()
        .and_then(|metadata| metadata.modified().ok())
}

/// Opaque cache token for the current source revision. Page caches include it
/// so saving a project copy reloads the page.
pub(super) fn revision(name: &str) -> Option<String> {
    stamp(name).map(|time| format!("{time:?}"))
}

fn project_text(name: &str) -> Option<String> {
    std::fs::read_to_string(project_path(name)).ok()
}

fn attempt(
    name: &str,
    source: &str,
    host: &Value,
    theme: &ThemeConfig,
    defaults: &ComponentDefaults,
    images: &[(String, iced::widget::image::Handle)],
) -> mlua::Result<WidgetNode> {
    let mut runtime = LuaRuntime::with_components(Vec::new())?;
    runtime.publish_component_defaults(defaults)?;
    runtime.load_named(source, &format!("settings/{name}"))?;
    Ok(runtime
        .component_view_with_images(&serde_json::json!({}), host, theme, false, images)?
        .0)
}

/// Decode one Settings page, preferring the project copy.
pub(super) fn render(
    name: &str,
    host: &Value,
    theme: &ThemeConfig,
    defaults: &ComponentDefaults,
    images: &[(String, iced::widget::image::Handle)],
) -> mlua::Result<WidgetNode> {
    render_with(name, project_text(name), host, theme, defaults, images)
}

/// Decode a page from an explicit project copy (or `None` for the bundled
/// source), skipping the filesystem lookup. Tests use this to render a page
/// source directly.
pub(super) fn render_with(
    name: &str,
    project: Option<String>,
    host: &Value,
    theme: &ThemeConfig,
    defaults: &ComponentDefaults,
    images: &[(String, iced::widget::image::Handle)],
) -> mlua::Result<WidgetNode> {
    match project {
        Some(text) => match attempt(name, &text, host, theme, defaults, images) {
            Ok(node) => Ok(node),
            // Report the project error only when the bundled page fails too.
            Err(error) => match bundled(name) {
                Some(fallback) => {
                    attempt(name, fallback, host, theme, defaults, images).or(Err(error))
                }
                None => Err(error),
            },
        },
        None => {
            let fallback = bundled(name).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("no bundled Settings page {name:?}"))
            })?;
            attempt(name, fallback, host, theme, defaults, images)
        }
    }
}

#[cfg(test)]
/// Read a dotted `a.b_c` path starting at `start`, returning it and the end.
fn path_at(source: &str, start: usize) -> (String, usize) {
    let bytes = source.as_bytes();
    let mut end = start;
    loop {
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end + 1 < bytes.len()
            && bytes[end] == b'.'
            && (bytes[end + 1].is_ascii_alphanumeric() || bytes[end + 1] == b'_')
        {
            end += 1;
        } else {
            break;
        }
    }
    (source[start..end].to_string(), end)
}

#[cfg(test)]
fn paths_after(source: &str, needle: &str, skip: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(at) = source[search..].find(needle) {
        let start = search + at + skip;
        let (path, end) = path_at(source, start);
        if !path.is_empty() {
            out.push(path);
        }
        search = end.max(start);
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
/// Every `props.<path>` a page source reads, as dotted paths.
pub(super) fn prop_reads(source: &str) -> Vec<String> {
    paths_after(source, "props.", "props.".len())
}

#[cfg(test)]
/// Paths a page reads whose parent resolves in `host` but the leaf does not.
///
/// Two idioms make a read conditional, and neither is reported:
/// - a missing *parent* means the guarded branch never runs, such as
///   `props.monitor.fits` when no display is connected;
/// - `type(props.x)` marks `x` itself as optional.
///
/// `injected` names props the runtime adds outside the snapshot, such as the
/// `images` table passed to `component_view_with_images`.
pub(super) fn missing_props(source: &str, host: &Value, injected: &[&str]) -> Vec<String> {
    let optional = paths_after(source, "type(props.", "type(props.".len());
    prop_reads(source)
        .into_iter()
        .filter(|path| {
            if optional.contains(path)
                || path
                    .split('.')
                    .next()
                    .is_some_and(|first| injected.contains(&first))
            {
                return false;
            }
            let mut segments = path.split('.').peekable();
            let mut node = host;
            while let Some(segment) = segments.next() {
                match node.get(segment) {
                    Some(next) => node = next,
                    // Report only a missing leaf; a missing parent is a guard.
                    None => return segments.peek().is_none(),
                }
            }
            false
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Value {
        serde_json::json!({
            "selected": "fast",
            "speeds": [{"id": "fast", "title": "Fast", "duration_ms": 80, "description": "x"}],
        })
    }

    #[test]
    fn every_settings_page_has_a_bundled_fallback() {
        for (name, _) in BUNDLED {
            assert!(bundled(name).is_some(), "{name}");
        }
        assert!(bundled("missing.lua").is_none());
    }

    #[test]
    fn prop_reads_extract_dotted_paths_and_ignore_guarded_branches() {
        assert_eq!(
            prop_reads("props.images and props.monitor.fits or props.fields"),
            vec!["fields", "images", "monitor.fits"]
        );
        let host = serde_json::json!({"monitor": {"caption": "DP-1"}, "fields": []});
        assert!(missing_props("return props.fields", &host, &[]).is_empty());
        // A missing leaf under a present parent is a real gap.
        assert_eq!(
            missing_props("return props.monitor.fits", &host, &[]),
            vec!["monitor.fits"]
        );
        // A missing parent means the guarded branch never runs.
        assert!(missing_props("return props.image.fields", &host, &[]).is_empty());
        // Runtime-injected props are not part of the snapshot.
        assert!(missing_props("return props.images", &host, &["images"]).is_empty());
        assert_eq!(
            missing_props("return props.images", &host, &[]),
            vec!["images"]
        );
        // `type(props.x)` marks a leaf as optional.
        assert!(
            missing_props(
                "if type(props.error) == 'string' then return props.error end",
                &host,
                &[]
            )
            .is_empty()
        );
    }

    #[test]
    fn project_copies_override_and_broken_copies_fall_back() {
        let theme = ThemeConfig::default();
        let defaults = ComponentDefaults::default();
        let override_source = r#"
            local app = {}
            function app:view(props)
                return iced.text("override " .. props.selected)
            end
            return app
        "#;
        let node = render_with(
            "animation.lua",
            Some(override_source.into()),
            &host(),
            &theme,
            &defaults,
            &[],
        )
        .unwrap();
        assert!(matches!(node, WidgetNode::Text { ref content, .. } if content == "override fast"));

        // A broken project copy must not take Settings down.
        let node = render_with(
            "animation.lua",
            Some("this is not lua".into()),
            &host(),
            &theme,
            &defaults,
            &[],
        )
        .unwrap();
        assert!(!matches!(node, WidgetNode::Text { .. }));

        // A broken copy of a page that also fails bundled still reports.
        assert!(
            render_with(
                "missing.lua",
                Some("broken".into()),
                &host(),
                &theme,
                &defaults,
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn bundled_sources_are_used_without_a_project_copy() {
        let theme = ThemeConfig::default();
        let defaults = ComponentDefaults::default();
        let node = render_with("animation.lua", None, &host(), &theme, &defaults, &[]).unwrap();
        assert!(!matches!(node, WidgetNode::Text { .. }));
    }
}
