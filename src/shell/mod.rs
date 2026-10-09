//! Wayland window lifecycle, host messages, and per-surface behavior.
mod context;
pub mod events;
pub mod screens;
pub mod state;
pub use events::*;
pub use state::{Plots, redraw_scope};
