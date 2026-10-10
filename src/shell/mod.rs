//! Wayland window lifecycle, host messages, and per-surface behavior.
mod animation;
mod catalog;
mod context;
mod desktop;
pub mod events;
mod input;
mod jobs;
mod notifications;
pub mod screens;
pub mod state;
mod windows;
pub use events::*;
pub use state::{Plots, redraw_scope};
