//! Row-at-a-time Lanczos3 palette sampling. Scratch storage is proportional to
//! source width, not source width times the 128-row palette image.
use image::{GenericImageView, Rgb, RgbImage};

const SIZE: u32 = 128;

fn sinc(value: f32) -> f32 {
    if value == 0.0 {
        1.0
    } else {
        let angle = value * std::f32::consts::PI;
        angle.sin() / angle
    }
}

fn weights(length: u32, output: u32, weights: &mut Vec<f32>) -> u32 {
    let ratio = length as f32 / SIZE as f32;
    let scale = ratio.max(1.0);
    let center = (output as f32 + 0.5) * ratio;
    let support = 3.0 * scale;
    let start = ((center - support).floor() as i64).clamp(0, i64::from(length) - 1) as u32;
    let end =
        ((center + support).ceil() as i64).clamp(i64::from(start) + 1, i64::from(length)) as u32;
    weights.clear();
    let mut sum = 0.0;
    for source in start..end {
        let distance = (source as f32 - (center - 0.5)) / scale;
        let weight = if distance.abs() < 3.0 {
            sinc(distance) * sinc(distance / 3.0)
        } else {
            0.0
        };
        weights.push(weight);
        sum += weight;
    }
    for weight in weights.iter_mut() {
        *weight /= sum;
    }
    start
}

/// Match `image::imageops::resize(..., 128, 128, Lanczos3)` for RGB pixels,
/// including vertical-first filtering, unclamped intermediate values, and
/// final nearest-integer rounding. Input must have nonzero dimensions.
pub(super) fn resize(source: &impl GenericImageView<Pixel = Rgb<u8>>) -> RgbImage {
    let (width, height) = source.dimensions();
    if (width, height) == (SIZE, SIZE) {
        return RgbImage::from_fn(SIZE, SIZE, |x, y| source.get_pixel(x, y));
    }
    let mut result = RgbImage::new(SIZE, SIZE);
    let mut row = vec![[0.0_f32; 3]; width as usize];
    let mut vertical = Vec::new();
    // Horizontal support is invariant across rows. Cache it once rather than
    // recomputing millions of sinc evaluations for every palette sample.
    let horizontal: Vec<_> = (0..SIZE)
        .map(|x| {
            let mut taps = Vec::new();
            let left = weights(width, x, &mut taps);
            (left, taps)
        })
        .collect();
    for y in 0..SIZE {
        let top = weights(height, y, &mut vertical);
        for x in 0..width {
            let mut channels = [0.0; 3];
            for (offset, weight) in vertical.iter().enumerate() {
                let pixel = source.get_pixel(x, top + offset as u32);
                for channel in 0..3 {
                    channels[channel] += f32::from(pixel[channel]) * weight;
                }
            }
            row[x as usize] = channels;
        }
        for (x, (left, taps)) in horizontal.iter().enumerate() {
            let mut channels = [0.0_f32; 3];
            for (offset, weight) in taps.iter().enumerate() {
                let pixel = row[*left as usize + offset];
                for channel in 0..3 {
                    channels[channel] += pixel[channel] * weight;
                }
            }
            result.put_pixel(
                x as u32,
                y,
                Rgb(channels.map(|value| value.clamp(0.0, 255.0).round() as u8)),
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanline_sample_matches_image_lanczos_for_upscaling_and_downscaling() {
        for (width, height) in [
            (1, 1),
            (17, 9),
            (128, 128),
            (129, 127),
            (511, 301),
            (3840, 216),
        ] {
            let source = RgbImage::from_fn(width, height, |x, y| {
                Rgb([
                    (x.wrapping_mul(37) ^ y.wrapping_mul(17)) as u8,
                    (x + y * 31) as u8,
                    (x * 13 + y * 7) as u8,
                ])
            });
            let expected =
                image::imageops::resize(&source, SIZE, SIZE, image::imageops::FilterType::Lanczos3);
            assert_eq!(resize(&source), expected, "{width}x{height}");
        }
    }

    /// Run alone in release mode. Set RICED_SAMPLE_REFERENCE=1 to measure the
    /// previous full-intermediate sampler in a separate process. Procedural
    /// pixels isolate sampling scratch memory from resident wallpaper storage.
    #[test]
    #[ignore = "manual isolated release memory comparison"]
    #[cfg(target_os = "linux")]
    fn palette_sampling_memory_probe() {
        struct Wallpaper;
        impl GenericImageView for Wallpaper {
            type Pixel = Rgb<u8>;
            fn dimensions(&self) -> (u32, u32) {
                (7680, 2160)
            }
            fn get_pixel(&self, x: u32, y: u32) -> Rgb<u8> {
                Rgb([(x / 30) as u8, (y / 9) as u8, ((x + y) / 40) as u8])
            }
        }
        fn memory() -> String {
            std::fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
                .collect::<Vec<_>>()
                .join(", ")
        }
        let reference = std::env::var_os("RICED_SAMPLE_REFERENCE").is_some();
        println!("reference={reference}, before: {}", memory());
        let start = std::time::Instant::now();
        for _ in 0..4 {
            let sample = if reference {
                image::imageops::resize(
                    &Wallpaper,
                    SIZE,
                    SIZE,
                    image::imageops::FilterType::Lanczos3,
                )
            } else {
                resize(&Wallpaper)
            };
            let checksum: u64 = sample.as_raw().iter().map(|&byte| u64::from(byte)).sum();
            std::hint::black_box(&sample);
            println!(
                "checksum={checksum}, elapsed={:?}, {}",
                start.elapsed(),
                memory()
            );
        }
        crate::theme_gen::trim_memory();
        println!("settled: {}", memory());
    }
}
