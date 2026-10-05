//! QML-`ListView`-style enter/exit transitions for every surface.
//!
//! iced has no `ListView` with `onAdded`/`onRemove` delegates, so this
//! module implements the same semantics once, generically, on top of
//! [`aura_anim`](https://crates.io/crates/aura-anim) — widget cell rows,
//! notification stacks, and any future panel list all share it:
//!
//! - Items are keyed by whatever stably identifies them: button
//!   `action` pairs for widget rows, `(output, id)` for notifications.
//!   Label-only changes on the same key swap instantly — like QML,
//!   transitions fire on add/remove, not edits.
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
use std::hash::Hash;
use std::time::Duration;

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

/// One keyed enter/exit transition set shared by every animated list
/// in the shell: widget cell rows key it `(widget, action)`,
/// notification stacks `(output, id)`, panel lists whatever identifies
/// their rows. Callers own diffing/placement; this owns motions.
#[derive(Debug)]
pub(crate) struct TransSet<K> {
    motions: HashMap<K, Motion<ItemSlide>>,
    ghosts: Vec<Ghost<K>>,
}

impl<K> Default for TransSet<K> {
    fn default() -> Self {
        Self {
            motions: HashMap::new(),
            ghosts: Vec::new(),
        }
    }
}

/// A removed-but-still-visible item: last-known node plus its list
/// index, rendered inert (no clicks) until the exit motion settles.
#[derive(Debug, Clone)]
pub(crate) struct Ghost<K> {
    pub key: K,
    pub index: usize,
    pub node: WidgetNode,
}

impl<K: Eq + Hash + Clone> TransSet<K> {
    /// Fresh key slides in from `ENTER_OFFSET`; a re-added mid-exit
    /// key flips its live motion home and drops its ghost.
    pub fn enter(&mut self, runtime: &mut MotionRuntime, timing: Timing, key: K) {
        match self.motions.get(&key) {
            Some(m) => {
                let _ = m.transition_to(ItemSlide { pad: 0.0 }, runtime);
            }
            None => {
                let m = runtime.motion_with(ItemSlide { pad: ENTER_OFFSET }, timing);
                let _ = m.transition_to(ItemSlide { pad: 0.0 }, runtime);
                self.motions.insert(key.clone(), m);
            }
        }
        self.ghosts.retain(|g| g.key != key);
    }

    /// Removed key becomes an inert ghost at `index`, sliding out from
    /// its current pad (mid-enter dismissals retarget smoothly instead
    /// of jumping).
    pub fn retire(
        &mut self,
        runtime: &mut MotionRuntime,
        timing: Timing,
        key: K,
        index: usize,
        node: WidgetNode,
    ) {
        match self.motions.get(&key) {
            Some(m) => {
                let _ = m.transition_to(ItemSlide { pad: ENTER_OFFSET }, runtime);
            }
            None => {
                let m = runtime.motion_with(ItemSlide { pad: 0.0 }, timing);
                let _ = m.transition_to(ItemSlide { pad: ENTER_OFFSET }, runtime);
                self.motions.insert(key.clone(), m);
            }
        }
        self.ghosts.retain(|g| g.key != key);
        self.ghosts.push(Ghost { key, index, node });
        self.ghosts.sort_by_key(|g| g.index);
    }

    /// Instant-settle one key (restart paths): motion and ghost gone.
    pub fn drop_key(&mut self, runtime: &mut MotionRuntime, key: &K) {
        if let Some(m) = self.motions.remove(key) {
            let _ = runtime.remove(m);
        }
        self.ghosts.retain(|g| &g.key != key);
    }

    /// Instant-settle every key (reloads): frees all runtime slots.
    pub fn clear_all(&mut self, runtime: &mut MotionRuntime) {
        self.clear_scope(runtime, |_| true);
    }

    /// Instant-settle every key matching `pred` (widget scope, output
    /// scope, shape flips, reloads).
    pub fn clear_scope(&mut self, runtime: &mut MotionRuntime, pred: impl Fn(&K) -> bool) {
        let dead: Vec<K> = self.motions.keys().filter(|k| pred(k)).cloned().collect();
        for key in dead {
            self.drop_key(runtime, &key);
        }
    }

    /// Drop settled motions (freeing their runtime slots) and orphan
    /// ghosts. The caller ticks the runtime once first; returns
    /// whether anything still animates.
    pub fn sweep(&mut self, runtime: &mut MotionRuntime) -> bool {
        let dead: Vec<K> = self
            .motions
            .iter()
            .filter(|(_, m)| m.is_completed(runtime).unwrap_or(true))
            .map(|(k, _)| k.clone())
            .collect();
        for key in dead {
            self.drop_key(runtime, &key);
        }
        !self.motions.is_empty()
    }

    /// Current pad for a key; `0.0` when settled or absent.
    pub fn pad(&self, runtime: &MotionRuntime, key: &K) -> f32 {
        self.motions
            .get(key)
            .and_then(|m| m.value(runtime).ok())
            .map(|v| v.pad)
            .unwrap_or(0.0)
    }

