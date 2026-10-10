//! App-backed shell chrome. Each host instance owns a bounded runtime;
//! source/library reloads replace it only after a successful view decode.
use super::LuaRuntime;
use crate::{
    config::{SourceComposable, ThemeConfig},
    ui::node::WidgetNode,
};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, time::SystemTime};

#[derive(Debug, Clone)]
pub(crate) struct Rendered {
    pub node: WidgetNode,
    pub props: Value,
}

#[derive(Debug, Clone, PartialEq)]
struct Revision {
    path: PathBuf,
    modified: Option<SystemTime>,
    length: Option<u64>,
    library: Option<SystemTime>,
    overrides: Value,
    dependencies: Value,
}

#[derive(Default)]
struct Instance {
    attempted: Option<Revision>,
    runtime: Option<LuaRuntime>,
    input: Option<Value>,
    rendered: Option<Rendered>,
    error: Option<String>,
}

#[derive(Default)]
pub(crate) struct ComposableRuntime {
    instances: HashMap<String, Instance>,
}

impl std::fmt::Debug for ComposableRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposableRuntime")
            .field("instances", &self.instances.len())
            .finish()
    }
}

pub(crate) fn source_path(src: &str) -> PathBuf {
    let path = PathBuf::from(src);
    if path.is_absolute() {
        path
    } else {
        crate::config::components_dir().join(path)
    }
}

impl ComposableRuntime {
    pub(crate) fn render(
        &mut self,
        instance: &str,
        config: &SourceComposable,
        host: Value,
        theme: &ThemeConfig,
        library: Option<SystemTime>,
    ) -> Option<Rendered> {
        self.render_checked(instance, config, host, theme, library, |_| Ok(()))
    }

    pub(crate) fn render_checked(
        &mut self,
        instance: &str,
        config: &SourceComposable,
        host: Value,
        theme: &ThemeConfig,
        library: Option<SystemTime>,
        validate: impl Fn(&Rendered) -> Result<(), String>,
    ) -> Option<Rendered> {
        let path = source_path(&config.src);
        let metadata = std::fs::metadata(&path).ok();
        let overrides = serde_json::to_value(&config.props).ok()?;
        let revision = Revision {
            path: path.clone(),
            modified: metadata.as_ref().and_then(|m| m.modified().ok()),
            length: metadata.as_ref().map(|m| m.len()),
            library,
            overrides: overrides.clone(),
            dependencies: serde_json::json!({
                "item_component": host.get("item_component"),
                "item_revision": host.get("_item_revision"),
            }),
        };
        let input = serde_json::json!({
            "overrides": overrides, "host": host,
            "theme": crate::theme::lua_palette(theme),
        });
        let state = self.instances.entry(instance.into()).or_default();
        let changed = state.attempted.as_ref() != Some(&revision);
        if !changed && state.input.as_ref() == Some(&input) {
            return state.rendered.clone();
        }
        if changed {
            state.attempted = Some(revision);
            let candidate = (|| -> mlua::Result<_> {
                let source = std::fs::read_to_string(&path).map_err(mlua::Error::external)?;
                let mut runtime = LuaRuntime::with_components(crate::config::component_files())?;
                runtime.publish_theme(theme)?;
                runtime.load_named(&source, &path.display().to_string())?;
                let (node, props) = runtime.component_view(&overrides, &host, theme)?;
                let rendered = Rendered { node, props };
                validate(&rendered).map_err(mlua::Error::RuntimeError)?;
                Ok((runtime, rendered))
            })();
            match candidate {
                Ok((runtime, rendered)) => {
                    state.runtime = Some(runtime);
                    state.input = Some(input);
                    state.rendered = Some(rendered.clone());
                    state.error = None;
                    return Some(rendered);
                }
                Err(error) => report_error(state, &path, error.to_string()),
            }
        }
        // A rejected replacement must not freeze a drag at its old size:
        // invoke the old app with the new host geometry and current theme.
        if let Some(runtime) = &mut state.runtime {
            match runtime.component_view(&overrides, &host, theme) {
                Ok((node, props)) => {
                    let rendered = Rendered { node, props };
                    match validate(&rendered) {
                        Ok(()) => state.rendered = Some(rendered),
                        Err(error) => report_error(state, &path, error),
                    }
                }
                Err(error) => report_error(state, &path, error.to_string()),
            }
        }
        state.input = Some(input);
        state.rendered.clone()
    }
}

