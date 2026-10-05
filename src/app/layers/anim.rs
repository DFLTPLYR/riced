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

/// QML-style list transition targets per item: horizontal offset `x`
/// (px, `0.0` settled), vertical nudge `y` (px, for `displaced`
/// glides), and `opacity` (`1.0` opaque, `0.0` gone). Enter runs
/// `+x -> 0` with `0 -> 1` fade; exit runs `0 -> -x` with `1 -> 0`
/// fade; survivors glide `y` toward their new slot.
#[derive(Clone, Debug, Animatable)]
pub(crate) struct ItemMotion {
    pub x: f32,
    pub y: f32,
    pub opacity: f32,
}

impl ItemMotion {
    /// Rest state: on-slot, fully visible.
    pub fn settled() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            opacity: 1.0,
        }
    }

    /// Snap any NaN/inf creep back into range (aura values come from
    /// easing math; guards keep layout and alpha finite).
    pub fn clamped(self) -> Self {
        Self {
            x: self.x.clamp(-4096.0, 4096.0),
            y: self.y.clamp(-4096.0, 4096.0),
            opacity: self.opacity.clamp(0.0, 1.0),
        }
    }
}

/// One keyed enter/exit transition set shared by every animated list
/// in the shell: widget cell rows key it `(widget, action)`,
/// notification stacks `(output, id)`, panel lists whatever identifies
/// their rows. Callers own diffing/placement; this owns motions.
///
/// Motions are QML-style: horizontal `x` offset + `opacity` on
/// add/remove, vertical `y` glide for survivors (`displaced`).
/// `settle_all` marks every tracked key at rest; `motion_of`
/// snapshots one key's animated values for the view.
#[derive(Debug)]
pub(crate) struct TransSet<K> {
    motions: HashMap<K, Motion<ItemMotion>>,
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
    /// Fresh key fades/slides in from `+x` (`from_bottom` flips the
    /// sign); a re-added mid-exit key flips its live motion home and
    /// drops its ghost. The global `duration` rides along so `nudge`
    /// glides share the same speed.
    pub fn enter(
        &mut self,
        runtime: &mut MotionRuntime,
        duration: Duration,
        key: K,
        from_bottom: bool,
    ) {
        let from_x = if from_bottom {
            -ENTER_OFFSET
        } else {
            ENTER_OFFSET
        };
        let timing = Timing::ease_out(duration);
        match self.motions.get(&key) {
            Some(m) => {
                let _ = m.transition_to(ItemMotion::settled(), runtime);
            }
            None => {
                let m = runtime.motion_with(
                    ItemMotion {
                        x: from_x,
                        y: 0.0,
                        opacity: 0.0,
                    },
                    timing,
                );
                let _ = m.transition_to(ItemMotion::settled(), runtime);
                self.motions.insert(key.clone(), m);
            }
        }
        self.ghosts.retain(|g| g.key != key);
    }

