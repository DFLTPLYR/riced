//! Namespaced Lua services (`system.*`, `theme.*`, `notifications.*`,
//! `wayland.*`).
//!
//! Each service publishes one read-only snapshot table per widget tick
//! (same pattern the old `sysinfo`/`gfxinfo` globals used): plain owned
//! data, no handles or Rust references cross into Lua, and no new
//! sandbox permissions are needed. Slow work stays out — see
//! [`hypr::HyprCache`], which is refreshed once per tick and shared by
//! every widget state.
mod hypr;
mod notifications;
mod registry;
mod system;
mod theme;
mod wayland;

pub use hypr::HyprCache;
pub use registry::{ServiceCtx, publish_all};
