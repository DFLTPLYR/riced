//! Wallpaper rasterization shared by generation and shell painting.
use crate::config::{BackgroundImage, Config};
use std::path::PathBuf;

/// Borrow already-resident applied pixels; theme sampling never shrinks the
/// displayed wallpaper or needs another full-size decoded frame.
#[derive(Debug, Clone)]
pub(crate) struct PixelView {
    pub width: u32,
    pub height: u32,
    pub pixels: bytes::Bytes,
}

enum View<'a> {
    Rgb(&'a image::RgbImage),
    Rgba(&'a PixelView),
}
impl View<'_> {
    fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Rgb(image) => image.dimensions(),
            Self::Rgba(image) => (image.width, image.height),
        }
    }
    fn pixel(&self, x: u32, y: u32) -> image::Rgb<u8> {
        match self {
            Self::Rgb(image) => *image.get_pixel(x, y),
            Self::Rgba(image) => {
                let offset = ((y as usize * image.width as usize) + x as usize) * 4;
                crate::wallpaper::raster::rgb_over_black(&image.pixels[offset..offset + 4])
            }
        }
    }
}

struct Strip<'a> {
    tiles: Vec<(u32, View<'a>)>,
    width: u32,
    height: u32,
}
impl image::GenericImageView for Strip<'_> {
    type Pixel = image::Rgb<u8>;
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn get_pixel(&self, x: u32, y: u32) -> Self::Pixel {
        for (start, tile) in &self.tiles {
            let (width, height) = tile.dimensions();
            if x >= *start && x - *start < width {
                return if y < height {
                    tile.pixel(x - *start, y)
                } else {
                    image::Rgb([0, 0, 0])
                };
            }
        }
        image::Rgb([0, 0, 0])
    }
}

fn sample_strip<'a>(views: impl IntoIterator<Item = View<'a>>) -> Option<image::RgbImage> {
    let mut strip = Strip {
        tiles: Vec::new(),
        width: 0,
        height: 0,
    };
    for view in views {
        let (width, height) = view.dimensions();
        if width == 0 || height == 0 {
            continue;
        }
        let start = strip.width;
        strip.width = strip.width.checked_add(width)?;
        strip.height = strip.height.max(height);
        strip.tiles.push((start, view));
    }
    if strip.width == 0 || strip.height == 0 {
        return None;
    }
    Some(image::imageops::resize(
        &strip,
        128,
        128,
        image::imageops::FilterType::Lanczos3,
    ))
}

pub(crate) fn sample_views(views: &[image::RgbImage]) -> Option<image::RgbImage> {
    sample_strip(views.iter().map(View::Rgb))
}
pub(crate) fn sample_pixels(views: &[PixelView]) -> Option<image::RgbImage> {
    sample_strip(
        views
            .iter()
            .filter(|view| {
                view.pixels.len()
                    == (view.width as usize)
                        .saturating_mul(view.height as usize)
                        .saturating_mul(4)
            })
            .map(View::Rgba),
    )
}

// ---------------------------------------------------------------------------
// Wallpaper sources
// ---------------------------------------------------------------------------

/// Image files backing `generate`: every configured wallpaper's resolved
/// path, deduplicated, skipping empty paths and missing files. This is
/// riced's equivalent of reshell passing per-monitor cropped wallpapers to
/// `ColorGen.generate(paths)`.
pub fn wallpaper_paths(config: &Config) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for img in &config.background.active_images() {
        let path = img.local_path();
        if path.as_os_str().is_empty() || out.contains(&path) || !path.is_file() {
            continue;
        }
        out.push(path);
    }
    out
}

