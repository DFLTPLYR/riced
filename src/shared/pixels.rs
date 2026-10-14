//! Reclaimable ownership for large decoded images shared with render handles.
use bytes::Bytes;
use image::{ColorType, ImageDecoder};

/// Decode large 8-bit images directly into anonymous storage. RGBA needs no
/// additional full-frame buffer; other 8-bit layouts convert from a temporary
/// mapping so both allocations are returned to the OS when their owners drop.
/// Less common high-bit-depth formats retain image's existing conversion.
pub(crate) fn decode(path: &std::path::Path) -> Result<(u32, u32, Bytes), String> {
    let decode = || -> Result<_, String> {
        let mut decoder = image::ImageReader::open(path)
            .map_err(|error| error.to_string())?
            .into_decoder()
            .map_err(|error| error.to_string())?;
        let (width, height) = decoder.dimensions();
        let mut limits = image::Limits::default();
        limits
            .reserve(decoder.total_bytes())
            .map_err(|error| error.to_string())?;
        decoder
            .set_limits(limits)
            .map_err(|error| error.to_string())?;
        let length = usize::try_from(decoder.total_bytes()).map_err(|error| error.to_string())?;
        let color = decoder.color_type();
        if length < 1024 * 1024
            || !matches!(
                color,
                ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8
            )
        {
            let image = image::DynamicImage::from_decoder(decoder)
                .map_err(|error| error.to_string())?
                .into_rgba8();
            return Ok((width, height, rgba(image)?));
        }
        let mut decoded = memmap2::MmapMut::map_anon(length).map_err(|error| error.to_string())?;
        decoder
            .read_image(&mut decoded)
            .map_err(|error| error.to_string())?;
        let rgba = if color == ColorType::Rgba8 {
            decoded
        } else {
            let pixels = (width as usize)
                .checked_mul(height as usize)
                .and_then(|count| count.checked_mul(4))
                .ok_or("image dimensions overflow")?;
            let mut output =
                memmap2::MmapMut::map_anon(pixels).map_err(|error| error.to_string())?;
            for (input, output) in decoded
                .chunks_exact(usize::from(color.channel_count()))
                .zip(output.chunks_exact_mut(4))
            {
                let pixel = match color {
                    ColorType::L8 => [input[0], input[0], input[0], 255],
                    ColorType::La8 => [input[0], input[0], input[0], input[1]],
                    ColorType::Rgb8 => [input[0], input[1], input[2], 255],
                    _ => unreachable!("8-bit layout checked above"),
                };
                output.copy_from_slice(&pixel);
            }
            output
        };
        let pixels = rgba.make_read_only().map_err(|error| error.to_string())?;
        Ok((width, height, Bytes::from_owner(pixels)))
    };
    decode().map_err(|error| format!("{}: {error}", path.display()))
}

/// Large preview pixels can be decoded on a pooled thread and finally freed
/// on the UI thread. Anonymous mappings avoid retaining that allocation in a
/// worker's malloc arena after the last handle drops. Small icons keep the
/// cheaper ordinary allocation. No source file is mapped or modified.
pub(crate) fn rgba(rgba: image::RgbaImage) -> Result<Bytes, String> {
    let length = rgba.as_raw().len();
    if length < 1024 * 1024 {
        return Ok(Bytes::from(rgba.into_raw()));
    }
    let mut mapping = memmap2::MmapMut::map_anon(length)
        .map_err(|error| format!("cannot allocate image pixels: {error}"))?;
    mapping.copy_from_slice(rgba.as_raw());
    // Release decoder-owned storage on the decoding thread, before returning
    // the mapping to another thread. Worker cleanup can trim that arena now.
    drop(rgba);
    let mapping = mapping
        .make_read_only()
        .map_err(|error| format!("cannot freeze image pixels: {error}"))?;
    Ok(Bytes::from_owner(mapping))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_decoding_matches_standard_rgba_conversion_for_every_layout() {
        use image::{DynamicImage, ImageBuffer, Luma, LumaA, Rgb, Rgba};
        let dir = std::env::temp_dir().join(format!("riced-mapped-layouts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let images = [
            DynamicImage::ImageLuma8(ImageBuffer::from_fn(1024, 1024, |x, y| {
                Luma([(x ^ y) as u8])
            })),
            DynamicImage::ImageLumaA8(ImageBuffer::from_fn(1024, 1024, |x, y| {
                LumaA([x as u8, y as u8])
            })),
            DynamicImage::ImageRgb8(ImageBuffer::from_fn(1024, 1024, |x, y| {
                Rgb([x as u8, y as u8, 137])
            })),
            DynamicImage::ImageRgba8(ImageBuffer::from_fn(1024, 1024, |x, y| {
                Rgba([x as u8, y as u8, 137, (x ^ y) as u8])
            })),
            DynamicImage::ImageRgb16(ImageBuffer::from_pixel(
                1024,
                1024,
                Rgb([12000, 35000, 64000]),
            )),
        ];
        for (index, image) in images.into_iter().enumerate() {
            let path = dir.join(format!("{index}.png"));
            image.save(&path).unwrap();
            let (width, height, pixels) = decode(&path).unwrap();
            assert_eq!((width, height), (1024, 1024));
            let expected = image::open(&path).unwrap().into_rgba8();
            assert_eq!(pixels.as_ref(), expected.as_raw());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mapped_pixels_preserve_content_and_handle_ownership() {
        let image = image::RgbaImage::from_pixel(1024, 512, image::Rgba([10, 20, 30, 127]));
        let pixels = rgba(image).unwrap();
        assert_eq!(pixels.len(), 1024 * 512 * 4);
        assert!(
            pixels
                .chunks_exact(4)
                .all(|pixel| pixel == [10, 20, 30, 127])
        );
        let handle = iced::widget::image::Handle::from_rgba(1024, 512, pixels.clone());
        drop(pixels);
        let iced::widget::image::Handle::Rgba { pixels, .. } = handle else {
            panic!("RGBA handle");
        };
        assert_eq!(&pixels[..4], &[10, 20, 30, 127]);
    }
}
