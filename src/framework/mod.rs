//! Project conventions and owned surface descriptions.
mod layer;
mod project;
pub(crate) mod tasks;

pub(crate) use layer::{LayerRoot, Surface, install};
pub(crate) use project::{AppConfig, Project, deserialize_apps};
