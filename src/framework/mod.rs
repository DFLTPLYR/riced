//! Project conventions and owned surface descriptions.
pub(crate) mod dialog;
pub(crate) mod extensions;
mod layer;
mod project;
pub(crate) mod storage;
pub(crate) mod tasks;

pub(crate) use layer::{LayerRoot, Surface, install};
pub(crate) use project::{
    AppConfig, ExtensionConfig, Project, deserialize_apps, deserialize_extensions,
};