pub(crate) fn render_outputs(
    specs: &[crate::wallpaper::OutputSpec],
    recipe: &crate::config::BackgroundConfig,
) -> Result<Vec<image::RgbImage>, String> {
    let images = crate::wallpaper::source_images(recipe, specs);
    let sources = crate::wallpaper::raster::load_sources(&images, Default::default())?;
    specs
        .iter()
        .map(|spec| {
            crate::wallpaper::raster::compose_output(spec, recipe, &sources)
                .map(|pixels| crate::wallpaper::raster::over_black(&pixels))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Views: rasterize what wallpaper_views paints, per output
// ---------------------------------------------------------------------------

/// Rasterize global-canvas rects with the same math as
/// `Background::wallpaper_views`: per view, overlay ascending-`z` image
/// crops (`MapLayer::resolved` → `overlap` with the view → `crop_for` to
/// source pixels) at their view-local offset, flattened over black.
///
/// Each entry of `view_rects` is one output's available rect — so the seed
/// color comes from the wallpaper set *combined across all outputs*, exactly
/// what the screens show. Views with no visible pixels still yield a black
/// tile (an empty monitor darkens the seed like it darkens the room).
/// Undecodable images contribute nothing; degenerate rects are skipped.
pub fn render_views(
    view_rects: &[(f32, f32, f32, f32)],
    images: &[BackgroundImage],
) -> Vec<image::RgbImage> {
    // Share the same full-source sampling/compositing as applied wallpapers.
    // Theme extraction retains its historical best-effort missing-file policy.
    let mut sources = crate::wallpaper::raster::Sources::new();
    for image in images {
        let path = image.local_path();
        if !sources.contains_key(&path)
            && let Ok(source) = crate::wallpaper::raster::Source::load(&path)
        {
            sources.insert(path, source);
        }
    }
    let visible: Vec<_> = images
        .iter()
        .filter(|image| sources.contains_key(&image.local_path()))
        .cloned()
        .collect();
    view_rects
        .iter()
        .filter_map(|&(ax, ay, aw, ah)| {
            let (w, h) = (aw.max(0.0) as u32, ah.max(0.0) as u32);
            if w == 0 || h == 0 {
                return None;
            }
            let canvas =
                crate::wallpaper::raster::compose((ax, ay, aw, ah), (w, h), &visible, &sources)
                    .ok()?;
            Some(crate::wallpaper::raster::over_black(&canvas))
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn test_image(
    dir: &std::path::Path,
    name: &str,
    size: u32,
    rgb: [u8; 3],
) -> crate::config::BackgroundImage {
    use image::{ImageBuffer, Rgb};
    let path = dir.join(name);
    ImageBuffer::<Rgb<u8>, _>::from_pixel(size, size, Rgb(rgb))
        .save(&path)
        .unwrap();
    BackgroundImage {
        path: path.display().to_string(),
        x: 0.0,
        y: 0.0,
        z: 0,
        scale: 1.0,
        width: size as f32,
        height: size as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::super::views::test_image;
    use super::*;

    #[test]
    fn borrowed_strip_sample_matches_full_stitch_including_black_padding() {
        let left = image::RgbImage::from_fn(12, 8, |x, y| {
            image::Rgb([(x * 17) as u8, (y * 23) as u8, 40])
        });
        let right = image::RgbImage::from_fn(5, 11, |x, y| {
            image::Rgb([90, (x * 39) as u8, (y * 19) as u8])
        });
        let mut stitched = image::RgbImage::new(17, 11);
        image::imageops::overlay(&mut stitched, &left, 0, 0);
        image::imageops::overlay(&mut stitched, &right, 12, 0);
        let expected =
            image::imageops::resize(&stitched, 128, 128, image::imageops::FilterType::Lanczos3);
        assert_eq!(sample_views(&[left, right]).unwrap(), expected);
    }

    #[test]
    fn resident_rgba_sampling_preserves_original_pixels_and_matches_rgb_composite() {
        let rgba = image::RgbaImage::from_fn(9, 7, |x, y| {
            image::Rgba([
                (x * 25) as u8,
                (y * 31) as u8,
                120,
                if x < 4 { 255 } else { 100 },
            ])
        });
        let expected = sample_views(&[crate::wallpaper::raster::over_black(&rgba)]).unwrap();
        let pixels = bytes::Bytes::from(rgba.into_raw());
        let snapshot = pixels.clone();
        let view = PixelView {
            width: 9,
            height: 7,
            pixels,
        };
        assert_eq!(
            sample_pixels(std::slice::from_ref(&view)).unwrap(),
            expected
        );
        assert_eq!(view.pixels, snapshot);
        assert!(
            sample_pixels(&[PixelView {
                width: 9,
                height: 7,
                pixels: bytes::Bytes::new()
            }])
            .is_none()
        );
    }

    #[test]
    fn render_views_matches_wallpaper_view_math() {
        // 100x100 red image at (0,0); view shows its bottom-right quarter:
        // tile pixels only in the top-left 50x50, black elsewhere.
        let dir = std::env::temp_dir().join(format!("riced-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = test_image(&dir, "red.png", 100, [255, 0, 0]);

        let views = render_views(&[(50.0, 50.0, 100.0, 100.0)], std::slice::from_ref(&img));
        assert_eq!(views.len(), 1);
        let tile = &views[0];
        assert_eq!((tile.width(), tile.height()), (100, 100));
        assert_eq!(tile.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(tile.get_pixel(49, 49).0, [255, 0, 0]);
        assert_eq!(tile.get_pixel(50, 50).0, [0, 0, 0]);
        assert_eq!(tile.get_pixel(99, 99).0, [0, 0, 0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_views_combines_one_tile_per_output() {
        // Two side-by-side outputs, one image each: two tiles stitched.
        let dir = std::env::temp_dir().join(format!("riced-views-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = test_image(&dir, "a.png", 60, [10, 20, 30]);
        a.x = 0.0;
        let mut b = test_image(&dir, "b.png", 40, [200, 100, 50]);
        b.x = 60.0;
        b.z = 1;
        let images = vec![a, b];

        let views = render_views(&[(0.0, 0.0, 60.0, 60.0), (60.0, 0.0, 40.0, 40.0)], &images);
        assert_eq!(views.len(), 2);
        assert_eq!((views[0].width(), views[0].height()), (60, 60));
        assert_eq!((views[1].width(), views[1].height()), (40, 40));
        assert_eq!(views[0].get_pixel(0, 0).0, [10, 20, 30]);
        assert_eq!(views[1].get_pixel(0, 0).0, [200, 100, 50]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wallpaper_paths_dedups_and_skips_missing() {
        let dir = std::env::temp_dir().join(format!("riced-wp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = test_image(&dir, "a.png", 8, [1, 2, 3]);
        let path = PathBuf::from(&img.path);

        let mut config = Config::default();
        config
            .background
            .image
            .push(crate::config::BackgroundImage {
                path: path.display().to_string(),
                ..Default::default()
            });
        // duplicate entry
        config
            .background
            .image
            .push(crate::config::BackgroundImage {
                path: path.display().to_string(),
                ..Default::default()
            });
        // missing file + empty path
        config
            .background
            .image
            .push(crate::config::BackgroundImage {
                path: "/no/such/file.png".into(),
                ..Default::default()
            });
        config
            .background
            .image
            .push(crate::config::BackgroundImage::default());

        assert_eq!(wallpaper_paths(&config), vec![path]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
