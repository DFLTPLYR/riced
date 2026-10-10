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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_listview_delegates_have_stable_keys_and_local_transitions() {
        let lua = crate::lua::sandbox::new_lua(crate::lua::sandbox::Profile::Widget).unwrap();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let value: mlua::Value = lua.load(r#"return iced.scrollable(iced.listview({{id=1,title='one'},{id=2,title='two'}}):id('center'):key('id'):pitch(108):spacing(8)
            :delegate(function(n) return iced.container(iced.text(n.title)):padding(10):radius(6) end)
            :onEntered({x={from=200,to=0},opacity={from=0,to=1},duration=250})
            :onExit({x={to=-200},opacity={to=0},duration=250}):onDisplaced({duration=250}))"#).eval().unwrap();
        let tree = crate::ui::decode::parse_node(&value).unwrap();
        let WidgetNode::Scrollable { child, .. } = &tree else {
            panic!("scrollable");
        };
        let WidgetNode::ListView {
            items,
            transitions,
            pitch,
            ..
        } = &**child
        else {
            panic!("listview");
        };
        assert_eq!(
            items
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(*pitch, 108.0);
        assert_eq!(transitions.0.from.x, 200.0);
        assert_eq!(transitions.1.to.x, -200.0);
        assert_eq!(transitions.2.duration, Some(Duration::from_millis(250)));
        let mut motion = aura_anim::core::runtime::MotionRuntime::new();
        let mut lists = WidgetLists::new();
        lists.insert("clock/center".into(), ListView::new(108.0));
        super::super::build_with_lists(&tree, "clock", 13.0, None, &motion, &lists).unwrap();
        for list in lists.values_mut() {
            list.clear_all(&mut motion);
        }
        assert_eq!(motion.motion_count(), 0);
    }
}
