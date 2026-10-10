//! Discovered widget definitions and library change tracking.
use crate::config::WidgetDefinition;
use std::time::SystemTime;

#[derive(Debug)]
pub(crate) struct WidgetCatalog {
    pub definitions: Vec<WidgetDefinition>,
    pub directory_revision: Option<SystemTime>,
    pub library_revision: Option<SystemTime>,
    pub retired_file_warned: bool,
}

impl WidgetCatalog {
    pub(crate) fn new(definitions: Vec<WidgetDefinition>) -> Self {
        Self {
            definitions,
            directory_revision: crate::config::widgets_dir_mtime(),
            library_revision: crate::config::components_mtime(),
            retired_file_warned: false,
        }
    }

    pub(crate) fn directory_changed(&mut self) -> bool {
        let revision = crate::config::widgets_dir_mtime();
        if revision == self.directory_revision {
            return false;
        }
        self.directory_revision = revision;
        true
    }

    pub(crate) fn library_changed(&mut self) -> bool {
        let Some(revision) = crate::config::poll_components(&self.library_revision) else {
            return false;
        };
        self.library_revision = revision;
        true
    }
}
