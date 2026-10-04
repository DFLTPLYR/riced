//! QML-`ListView`-style enter/exit transitions for `ui` button rows.
//!
//! iced has no `ListView` with `onAdded`/`onRemove` delegates, so this
//! module implements the same semantics by hand on top of
//! [`aura_anim`](https://crates.io/crates/aura-anim):
//!
//! - Items are keyed by button `action` (the only stable identity a
//!   `ui` tree offers). Label-only changes on the same key swap
//!   instantly — like QML, transitions fire on add/remove, not edits.
//! - Added keys slide in from `ENTER_OFFSET` px leading padding.
//! - Removed keys are retained as inert ghosts at their old index and
//!   slide out, then swept once their motion completes.
//! - First paint settles instantly (no mass animation on startup or
//!   hot-reload).
//!
//! Honest limits (iced 0.14 gives us no opacity widget and no FLIP
//! layout): transitions are geometric (slide), not fades, and
//! surviving siblings reflow instantly instead of gliding
//! (`displaced` is out of scope). Durations follow the global
//! [`crate::config::AnimationSpeed`].
use aura_anim::core::{
    macros::Animatable,
    runtime::{Motion, MotionRuntime},
    timing::Timing,
};
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use super::top::WidgetNode;

/// `MotionRuntime` without a `Debug` impl, wrapped so `Plots` can
/// keep deriving it. Derefs to the runtime for ergonomic use.
#[derive(Default)]
pub(crate) struct AnimRuntime(MotionRuntime);

impl fmt::Debug for AnimRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnimRuntime")
            .field("active", &self.0.has_active())
            .field("motions", &self.0.motion_count())
            .finish()
    }
}

