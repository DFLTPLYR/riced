//! Host adapter for the `wayland-workspaces` extension.
//!
//! The `ext-workspace-v1` listener and its snapshot live in
//! `riced-extension-wayland-workspaces`. This module only adapts the host's
//! coalesced native wakeup to the extension's notify closure, so the core
//! `wayland.workspaces` table and the extension's `workspaces` service share a
//! single implementation.
pub use riced_extension_wayland_workspaces::{Workspace, WorkspaceCache};

/// Spawn the listener with the host's native change wakeup.
pub(crate) fn spawn(changes: super::changes::Changes) -> WorkspaceCache {
    WorkspaceCache::spawn(std::sync::Arc::new(move || changes.notify()))
}
