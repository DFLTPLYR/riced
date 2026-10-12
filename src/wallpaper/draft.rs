//! An editor-owned draft never mutates applied desktop configuration.
use super::raster::Sources;
use crate::config::{BackgroundConfig, BackgroundImage, ConfigPatch};

#[derive(Debug, Clone)]
pub(crate) struct Draft {
    pub recipe: BackgroundConfig,
    pub base: BackgroundConfig,
    pub selected_output: Option<String>,
    pub preview_outputs: Vec<super::OutputSpec>,
    pub sources: Sources,
    pub revision: u64,
    pub assets_revision: u64,
    pub loading: bool,
    pub preview_running: Option<u64>,
    pub applying: bool,
    pub error: Option<String>,
}

impl Draft {
    pub fn new(recipe: &BackgroundConfig) -> Self {
        Self {
            recipe: recipe.clone(),
            base: recipe.clone(),
            selected_output: None,
            preview_outputs: Vec::new(),
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
        self.recipe != self.base
    }

    pub fn patch(&mut self, patch: ConfigPatch) {
        match patch {
            ConfigPatch::WallpaperMode(mode) => {
                self.recipe.mode = mode;
                self.assets_revision += 1;
                self.loading = true;
            }
            ConfigPatch::MonitorWallpaper(wallpaper) => {
                if let Some(existing) = self
                    .recipe
                    .monitor
                    .iter_mut()
                    .find(|m| m.output == wallpaper.output)
                {
                    *existing = wallpaper;
                } else {
                    self.recipe.monitor.push(wallpaper);
                }
                self.assets_revision += 1;
                self.loading = true;
            }
            ConfigPatch::AddImage(image) => {
                self.recipe.image.push(image);
                self.assets_revision += 1;
                self.loading = true;
            }
            ConfigPatch::RemoveImage { index } => {
                if index < self.recipe.image.len() {
                    self.recipe.image.remove(index);
                }
                let paths: std::collections::HashSet<_> = self
                    .recipe
                    .image
                    .iter()
                    .map(BackgroundImage::local_path)
                    .collect();
                self.sources.retain(|path, _| paths.contains(path));
                self.assets_revision += 1;
                self.loading = true;
            }
            ConfigPatch::MoveImage { index, x, y } => {
                if let Some(image) = self.recipe.image.get_mut(index) {
                    image.x = x;
                    image.y = y;
                }
            }
            ConfigPatch::SetImageScale { index, scale } => {
                if let Some(image) = self.recipe.image.get_mut(index) {
                    image.scale = scale.max(0.01);
                }
            }
            ConfigPatch::SetImageSize {
                index,
                width,
                height,
            } => {
                if let Some(image) = self.recipe.image.get_mut(index) {
                    image.width = width.max(0.0);
                    image.height = height.max(0.0);
                }
            }
            ConfigPatch::ScaleImage { index, x, y, scale } => {
                if let Some(image) = self.recipe.image.get_mut(index) {
                    image.x = x;
                    image.y = y;
                    image.scale = scale.max(0.01);
                }
            }
            ConfigPatch::SetImageZ { index, z } => {
                if let Some(image) = self.recipe.image.get_mut(index) {
                    image.z = z;
                }
            }
            _ => return,
        }
        self.revision += 1;
        self.error = None;
    }
}