fn report_error(state: &mut Instance, path: &std::path::Path, error: String) {
    super::error::report_once(
        &mut state.error,
        &format!("composable {}: keeping last working view: ", path.display()),
        error,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PropValue;
    use serde_json::json;

    #[test]
    fn cache_merges_props_and_failed_reload_keeps_live_geometry() {
        let dir = std::env::temp_dir().join(format!("riced-chrome-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.lua");
        std::fs::write(
            &path,
            r#"
            local app = {defaults={label="default", width=10}, calls=0}
            function app:view(props)
                assert(self.props == props)
                self.calls = self.calls + 1
                return ui.container(ui.text(props.label .. self.calls)):width(props.width)
            end
            return app
        "#,
        )
        .unwrap();
        let mut config = SourceComposable {
            src: path.to_string_lossy().into(),
            ..Default::default()
        };
        config
            .props
            .insert("label".into(), PropValue::Text("custom".into()));
        config.props.insert("width".into(), PropValue::Number(30.0));
        let mut engine = ComposableRuntime::default();
        let theme = ThemeConfig::default();
        let first = engine
            .render("one", &config, json!({"width": 40}), &theme, None)
            .unwrap();
        assert_eq!(first.props["width"], 40);
        assert_eq!(first.props["label"], "custom");
        let cached = engine
            .render("one", &config, json!({"width": 40}), &theme, None)
            .unwrap();
        assert_eq!(first.node, cached.node);
        let other = engine
            .render("two", &config, json!({"width": 40}), &theme, None)
            .unwrap();
        assert_eq!(first.node, other.node);

        std::fs::write(&path, "this is broken Lua").unwrap();
        let retained = engine
            .render("one", &config, json!({"width": 80}), &theme, None)
            .unwrap();
        assert_eq!(retained.props["width"], 80);
        match retained.node {
            WidgetNode::Container {
                width: crate::ui::node::NodeLength::Fixed(80.0),
                child,
                ..
            } => {
                assert!(matches!(*child, WidgetNode::Text {content, ..} if content == "custom2"));
            }
            other => panic!("unexpected {other:?}"),
        }
        std::fs::write(
            &path,
            "return {defaults={}, view=function(self, props) return ui.text('recovered') end}",
        )
        .unwrap();
        let repaired = engine
            .render("one", &config, json!({"width": 80}), &theme, None)
            .unwrap();
        assert!(matches!(repaired.node, WidgetNode::Text {content, ..} if content == "recovered"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nested_item_source_recovers_and_invalid_geometry_keeps_last_good() {
        let dir =
            std::env::temp_dir().join(format!("riced-context-dependencies-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let frame_path = dir.join("frame.lua");
        let item_path = dir.join("item.lua");
        let _ = std::fs::remove_file(&item_path);
        std::fs::write(&frame_path, crate::config::SEED_CONTEXT_MENU).unwrap();
        let mut frame = SourceComposable::new(&frame_path.to_string_lossy());
        frame
            .props
            .insert("auto_sizing".into(), PropValue::Bool(false));
        let item = SourceComposable::new(&item_path.to_string_lossy());
        let host = || crate::shell::screens::background::context_menu_host(&item);
        let validate = |rendered: &Rendered| match &rendered.node {
            WidgetNode::Container {
                width: crate::ui::node::NodeLength::Fixed(w),
                height: crate::ui::node::NodeLength::Fixed(h),
                ..
            } if *w > 0.0 && *h > 0.0 => Ok(()),
            WidgetNode::Container {
                width: crate::ui::node::NodeLength::Fixed(w),
                height: crate::ui::node::NodeLength::Shrink,
                ..
            } if *w > 0.0 => Ok(()),
            _ => Err("fixed or automatic geometry required".into()),
        };
        let theme = ThemeConfig::default();
        let mut engine = ComposableRuntime::default();
        assert!(
            engine
                .render_checked("menu", &frame, host(), &theme, None, validate)
                .is_none()
        );
        std::fs::write(&item_path, crate::config::SEED_CONTEXT_MENU_ITEM).unwrap();
        let initial = engine
            .render_checked("menu", &frame, host(), &theme, None, validate)
            .unwrap();
        assert_eq!(initial.props["width"], 178);
        // The item is outside the shared-library directory; its dependency
        // revision must recover the failed app without a parent-file edit.
        std::fs::write(
            &item_path,
            "return {view=function(self,p) error('bad item reload') end}",
        )
        .unwrap();
        let retained = engine
            .render_checked("menu", &frame, host(), &theme, None, validate)
            .unwrap();
        assert_eq!(retained.node, initial.node);
        std::fs::write(&item_path, crate::config::SEED_CONTEXT_MENU_ITEM).unwrap();
        engine
            .render_checked("menu", &frame, host(), &theme, None, validate)
            .unwrap();
        std::fs::write(&frame_path, "return {view=function(self,p) return ui.container(ui.text('invalid')):width(p.width):height('fill') end}").unwrap();
        frame.props.insert("width".into(), PropValue::Number(220.0));
        let retained = engine
            .render_checked("menu", &frame, host(), &theme, None, validate)
            .unwrap();
        assert!(matches!(
            retained.node,
            WidgetNode::Container {
                width: crate::ui::node::NodeLength::Fixed(220.0),
                height: crate::ui::node::NodeLength::Fixed(92.0),
                ..
            }
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn selection_seed_uses_live_theme_and_declares_appearance_defaults() {
        let mut runtime = LuaRuntime::new().unwrap();
        let mut theme = ThemeConfig::default();
        runtime.publish_theme(&theme).unwrap();
        runtime.load(crate::config::SEED_SELECTION_RECT).unwrap();
        let host = json!({"width": 140, "height": 60, "opacity": 0.4});
        let (node, props) = runtime.component_view(&json!({}), &host, &theme).unwrap();
        assert_eq!(props["radius"], 0);
        assert_eq!(props["border_width"], 1);
        assert_eq!(props["fill_alpha"], 0.5);
        let primary = crate::theme::lua_palette(&theme)
            .into_iter()
            .find(|(key, _)| *key == "primary")
            .unwrap()
            .1;
        match &node {
            WidgetNode::Container {
                background: Some(background),
                border: Some(border),
                ..
            } => {
                assert!((background.a - 128.0 / 255.0).abs() < 0.001);
                assert_eq!(*border, crate::theme::parse_hex(&primary).unwrap());
            }
            other => panic!("unexpected {other:?}"),
        }
        crate::ui::build::build_node_opacity(&node, 13.0, None, 0.4).unwrap();
        theme.darkmode = !theme.darkmode;
        let (updated, _) = runtime.component_view(&json!({}), &host, &theme).unwrap();
        assert_ne!(node, updated);
    }

    #[test]
    fn runaway_component_is_bounded_and_missing_file_can_recover() {
        let dir = std::env::temp_dir().join(format!("riced-chrome-budget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.lua");
        let _ = std::fs::remove_file(&path);
        let config = SourceComposable {
            src: path.to_string_lossy().into(),
            ..Default::default()
        };
        let mut engine = ComposableRuntime::default();
        let theme = ThemeConfig::default();
        assert!(
            engine
                .render("one", &config, json!({}), &theme, None)
                .is_none()
        );
        std::fs::write(
            &path,
            "return {view=function(self, props) while true do end end}",
        )
        .unwrap();
        assert!(
            engine
                .render("one", &config, json!({}), &theme, None)
                .is_none()
        );
        std::fs::write(&path, "local app={defaults={color=theme.primary}}; function app:view(props) return ui.text('ready'):color(props.color) end; return app").unwrap();
        assert!(
            engine
                .render("one", &config, json!({}), &theme, None)
                .is_some()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
