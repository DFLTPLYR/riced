//! Quality-preserving wallpaper snapshots and lossless, derived per-output tiles.
pub(crate) mod draft;
pub(crate) mod raster;
use crate::config::{BackgroundConfig, BackgroundImage, WallpaperMode};
use iced::{widget::image::Handle, window};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use raster::Sources;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSpec {
    pub key: String,
    pub rect: (f32, f32, f32, f32),
    pub pixels: (u32, u32),
}

pub(crate) fn outputs(infos: &HashMap<OutputId, OutputInfo>) -> Vec<(OutputId, OutputSpec)> {
    let mut specs: Vec<_> = infos
        .iter()
        .map(|(id, info)| {
            let rect = crate::shared::geometry::output_geometry(info);
            let pixels = info
                .modes
                .iter()
                .find(|mode| mode.current)
                .map(|mode| {
                    crate::shared::geometry::oriented_pixels(mode.dimensions, info.transform)
                })
                .unwrap_or_else(|| {
                    (
                        (rect.2 * info.scale_factor.max(1) as f32).ceil().max(1.0) as u32,
                        (rect.3 * info.scale_factor.max(1) as f32).ceil().max(1.0) as u32,
                    )
                });
            (
                *id,
                OutputSpec {
                    key: info
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("output-{}", id.0)),
                    rect,
                    pixels,
                },
            )
        })
        .collect();
    specs.sort_by(|a, b| a.1.key.cmp(&b.1.key));
    specs
}

pub(crate) fn source_images(
    recipe: &BackgroundConfig,
    outputs: &[OutputSpec],
) -> Vec<BackgroundImage> {
    match recipe.mode {
        WallpaperMode::FreeForm => recipe.image.clone(),
        WallpaperMode::PerMonitor => recipe
            .monitor
            .iter()
            .filter(|m| !m.path.is_empty() && outputs.iter().any(|spec| spec.key == m.output))
            .map(|m| BackgroundImage {
                path: m.path.clone(),
                ..Default::default()
            })
            .collect(),
    }
}

