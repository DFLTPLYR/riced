//! Project conventions and owned surface descriptions.
pub(crate) mod dialog;
mod layer;
mod project;
pub(crate) mod storage;
pub(crate) mod tasks;

pub(crate) use layer::{LayerRoot, Surface, install};
pub(crate) use project::{AppConfig, Project, deserialize_apps};
