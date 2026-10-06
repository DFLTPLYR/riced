//! Native iced list component with QML-`ListView`-style transitions.
//!
//! One [`ListView`] owns identity, motion, and ghosts for a single
//! animated list: widget cell rows key it `(widget, action)`,
//! notification stacks `(output, id)`. Callers feed key diffs through
//! [`ListView::update`] and render through [`ListView::items`]; the
//! component owns everything in between:
//!
//! - **enter** ([`ListView::on_entered`]): added keys fade/slide in
//!   from the transition's start state.
//! - **exit** ([`ListView::on_exit`]): removed keys become inert ghosts
//!   at their old index, fading/sliding out, then swept once settled.
//! - **displaced** ([`ListView::on_displaced`]): survivors whose index
//!   shifted glide along the list axis toward their new slot instead
//!   of jumping.
//!
//! Motion values ride [`ItemMotion`] over aura-anim; the view wraps
//! items in the [`motion`](super::motion) offset widget, so layout
//! never reflows mid-transition. Opacity stays style-level (iced 0.14
//! has no group-alpha primitive) — the `render` closure receives the
//! live [`ItemMotion`] and fades its own chrome.
//!
//! Honest limits: displaced distance is `index_delta × pitch`, and
//! `pitch` is a caller-supplied estimate (iced can't measure items in
//! view code). Mid-enter keys that shift keep their enter motion (no
//! nudge) rather than corrupting it.

use super::anim::{ENTER_OFFSET, ItemMotion};
use aura_anim::core::{
    runtime::{Motion, MotionRuntime},
    timing::Timing,
    tween::Tween,
};
use iced::Element;
use std::collections::HashMap;
use std::hash::Hash;
use std::time::Duration;

/// List direction: picks the displaced glide axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    Vertical,
    Horizontal,
}

/// One QML-style transition block: start/end [`ItemMotion`] states
/// plus an optional duration (`None` = the list's resolved duration).
///
/// Meaning of `from`/`to` depends on the slot: `on_entered` starts
/// motions AT `from` and runs toward `to` (default: settled);
/// `on_exit` runs the live motion TOWARD `to` (default: the exit
/// state); `on_displaced` ignores both (distance comes from the index
/// shift) and only uses the duration.
#[derive(Debug, Clone)]
pub(crate) struct Transition {
    pub from: ItemMotion,
    pub to: ItemMotion,
    pub duration: Option<Duration>,
}

impl Transition {
    /// Slide in/out along `x` with a full fade: enter starts at
    /// `(from_x, 0, transparent)` and settles; exit ends at
    /// `(from_x, 0, gone)`.
    pub fn slide_fade(from_x: f32) -> Self {
        Self {
            from: ItemMotion {
                x: from_x,
                y: 0.0,
                opacity: 0.0,
            },
            to: ItemMotion::settled(),
            duration: None,
        }
    }

    /// Exit twin of [`Transition::slide_fade`]: starts settled, ends at
    /// `(to_x, 0, gone)`.
    pub fn slide_fade_out(to_x: f32) -> Self {
        Self {
            from: ItemMotion::settled(),
            to: ItemMotion {
                x: to_x,
                y: 0.0,
                opacity: 0.0,
            },
            duration: None,
        }
    }
}

/// Item renderer for [`ListView::items`]: key, owned content, live
/// motion, and liveness (ghosts render inert) into an element.
/// Caller closures are usually `move` (copying the `&Plots` ref and
/// any `Copy` config) so the returned elements borrow only state that
/// outlives the view.
type ItemRender<'a, K, C, Message, Theme, Renderer> =
    dyn Fn(&K, C, ItemMotion, bool) -> Element<'a, Message, Theme, Renderer> + 'a;

/// A removed-but-still-visible item: last-known content plus its list
/// index, rendered inert (no clicks) until the exit motion settles.
#[derive(Debug, Clone)]
pub(crate) struct ListGhost<K, C> {
    pub key: K,
    pub index: usize,
    pub content: C,
}