impl std::ops::Deref for AnimRuntime {
    type Target = MotionRuntime;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for AnimRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Leading slide distance (px) for entering/exiting items.
pub(crate) const ENTER_OFFSET: f32 = 16.0;

/// Animated per-item geometry: leading padding px. `0.0` is settled.
#[derive(Clone, Debug, Animatable)]
pub(crate) struct ItemSlide {
    pub pad: f32,
}

/// Live enter/exit motions keyed by (widget, button action).
pub(crate) type ItemMotions = HashMap<(String, String), Motion<ItemSlide>>;
/// Retained removed items per widget, rendered inert until settled.
pub(crate) type ItemGhosts = HashMap<String, Vec<GhostItem>>;

/// A removed-but-still-visible item: last-known node plus its list
/// index, rendered inert (no clicks) until the exit motion settles.
#[derive(Debug, Clone)]
pub(crate) struct GhostItem {
    pub index: usize,
    pub key: String,
    pub node: WidgetNode,
}

/// Keyed diff of one button list: keys present in `new` but not `old`
/// entered; keys in `old` with their old index left.
pub(crate) fn diff_keys(old: &[String], new: &[String]) -> (Vec<String>, Vec<(usize, String)>) {
    let added = new.iter().filter(|k| !old.contains(k)).cloned().collect();
    let removed = old
        .iter()
        .enumerate()
        .filter(|(_, k)| !new.contains(k))
        .map(|(i, k)| (i, k.clone()))
        .collect();
    (added, removed)
}

/// Direct button children `(action key, node)` of a top-level row or
/// column, in order. Anything else has no stable key and never
/// transitions.
pub(crate) fn button_children(node: &WidgetNode) -> Vec<(String, WidgetNode)> {
    match node {
        WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => children
            .iter()
            .filter_map(|child| match child {
                WidgetNode::Button { action, .. } => Some((action.clone(), child.clone())),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Reconcile one widget's list animations after a re-render: `old` is
/// the previous tree (`None` on first paint — everything settles),
/// `new` the fresh one. Entering keys slide from `ENTER_OFFSET`;
/// removed keys become inert ghosts sliding out. Unchanged keys are
/// untouched (in-flight motions keep running). Non-list trees clear
/// the widget's entries.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_list_anims(
    runtime: &mut MotionRuntime,
    motions: &mut ItemMotions,
    ghosts: &mut ItemGhosts,
    widget: &str,
    old: Option<&WidgetNode>,
    new: &WidgetNode,
    duration: Duration,
) {
    let timing = Timing::ease_out(duration);
    let new_kids = button_children(new);
    let Some(old_node) = old else {
        // First paint (or shape flip into a list): settle instantly.
        // Absent entries read as pad 0, so just clear stale state.
        motions.retain(|(w, _), _| w != widget);
        ghosts.remove(widget);
        return;
    };
    let old_kids = button_children(old_node);
    let old_keys: Vec<String> = old_kids.iter().map(|(k, _)| k.clone()).collect();
    let new_keys: Vec<String> = new_kids.iter().map(|(k, _)| k.clone()).collect();
    // A non-list on either side means wholesale change: settle.
    let old_is_list = matches!(old_node, WidgetNode::Row { .. } | WidgetNode::Column { .. });
    let new_is_list = matches!(new, WidgetNode::Row { .. } | WidgetNode::Column { .. });
    if !(old_is_list && new_is_list) {
        motions.retain(|(w, _), _| w != widget);
        ghosts.remove(widget);
        return;
    }
    let (added, removed) = diff_keys(&old_keys, &new_keys);
    let by_key: HashMap<&str, &WidgetNode> =
        old_kids.iter().map(|(k, n)| (k.as_str(), n)).collect();
    for key in added {
        let id = (widget.to_string(), key.clone());
        match motions.get(&id) {
            // Re-added mid-exit: retarget the live motion home.
            Some(m) => {
                let _ = m.transition_to(ItemSlide { pad: 0.0 }, runtime);
            }
            None => {
                let m = runtime.motion_with(ItemSlide { pad: ENTER_OFFSET }, timing);
                let _ = m.transition_to(ItemSlide { pad: 0.0 }, runtime);
                motions.insert(id, m);
            }
        }
        // No longer a ghost.
        if let Some(list) = ghosts.get_mut(widget) {
            list.retain(|g| g.key != key);
        }
        if ghosts.get(widget).is_some_and(Vec::is_empty) {
            ghosts.remove(widget);
        }
    }
    for (index, key) in removed {
        let id = (widget.to_string(), key.clone());
        let node = by_key
            .get(key.as_str())
            .cloned()
            .cloned()
            .unwrap_or(WidgetNode::Text {
                content: String::new(),
                size: None,
                width: None,
                height: None,
            });
        match motions.get(&id) {
            Some(m) => {
                let _ = m.transition_to(ItemSlide { pad: ENTER_OFFSET }, runtime);
            }
            None => {
                let m = runtime.motion_with(ItemSlide { pad: 0.0 }, timing);
                let _ = m.transition_to(ItemSlide { pad: ENTER_OFFSET }, runtime);
                motions.insert(id, m);
            }
        }
        let list = ghosts.entry(widget.to_string()).or_default();
        if !list.iter().any(|g| g.key == key) {
            list.push(GhostItem { index, key, node });
            list.sort_by_key(|g| g.index);
        }
    }
}

/// Advance the runtime and drop settled entries: completed motions
/// leave the map (pad reads as 0 when absent) and ghosts without a
/// live motion are gone. Returns whether anything is still animating.
pub(crate) fn sweep_anims(
    runtime: &mut MotionRuntime,
    motions: &mut ItemMotions,
    ghosts: &mut ItemGhosts,
    now: Instant,
) -> bool {
    runtime.tick_at(now);
    let mut dead = Vec::new();
    for (id, m) in motions.iter() {
        if m.is_completed(runtime).unwrap_or(true) {
            dead.push(id.clone());
        }
    }
    for id in dead {
        if let Some(m) = motions.remove(&id) {
            let _ = runtime.remove(m);
        }
    }
    ghosts.retain(|widget, list| {
        list.retain(|g| motions.contains_key(&(widget.clone(), g.key.clone())));
        !list.is_empty()
    });
    !motions.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::layers::top::WidgetNode;

    fn btn(action: &str) -> WidgetNode {
        WidgetNode::Button {
            label: action.to_string(),
            action: action.to_string(),
            width: None,
            height: None,
            padding: None,
        }
    }

    fn row(keys: &[&str]) -> WidgetNode {
        WidgetNode::Row {
            children: keys.iter().map(|k| btn(k)).collect(),
            spacing: 4.0,
            width: crate::app::layers::top::NodeLength::Shrink,
            height: crate::app::layers::top::NodeLength::Shrink,
        }
    }

    fn harness() -> (MotionRuntime, ItemMotions, ItemGhosts) {
        (MotionRuntime::new(), HashMap::new(), HashMap::new())
    }

    #[test]
    fn diff_keys_reports_added_and_removed_with_index() {
        let old = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let new = vec!["b".to_string(), "d".to_string()];
        let (added, removed) = diff_keys(&old, &new);
        assert_eq!(added, vec!["d".to_string()]);
        assert_eq!(removed, vec![(0, "a".to_string()), (2, "c".to_string())]);
        // Reorder alone is neither add nor remove.
        let (added, removed) = diff_keys(
            &["b".to_string(), "d".to_string()],
            &["d".to_string(), "b".to_string()],
        );
        assert!(added.is_empty() && removed.is_empty());
    }

    #[test]
    fn entering_keys_slide_home_and_settle() {
        let (mut rt, mut motions, mut ghosts) = harness();
        let dur = Duration::from_millis(150);
        let old = row(&["a"]);
        let new = row(&["a", "b"]);
        sync_list_anims(
            &mut rt,
            &mut motions,
            &mut ghosts,
            "w",
            Some(&old),
            &new,
            dur,
        );
        // New key animates; old key untouched (no entry = settled).
        assert_eq!(motions.len(), 1);
        assert!(motions.contains_key(&("w".to_string(), "b".to_string())));
        assert!(!ghosts.contains_key("w"));
        let m = motions[&("w".to_string(), "b".to_string())];
        assert_eq!(m.value(&rt).unwrap().pad, ENTER_OFFSET);
        rt.tick(Duration::from_millis(75));
        let mid = m.value(&rt).unwrap().pad;
        assert!(mid > 0.0 && mid < ENTER_OFFSET, "mid {mid}");
        rt.tick(Duration::from_millis(200));
        assert!(m.is_completed(&rt).unwrap());
        // Sweep drops settled entries (pad reads as 0 when absent).
        assert!(!sweep_anims(
            &mut rt,
            &mut motions,
            &mut ghosts,
            Instant::now()
        ));
        assert!(motions.is_empty());
    }

    #[test]
    fn removed_keys_become_inert_ghosts_then_leave() {
        let (mut rt, mut motions, mut ghosts) = harness();
        let dur = Duration::from_millis(150);
        let old = row(&["a", "b", "c"]);
        let new = row(&["a", "c"]);
        sync_list_anims(
            &mut rt,
            &mut motions,
            &mut ghosts,
            "w",
            Some(&old),
            &new,
            dur,
        );
        let list = ghosts.get("w").expect("ghost retained");
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].index, list[0].key.as_str()), (1, "b"));
        assert!(matches!(list[0].node, WidgetNode::Button { .. }));
        // Exit runs 0 -> offset.
        let m = motions[&("w".to_string(), "b".to_string())];
        assert_eq!(m.value(&rt).unwrap().pad, 0.0);
        rt.tick(Duration::from_millis(300));
        assert!(!sweep_anims(
            &mut rt,
            &mut motions,
            &mut ghosts,
            Instant::now()
        ));
        assert!(!ghosts.contains_key("w"));
    }

    #[test]
    fn first_paint_and_shape_flips_settle_instantly() {
        let (mut rt, mut motions, mut ghosts) = harness();
        let dur = Duration::from_millis(150);
        // No old tree: everything settles, nothing animates.
        sync_list_anims(
            &mut rt,
            &mut motions,
            &mut ghosts,
            "w",
            None,
            &row(&["a", "b"]),
            dur,
        );
        assert!(motions.is_empty() && !ghosts.contains_key("w"));
        // List -> text: stale entries cleared.
        sync_list_anims(
            &mut rt,
            &mut motions,
            &mut ghosts,
            "w",
            Some(&row(&["a"])),
            &WidgetNode::Text {
                content: "x".to_string(),
                size: None,
                width: None,
                height: None,
            },
            dur,
        );
        assert!(motions.is_empty() && !ghosts.contains_key("w"));
    }

    #[test]
    fn readded_mid_exit_ghost_flips_home() {
        let (mut rt, mut motions, mut ghosts) = harness();
        let dur = Duration::from_millis(150);
        sync_list_anims(
            &mut rt,
            &mut motions,
            &mut ghosts,
            "w",
            Some(&row(&["a", "b"])),
            &row(&["a"]),
            dur,
        );
        assert!(ghosts.contains_key("w"));
        // Re-add before the exit settles: ghost gone, motion retargets.
        sync_list_anims(
            &mut rt,
            &mut motions,
            &mut ghosts,
            "w",
            Some(&row(&["a"])),
            &row(&["a", "b"]),
            dur,
        );
        assert!(!ghosts.contains_key("w"));
        assert!(motions.contains_key(&("w".to_string(), "b".to_string())));
    }
}
