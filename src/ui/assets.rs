//! Full-resolution decoded-image reuse. Only pixels are cached, not UI elements.
use iced::widget::image::Handle;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

type Revision = Option<crate::shared::files::FileStamp>;
struct Entry {
    revision: Revision,
    image: Option<(u32, u32, Handle)>,
    bytes: usize,
    used: u64,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<PathBuf, Entry>,
    clock: u64,
}
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));

pub(crate) fn release_unused() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.entries.retain(|_, entry| match &entry.image {
            Some((_, _, Handle::Rgba { pixels, .. })) => !pixels.is_unique(),
            _ => false,
        });
    }
}

pub(crate) fn image(path: &Path) -> Option<Handle> {
    let revision = crate::shared::files::stamp(path);
    {
        let mut cache = CACHE.lock().ok()?;
        cache.clock += 1;
        let clock = cache.clock;
        if let Some(entry) = cache.entries.get_mut(path)
            && entry.revision == revision
        {
            entry.used = clock;
            return entry.image.as_ref().map(|(_, _, handle)| handle.clone());
        }
    }
    let decoded = crate::config::decode_handle(path);
    let handle = decoded.as_ref().map(|(_, _, handle)| handle.clone());
    let bytes = decoded.as_ref().map_or(0, |(width, height, _)| {
        (*width as usize)
            .saturating_mul(*height as usize)
            .saturating_mul(4)
    });
    if let Ok(mut cache) = CACHE.lock() {
        let clock = cache.clock;
        cache.entries.insert(
            path.to_path_buf(),
            Entry {
                revision,
                image: decoded,
                bytes,
                used: clock,
            },
        );
        // Evict old references, never shrink pixels. A single oversized image
        // remains full resolution rather than being decoded on every frame.
        while cache.entries.len() > 1
            && (cache.entries.len() > 32
                || cache
                    .entries
                    .values()
                    .map(|entry| entry.bytes)
                    .sum::<usize>()
                    > 128 * 1024 * 1024)
        {
            let oldest = cache
                .entries
                .iter()
                .filter(|(key, _)| key.as_path() != path)
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                cache.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
    handle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_releases_only_cache_owned_pixels_and_preserves_live_handles() {
        let dir =
            std::env::temp_dir().join(format!("riced-ui-assets-lifetime-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("source.png");
        image::RgbaImage::from_pixel(17, 9, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let active = image(&path).unwrap();
        let id = active.id();
        release_unused();
        assert_eq!(image(&path).unwrap().id(), id);
        drop(active);
        release_unused();
        assert_ne!(
            image(&path).unwrap().id(),
            id,
            "unused cache pixels must be released and decoded anew on demand"
        );
        std::fs::remove_dir_all(dir).unwrap();
        release_unused();
    }

    #[test]
    fn cache_reuses_original_resolution_and_recovers_after_source_replacement() {
        let dir = std::env::temp_dir().join(format!("riced-ui-assets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("source.png");
        assert!(image(&path).is_none());
        image::RgbaImage::from_pixel(7, 3, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let first = image(&path).unwrap();
        assert_eq!(first.id(), image(&path).unwrap().id());
        if let Handle::Rgba { width, height, .. } = first {
            assert_eq!((width, height), (7, 3));
        } else {
            panic!("decoded pixels");
        }
        image::RgbaImage::from_pixel(9, 4, image::Rgba([40, 50, 60, 255]))
            .save(&path)
            .unwrap();
        let changed = image(&path).unwrap();
        assert_ne!(first.id(), changed.id());
        std::fs::remove_file(&path).unwrap();
        assert!(image(&path).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