/// Native animated list: keyed motions + ghosts + diff, rendered
/// through a caller closure. See the module docs for the QML mapping.
#[derive(Debug)]
pub(crate) struct ListView<K, C> {
    motions: HashMap<K, Motion<ItemMotion>>,
    ghosts: Vec<ListGhost<K, C>>,
    pitch: f32,
    duration_override: Option<Duration>,
    enter: Transition,
    exit: Transition,
    displaced: Transition,
    /// Set when a caller declares custom transitions, so per-call
    /// corner overrides (notification slide side) don't clobber them.
    enter_locked: bool,
}

impl<K, C> Default for ListView<K, C> {
    fn default() -> Self {
        Self::new(32.0)
    }
}

impl<K, C> ListView<K, C> {
    /// Empty list with default slide-fade transitions and no duration
    /// override. Set the row pitch (px per item, displaced math) and
    /// axis/enter direction per [`ListView::update`] call.
    pub fn new(pitch: f32) -> Self {
        Self {
            motions: HashMap::new(),
            ghosts: Vec::new(),
            pitch: pitch.max(1.0),
            duration_override: None,
            enter: Transition::slide_fade(ENTER_OFFSET),
            exit: Transition::slide_fade_out(-ENTER_OFFSET),
            displaced: Transition {
                from: ItemMotion::settled(),
                to: ItemMotion::settled(),
                duration: None,
            },
            enter_locked: false,
        }
    }

    /// Override the duration for every transition of this list
    /// (`None` = use the global speed passed to [`ListView::update`]).
    /// Per-list tuning hook: unused by the shipped lists (global speed
    /// applies), exercised in tests, available for future lists.
    #[allow(dead_code)]
    pub fn duration_override(mut self, duration: Duration) -> Self {
        self.duration_override = Some(duration);
        self
    }

    /// Transition for added items (`onEntered`). Per-list tuning hook:
    /// unused by the shipped lists (defaults apply), exercised in tests.
    #[allow(dead_code)]
    pub fn on_entered(mut self, transition: Transition) -> Self {
        self.enter = transition;
        self
    }

    /// Transition for removed items (`onExit`). Per-list tuning hook:
    /// unused by the shipped lists (defaults apply), exercised in tests.
    #[allow(dead_code)]
    pub fn on_exit(mut self, transition: Transition) -> Self {
        self.exit = transition;
        self
    }

    /// Transition for survivors that shift index (`onDisplaced`).
    /// Per-list tuning hook: unused by the shipped lists (defaults
    /// apply), exercised in tests.
    #[allow(dead_code)]
    pub fn on_displaced(mut self, transition: Transition) -> Self {
        self.displaced = transition;
        self
    }

    /// Replace all three transitions at once (Lua `transitions()`
    /// specs land here after parsing; see `top::parse_transitions`).
    /// `custom` locks the enter side so corner overrides don't apply.
    pub fn set_transitions(
        &mut self,
        enter: Transition,
        exit: Transition,
        displaced: Transition,
        custom: bool,
    ) {
        self.enter = enter;
        self.exit = exit;
        self.displaced = displaced;
        self.enter_locked = self.enter_locked || custom;
    }

    /// Current transition specs (Lua specs layer over these).
    pub fn enter_spec(&self) -> Transition {
        self.enter.clone()
    }

    /// Current exit spec (Lua specs layer over this).
    pub fn exit_spec(&self) -> Transition {
        self.exit.clone()
    }

    /// Current displaced spec (Lua specs layer over this).
    pub fn displaced_spec(&self) -> Transition {
        self.displaced.clone()
    }

    /// Set the enter transition's `from.x` (the notification list
    /// mirrors its slide side per anchored corner) while keeping the
    /// fade and `to`. No-op once a caller declared custom transitions.
    pub fn set_enter_from_x(&mut self, x: f32) {
        if !self.enter_locked {
            self.enter.from.x = x;
        }
    }
}

