//! Sample original pixels in desktop coordinates, at an output's pixel density.
use crate::config::BackgroundImage;
use bytes::Bytes;
use iced::widget::image::Handle;
use std::{collections::HashMap, path::PathBuf, sync::Arc};

#[derive(Debug)]
pub struct Source {
    pub width: u32,
    pub height: u32,
    pub pixels: Bytes,
    pub handle: Handle,
    pub(crate) revision: Option<crate::shared::files::FileStamp>,
}

impl Source {
    pub fn load(path: &std::path::Path) -> Result<Arc<Self>, String> {
        let revision = crate::shared::files::stamp(path);
        let rgba = image::open(path)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .to_rgba8();
        let (width, height) = rgba.dimensions();
        if width == 0 || height == 0 {
            return Err(format!("{}: image is empty", path.display()));
        }
        let pixels = Bytes::from(rgba.into_raw());
        let handle = Handle::from_rgba(width, height, pixels.clone());
        Ok(Arc::new(Self {
            width,
            height,
            pixels,
            handle,
            revision,
        }))
    }

    fn pixel(&self, x: u32, y: u32) -> &[u8] {
        let offset = ((y as usize * self.width as usize) + x as usize) * 4;
        &self.pixels[offset..offset + 4]
    }
}

pub(crate) type Sources = HashMap<PathBuf, Arc<Source>>;

/// Reuse unchanged preview pixels, re-decode changed paths, and release removals.
/// Apply uses `load_sources` instead, keeping its accepted pixel snapshot intact.
pub(crate) fn load_preview_sources(
    images: &[BackgroundImage],
    mut sources: Sources,
) -> Result<Sources, String> {
    sources.retain(|path, source| source.revision == crate::shared::files::stamp(path));
    load_sources(images, sources)
}

/// Resolve per-monitor placement in oriented native pixels, then convert to
/// desktop coordinates. Center therefore stays 1:1 even on scaled displays.
pub(crate) fn monitor_image(
    spec: &super::OutputSpec,
    wallpaper: &crate::config::MonitorWallpaper,
    source: &Source,
) -> BackgroundImage {
    use crate::config::WallpaperFit;
    let (sw, sh) = (source.width as f32, source.height as f32);
    let (pw, ph) = (spec.pixels.0 as f32, spec.pixels.1 as f32);
    let factor = match wallpaper.fit {
        WallpaperFit::Fit => (pw / sw).min(ph / sh),
        WallpaperFit::Expand => (pw / sw).max(ph / sh),
        WallpaperFit::Center | WallpaperFit::Stretch => 1.0,
    };
    let (w, h) = if wallpaper.fit == WallpaperFit::Stretch {
        (pw, ph)
    } else {
        (sw * factor, sh * factor)
    };
    BackgroundImage {
        path: wallpaper.path.clone(),
        x: spec.rect.0 + (pw - w) * 0.5 * spec.rect.2 / pw,
        y: spec.rect.1 + (ph - h) * 0.5 * spec.rect.3 / ph,
        width: w * spec.rect.2 / pw,
        height: h * spec.rect.3 / ph,
        ..Default::default()
    }
}

pub(crate) fn compose_output(
    spec: &super::OutputSpec,
    recipe: &crate::config::BackgroundConfig,
    sources: &Sources,
) -> Result<image::RgbaImage, String> {
    if recipe.mode == crate::config::WallpaperMode::FreeForm {
        return compose(spec.rect, spec.pixels, &recipe.image, sources);
    }
    let images = match recipe
        .monitor
        .iter()
        .find(|m| m.output == spec.key && !m.path.is_empty())
    {
        Some(wallpaper) => {
            let image = BackgroundImage {
                path: wallpaper.path.clone(),
                ..Default::default()
            };
            let source = sources
                .get(&image.local_path())
                .ok_or_else(|| format!("{}: source was not loaded", wallpaper.path))?;
            vec![monitor_image(spec, wallpaper, source)]
        }
        None => Vec::new(),
    };
    let mut pixels = compose(spec.rect, spec.pixels, &images, sources)?;
    // Per-monitor margins and unassigned displays are explicitly black.
    // Composite alpha in place so this needs no second full-resolution buffer.
    for pixel in pixels.pixels_mut() {
        let rgb = rgb_over_black(&pixel.0);
        *pixel = image::Rgba([rgb[0], rgb[1], rgb[2], 255]);
    }
    Ok(pixels)
}

pub(crate) fn load_sources(
    images: &[BackgroundImage],
    mut sources: Sources,
) -> Result<Sources, String> {
    let paths: std::collections::HashSet<_> =
        images.iter().map(BackgroundImage::local_path).collect();
    sources.retain(|path, _| paths.contains(path));
    for path in paths {
        if path.as_os_str().is_empty() {
            return Err("A wallpaper has no source path".into());
        }
        if !sources.contains_key(&path) {
            sources.insert(path.clone(), Source::load(&path)?);
        }
    }
    Ok(sources)
}

pub(crate) fn rect(image: &BackgroundImage, source: &Source) -> Option<(f32, f32, f32, f32)> {
    let width = if image.width > 0.0 {
        image.width
    } else {
        source.width as f32
    };
    let height = if image.height > 0.0 {
        image.height
    } else {
        source.height as f32
    };
    let scale = image.scale.max(0.01);
    let rect = (image.x, image.y, width * scale, height * scale);
    (rect.0.is_finite()
        && rect.1.is_finite()
        && rect.2.is_finite()
        && rect.3.is_finite()
        && rect.2 > 0.0
        && rect.3 > 0.0)
        .then_some(rect)
}

