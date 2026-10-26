//! Host adapter for the `wayland-toplevels` extension.
//!
//! The `ext-foreign-toplevel-list-v1` listener and its snapshot live in
//! `riced-extension-wayland-toplevels`. This module only adapts the host's
//! coalesced native wakeup to the extension's notify closure, so the core
//! `wayland.toplevels` table and the extension's `toplevels` service share a
//! single implementation.
pub use riced_extension_wayland_toplevels::{Toplevel, ToplevelCache};

/// Spawn the listener with the host's native change wakeup.
pub(crate) fn spawn(changes: super::changes::Changes) -> ToplevelCache {
    ToplevelCache::spawn(std::sync::Arc::new(move || changes.notify()))
}
