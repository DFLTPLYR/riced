//! Bar/list animation adapter. Only the animation domain is borrowed here.
use crate::ui::{
    anim::AnimRuntime,
    listview::{Axis, ListView},
    node::WidgetNode,
};
use std::{collections::HashMap, time::Duration};

pub(crate) type WidgetLists = HashMap<String, ListView<(String, String), WidgetNode>>;

fn declared_lists<'a>(
    node: &'a WidgetNode,
    out: &mut HashMap<String, &'a WidgetNode>,
) -> Result<(), String> {
    match node {
        WidgetNode::ListView { id, items, .. } => {
            if out.insert(id.clone(), node).is_some() {
                return Err(format!("duplicate listview id {id:?}"));
            }
            for (_, child) in items {
                declared_lists(child, out)?;
            }
        }
        WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => {
            for child in children {
                declared_lists(child, out)?;
            }
        }
        WidgetNode::Container { child, .. } | WidgetNode::Scrollable { child, .. } => {
            declared_lists(child, out)?
        }
        _ => {}
    }
    Ok(())
}

pub(crate) struct ListContext<'a> {
    pub lists: &'a mut WidgetLists,
    pub motion: &'a mut AnimRuntime,
    pub duration: Duration,
}

impl ListContext<'_> {
    pub(crate) fn synchronize(
        &mut self,
        widget: &str,
        old: Option<&WidgetNode>,
        new: &WidgetNode,
    ) -> Result<(), String> {
        let mut before = HashMap::new();
        let mut after = HashMap::new();
        if let Some(old) = old {
            declared_lists(old, &mut before)?;
        }
        declared_lists(new, &mut after)?;
        for id in before.keys().filter(|id| !after.contains_key(*id)) {
            if let Some(mut list) = self.lists.remove(&format!("{widget}/{id}")) {
                list.clear_all(self.motion);
            }
        }
        for (id, node) in after {
            let WidgetNode::ListView {
                items,
                horizontal,
                pitch,
                transitions,
                ..
            } = node
            else {
                continue;
            };
            let owner = format!("{widget}/{id}");
            let list = self
                .lists
                .entry(owner.clone())
                .or_insert_with(|| ListView::new(*pitch));
            list.set_pitch(*pitch);
            list.set_transitions(
                transitions.0.clone(),
                transitions.1.clone(),
                transitions.2.clone(),
                true,
            );
            let Some(WidgetNode::ListView {
                items: old_items, ..
            }) = before.get(&id).copied()
            else {
                continue;
            };
            let old_keys: Vec<_> = old_items
                .iter()
                .map(|(key, _)| (owner.clone(), key.clone()))
                .collect();
            let keys: Vec<_> = items
                .iter()
                .map(|(key, _)| (owner.clone(), key.clone()))
                .collect();
            let removed: Vec<_> = old_items
                .iter()
                .enumerate()
                .filter(|(_, (key, _))| !items.iter().any(|(k, _)| k == key))
                .map(|(index, (key, child))| (index, (owner.clone(), key.clone()), child.clone()))
                .collect();
            list.update(
                self.motion,
                self.duration,
                if *horizontal {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                },
                &old_keys,
                &keys,
                &removed,
            );
        }
        Ok(())
    }
}
