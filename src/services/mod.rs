//! Namespaced Lua services (`system.*`, `theme.*`, `notifications.*`,
//! `wayland.*`, `systemtray.*`).
//!
//! Services publish owned data and opaque image tokens at Lua entries;
//! no borrowed Rust references cross into Lua. Native Wayland listeners
//! wake the UI on cache changes, bypassing widget refresh intervals.
//! Wayland namespace/list tables update in place so captured list references
//! stay current; rows are snapshot values.
mod bar;
pub(crate) mod changes;
mod notifications;
mod registry;
pub(crate) mod state;
mod system;
mod theme;
mod toplevels;
mod tray;
mod wayland;
mod workspaces;

pub use bar::publish as publish_bar;
pub use registry::{ServiceCtx, publish_all};
pub use toplevels::{Toplevel, ToplevelCache};
pub use workspaces::{Workspace, WorkspaceCache};
pub(crate) mod gpu;
