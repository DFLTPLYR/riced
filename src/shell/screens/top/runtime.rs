//! Placement execution and animation adapter with explicit domain borrows.
use super::animation::ListContext;
use super::{TopLocal, button_children};
use crate::lua::{metadata::load_widget_meta, transitions::parse_transitions};
use crate::{
    config::WidgetDefinition,
    lua::widgets::{EntryContext, ResolvedWidget, WidgetState, load_widget_script, new_widget_lua},
    services::ServiceCtx,
    ui::{
        decode::parse_node,
        listview::{Axis, ListView},
        node::WidgetNode,
    },
};
use iced::window;
use mlua::Value;
use std::{collections::HashMap, path::PathBuf};

pub(super) struct RuntimeContext<'a> {
    pub placements: &'a mut WidgetState,
    pub catalog: &'a mut [WidgetDefinition],
    pub animation: ListContext<'a>,
    pub services: ServiceCtx<'a>,
    pub outputs: HashMap<window::Id, String>,
    pub paths: Vec<(String, PathBuf)>,
}

impl RuntimeContext<'_> {
    pub fn sync_revision(&mut self, resolved: &ResolvedWidget) -> bool {
        let ids = self
            .paths
            .iter()
            .filter(|(_, path)| *path == resolved.path)
            .map(|(id, _)| id.as_str());
        if !self.placements.sync_revision(&resolved.path, ids) {
            return false;
        }
        for definition in self
            .catalog
            .iter_mut()
            .filter(|definition| definition.file == resolved.path)
        {
            if let Ok(source) = std::fs::read_to_string(&resolved.path)
                && let Ok(lua) = new_widget_lua()
                && load_widget_script(&lua, &definition.name, &source).is_ok()
            {
                let (defaults, schema) = load_widget_meta(&lua, &definition.name);
                definition.defaults = defaults;
                definition.schema = schema;
            }
        }
        true
    }

    pub fn render(&mut self, resolved: &ResolvedWidget, bar: window::Id) -> Result<Value, String> {
        self.sync_revision(resolved);
        self.placements.render(
            resolved,
            &EntryContext {
                services: ServiceCtx {
                    sys: self.services.sys,
                    gpu: self.services.gpu,
                    theme: self.services.theme,
                    outputs: self.services.outputs,
                    notifications: self.services.notifications,
                    toplevels: self.services.toplevels,
                    workspaces: self.services.workspaces,
                },
                output: self
                    .outputs
                    .get(&bar)
                    .map(String::as_str)
                    .unwrap_or_default(),
            },
        )
    }

    fn note_error(&mut self, label: &str, error: String) {
        crate::lua::error::report_keyed(
            &mut self.placements.errors,
            label,
            &format!("riced: widget {label:?}: "),
            error,
        );
    }

    pub fn ingest(
        &mut self,
        resolved: &ResolvedWidget,
        bar: window::Id,
        result: Result<Value, String>,
    ) -> bool {
        let key = (bar, resolved.id.clone());
        let scope = format!("{bar:?}/{}", resolved.id);
        let label = resolved.label();
        match result {
            Ok(Value::Table(table)) => {
                self.placements.errors.remove(&resolved.id);
                let had_text = self.placements.outputs.remove(&key).is_some();
                match parse_node(&Value::Table(table)) {
                    Ok(node) => {
                        let changed = had_text || self.placements.trees.get(&key) != Some(&node);
                        if changed
                            && let Err(error) = self.animation.synchronize(
                                &scope,
                                self.placements.trees.get(&key),
                                &node,
                            )
                        {
                            self.note_error(&label, error);
                            return had_text;
                        }
                        let list = self
                            .animation
                            .lists
                            .entry(scope.clone())
                            .or_insert_with(|| ListView::new(TopLocal::ROW_PITCH));
                        if let Some(lua) = self.placements.instances.get(&resolved.id) {
                            let defaults: ListView<(String, String), WidgetNode> =
                                ListView::new(TopLocal::ROW_PITCH);
                            match parse_transitions(
                                lua,
                                &defaults.enter_spec(),
                                &defaults.exit_spec(),
                                &defaults.displaced_spec(),
                            ) {
                                Ok((enter, exit, displaced, custom)) => {
                                    list.set_transitions(enter, exit, displaced, custom)
                                }
                                Err(error) => {
                                    list.clear_all(self.animation.motion);
                                    self.note_error(&label, error);
                                    self.animation.lists.remove(&scope);
                                    self.placements.trees.insert(key, node);
                                    return changed;
                                }
                            }
                        }
                        if !changed {
                            return false;
                        }
                        // Move the previous tree instead of cloning it for diffing.
                        let old = self.placements.trees.remove(&key);
                        let old_is_list = matches!(
                            old,
                            Some(WidgetNode::Row { .. } | WidgetNode::Column { .. })
                        );
                        let new_is_list =
                            matches!(node, WidgetNode::Row { .. } | WidgetNode::Column { .. });
                        if !old_is_list || !new_is_list {
                            if let Some(list) = self.animation.lists.get_mut(&scope) {
                                list.clear_scope(self.animation.motion, move |(owner, _)| {
                                    *owner == scope
                                });
                            }
                        } else {
                            let old_kids = button_children(old.as_ref().expect("list checked"));
                            let new_kids = button_children(&node);
                            let old_keys: Vec<_> = old_kids
                                .iter()
                                .map(|(id, _)| (scope.clone(), id.clone()))
                                .collect();
                            let keys: Vec<_> = new_kids
                                .iter()
                                .map(|(id, _)| (scope.clone(), id.clone()))
                                .collect();
                            let by_key: HashMap<&str, &WidgetNode> = old_kids
                                .iter()
                                .map(|(id, node)| (id.as_str(), node))
                                .collect();
                            let removed: Vec<_> = old_keys
                                .iter()
                                .enumerate()
                                .filter(|(_, key)| !keys.contains(key))
                                .filter_map(|(index, key)| {
                                    by_key
                                        .get(key.1.as_str())
                                        .map(|node| (index, key.clone(), (*node).clone()))
                                })
                                .collect();
                            let axis = if matches!(node, WidgetNode::Row { .. }) {
                                Axis::Horizontal
                            } else {
                                Axis::Vertical
                            };
                            if let Some(list) = self.animation.lists.get_mut(&scope) {
                                list.update(
                                    self.animation.motion,
                                    self.animation.duration,
                                    axis,
                                    &old_keys,
                                    &keys,
                                    &removed,
                                );
                            }
                        }
                        self.placements.trees.insert(key, node);
                        true
                    }
                    Err(error) => {
                        let changed = self.placements.trees.remove(&key).is_some() || had_text;
                        self.note_error(&label, error);
                        changed
                    }
                }
            }
            Ok(value) => {
                self.placements.errors.remove(&resolved.id);
                let had_tree = self.placements.trees.remove(&key).is_some();
                match crate::lua::value::coerce_text(value, "render()") {
                    Ok(text) => {
                        let changed = had_tree || self.placements.outputs.get(&key) != Some(&text);
                        if changed {
                            self.placements.outputs.insert(key, text);
                        }
                        changed
                    }
                    Err(error) => {
                        self.note_error(&label, error);
                        had_tree
                    }
                }
            }
            Err(error) => {
                self.note_error(&label, error);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_views_keep_owned_trees_and_still_execute_lua_entries() {
        let mut placements = WidgetState::default();
        let resolved = ResolvedWidget {
            id: "unchanged".into(),
            name: "unchanged".into(),
            path: "/not-real-unchanged.lua".into(),
            interval: 1.0,
            size: 13.0,
            props: HashMap::new(),
        };
        let lua = new_widget_lua().unwrap();
        load_widget_script(&lua,"unchanged",r#"return {
            calls=0, transition_calls=0,
            view=function(self) self.calls=self.calls+1; return iced.column({iced.text(self.label or 'Same')}) end,
            transitions=function(self) self.transition_calls=self.transition_calls+1; return {} end
        }"#).unwrap();
        let app: mlua::Table = lua.named_registry_value("riced.widget.app").unwrap();
        placements.instances.insert(resolved.id.clone(), lua);
        let mut catalog = vec![];
        let mut lists = super::super::animation::WidgetLists::new();
        let mut motion = crate::ui::anim::AnimRuntime::default();
        let system = sysinfo::System::new();
        let theme = crate::config::ThemeConfig::default();
        let outputs = HashMap::new();
        let notifications = std::collections::VecDeque::new();
        let toplevels = crate::services::ToplevelCache::default();
        let workspaces = crate::services::WorkspaceCache::default();
        let mut context = RuntimeContext {
            placements: &mut placements,
            catalog: &mut catalog,
            animation: ListContext {
                lists: &mut lists,
                motion: &mut motion,
                duration: std::time::Duration::ZERO,
            },
            services: ServiceCtx {
                sys: &system,
                gpu: None,
                theme: &theme,
                outputs: &outputs,
                notifications: &notifications,
                toplevels: &toplevels,
                workspaces: &workspaces,
            },
            outputs: HashMap::new(),
            paths: vec![],
        };
        let bar = window::Id::unique();
        let value = context.render(&resolved, bar);
        assert!(context.ingest(&resolved, bar, value));
        let key = (bar, resolved.id.clone());
        let WidgetNode::Column { children, .. } = &context.placements.trees[&key] else {
            panic!("column");
        };
        let original_children = children.as_ptr();
        let value = context.render(&resolved, bar);
        assert!(!context.ingest(&resolved, bar, value));
        let WidgetNode::Column { children, .. } = &context.placements.trees[&key] else {
            panic!("column");
        };
        assert_eq!(
            children.as_ptr(),
            original_children,
            "unchanged IR must not be cloned or replaced"
        );
        assert_eq!(app.get::<u32>("calls").unwrap(), 2);
        assert_eq!(app.get::<u32>("transition_calls").unwrap(), 2);
        app.set("label", "Changed").unwrap();
        let value = context.render(&resolved, bar);
        assert!(context.ingest(&resolved, bar, value));
        let WidgetNode::Column { children, .. } = &context.placements.trees[&key] else {
            panic!("column");
        };
        assert!(matches!(&children[0],WidgetNode::Text {content,..} if content == "Changed"));
        assert!(!context.ingest(&resolved, bar, Err("temporary error".into())));
        assert!(context.placements.trees.contains_key(&key));
    }
}