    /// Removed key becomes an inert ghost at `index`, fading/sliding
    /// out toward `-x` from its current values (mid-enter dismissals
    /// retarget smoothly instead of jumping).
    pub fn retire(
        &mut self,
        runtime: &mut MotionRuntime,
        duration: Duration,
        key: K,
        index: usize,
        node: WidgetNode,
    ) {
        let exit = ItemMotion {
            x: -ENTER_OFFSET,
            y: 0.0,
            opacity: 0.0,
        };
        let timing = Timing::ease_out(duration);
        match self.motions.get(&key) {
            Some(m) => {
                let _ = m.transition_to(exit, runtime);
            }
            None => {
                let m = runtime.motion_with(ItemMotion::settled(), timing);
                let _ = m.transition_to(exit, runtime);
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

    /// Current motion values for a key; settled when absent.
    pub fn motion_of(&self, runtime: &MotionRuntime, key: &K) -> ItemMotion {
        self.motions
            .get(key)
            .and_then(|m| m.value(runtime).ok())
            .map(|v| v.clamped())
            .unwrap_or_else(ItemMotion::settled)
    }

    /// Legacy pad readout (vertical nudge magnitude); `0.0` when
    /// settled or absent. Kept for the padding fallback in tests.
    pub fn pad(&self, runtime: &MotionRuntime, key: &K) -> f32 {
        self.motion_of(runtime, key).x.abs()
    }

    /// Nudge every settled key's `y` by `dy` (survivor `displaced`
    /// glide): completed tweens replay via [`MotionRuntime::play`] on
    /// the same slot, easing current -> nudged. In-flight keys
    /// (mid-enter, mid-exit) are skipped — their own motion owns the
    /// frame. Pair with [`Self::settle_nudged`] on a later frame to
    /// glide home. `duration` keeps the glide at the global speed.
    /// Returns how many keys started gliding.
    #[allow(dead_code)]
    pub fn nudge_all(&mut self, runtime: &mut MotionRuntime, duration: Duration, dy: f32) -> usize {
        if dy.abs() < 0.5 {
            return 0;
        }
        let timing = Timing::ease_out(duration);
        let keys: Vec<K> = self.motions.keys().cloned().collect();
        let mut nudged = 0;
        for key in keys {
            let Some(m) = self.motions.get(&key).cloned() else {
                continue;
            };
            if !m.is_completed(runtime).unwrap_or(false) {
                continue;
            }
            if let Ok(cur) = m.value(runtime) {
                use aura_anim::core::tween::Tween;
                let to = ItemMotion {
                    x: cur.x,
                    y: cur.y + dy,
                    opacity: cur.opacity,
                };
                let _ = runtime.play(m, Tween::between(cur, to, timing));
                nudged += 1;
            }
        }
        nudged
    }

    /// Nudge one key by `dy`, creating its motion when the key has no
    /// entry (already swept after settling). Fresh entries start AT
    /// the nudged offset and glide home, so survivors whose enter
    /// motion long since completed still `displaced`-glide.
    pub fn nudge_one(&mut self, runtime: &mut MotionRuntime, duration: Duration, key: K, dy: f32) {
        if dy.abs() < 0.5 {
            return;
        }
        let timing = Timing::ease_out(duration);
        let cur = self.motion_of(runtime, &key);
        let nudged = ItemMotion {
            x: cur.x,
            y: cur.y + dy,
            opacity: cur.opacity,
        };
        match self.motions.get(&key).cloned() {
            Some(m) => {
                use aura_anim::core::tween::Tween;
                let _ = runtime.play(m, Tween::between(cur, nudged, timing));
            }
            None => {
                let m = runtime.motion_with(nudged, timing);
                let _ = m.transition_to(
                    ItemMotion {
                        x: cur.x,
                        y: 0.0,
                        opacity: cur.opacity,
                    },
                    runtime,
                );
                self.motions.insert(key, m);
            }
        }
    }

    /// Glide every nudged key's `y` back to `0` (second half of a
    /// `displaced` pair): call on the frame after [`Self::nudge_all`]
    /// so survivors ease out-and-back instead of jumping. In-place
    /// retarget keeps the visible path smooth.
    pub fn settle_nudged(&self, runtime: &mut MotionRuntime) {
        for m in self.motions.values() {
            if let Ok(cur) = m.value(runtime)
                && cur.y.abs() >= 0.5
            {
                let _ = m.transition_to(
                    ItemMotion {
                        x: cur.x,
                        y: 0.0,
                        opacity: cur.opacity,
                    },
                    runtime,
                );
            }
        }
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
        set.enter(runtime, duration, (widget.to_string(), key), false);
    }
    for (index, key) in removed {
        if let Some(node) = by_key.get(key.as_str()) {
            set.retire(
                runtime,
                duration,
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

    fn dur() -> Duration {
        Duration::from_millis(150)
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
        set.enter(&mut rt, dur(), wkey("w", "b"), false);
        let start = set.motion_of(&rt, &wkey("w", "b"));
        assert_eq!((start.x, start.opacity), (ENTER_OFFSET, 0.0));
        // Unknown keys read settled.
        let clean = set.motion_of(&rt, &wkey("w", "a"));
        assert_eq!((clean.x, clean.y, clean.opacity), (0.0, 0.0, 1.0));
        rt.tick(Duration::from_millis(75));
        let mid = set.motion_of(&rt, &wkey("w", "b"));
        assert!(mid.x > 0.0 && mid.x < ENTER_OFFSET, "mid {}", mid.x);
        assert!(
            mid.opacity > 0.0 && mid.opacity < 1.0,
            "mid {}",
            mid.opacity
        );
        rt.tick(Duration::from_millis(200));
        // Sweep drops settled entries (absent reads settled).
        assert!(!set.sweep(&mut rt));
        let done = set.motion_of(&rt, &wkey("w", "b"));
        assert_eq!((done.x, done.opacity), (0.0, 1.0));
    }

    #[test]
    fn removed_keys_become_inert_ghosts_then_leave() {
        let (mut rt, mut set) = harness();
        set.retire(&mut rt, dur(), wkey("w", "b"), 1, btn("b"));
        let ghosts = set.ghosts_for(|(w, _)| w == "w");
        assert_eq!(ghosts.len(), 1);
        assert_eq!((ghosts[0].index, ghosts[0].key.1.as_str()), (1, "b"));
        assert!(matches!(ghosts[0].node, WidgetNode::Button { .. }));
        // Exit starts settled, then fades/slides toward -x.
        let start = set.motion_of(&rt, &wkey("w", "b"));
        assert_eq!((start.x, start.opacity), (0.0, 1.0));
        rt.tick(Duration::from_millis(75));
        let mid = set.motion_of(&rt, &wkey("w", "b"));
        assert!(mid.x < 0.0 && mid.opacity < 1.0, "mid {mid:?}");
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        assert!(set.ghosts_for(|(w, _)| w == "w").is_empty());
    }

    #[test]
    fn readded_mid_exit_ghost_flips_home() {
        let (mut rt, mut set) = harness();
        set.retire(&mut rt, dur(), wkey("w", "b"), 0, btn("b"));
        assert_eq!(set.ghosts_for(|_| true).len(), 1);
        // Re-add before the exit settles: ghost gone, motion retargets.
        set.enter(&mut rt, dur(), wkey("w", "b"), false);
        assert!(set.ghosts_for(|_| true).is_empty());
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
    }

    #[test]
    fn clear_scope_settles_one_scope_only() {
        let (mut rt, mut set) = harness();
        set.enter(&mut rt, dur(), wkey("w1", "a"), false);
        set.enter(&mut rt, dur(), wkey("w2", "a"), false);
        set.retire(&mut rt, dur(), wkey("w2", "b"), 0, btn("b"));
        set.clear_scope(&mut rt, |(w, _)| w == "w1");
        let settled = set.motion_of(&rt, &wkey("w1", "a"));
        assert_eq!((settled.x, settled.opacity), (0.0, 1.0));
        // Other scopes untouched.
        assert!(set.motion_of(&rt, &wkey("w2", "a")).x > 0.0);
        assert_eq!(set.ghosts_for(|(w, _)| w == "w2").len(), 1);
    }

    #[test]
    fn nudge_all_glides_survivors_home() {
        let (mut rt, mut set) = harness();
        set.enter(&mut rt, dur(), wkey("w", "a"), false);
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        // Removal nudges survivors down one slot (recreated: the
        // enter motion was swept after settling)...
        set.nudge_one(&mut rt, dur(), wkey("w", "a"), 40.0);
        rt.tick(Duration::from_millis(75));
        let mid = set.motion_of(&rt, &wkey("w", "a"));
        assert!(mid.y > 0.0, "mid {mid:?}");
        // ...then the next frame glides them home.
        set.settle_nudged(&mut rt);
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        let done = set.motion_of(&rt, &wkey("w", "a"));
        assert_eq!((done.x, done.y, done.opacity), (0.0, 0.0, 1.0));
    }

    #[test]
    fn nudge_all_restarts_tracked_settled_keys() {
        let (mut rt, mut set) = harness();
        set.enter(&mut rt, dur(), wkey("w", "a"), false);
        // No sweep: the settled enter motion is still tracked.
        rt.tick(Duration::from_millis(300));
        let n = set.nudge_all(&mut rt, dur(), 40.0);
        assert_eq!(n, 1);
        rt.tick(Duration::from_millis(75));
        let mid = set.motion_of(&rt, &wkey("w", "a"));
        assert!(mid.y > 0.0, "mid {mid:?}");
    }

    #[test]
    fn displaced_runs_alongside_exits() {
        let (mut rt, mut set) = harness();
        set.enter(&mut rt, dur(), wkey("w", "a"), false);
        set.enter(&mut rt, dur(), wkey("w", "b"), false);
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        // Remove one: its ghost exits while the survivor nudges.
        set.retire(&mut rt, dur(), wkey("w", "a"), 0, btn("a"));
        set.nudge_one(&mut rt, dur(), wkey("w", "b"), 40.0);
        rt.tick(Duration::from_millis(75));
        let exit = set.motion_of(&rt, &wkey("w", "a"));
        let survivor = set.motion_of(&rt, &wkey("w", "b"));
        assert!(exit.x < 0.0 && exit.opacity < 1.0, "exit {exit:?}");
        assert!(survivor.y > 0.0, "survivor {survivor:?}");
        assert_eq!(set.ghosts_for(|_| true).len(), 1);
    }

    #[test]
    fn sync_list_anims_diffs_old_and_new_trees() {
        let (mut rt, mut set) = harness();
        let dur = Duration::from_millis(150);
        let old = row(&["a", "b", "c"]);
        let new = row(&["a", "c", "d"]);
        sync_list_anims(&mut set, &mut rt, "w", Some(&old), &new, dur);
        // Added key enters; removed key ghosts at its old index.
        assert!(set.motion_of(&rt, &wkey("w", "d")).x > 0.0);
        let kept = set.motion_of(&rt, &wkey("w", "a"));
        assert_eq!((kept.x, kept.opacity), (0.0, 1.0));
        let ghosts = set.ghosts_for(|(w, _)| w == "w");
        assert_eq!(ghosts.len(), 1);
        assert_eq!((ghosts[0].index, ghosts[0].key.1.as_str()), (1, "b"));
        // First paint and shape flips settle instantly.
        sync_list_anims(&mut set, &mut rt, "w", None, &row(&["x"]), dur);
        let dropped = set.motion_of(&rt, &wkey("w", "d"));
        assert_eq!((dropped.x, dropped.opacity), (0.0, 1.0));
        assert!(set.ghosts_for(|_| true).is_empty());
    }
}