fn linear(value: u8) -> f32 {
    let value = value as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn encoded(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let value = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (value * 255.0).round() as u8
}

/// Linear filtering in premultiplied linear color avoids transparent-edge halos.
fn sample(source: &Source, x: f32, y: f32, colors: &[f32; 256]) -> [f32; 4] {
    let x = x.clamp(0.0, source.width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, source.height.saturating_sub(1) as f32);
    let (ix, iy) = (x.floor() as u32, y.floor() as u32);
    let (fx, fy) = (x - ix as f32, y - iy as f32);
    let mut result = [0.0; 4];
    for (px, py, weight) in [
        (ix, iy, (1.0 - fx) * (1.0 - fy)),
        ((ix + 1).min(source.width - 1), iy, fx * (1.0 - fy)),
        (ix, (iy + 1).min(source.height - 1), (1.0 - fx) * fy),
        (
            (ix + 1).min(source.width - 1),
            (iy + 1).min(source.height - 1),
            fx * fy,
        ),
    ] {
        if weight == 0.0 {
            continue;
        }
        let pixel = source.pixel(px, py);
        let alpha = pixel[3] as f32 / 255.0;
        for channel in 0..3 {
            result[channel] += colors[pixel[channel] as usize] * alpha * weight;
        }
        result[3] += alpha * weight;
    }
    result
}

pub(crate) fn compose(
    view: (f32, f32, f32, f32),
    pixels: (u32, u32),
    images: &[BackgroundImage],
    sources: &Sources,
) -> Result<image::RgbaImage, String> {
    if pixels.0 == 0
        || pixels.1 == 0
        || ![view.0, view.1, view.2, view.3]
            .iter()
            .all(|value| value.is_finite())
        || view.2 <= 0.0
        || view.3 <= 0.0
    {
        return Err("Output has invalid wallpaper dimensions".into());
    }
    let mut canvas = image::RgbaImage::new(pixels.0, pixels.1);
    let colors = std::array::from_fn(|index| linear(index as u8));
    let mut order: Vec<_> = images.iter().collect();
    order.sort_by_key(|image| image.z);
    for image in order {
        let source = sources
            .get(&image.local_path())
            .ok_or_else(|| format!("{}: source was not loaded", image.path))?;
        let (ix, iy, iw, ih) = rect(image, source)
            .ok_or_else(|| format!("{}: invalid image placement", image.path))?;
        let left = ix.max(view.0);
        let top = iy.max(view.1);
        let right = (ix + iw).min(view.0 + view.2);
        let bottom = (iy + ih).min(view.1 + view.3);
        if right <= left || bottom <= top {
            continue;
        }
        let x0 = (((left - view.0) / view.2 * pixels.0 as f32).floor() as u32).min(pixels.0);
        let y0 = (((top - view.1) / view.3 * pixels.1 as f32).floor() as u32).min(pixels.1);
        let x1 = (((right - view.0) / view.2 * pixels.0 as f32).ceil() as u32).min(pixels.0);
        let y1 = (((bottom - view.1) / view.3 * pixels.1 as f32).ceil() as u32).min(pixels.1);
        for y in y0..y1 {
            for x in x0..x1 {
                let gx = view.0 + (x as f32 + 0.5) * view.2 / pixels.0 as f32;
                let gy = view.1 + (y as f32 + 0.5) * view.3 / pixels.1 as f32;
                if gx < ix || gx >= ix + iw || gy < iy || gy >= iy + ih {
                    continue;
                }
                let sampled = sample(
                    source,
                    (gx - ix) / iw * source.width as f32 - 0.5,
                    (gy - iy) / ih * source.height as f32 - 0.5,
                    &colors,
                );
                if sampled[3] == 0.0 {
                    continue;
                }
                let destination = canvas.get_pixel_mut(x, y);
                let old_alpha = destination[3] as f32 / 255.0;
                let alpha = sampled[3] + old_alpha * (1.0 - sampled[3]);
                if alpha <= 0.0 {
                    continue;
                }
                for channel in 0..3 {
                    destination[channel] = encoded(
                        (sampled[channel]
                            + colors[destination[channel] as usize]
                                * old_alpha
                                * (1.0 - sampled[3]))
                            / alpha,
                    );
                }
                destination[3] = (alpha * 255.0).round() as u8;
            }
        }
    }
    Ok(canvas)
}

pub(crate) fn over_black(image: &image::RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
        rgb_over_black(&image.get_pixel(x, y).0)
    })
}

pub(crate) fn rgb_over_black(pixel: &[u8]) -> image::Rgb<u8> {
    if pixel[3] == 255 {
        return image::Rgb([pixel[0], pixel[1], pixel[2]]);
    }
    if pixel[3] == 0 {
        return image::Rgb([0, 0, 0]);
    }
    let alpha = pixel[3] as f32 / 255.0;
    image::Rgb([
        encoded(linear(pixel[0]) * alpha),
        encoded(linear(pixel[1]) * alpha),
        encoded(linear(pixel[2]) * alpha),
    ])
}
