//! Shared motion primitives for animated lists.
//!
//! The QML-`ListView` semantics themselves (enter/exit/displaced,
//! ghosts, diffing) live in [`super::listview::ListView`], built on top
//! of [`aura_anim`](https://crates.io/crates/aura-anim). This module
//! keeps the pieces every list shares:
//!
//! - [`ItemMotion`]: per-item `(x, y, opacity)` targets. Enter runs
//!   `+x -> 0` with `0 -> 1` fade; exit runs `0 -> -x` with `1 -> 0`
//!   fade; survivors glide toward their new slot (`displaced`).
//! - [`AnimRuntime`]: the shared [`MotionRuntime`], ticked by the 16ms
//!   animation subscription while anything moves.
//! - [`ENTER_OFFSET`]: default slide travel (px).
//!
//! Honest limits (iced 0.14 gives us no group-alpha primitive and no
//! FLIP layout): opacity rides style alpha (see `layers::motion`), and
//! displaced distance is an index-delta × pitch estimate. Durations
//! follow the global [`crate::config::AnimationSpeed`] unless a list
//! overrides them.
use aura_anim::core::{macros::Animatable, runtime::MotionRuntime};
use std::fmt;

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

    /// Snap runaway values back into range (keeps layout offsets and
    /// alpha finite for the renderer).
    pub fn clamped(self) -> Self {
        Self {
            x: self.x.clamp(-4096.0, 4096.0),
            y: self.y.clamp(-4096.0, 4096.0),
            opacity: self.opacity.clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_is_on_slot_and_opaque() {
        let s = ItemMotion::settled();
        assert_eq!((s.x, s.y, s.opacity), (0.0, 0.0, 1.0));
    }

    #[test]
    fn clamped_snaps_finite_ranges() {
        let m = ItemMotion {
            x: f32::INFINITY,
            y: -1e10,
            opacity: 2.0,
        }
        .clamped();
        assert_eq!((m.x, m.y, m.opacity), (4096.0, -4096.0, 1.0));
    }
}
