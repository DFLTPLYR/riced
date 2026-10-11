//! An editor-owned draft never mutates applied desktop configuration.
use super::raster::Sources;
use crate::config::{BackgroundImage, ConfigPatch};

#[derive(Debug, Clone)]
pub(crate) struct Draft {
    pub images: Vec<BackgroundImage>,
    pub base: Vec<BackgroundImage>,
    pub sources: Sources,
    pub revision: u64,
    pub assets_revision: u64,
    pub loading: bool,
    pub preview_running: Option<u64>,
    pub applying: bool,
    pub error: Option<String>,
}

impl Draft {
    pub fn new(images: &[BackgroundImage]) -> Self {
        Self {
            images: images.to_vec(),
            base: images.to_vec(),
            sources: Sources::new(),
            revision: 0,
            assets_revision: 0,
            loading: true,
            preview_running: None,
            applying: false,
            error: None,
        }
    }
    pub fn dirty(&self) -> bool {
        self.images != self.base
    }

    pub fn patch(&mut self, patch: ConfigPatch) {
        match patch {
            ConfigPatch::AddImage(image) => {
                self.images.push(image);
                self.assets_revision += 1;
                self.loading = true;
            }
            ConfigPatch::RemoveImage { index } => {
                if index < self.images.len() {
                    self.images.remove(index);
                }
                let paths: std::collections::HashSet<_> = self
                    .images
                    .iter()
                    .map(BackgroundImage::local_path)
                    .collect();
                self.sources.retain(|path, _| paths.contains(path));
                self.assets_revision += 1;
                self.loading = true;
            }
            ConfigPatch::MoveImage { index, x, y } => {
                if let Some(image) = self.images.get_mut(index) {
                    image.x = x;
                    image.y = y;
                }
            }
            ConfigPatch::SetImageScale { index, scale } => {
                if let Some(image) = self.images.get_mut(index) {
                    image.scale = scale.max(0.01);
                }
            }
            ConfigPatch::SetImageSize {
                index,
                width,
                height,
            } => {
                if let Some(image) = self.images.get_mut(index) {
                    image.width = width.max(0.0);
                    image.height = height.max(0.0);
                }
            }
            ConfigPatch::ScaleImage { index, x, y, scale } => {
                if let Some(image) = self.images.get_mut(index) {
                    image.x = x;
                    image.y = y;
                    image.scale = scale.max(0.01);
                }
            }
            ConfigPatch::SetImageZ { index, z } => {
                if let Some(image) = self.images.get_mut(index) {
                    image.z = z;
                }
            }
            _ => return,
        }
        self.revision += 1;
        self.error = None;
    }
}