impl<K: Eq + Hash + Clone, C: Clone> ListView<K, C> {
    fn resolve(&self, slot: Option<Duration>, global: Duration) -> Timing {
        Timing::ease_out(slot.or(self.duration_override).unwrap_or(global))
    }

    /// Reconcile one frame of list diff: keys in `new_keys` but not
    /// `old_keys` enter (from the enter transition's start state);
    /// `removed` entries `(old index, key, content)` retire as ghosts;
    /// survivors whose index moved glide along `axis` by
    /// `index_delta × pitch` (displaced). Re-added mid-exit keys flip
    /// home and drop their ghost. First paints and shape flips should
    /// [`ListView::clear_scope`] first (caller-owned, like before).
    ///
    /// Eight params is the whole diff in one call (old/new keys +
    /// removed content + direction + timing) — splitting it would
    /// scatter the enter/exit/displaced pairing the component exists
    /// to keep together.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        runtime: &mut MotionRuntime,
        global_duration: Duration,
        axis: Axis,
        old_keys: &[K],
        new_keys: &[K],
        removed: &[(usize, K, C)],
    ) {
        let enter_timing = self.resolve(self.enter.duration, global_duration);
        let exit_timing = self.resolve(self.exit.duration, global_duration);
        let displaced_timing = self.resolve(self.displaced.duration, global_duration);
        // Exits first: ghosts claim their index before survivors move.
        for (index, key, content) in removed {
            let exit = self.exit.to.clone();
            match self.motions.get(key) {
                Some(m) => {
                    let _ = m.transition_to(exit, runtime);
                }
                None => {
                    let m = runtime.motion_with(ItemMotion::settled(), exit_timing);
                    let _ = m.transition_to(exit, runtime);
                    self.motions.insert(key.clone(), m);
                }
            }
            self.ghosts.retain(|g| &g.key != key);
            self.ghosts.push(ListGhost {
                key: key.clone(),
                index: *index,
                content: content.clone(),
            });
        }
        self.ghosts.sort_by_key(|g| g.index);
        // Entries: fresh keys start at the enter offset. Keys already
        // present keep their motion — except a re-notify racing a
        // ghost exit, which flips home and drops the ghost (the old
        // enter() path did the same on every arrival).
        for key in new_keys {
            if old_keys.contains(key) {
                if self.ghosts.iter().any(|g| &g.key == key) {
                    let home = self.enter.to.clone();
                    if let Some(m) = self.motions.get(key) {
                        let _ = m.transition_to(home, runtime);
                    }
                    self.ghosts.retain(|g| &g.key != key);
                }
                continue;
            }
            let start = self.enter.from.clone();
            let home = self.enter.to.clone();
            match self.motions.get(key) {
                Some(m) => {
                    let _ = m.transition_to(home, runtime);
                }
                None => {
                    let m = runtime.motion_with(start, enter_timing);
                    let _ = m.transition_to(home, runtime);
                    self.motions.insert(key.clone(), m);
                }
            }
            self.ghosts.retain(|g| &g.key != key);
        }
        // Displaced: survivors that moved index paint shifted from
        // their old slot, then glide home. Only keys below a removal
        // move (delta > 0 along the list); keys above are untouched.
        for (new_index, key) in new_keys.iter().enumerate() {
            let Some(old_index) = old_keys.iter().position(|k| k == key) else {
                continue;
            };
            if old_index == new_index {
                continue;
            }
            let delta = (old_index as f32 - new_index as f32) * self.pitch;
            let (dx, dy) = match axis {
                Axis::Vertical => (0.0, delta),
                Axis::Horizontal => (delta, 0.0),
            };
            let Some(m) = self.motions.get(key).cloned() else {
                // Swept long ago: recreate at the displaced offset,
                // gliding home (the nudge-then-settle pair in one).
                let at = ItemMotion {
                    x: dx,
                    y: dy,
                    opacity: 1.0,
                };
                let motion = runtime.motion_with(at, displaced_timing);
                let _ = motion.transition_to(ItemMotion::settled(), runtime);
                self.motions.insert(key.clone(), motion);
                continue;
            };
            if m.is_completed(runtime).unwrap_or(false)
                && let Ok(cur) = m.value(runtime)
            {
                let to = ItemMotion {
                    x: cur.x + dx,
                    y: cur.y + dy,
                    opacity: cur.opacity,
                };
                let _ = runtime.play(m, Tween::between(cur, to, displaced_timing));
            }
        }
    }

    /// Build the merged item list: live keys in order with ghosts
    /// inserted at their old indices (clamped). Content resolves owned
    /// per key (elements never borrow it — iced widgets own their
    /// data), each entry renders through `render(key, content, motion,
    /// live)` and shifts by its live `(x, y)` without disturbing
    /// layout; ghosts render inert. Settled items skip the wrapper
    /// (flat tree at rest).
    pub fn items<'a, Message, Theme, Renderer>(
        &self,
        runtime: &MotionRuntime,
        live_keys: &[K],
        content: &dyn Fn(&K) -> Option<C>,
        render: &ItemRender<'a, K, C, Message, Theme, Renderer>,
    ) -> Vec<Element<'a, Message, Theme, Renderer>>
    where
        Message: 'a,
        Theme: 'a,
        Renderer: iced::advanced::Renderer + 'a,
    {
        let mut out: Vec<Element<'a, Message, Theme, Renderer>> = Vec::new();
        for key in live_keys {
            let Some(node) = content(key) else {
                continue;
            };
            let motion = self.motion_of(runtime, key);
            let el = render(key, node, motion.clone(), true);
            out.push(super::motion::shifted(el, motion.x, motion.y, true));
        }
        for ghost in self.ghosts.iter() {
            let motion = self.motion_of(runtime, &ghost.key);
            let el = render(&ghost.key, ghost.content.clone(), motion.clone(), false);
            let el = super::motion::shifted(el, motion.x, motion.y, false);
            let at = ghost.index.min(out.len());
            out.push(el);
            let last = out.len() - 1;
            if at < last {
                let el = out.remove(last);
                out.insert(at, el);
            }
        }
        out
    }

    /// Current motion values for a key; settled when absent.
    pub fn motion_of(&self, runtime: &MotionRuntime, key: &K) -> ItemMotion {
        self.motions
            .get(key)
            .and_then(|m| m.value(runtime).ok())
            .map(|v| v.clamped())
            .unwrap_or_else(ItemMotion::settled)
    }

    /// Ghosts matching `pred`, in index order, for mask math and tests.
    pub fn ghosts_for(&self, pred: impl Fn(&K) -> bool) -> Vec<&ListGhost<K, C>> {
        let mut out: Vec<&ListGhost<K, C>> = self.ghosts.iter().filter(|g| pred(&g.key)).collect();
        out.sort_by_key(|g| g.index);
        out
    }

    /// Instant-settle every key matching `pred` (first paints, shape
    /// flips, output removal).
    pub fn clear_scope(&mut self, runtime: &mut MotionRuntime, pred: impl Fn(&K) -> bool) {
        let dead: Vec<K> = self.motions.keys().filter(|k| pred(k)).cloned().collect();
        for key in dead {
            self.drop_key(runtime, &key);
        }
    }

    fn drop_key(&mut self, runtime: &mut MotionRuntime, key: &K) {
        if let Some(m) = self.motions.remove(key) {
            let _ = runtime.remove(m);
        }
        self.ghosts.retain(|g| &g.key != key);
    }

    /// Glide every nudged axis offset back to rest (second half of the
    /// displaced pair): call on the frame after [`ListView::update`].
    pub fn settle_nudged(&self, runtime: &mut MotionRuntime) {
        for m in self.motions.values() {
            if let Ok(cur) = m.value(runtime)
                && (cur.x.abs() >= 0.5 || cur.y.abs() >= 0.5)
            {
                let _ = m.transition_to(
                    ItemMotion {
                        x: 0.0,
                        y: 0.0,
                        opacity: cur.opacity,
                    },
                    runtime,
                );
            }
        }
    }

    /// Drop settled motions (freeing runtime slots). Returns whether
    /// anything still animates.
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

    /// Read-only twin of [`ListView::sweep`] for shared borrows: whether
    /// anything still animates (settled ghosts count as idle).
    pub fn sweep_check(&self, runtime: &MotionRuntime) -> bool {
        self.motions
            .values()
            .any(|m| !m.is_completed(runtime).unwrap_or(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_anim::core::runtime::MotionRuntime;

    fn harness() -> (MotionRuntime, ListView<String, String>) {
        (MotionRuntime::new(), ListView::new(40.0))
    }

    fn dur() -> Duration {
        Duration::from_millis(150)
    }

    fn keys(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn enter_runs_slide_fade_home() {
        let (mut rt, mut set) = harness();
        set.update(&mut rt, dur(), Axis::Vertical, &[], &keys(&["a"]), &[]);
        let start = set.motion_of(&rt, &"a".to_string());
        assert_eq!((start.x, start.opacity), (16.0, 0.0));
        rt.tick(Duration::from_millis(75));
        let mid = set.motion_of(&rt, &"a".to_string());
        assert!(mid.x > 0.0 && mid.x < 16.0, "mid {mid:?}");
        assert!(mid.opacity > 0.0 && mid.opacity < 1.0, "mid {mid:?}");
        rt.tick(Duration::from_millis(200));
        assert!(!set.sweep(&mut rt));
        let done = set.motion_of(&rt, &"a".to_string());
        assert_eq!((done.x, done.y, done.opacity), (0.0, 0.0, 1.0));
    }

    #[test]
    fn custom_enter_transition_sets_start_state() {
        let (mut rt, mut set) = harness();
        set = set.on_entered(Transition {
            from: ItemMotion {
                x: 0.0,
                y: -24.0,
                opacity: 0.0,
            },
            to: ItemMotion::settled(),
            duration: None,
        });
        // The transition fully owns the start state now.
        set.update(&mut rt, dur(), Axis::Vertical, &[], &keys(&["a"]), &[]);
        let start = set.motion_of(&rt, &"a".to_string());
        assert_eq!((start.x, start.y, start.opacity), (0.0, -24.0, 0.0));
    }

    #[test]
    fn exit_ghosts_then_leaves() {
        let (mut rt, mut set) = harness();
        set.update(&mut rt, dur(), Axis::Vertical, &[], &keys(&["a", "b"]), &[]);
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        set.update(
            &mut rt,
            dur(),
            Axis::Vertical,
            &keys(&["a", "b"]),
            &keys(&["b"]),
            &[(0, "a".to_string(), "A".to_string())],
        );
        assert_eq!(set.ghosts_for(|_| true).len(), 1);
        let mid = set.motion_of(&rt, &"a".to_string());
        // Exit starts at rest, then heads toward -x + transparent.
        assert_eq!((mid.x, mid.opacity), (0.0, 1.0));
        rt.tick(Duration::from_millis(75));
        let mid = set.motion_of(&rt, &"a".to_string());
        assert!(mid.x < 0.0 && mid.opacity < 1.0, "mid {mid:?}");
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        assert!(set.ghosts_for(|_| true).is_empty());
    }

    #[test]
    fn displaced_moves_only_survivors_below() {
        let (mut rt, mut set) = harness();
        set.update(
            &mut rt,
            dur(),
            Axis::Vertical,
            &[],
            &keys(&["a", "b", "c"]),
            &[],
        );
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        // Remove the head: b and c shift up one slot each.
        set.update(
            &mut rt,
            dur(),
            Axis::Vertical,
            &keys(&["a", "b", "c"]),
            &keys(&["b", "c"]),
            &[(0, "a".to_string(), "A".to_string())],
        );
        // b/c were swept after settling: recreated at the offset.
        rt.tick(Duration::from_millis(75));
        let b = set.motion_of(&rt, &"b".to_string());
        let c = set.motion_of(&rt, &"c".to_string());
        assert!(b.y > 0.0, "b {b:?}");
        assert!(c.y > 0.0, "c {c:?}");
        set.settle_nudged(&mut rt);
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        for k in ["b", "c"] {
            let done = set.motion_of(&rt, &k.to_string());
            assert_eq!((done.x, done.y, done.opacity), (0.0, 0.0, 1.0));
        }
    }

    #[test]
    fn horizontal_lists_displace_along_x() {
        let (mut rt, mut set) = harness();
        set.update(
            &mut rt,
            dur(),
            Axis::Horizontal,
            &[],
            &keys(&["a", "b"]),
            &[],
        );
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        set.update(
            &mut rt,
            dur(),
            Axis::Horizontal,
            &keys(&["a", "b"]),
            &keys(&["b"]),
            &[(0, "a".to_string(), "A".to_string())],
        );
        rt.tick(Duration::from_millis(75));
        let b = set.motion_of(&rt, &"b".to_string());
        assert!(b.x > 0.0 && b.y == 0.0, "b {b:?}");
    }

    #[test]
    fn items_merge_ghosts_at_index() {
        use iced::widget::text;
        let (mut rt, mut set) = harness();
        set.update(
            &mut rt,
            dur(),
            Axis::Vertical,
            &[],
            &keys(&["a", "b", "c"]),
            &[],
        );
        set.update(
            &mut rt,
            dur(),
            Axis::Vertical,
            &keys(&["a", "b", "c"]),
            &keys(&["a", "c"]),
            &[(1, "b".to_string(), "B".to_string())],
        );
        let content = |k: &String| Some(k.clone());
        let render = &|_k: &String, c: String, _m: ItemMotion, _live: bool| text(c).into();
        let els = set.items::<String, iced::Theme, iced::Renderer>(
            &rt,
            &keys(&["a", "c"]),
            &content,
            render,
        );
        assert_eq!(els.len(), 3);
    }

    #[test]
    fn custom_exit_and_displaced_transitions_apply() {
        let (mut rt, mut set) = harness();
        // QML-style: exit flies to x = -200, displaced runs long.
        set = set
            .on_exit(Transition {
                from: ItemMotion::settled(),
                to: ItemMotion {
                    x: -200.0,
                    y: 0.0,
                    opacity: 0.0,
                },
                duration: None,
            })
            .on_displaced(Transition {
                from: ItemMotion::settled(),
                to: ItemMotion::settled(),
                duration: Some(Duration::from_millis(500)),
            });
        set.update(&mut rt, dur(), Axis::Vertical, &[], &keys(&["a", "b"]), &[]);
        rt.tick(Duration::from_millis(300));
        assert!(!set.sweep(&mut rt));
        set.update(
            &mut rt,
            dur(),
            Axis::Vertical,
            &keys(&["a", "b"]),
            &keys(&["b"]),
            &[(0, "a".to_string(), "A".to_string())],
        );
        rt.tick(Duration::from_millis(75));
        // Exit heads toward the custom -200 target...
        let exit = set.motion_of(&rt, &"a".to_string());
        assert!(exit.x < -16.0, "exit {exit:?}");
        // ...while the survivor glides on the long displaced timing.
        let survivor = set.motion_of(&rt, &"b".to_string());
        assert!(survivor.y > 0.0, "survivor {survivor:?}");
        rt.tick(Duration::from_millis(200));
        assert!(set.motion_of(&rt, &"b".to_string()).y > 0.0);
    }

    #[test]
    fn duration_override_beats_global() {
        let (mut rt, mut set) = harness();
        set = set.duration_override(Duration::from_millis(600));
        set.update(&mut rt, dur(), Axis::Vertical, &[], &keys(&["a"]), &[]);
        // Global 150ms would settle by now; the 600ms override runs on.
        rt.tick(Duration::from_millis(300));
        let mid = set.motion_of(&rt, &"a".to_string());
        assert!(mid.x > 0.0, "mid {mid:?}");
    }
}