/// Headless theme generation uses the last applied monitor geometry.
pub(crate) fn cached_outputs() -> Vec<OutputSpec> {
    std::fs::read(cache_dir().join("applied.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
        .filter(|manifest| manifest.version == 2)
        .map(|manifest| manifest.tiles.into_iter().map(|tile| tile.spec).collect())
        .unwrap_or_default()
}

/// Commit the derived manifest and editable recipe, restoring the previous
/// manifest if configuration persistence fails. Originals are never rewritten.
pub(crate) fn commit(
    config: &crate::config::Config,
    images: &BackgroundConfig,
    batch: &Batch,
    dir: &Path,
    config_path: &Path,
) -> Result<(crate::config::Config, Option<std::time::SystemTime>), String> {
    let previous = std::fs::read(dir.join("applied.json")).ok();
    save_manifest(dir, images, batch)?;
    let mut next = config.clone();
    next.background = images.clone();
    match next.save_atomic_to(config_path) {
        Ok(mtime) => Ok((next, mtime)),
        Err(error) => {
            if let Some(previous) = previous {
                let _ = std::fs::write(dir.join("applied.json"), previous);
            } else {
                let _ = std::fs::remove_file(dir.join("applied.json"));
            }
            Err(error)
        }
    }
}

pub(crate) fn prune(dir: &Path, images: &[BackgroundImage], tiles: &HashMap<OutputId, Tile>) {
    let mut keep: std::collections::HashSet<PathBuf> = tiles
        .values()
        .filter_map(|tile| tile.path.parent().map(Path::to_path_buf))
        .collect();
    for image in images {
        if let Some(parent) = image.local_path().parent() {
            keep.insert(parent.to_path_buf());
        }
    }
    let keep: Vec<_> = keep
        .into_iter()
        .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
        .collect();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let resolved = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if name.len() == 16
                && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                && path.is_dir()
                && !keep.iter().any(|parent| parent.starts_with(&resolved))
            {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("riced/wallpapers")
}

#[derive(Debug, Clone)]
pub struct Tile {
    pub spec: OutputSpec,
    pub path: PathBuf,
    pub handle: Handle,
}

#[derive(Debug, Clone)]
pub struct Batch {
    pub tiles: HashMap<OutputId, Tile>,
}

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub generation: u64,
    pub editor: Option<(window::Id, u64)>,
    pub images: BackgroundConfig,
    pub base: BackgroundConfig,
    pub outputs: Vec<(OutputId, OutputSpec)>,
    pub sources: Sources,
}

#[derive(Debug, Default)]
pub(crate) struct State {
    pub applied: HashMap<OutputId, Tile>,
    pub generation: u64,
    pub pending: Option<Request>,
    pub running: Option<u64>,
    pub layout: BackgroundConfig,
    pub targets: Vec<(OutputId, OutputSpec)>,
}

impl State {
    pub(crate) fn publish_request(&mut self, request: &Request, batch: Batch) -> Publication {
        let pixels_changed = self.publish(batch);
        self.layout = request.images.clone();
        self.targets = request.outputs.clone();
        Publication {
            pixels_changed,
            regenerate_theme: request.editor.is_some(),
        }
    }

    /// Stable image IDs let the renderer reuse textures for identical pixels.
    /// Metadata and derived paths still advance to the accepted snapshot.
    pub(crate) fn publish(&mut self, mut batch: Batch) -> bool {
        let mut changed = self.applied.len() != batch.tiles.len();
        for (output, tile) in &mut batch.tiles {
            match self.applied.get(output) {
                Some(previous) if same_pixels(&previous.handle, &tile.handle) => {
                    tile.handle = previous.handle.clone();
                    changed |= previous.spec != tile.spec;
                }
                _ => changed = true,
            }
        }
        self.applied = batch.tiles;
        changed
    }
}

pub(crate) struct Publication {
    pub pixels_changed: bool,
    pub regenerate_theme: bool,
}

fn same_pixels(left: &Handle, right: &Handle) -> bool {
    match (left, right) {
        (
            Handle::Rgba {
                width: lw,
                height: lh,
                pixels: lp,
                ..
            },
            Handle::Rgba {
                width: rw,
                height: rh,
                pixels: rp,
                ..
            },
        ) => lw == rw && lh == rh && lp == rp,
        _ => left.id() == right.id(),
    }
}

#[derive(Serialize, Deserialize)]
struct SavedTile {
    spec: OutputSpec,
    path: PathBuf,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    images: BackgroundConfig,
    tiles: Vec<SavedTile>,
}

fn load_cached(
    dir: &Path,
    images: &BackgroundConfig,
    outputs: &[(OutputId, OutputSpec)],
) -> Option<Batch> {
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(dir.join("applied.json")).ok()?).ok()?;
    if manifest.version != 2 || manifest.images != *images {
        return None;
    }
    let mut tiles = HashMap::new();
    for (id, spec) in outputs {
        let saved = manifest.tiles.iter().find(|tile| tile.spec == *spec)?;
        // Derived paths must remain inside this cache, even for a stale manifest.
        if !saved.path.starts_with(dir) {
            return None;
        }
        let source = raster::Source::load(&saved.path).ok()?;
        if (source.width, source.height) != spec.pixels {
            return None;
        }
        tiles.insert(
            *id,
            Tile {
                spec: spec.clone(),
                path: saved.path.clone(),
                handle: source.handle.clone(),
            },
        );
    }
    Some(Batch { tiles })
}

pub(crate) fn bake(request: &Request, dir: &Path) -> Result<Batch, String> {
    if request.outputs.is_empty() {
        return Err("No outputs are available to apply wallpaper".into());
    }
    if request.images.mode == WallpaperMode::FreeForm && request.images.image.is_empty() {
        return Ok(Batch {
            tiles: HashMap::new(),
        });
    }
    if request.editor.is_none()
        && let Some(batch) = load_cached(dir, &request.images, &request.outputs)
    {
        return Ok(batch);
    }
    // Disconnected assignments are retained without requiring their sources.
    let specs: Vec<_> = request
        .outputs
        .iter()
        .map(|(_, spec)| spec.clone())
        .collect();
    let active = source_images(&request.images, &specs);
    let sources = raster::load_sources(&active, request.sources.clone())?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    2_u32.hash(&mut hash);
    serde_json::to_vec(&request.images)
        .map_err(|error| error.to_string())?
        .hash(&mut hash);
    for (_, spec) in &request.outputs {
        serde_json::to_vec(spec)
            .map_err(|error| error.to_string())?
            .hash(&mut hash);
    }
    let mut paths: Vec<_> = sources.keys().collect();
    paths.sort();
    for path in paths {
        path.hash(&mut hash);
        sources[path].width.hash(&mut hash);
        sources[path].height.hash(&mut hash);
        sources[path].pixels.hash(&mut hash);
    }
    let generation_dir = dir.join(format!("{:016x}", hash.finish()));
    std::fs::create_dir_all(&generation_dir).map_err(|error| error.to_string())?;
    let mut tiles = HashMap::new();
    for (index, (id, spec)) in request.outputs.iter().enumerate() {
        let canvas = raster::compose_output(spec, &request.images, &sources)?;
        let path = generation_dir.join(format!("output-{index}.png"));
        let temporary = generation_dir.join(format!("output-{index}.tmp.png"));
        canvas.save(&temporary).map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
        let handle = Handle::from_rgba(
            canvas.width(),
            canvas.height(),
            bytes::Bytes::from(canvas.into_raw()),
        );
        tiles.insert(
            *id,
            Tile {
                spec: spec.clone(),
                path,
                handle,
            },
        );
    }
    Ok(Batch { tiles })
}

pub(crate) fn save_manifest(
    dir: &Path,
    images: &BackgroundConfig,
    batch: &Batch,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let manifest = Manifest {
        version: 2,
        images: images.clone(),
        tiles: batch
            .tiles
            .values()
            .map(|tile| SavedTile {
                spec: tile.spec.clone(),
                path: tile.path.clone(),
            })
            .collect(),
    };
    let temporary = dir.join("applied.tmp.json");
    std::fs::write(
        &temporary,
        serde_json::to_vec(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    std::fs::rename(temporary, dir.join("applied.json")).map_err(|error| error.to_string())
}