    /// Ghosts matching `pred`, in index order, for merging into a view.
    pub fn ghosts_for(&self, pred: impl Fn(&K) -> bool) -> Vec<&Ghost<K>> {
        let mut out: Vec<&Ghost<K>> = self.ghosts.iter().filter(|g| pred(&g.key)).collect();
        out.sort_by_key(|g| g.index);
        out
    }
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
/// `new` the fresh one. Thin wrapper translating tree diffs into
/// [`TransSet`] enter/retire calls keyed `(widget, action)`.
/// Unchanged keys are untouched (in-flight motions keep running).
/// Non-list trees clear the widget's entries.
pub(crate) fn sync_list_anims(
    set: &mut TransSet<(String, String)>,
    runtime: &mut MotionRuntime,
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
        set.clear_scope(runtime, |(w, _)| w == widget);
        return;
    };
    let old_kids = button_children(old_node);
    let old_keys: Vec<String> = old_kids.iter().map(|(k, _)| k.clone()).collect();
    let new_keys: Vec<String> = new_kids.iter().map(|(k, _)| k.clone()).collect();
    // A non-list on either side means wholesale change: settle.
    let old_is_list = matches!(old_node, WidgetNode::Row { .. } | WidgetNode::Column { .. });
    let new_is_list = matches!(new, WidgetNode::Row { .. } | WidgetNode::Column { .. });
    if !(old_is_list && new_is_list) {
        set.clear_scope(runtime, |(w, _)| w == widget);
        return;
    }
    let (added, removed) = diff_keys(&old_keys, &new_keys);
    let by_key: HashMap<&str, &WidgetNode> =
        old_kids.iter().map(|(k, n)| (k.as_str(), n)).collect();
    for key in added {
        set.enter(runtime, timing, (widget.to_string(), key));
    }
    for (index, key) in removed {
        if let Some(node) = by_key.get(key.as_str()) {
            set.retire(
                runtime,
                timing,
                (widget.to_string(), key),
                index,
                (*node).clone(),
            );
        }
    }
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

    fn timing() -> Timing {
        Timing::ease_out(Duration::from_millis(150))
    }

    fn harness() -> (MotionRuntime, TransSet<(String, String)>) {
        (MotionRuntime::new(), TransSet::default())
    }

    fn wkey(w: &str, a: &str) -> (String, String) {
        (w.to_string(), a.to_string())
    }

    #[test]
    fn entering_keys_slide_home_and_settle() {
        let (mut rt, mut set) = harness();
        set.enter(&mut rt, timing(), wkey("w", "b"));
        assert_eq!(set.pad(&rt, &wkey("w", "b")), ENTER_OFFSET);
        // Unknown keys read settled.
        assert_eq!(set.pad(&rt, &wkey("w", "a")), 0.0);
        rt.tick(Duration::from_millis(75));
        let mid = set.pad(&rt, &wkey("w", "b"));
        assert!(mid > 0.0 && mid < ENTER_OFFSET, "mid {mid}");
        rt.tick(Duration::from_millis(200));
        // Sweep drops settled entries (pad reads as 0 when absent).
        assert!(!set.sweep(&mut rt));
        assert_eq!(set.pad(&rt, &wkey("w", "b")), 0.0);
    }

    #[test]
    fn removed_keys_become_inert_ghosts_then_leave() {
        let (mut rt, mut set) = harness();
        set.retire(&mut rt, timing(), wkey("w", "b"), 1, btn("b"));
        let ghosts = set.ghosts_for(|(w, _)| w == "w");
        assert_eq!(ghosts.len(), 1);
        assert_eq!((ghosts[0].index, ghosts[0].key.1.as_str()), (1, "b"));
        assert!(matches!(ghosts[0].node, WidgetNode::Button { .. }));
        // Exit runs 0 -> offset.
        assert_eq!(set.pad(&rt, &wkey("w", "b")), 0.0);
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        assert!(set.ghosts_for(|(w, _)| w == "w").is_empty());
    }

    #[test]
    fn readded_mid_exit_ghost_flips_home() {
        let (mut rt, mut set) = harness();
        set.retire(&mut rt, timing(), wkey("w", "b"), 0, btn("b"));
        assert_eq!(set.ghosts_for(|_| true).len(), 1);
        // Re-add before the exit settles: ghost gone, motion retargets.
        set.enter(&mut rt, timing(), wkey("w", "b"));
        assert!(set.ghosts_for(|_| true).is_empty());
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
    }

    #[test]
    fn clear_scope_settles_one_scope_only() {
        let (mut rt, mut set) = harness();
        set.enter(&mut rt, timing(), wkey("w1", "a"));
        set.enter(&mut rt, timing(), wkey("w2", "a"));
        set.retire(&mut rt, timing(), wkey("w2", "b"), 0, btn("b"));
        set.clear_scope(&mut rt, |(w, _)| w == "w1");
        assert_eq!(set.pad(&rt, &wkey("w1", "a")), 0.0);
        // Other scopes untouched.
        assert!(set.pad(&rt, &wkey("w2", "a")) > 0.0);
        assert_eq!(set.ghosts_for(|(w, _)| w == "w2").len(), 1);
    }

    #[test]
    fn sync_list_anims_diffs_old_and_new_trees() {
        let (mut rt, mut set) = harness();
        let dur = Duration::from_millis(150);
        let old = row(&["a", "b", "c"]);
        let new = row(&["a", "c", "d"]);
        sync_list_anims(&mut set, &mut rt, "w", Some(&old), &new, dur);
        // Added key enters; removed key ghosts at its old index.
        assert!(set.pad(&rt, &wkey("w", "d")) > 0.0);
        assert_eq!(set.pad(&rt, &wkey("w", "a")), 0.0);
        let ghosts = set.ghosts_for(|(w, _)| w == "w");
        assert_eq!(ghosts.len(), 1);
        assert_eq!((ghosts[0].index, ghosts[0].key.1.as_str()), (1, "b"));
        // First paint and shape flips settle instantly.
        sync_list_anims(&mut set, &mut rt, "w", None, &row(&["x"]), dur);
        assert_eq!(set.pad(&rt, &wkey("w", "d")), 0.0);
        assert!(set.ghosts_for(|_| true).is_empty());
    }
}
