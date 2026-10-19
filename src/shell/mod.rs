//! Wayland window lifecycle, host messages, and per-surface behavior.
mod actions;
mod animation;
mod catalog;
mod context;
mod desktop;
mod desktop_widgets;
pub mod events;
mod input;
mod jobs;
mod notifications;
pub mod screens;
pub mod state;
mod wallpaper;
mod windows;
pub use events::*;
pub use state::{Plots, redraw_scope};
pub(crate) mod overlay;
