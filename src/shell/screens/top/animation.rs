//! Bar/list animation adapter. Only the animation domain is borrowed here.
use crate::ui::{
    anim::AnimRuntime,
    listview::{Axis, ListView},
    node::WidgetNode,
};
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Default)]
pub(crate) struct WidgetLists {
    lists: HashMap<String, ListView<(String, String), WidgetNode>>,
    retiring: HashMap<String, Retirement>,
}

#[derive(Debug)]
struct Retirement {
    tree: WidgetNode,
    lists: Vec<String>,
}

impl std::ops::Deref for WidgetLists {
    type Target = HashMap<String, ListView<(String, String), WidgetNode>>;
    fn deref(&self) -> &Self::Target {
        &self.lists
    }
}

impl std::ops::DerefMut for WidgetLists {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.lists
    }
}

impl WidgetLists {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn display<'a>(
        &'a self,
        widget: &str,
        current: &'a WidgetNode,
    ) -> (&'a WidgetNode, bool) {
        self.retiring
            .get(widget)
            .filter(|retirement| {
                retirement.lists.iter().any(|id| {
                    self.lists
                        .get(&format!("{widget}/{id}"))
                        .is_some_and(ListView::has_ghosts)
                })
            })
            .map_or((current, false), |retirement| (&retirement.tree, true))
    }

    pub(crate) fn is_retiring(&self, widget: &str, list: &str) -> bool {
        self.retiring
            .get(widget)
            .is_some_and(|retirement| retirement.lists.iter().any(|id| id == list))
    }

    pub(crate) fn finish_retirements(&mut self, motion: &mut AnimRuntime) {
        self.retiring.retain(|widget, retirement| {
            let active = retirement.lists.iter().any(|id| {
                self.lists
                    .get(&format!("{widget}/{id}"))
                    .is_some_and(ListView::has_ghosts)
            });
            if !active {
                for id in &retirement.lists {
                    if let Some(mut list) = self.lists.remove(&format!("{widget}/{id}")) {
                        list.clear_all(motion);
                    }
                }
            }
            active
        });
    }

    pub(crate) fn forget_retirements(&mut self, prefix: &str) {
        self.retiring
            .retain(|widget, _| !widget.starts_with(prefix));
    }

    pub(crate) fn clear(&mut self) {
        self.lists.clear();
        self.retiring.clear();
    }
}

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
        WidgetNode::Row { children, .. }
        | WidgetNode::Column { children, .. }
        | WidgetNode::Stack { children, .. } => {
            for child in children {
                declared_lists(child, out)?;
            }
        }
        WidgetNode::Container { child, .. }
        | WidgetNode::Popup { child, .. }
        | WidgetNode::Scrollable { child, .. }
        | WidgetNode::Anchored { child, .. }
        | WidgetNode::Clickable { child, .. }
        | WidgetNode::Aligned { child, .. }
        | WidgetNode::Animated { child, .. } => declared_lists(child, out)?,
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
        // A conditional empty-state branch is a list removal, not a host teardown.
        // Keep its last layout inert until the departing rows have finished.
        if let Some(retirement) = self.lists.retiring.get_mut(widget) {
            retirement.lists.retain(|id| !after.contains_key(id));
        }
        if self
            .lists
            .retiring
            .get(widget)
            .is_some_and(|retirement| retirement.lists.is_empty())
        {
            self.lists.retiring.remove(widget);
        }
        let removed_lists: Vec<_> = before
            .keys()
            .filter(|id| !after.contains_key(*id))
            .cloned()
            .collect();
        for id in &removed_lists {
            let WidgetNode::ListView {
                items, horizontal, ..
            } = before[id]
            else {
                continue;
            };
            let owner = format!("{widget}/{id}");
            if let Some(list) = self.lists.get_mut(&owner) {
                let keys: Vec<_> = items
                    .iter()
                    .map(|(key, _)| (owner.clone(), key.clone()))
                    .collect();
                let removed: Vec<_> = items
                    .iter()
                    .enumerate()
                    .map(|(index, (key, child))| {
                        (index, (owner.clone(), key.clone()), child.clone())
                    })
                    .collect();
                list.update(
                    self.motion,
                    self.duration,
                    if *horizontal {
                        Axis::Horizontal
                    } else {
                        Axis::Vertical
                    },
                    &keys,
                    &[],
                    &removed,
                );
                list.sweep(self.motion);
            }
        }
        if !removed_lists.is_empty()
            && let Some(old) = old
        {
            self.lists
                .retiring
                .entry(widget.into())
                .or_insert_with(|| Retirement {
                    tree: old.clone(),
                    lists: Vec::new(),
                })
                .lists
                .extend(removed_lists);
        }
        for (id, node) in after {
            if before.get(&id).copied() == Some(node) {
                continue;
            }
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
            for node in [before.get(&id).copied(), Some(node)].into_iter().flatten() {
                if let WidgetNode::ListView {
                    callbacks, items, ..
                } = node
                {
                    for (key, _) in items {
                        list.clear_callback_spec(&(owner.clone(), key.clone()));
                    }
                    list.set_callbacks(callbacks, |key| (owner.clone(), key.into()));
                }
            }
            list.set_pitch(*pitch);
            list.set_transitions(
                transitions.0.clone(),
                transitions.1.clone(),
                transitions.2.clone(),
                true,
            );
            let old_items = match before.get(&id).copied() {
                Some(WidgetNode::ListView { items, .. }) => items.as_slice(),
                _ if list.has_ghosts() => &[],
                _ => continue,
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
    fn conditional_list_removal_finishes_exits_and_readdition_cancels_retirement() {
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        let parse =
            |source| crate::ui::decode::parse_node(&lua.load(source).eval().unwrap()).unwrap();
        let full = parse(
            "return iced.listview({{id=1}}):id('items'):key('id'):delegate(function() return iced.text('one') end):onExit({opacity={to=0},duration=150})",
        );
        let empty = parse("return iced.text('No items')");
        let mut lists = WidgetLists::new();
        let mut motion = AnimRuntime::default();
        let mut context = ListContext {
            lists: &mut lists,
            motion: &mut motion,
            duration: Duration::from_millis(150),
        };
        context.synchronize("popup", None, &full).unwrap();
        context.synchronize("popup", Some(&full), &empty).unwrap();
        assert!(context.lists.display("popup", &empty).1);
        assert!(context.lists["popup/items"].has_ghosts());
        context.synchronize("popup", Some(&empty), &full).unwrap();
        assert!(!context.lists.display("popup", &full).1);
        assert!(!context.lists["popup/items"].has_ghosts());
        context.synchronize("popup", Some(&full), &empty).unwrap();
        context.motion.tick(Duration::from_millis(300));
        for list in context.lists.values_mut() {
            list.sweep(context.motion);
        }
        context.lists.finish_retirements(context.motion);
        assert!(!context.lists.display("popup", &empty).1);
        assert!(!context.lists.contains_key("popup/items"));
        assert_eq!(context.motion.motion_count(), 0);
    }

    #[test]
    fn nested_animated_lists_preserve_native_sizing_and_explicit_overrides() {
        use iced::advanced::{layout, renderer::Headless, widget::Tree};
        use iced::{Element, Length, Size};
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let lua = crate::lua::sandbox::new_lua(crate::lua::sandbox::Profile::Widget).unwrap();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        for suffix in ["", ":width(80)", ":width('shrink')", ":height(60)"] {
            let source = format!(
                "return iced.stack({{iced.container(iced.scrollable(iced.column({{iced.listview({{{{id=1}}}}):id('sizing'):key('id'):delegate(function() return iced.space():width('fill'):height(20) end){suffix}}}))):padding(7), iced.text('overlay')}})"
            );
            let value: mlua::Value = lua.load(source).eval().unwrap();
            let node = crate::ui::decode::parse_node(&value).unwrap();
            let mut motion = AnimRuntime::default();
            let mut lists = WidgetLists::new();
            ListContext {
                lists: &mut lists,
                motion: &mut motion,
                duration: Duration::ZERO,
            }
            .synchronize("widget", None, &node)
            .unwrap();
            assert!(lists.contains_key("widget/sizing"));
            let mut built =
                super::super::build_with_lists(&node, "widget", 13.0, None, &motion, &lists, None)
                    .unwrap();
            let mut list =
                iced::widget::column![iced::widget::Space::new().width(Length::Fill).height(20)]
                    .spacing(4);
            list = match suffix {
                ":width(80)" => list.width(80),
                ":width('shrink')" => list.width(Length::Shrink),
                ":height(60)" => list.height(60),
                _ => list,
            };
            let mut native: Element<'static, crate::shell::Plant> = iced::widget::stack![
                iced::widget::container(iced::widget::scrollable(
                    iced::widget::column![list].spacing(4)
                ))
                .padding(7),
                iced::widget::text("overlay").size(13),
            ]
            .into();
            assert_eq!(
                built.as_widget().size_hint(),
                native.as_widget().size_hint(),
                "{suffix}"
            );
            let limits = layout::Limits::new(Size::ZERO, Size::new(300.0, 200.0));
            let mut built_tree = Tree::new(&built);
            let mut native_tree = Tree::new(&native);
            let actual = built
                .as_widget_mut()
                .layout(&mut built_tree, &renderer, &limits);
            let expected = native
                .as_widget_mut()
                .layout(&mut native_tree, &renderer, &limits);
            assert_eq!(actual.bounds(), expected.bounds(), "{suffix}");
        }
    }

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
        super::super::build_with_lists(&tree, "clock", 13.0, None, &motion, &lists, None).unwrap();
        for list in lists.values_mut() {
            list.clear_all(&mut motion);
        }
        assert_eq!(motion.motion_count(), 0);
    }
}
