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
    ) {
        let key = (bar, resolved.id.clone());
        let scope = format!("{bar:?}/{}", resolved.id);
        let label = resolved.label();
        match result {
            Ok(Value::Table(table)) => {
                self.placements.errors.remove(&resolved.id);
                self.placements.outputs.remove(&key);
                match parse_node(&Value::Table(table)) {
                    Ok(node) => {
                        let old = self.placements.trees.get(&key).cloned();
                        if let Err(error) = self.animation.synchronize(&scope, old.as_ref(), &node)
                        {
                            self.note_error(&label, error);
                            return;
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
                                    return;
                                }
                            }
                        }
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
                    }
                    Err(error) => {
                        self.placements.trees.remove(&key);
                        self.note_error(&label, error);
                    }
                }
            }
            Ok(value) => {
                self.placements.errors.remove(&resolved.id);
                self.placements.trees.remove(&key);
                match crate::lua::value::coerce_text(value, "render()") {
                    Ok(text) => {
                        self.placements.outputs.insert(key, text);
                    }
                    Err(error) => self.note_error(&label, error),
                }
            }
            Err(error) => self.note_error(&label, error),
        }
    }
}
