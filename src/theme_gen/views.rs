//! Wallpaper rasterization shared by generation and shell painting.
use crate::config::{BackgroundImage, Config};
use crate::ui::widgets::display_map::MapLayer;
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Wallpaper sources
// ---------------------------------------------------------------------------

/// Image files backing `generate`: every configured wallpaper's resolved
/// path, deduplicated, skipping empty paths and missing files. This is
/// riced's equivalent of reshell passing per-monitor cropped wallpapers to
/// `ColorGen.generate(paths)`.
pub fn wallpaper_paths(config: &Config) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for img in &config.background.image {
        let path = img.local_path();
        if path.as_os_str().is_empty() || out.contains(&path) || !path.is_file() {
            continue;
        }
        out.push(path);
    }
    out
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
    // Decode once; every view shares the pixels.
    let mut decoded: HashMap<PathBuf, image::RgbImage> = HashMap::new();
    let mut order: Vec<usize> = (0..images.len()).collect();
    order.sort_by_key(|&i| images[i].z);

    view_rects
        .iter()
        .filter_map(|&(ax, ay, aw, ah)| {
            let (w, h) = (aw.max(0.0) as u32, ah.max(0.0) as u32);
            if w == 0 || h == 0 {
                return None;
            }
            let mut canvas = image::RgbImage::new(w, h);
            for &i in &order {
                let img = &images[i];
                let (Some(rect), Some(native)) =
                    (MapLayer::resolved(img), MapLayer::native_size(img))
                else {
                    continue;
                };
                let Some(overlap) = MapLayer::overlap(rect, (ax, ay, aw, ah)) else {
                    continue;
                };
                let (ox, oy, _, _) = overlap;
                let Some(crop) = MapLayer::crop_for(rect, native, overlap) else {
                    continue;
                };
                let pixels = decoded.entry(img.local_path()).or_insert_with(|| {
                    image::open(img.local_path())
                        .map(|d| d.to_rgb8())
                        .unwrap_or_else(|_| image::RgbImage::new(0, 0))
                });
                let (dw, dh) = pixels.dimensions();
                // Stored width/height can disagree with the file (user-set);
                // clamp so a stale crop never panics.
                let cx = crop.x.min(dw);
                let cy = crop.y.min(dh);
                let cw = crop.width.min(dw.saturating_sub(cx));
                let ch = crop.height.min(dh.saturating_sub(cy));
                if cw == 0 || ch == 0 {
                    continue;
                }
                let tile = image::imageops::crop_imm(pixels, cx, cy, cw, ch).to_image();
                image::imageops::overlay(&mut canvas, &tile, (ox - ax) as i64, (oy - ay) as i64);
            }
            Some(canvas)
        })
        .collect()
}

/// Stitch view tiles side-by-side (sys lays monitor strips out the same
/// way) into a temp PNG for seed-color extraction. `None` when there is
/// nothing to read.
pub(crate) fn combine_views(views: &[image::RgbImage]) -> Option<PathBuf> {
    let tiles: Vec<&image::RgbImage> = views
        .iter()
        .filter(|v| v.width() > 0 && v.height() > 0)
        .collect();
    if tiles.is_empty() {
        return None;
    }
    let total_width: u32 = tiles.iter().map(|v| v.width()).sum();
    let height: u32 = tiles.iter().map(|v| v.height()).max().unwrap_or(0);
    if total_width == 0 || height == 0 {
        return None;
    }
    let mut combined = image::RgbImage::new(total_width, height);
    let mut x = 0i64;
    for tile in tiles {
        image::imageops::overlay(&mut combined, tile, x, 0);
        x += tile.width() as i64;
    }
    let output = std::env::temp_dir().join("riced_combined_wallpaper.png");
    combined.save(&output).ok()?;
    Some(output)
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
