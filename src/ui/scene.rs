//! Generic measured scene/list realization. Notification policy never enters here.
use super::{
    action::Sender,
    anim::{AnimRuntime, ItemMotion},
    listview::{Axis, ListView},
    node::WidgetNode,
    widgets::measured::{self, Marker, Measurement},
};
use crate::shell::Plant;
use aura_anim::core::{runtime::Motion, timing::Timing, tween::Tween};
use iced::{Element, Length, Rectangle, Vector};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};
type Key = (String, String);
fn item_id(list: &str, key: &str) -> String {
    format!("{}:{list}{key}", list.len())
}
fn list_items(node: Option<&WidgetNode>) -> &[(String, WidgetNode)] {
    if let Some(WidgetNode::ListView { items, .. }) = node {
        items
    } else {
        &[]
    }
}

#[cfg(test)]
mod tests;

#[derive(Debug, Default)]
pub(crate) struct Lists {
    pub scope: String,
    resets: Vec<String>,
    active: HashSet<String>,
    displace: HashSet<String>,
    pub lists: HashMap<String, ListView<Key, WidgetNode>>,
    origins: HashMap<Key, Rectangle>,
    moves: HashMap<Key, Motion<ItemMotion>>,
    previous: BTreeMap<String, Rectangle>,
    pending: bool,
}
fn declared<'a>(
    node: &'a WidgetNode,
    out: &mut HashMap<String, &'a WidgetNode>,
) -> Result<(), String> {
    match node {
        WidgetNode::ListView { id, items, .. } => {
            if out.insert(id.clone(), node).is_some() {
                return Err(format!("duplicate listview id {id:?}"));
            }
            for (_, node) in items {
                declared(node, out)?;
            }
        }
        WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => {
            for node in children {
                declared(node, out)?;
            }
        }
        WidgetNode::Container { child, .. }
        | WidgetNode::Scrollable { child, .. }
        | WidgetNode::Anchored { child, .. }
        | WidgetNode::Clickable { child, .. } => declared(child, out)?,
        _ => {}
    }
    Ok(())
}
impl Lists {
    pub(crate) fn scroll_id(&self, id: &str) -> String {
        format!("{}:list:{id}", self.scope)
    }
    pub(crate) fn take_resets(&mut self) -> Vec<String> {
        std::mem::take(&mut self.resets)
    }
    pub(crate) fn synchronize(
        &mut self,
        old: Option<&WidgetNode>,
        new: &WidgetNode,
        frame: &Measurement,
        runtime: &mut AnimRuntime,
        duration: Duration,
    ) -> Result<(), String> {
        let mut before = HashMap::new();
        let mut after = HashMap::new();
        if let Some(old) = old {
            declared(old, &mut before)?;
        }
        declared(new, &mut after)?;
        self.previous = frame.items.clone();
        self.pending = true;
        self.displace.clear();
        self.active = after.keys().cloned().collect();
        let ids: HashSet<_> = before.keys().chain(after.keys()).cloned().collect();
        for id in ids {
            let new = after.get(&id).copied();
            let old = before.get(&id).copied();
            if new == old {
                continue;
            }
            if old.is_some() && new.is_some() {
                self.displace.insert(id.clone());
            }
            let spec = new.or(old).ok_or("list missing")?;
            let WidgetNode::ListView {
                transitions,
                horizontal,
                ..
            } = spec
            else {
                continue;
            };
            let old_items = list_items(old);
            let new_items = list_items(new);
            if old_items.is_empty() && !new_items.is_empty() {
                self.resets.push(self.scroll_id(&id));
            }
            let old_keys: Vec<_> = old_items
                .iter()
                .map(|(key, _)| (id.clone(), key.clone()))
                .collect();
            let keys: Vec<_> = new_items
                .iter()
                .map(|(key, _)| (id.clone(), key.clone()))
                .collect();
            let list = self
                .lists
                .entry(id.clone())
                .or_insert_with(|| ListView::new(0.0));
            list.set_pitch(0.0); // Displacement/exit positions are measured, never index × fixed height.
            list.set_transitions(
                transitions.0.clone(),
                transitions.1.clone(),
                transitions.2.clone(),
                true,
            );
            let removed: Vec<_> = old_items
                .iter()
                .enumerate()
                .filter(|(_, (key, _))| !new_items.iter().any(|(new, _)| new == key))
                .map(|(index, (key, node))| {
                    let key = (id.clone(), key.clone());
                    if let Some(bounds) = frame.items.get(&item_id(&id, &key.1)) {
                        let motion = list.motion_of(runtime, &key);
                        self.origins
                            .insert(key.clone(), *bounds - Vector::new(motion.x, motion.y));
                    }
                    (index, key, node.clone())
                })
                .collect();
            list.update(
                runtime,
                duration,
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
    pub(crate) fn layout(
        &mut self,
        data: &Measurement,
        runtime: &mut AnimRuntime,
        duration: Duration,
    ) -> bool {
        if !self.pending {
            return false;
        }
        self.pending = false;
        let mut moved = false;
        for (id, list) in &self.lists {
            if !self.displace.contains(id) {
                continue;
            }
            for (key, old) in &self.previous {
                let Some((_, item)) = key.split_once(':') else {
                    continue;
                };
                let Some(item) = item.strip_prefix(id) else {
                    continue;
                };
                if key != &item_id(id, item) {
                    continue;
                }
                let Some(new) = data.items.get(key) else {
                    continue;
                };
                let delta = old.position() - new.position();
                if delta.x.abs() < 0.5 && delta.y.abs() < 0.5 {
                    continue;
                }
                let key = (id.clone(), item.into());
                let current = self
                    .moves
                    .get(&key)
                    .and_then(|motion| motion.value(runtime).ok())
                    .unwrap_or_else(ItemMotion::settled);
                let from = ItemMotion {
                    x: current.x + delta.x,
                    y: current.y + delta.y,
                    opacity: 1.0,
                };
                let timing = Timing::ease_out(list.displaced_spec().duration.unwrap_or(duration));
                let motion = *self
                    .moves
                    .entry(key)
                    .or_insert_with(|| runtime.motion_with(from.clone(), timing));
                if let Err(error) =
                    runtime.play(motion, Tween::between(from, ItemMotion::settled(), timing))
                {
                    eprintln!("scene displacement: {error:?}");
                }
                moved = true;
            }
        }
        self.previous.clear();
        moved
    }
    pub(crate) fn sweep(&mut self, runtime: &mut AnimRuntime) -> bool {
        let before: usize = self
            .lists
            .values()
            .map(|list| list.ghosts_for(|_| true).len())
            .sum();
        for list in self.lists.values_mut() {
            list.sweep(runtime);
        }
        self.lists.retain(|id, list| {
            if self.active.contains(id) || !list.ghosts_for(|_| true).is_empty() {
                true
            } else {
                list.clear_all(runtime);
                false
            }
        });
        let retained: HashSet<_> = self
            .lists
            .values()
            .flat_map(|list| {
                list.ghosts_for(|_| true)
                    .into_iter()
                    .map(|ghost| ghost.key.clone())
            })
            .collect();
        self.origins.retain(|key, _| retained.contains(key));
        self.moves.retain(|_, motion| {
            if motion.is_completed(runtime).unwrap_or(true) {
                let _ = runtime.remove(*motion);
                false
            } else {
                true
            }
        });
        before
            != self
                .lists
                .values()
                .map(|list| list.ghosts_for(|_| true).len())
                .sum::<usize>()
    }
    pub(crate) fn clear(&mut self, runtime: &mut AnimRuntime) {
        for list in self.lists.values_mut() {
            list.clear_all(runtime);
        }
        for motion in self.moves.values() {
            let _ = runtime.remove(*motion);
        }
        self.lists.clear();
        self.moves.clear();
        self.origins.clear();
        self.previous.clear();
        self.active.clear();
        self.displace.clear();
        self.resets.clear();
    }
    pub(crate) fn has_images(&self) -> bool {
        self.lists.values().any(|list| {
            list.ghosts_for(|_| true)
                .iter()
                .any(|ghost| super::node::contains_images(&ghost.content))
        })
    }
    pub(crate) fn build(
        &self,
        node: &WidgetNode,
        runtime: &AnimRuntime,
        sender: Sender,
    ) -> Result<Element<'static, Plant>, String> {
        let live = self.node(node, runtime, sender, true, 1.0)?;
        let mut layer = iced::widget::stack![
            iced::widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fill),
            live
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .clip(false);
        for list in self.lists.values() {
            for ghost in list.ghosts_for(|_| true) {
                let Some(bounds) = self.origins.get(&ghost.key) else {
                    continue;
                };
                let motion = list.motion_of(runtime, &ghost.key);
                let body =
                    super::build::build_node_opacity(&ghost.content, 13.0, None, motion.opacity)?;
                let body = iced::widget::container(body)
                    .width(bounds.width)
                    .height(bounds.height);
                layer = layer.push(super::motion::ghost(
                    body.into(),
                    bounds.x + motion.x,
                    bounds.y + motion.y,
                ));
            }
        }
        Ok(layer.into())
    }
    fn node(
        &self,
        node: &WidgetNode,
        runtime: &AnimRuntime,
        sender: Sender,
        active: bool,
        opacity: f32,
    ) -> Result<Element<'static, Plant>, String> {
        match node {
            WidgetNode::Anchored {
                child,
                placement,
                margin,
            } => Ok(super::widgets::anchored::placed(
                self.node(child, runtime, sender, active, opacity)?,
                *placement,
                *margin,
            )),
            WidgetNode::Clickable { child, action } => {
                let body = self.node(child, runtime, sender.clone(), active, opacity)?;
                let element = iced::widget::mouse_area(body)
                    .on_press(sender(action.clone().into()))
                    .into();
                Ok(if active {
                    measured::marked(element, Marker::Interactive)
                } else {
                    super::motion::shifted(element, 0.0, 0.0, false)
                })
            }
            WidgetNode::Container {
                child,
                width,
                height,
                padding,
                background,
                radius,
                border,
                border_width,
            } => {
                let bg = *background;
                let border = *border;
                let radius = *radius;
                let border_width = *border_width;
                let element =
                    iced::widget::container(self.node(child, runtime, sender, active, opacity)?);
                let hints = iced::advanced::Widget::size(&element);
                Ok(element
                    .width(width.resolve(hints.width))
                    .height(height.resolve(hints.height))
                    .padding(*padding)
                    .style(move |_| iced::widget::container::Style {
                        background: bg.map(|color| color.scale_alpha(opacity).into()),
                        border: iced::Border {
                            color: border.unwrap_or_default().scale_alpha(opacity),
                            width: border_width,
                            radius: radius.into(),
                        },
                        ..Default::default()
                    })
                    .into())
            }
            WidgetNode::Scrollable {
                child,
                width,
                height,
            } => {
                let item =
                    iced::widget::scrollable(self.node(child, runtime, sender, active, opacity)?);
                let hints = iced::advanced::Widget::size(&item);
                Ok(item
                    .width(width.resolve(hints.width))
                    .height(height.resolve(hints.height))
                    .into())
            }
            WidgetNode::Row {
                children,
                width,
                height,
                spacing,
            }
            | WidgetNode::Column {
                children,
                width,
                height,
                spacing,
            } => {
                let children = children
                    .iter()
                    .map(|child| self.node(child, runtime, sender.clone(), active, opacity))
                    .collect::<Result<Vec<_>, _>>()?;
                if matches!(node, WidgetNode::Row { .. }) {
                    let item = iced::widget::Row::with_children(children).spacing(*spacing);
                    let hints = iced::advanced::Widget::size(&item);
                    Ok(item
                        .width(width.resolve(hints.width))
                        .height(height.resolve(hints.height))
                        .into())
                } else {
                    let item = iced::widget::Column::with_children(children).spacing(*spacing);
                    let hints = iced::advanced::Widget::size(&item);
                    Ok(item
                        .width(width.resolve(hints.width))
                        .height(height.resolve(hints.height))
                        .into())
                }
            }
            WidgetNode::ListView {
                id,
                items,
                horizontal,
                spacing,
                width,
                height,
                ..
            } => {
                let mut keys = Vec::new();
                let mut elements = Vec::new();
                for (key, child) in items {
                    let owner = (id.clone(), key.clone());
                    let mut motion = self
                        .lists
                        .get(id)
                        .map(|list| list.motion_of(runtime, &owner))
                        .unwrap_or_else(ItemMotion::settled);
                    if let Some(moved) = self
                        .moves
                        .get(&owner)
                        .and_then(|motion| motion.value(runtime).ok())
                    {
                        motion.x += moved.x;
                        motion.y += moved.y;
                    }
                    let child = self.node(
                        child,
                        runtime,
                        sender.clone(),
                        active,
                        opacity * motion.opacity,
                    )?;
                    let child = measured::marked(child, Marker::Item(item_id(id, key)));
                    use std::hash::{Hash, Hasher};
                    let mut hash = std::collections::hash_map::DefaultHasher::new();
                    key.hash(&mut hash);
                    elements.push(super::motion::shifted(child, motion.x, motion.y, active));
                    keys.push(hash.finish());
                }
                // from_vecs does not infer children's Fill hints. Resolve the
                // cross axis before moving elements into the keyed column, or
                // ordinary fill-width cards collapse to a zero-width column.
                let hints = elements.iter().fold(
                    iced::Size::new(Length::Shrink, Length::Shrink),
                    |hints, child| {
                        let child = child.as_widget().size_hint();
                        iced::Size::new(
                            hints.width.enclose(child.width),
                            hints.height.enclose(child.height),
                        )
                    },
                );
                let content: Element<'static, Plant> = if *horizontal {
                    iced::widget::Row::with_children(elements)
                        .spacing(*spacing)
                        .height(height.resolve(hints.height))
                        .into()
                } else {
                    iced::widget::keyed::Column::from_vecs(keys, elements)
                        .spacing(*spacing)
                        .width(width.resolve(hints.width))
                        .into()
                };
                let ghosts = self
                    .lists
                    .get(id)
                    .map(|list| list.ghosts_for(|_| true))
                    .unwrap_or_default();
                let reserve = ghosts
                    .iter()
                    .filter_map(|ghost| self.origins.get(&ghost.key))
                    .map(|bounds| {
                        if *horizontal {
                            bounds.width
                        } else {
                            bounds.height
                        }
                    })
                    .sum::<f32>()
                    + *spacing * ghosts.len().saturating_sub(usize::from(items.is_empty())) as f32;
                let content: Element<'static, Plant> = if reserve <= 0.0 {
                    content
                } else if *horizontal {
                    iced::widget::row![content, iced::widget::Space::new().width(reserve)].into()
                } else {
                    iced::widget::column![content, iced::widget::Space::new().height(reserve)]
                        .into()
                };
                let hints = content.as_widget().size_hint();
                let direction = if *horizontal {
                    iced::widget::scrollable::Direction::Horizontal(
                        iced::widget::scrollable::Scrollbar::hidden(),
                    )
                } else {
                    iced::widget::scrollable::Direction::Vertical(
                        iced::widget::scrollable::Scrollbar::hidden(),
                    )
                };
                let item = iced::widget::scrollable(content)
                    .id(self.scroll_id(id))
                    .direction(direction);
                Ok(item
                    .width(width.resolve(hints.width))
                    .height(height.resolve(Length::Shrink))
                    .into())
            }
            _ => {
                let element = super::build::build_node_opacity(
                    node,
                    13.0,
                    active.then_some(sender),
                    opacity,
                )?;
                let interactive = matches!(
                    node,
                    WidgetNode::Button { .. }
                        | WidgetNode::Checkbox {
                            on_toggle: Some(_),
                            ..
                        }
                        | WidgetNode::Toggler {
                            on_toggle: Some(_),
                            ..
                        }
                        | WidgetNode::Slider {
                            on_change: Some(_),
                            ..
                        }
                        | WidgetNode::TextInput {
                            on_input: Some(_),
                            ..
                        }
                        | WidgetNode::PickList {
                            on_select: Some(_),
                            ..
                        }
                        | WidgetNode::Radio {
                            on_select: Some(_),
                            ..
                        }
                );
                Ok(if active && interactive {
                    measured::marked(element, Marker::Interactive)
                } else {
                    element
                })
            }
        }
    }
}
