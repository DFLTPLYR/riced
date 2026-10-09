//! Namespaced Lua services (`system.*`, `theme.*`, `notifications.*`,
//! `wayland.*`).
//!
//! Each service publishes one read-only snapshot table per widget tick
//! (same pattern the old `sysinfo`/`gfxinfo` globals used): plain owned
//! data, no handles or Rust references cross into Lua, and no new
//! sandbox permissions are needed. Slow or event-driven work stays out
//! of the tick path — see [`toplevels::ToplevelCache`], whose listener
//! thread blocks on the compositor socket while ticks just clone the
//! latest snapshot.
mod bar;
mod notifications;
mod registry;
mod system;
mod theme;
mod toplevels;
mod wayland;
mod workspaces;

pub use bar::publish as publish_bar;
pub use registry::{ServiceCtx, publish_all};
pub use toplevels::{Toplevel, ToplevelCache};
pub use workspaces::{Workspace, WorkspaceCache};
pub(crate) mod gpu;
