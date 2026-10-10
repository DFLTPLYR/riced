//! Pixel ingestion independent of notification D-Bus dispatch.
pub(super) const MAX_ICON_PX: i32 = 512;
pub(super) fn decode_pixbuf(
    width: i32,
    height: i32,
    rowstride: i32,
    bps: i32,
    channels: i32,
    data: &[u8],
) -> Option<iced::widget::image::Handle> {
    if bps != 8 || !(1..=MAX_ICON_PX).contains(&width) || !(1..=MAX_ICON_PX).contains(&height) {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let stride = rowstride as usize;
    let ch = channels as usize;
    if (ch != 3 && ch != 4) || stride < w * ch || data.len() < stride * h {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let base = y * stride;
        for x in 0..w {
            let p = base + x * ch;
            rgba.push(data[p]);
            rgba.push(data[p + 1]);
            rgba.push(data[p + 2]);
            rgba.push(if ch == 4 { data[p + 3] } else { 255 });
        }
    }
    Some(iced::widget::image::Handle::from_rgba(
        w as u32,
        h as u32,
        bytes::Bytes::from(rgba),
    ))
}
use super::icon_theme::theme_icon_file;
use std::collections::HashMap;

/// `image-data` hint struct to pixels (`None` on any shape/type
/// mismatch — senders vary, never trust the bus).
pub(crate) fn hint_image_data(
    hints: &HashMap<String, zbus::zvariant::OwnedValue>,
) -> Option<iced::widget::image::Handle> {
    use zbus::zvariant::Value;
    let Value::Structure(image) = Value::try_from(hints.get("image-data")?).ok()? else {
        return None;
    };
    let f = image.fields();
    let num = |i: usize| match f.get(i) {
        Some(Value::I32(n)) => Some(*n),
        _ => None,
    };
    let (width, height, rowstride, bps, channels) = (num(0)?, num(1)?, num(2)?, num(4)?, num(5)?);
    let data: Vec<u8> = match f.get(6) {
        Some(Value::Array(bytes)) => bytes.iter().filter_map(|v| u8::try_from(v).ok()).collect(),
        _ => return None,
    };
    decode_pixbuf(width, height, rowstride, bps, channels, &data)
}
/// Plain image file (`image-path` hint or `app_icon`): absolute paths
/// (and `file://` URIs) only. SVG goes through [`rasterize_svg`];
/// bare theme names go through [`theme_icon_file`] instead.
pub(crate) fn image_file(path: &str) -> Option<iced::widget::image::Handle> {
    let path = path.strip_prefix("file://").unwrap_or(path);
    if !path.starts_with('/') {
        return None;
    }
    if std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext == "svg")
    {
        return std::fs::read(path)
            .ok()
            .and_then(|data| rasterize_svg(&data));
    }
    let img = image::open(path).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }
    Some(iced::widget::image::Handle::from_rgba(
        w,
        h,
        bytes::Bytes::from(rgba.into_raw()),
    ))
}
/// Convert premultiplied RGBA (tiny-skia's working space) to straight
/// RGBA in place. Returns whether any pixel is visible; fully
/// transparent buffers report `false` so callers can fall back.
pub(crate) fn straighten_rgba(rgba: &mut [u8]) -> bool {
    let mut visible = false;
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 {
            continue;
        }
        visible = true;
        if a != 255 {
            px[0] = ((px[0] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[1] = ((px[1] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[2] = ((px[2] as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
    visible
}
/// Rasterize SVG bytes to an iced image handle, fitting within 96px.
/// `None` on parse/render failure or degenerate sizing. No system
/// fonts are loaded (matching the `resvg/text` feature set already in
/// the tree), so text inside icons may come out empty — acceptable for
/// glyphs, which is what notification icons are.
pub(crate) fn rasterize_svg(data: &[u8]) -> Option<iced::widget::image::Handle> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }
    const FIT: f32 = 96.0;
    let scale = (FIT / size.width()).min(FIT / size.height());
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let (w, h) = (
        (size.width() * scale).ceil() as u32,
        (size.height() * scale).ceil() as u32,
    );
    if w == 0 || h == 0 || w > 512 || h > 512 {
        return None;
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia works premultiplied; unpremultiply to straight RGBA.
    let mut rgba = pixmap.take();
    if !straighten_rgba(&mut rgba) {
        // Nothing painted (empty document, display:none, …) — callers
        // fall back to text instead of an empty image box.
        return None;
    }
    Some(iced::widget::image::Handle::from_rgba(
        w,
        h,
        bytes::Bytes::from(rgba),
    ))
}
/// Resolve a notification image: `image-data` first, then the
/// `image-path` hint, then `app_icon` as a path, then `app_icon` as a
/// freedesktop theme name. Anything unusable is `None` — the card
/// renders text-only either way.
pub(crate) fn image_of(
    hints: &HashMap<String, zbus::zvariant::OwnedValue>,
    app_icon: &str,
) -> Option<iced::widget::image::Handle> {
    hint_image_data(hints).or_else(|| {
        hints
            .get("image-path")
            .and_then(|v| <&str>::try_from(v).ok())
            .or(Some(app_icon))
            .filter(|p| !p.is_empty())
            .and_then(|p| image_file(p).or_else(|| theme_icon_file(p)))
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn decode_pixbuf_handles_rgb_rgba_and_stride() {
        // 2x1 RGB, no padding: red, green.
        let rgb = vec![255, 0, 0, 0, 255, 0];
        assert!(decode_pixbuf(2, 1, 6, 8, 3, &rgb).is_some());
        // 1x1 RGBA with rowstride padding after the pixel.
        let rgba_pad = vec![10, 20, 30, 40, 0, 0];
        assert!(decode_pixbuf(1, 1, 6, 8, 4, &rgba_pad).is_some());
        // Rejects: wrong bit depth, bad channels, oversize, short data.
        assert!(decode_pixbuf(2, 1, 6, 1, 3, &rgb).is_none());
        assert!(decode_pixbuf(2, 1, 6, 8, 2, &rgb).is_none());
        assert!(decode_pixbuf(600, 1, 1800, 8, 3, &vec![0; 1800]).is_none());
        assert!(decode_pixbuf(0, 1, 6, 8, 3, &rgb).is_none());
        assert!(decode_pixbuf(2, 1, 6, 8, 3, &[1, 2, 3]).is_none());
        // Stride narrower than the row is corrupt.
        assert!(decode_pixbuf(2, 1, 5, 8, 3, &rgb).is_none());
    }
    #[test]
    fn image_of_prefers_data_over_paths() {
        use zbus::zvariant::{OwnedValue, Value};
        // image-data wins even when a path is present.
        // (u8 annotated: plain literals would infer Vec<i32>.)
        let rgb = vec![255u8, 0, 0, 0, 255, 0];
        let structure = zbus::zvariant::StructureBuilder::new()
            .add_field(2i32)
            .add_field(1i32)
            .add_field(6i32)
            .add_field(true)
            .add_field(8i32)
            .add_field(3i32)
            .add_field(rgb)
            .build()
            .unwrap();
        let mut hints = HashMap::new();
        hints.insert(
            "image-data".to_string(),
            OwnedValue::try_from(Value::Structure(structure)).unwrap(),
        );
        hints.insert(
            "image-path".to_string(),
            OwnedValue::try_from(Value::new("/nope.png")).unwrap(),
        );
        assert!(image_of(&hints, "/also-nope.png").is_some());
        // Garbage struct falls back to paths (missing here -> None).
        let mut bad = HashMap::new();
        bad.insert(
            "image-data".to_string(),
            OwnedValue::try_from(Value::new("junk")).unwrap(),
        );
        assert!(image_of(&bad, "themed-name").is_none());
        // Non-paths never touch the filesystem.
        assert!(image_of(&HashMap::new(), "themed-name").is_none());
        assert!(image_of(&HashMap::new(), "").is_none());
    }
    #[test]
    fn straighten_rgba_unpremultiplies_and_reports_visibility() {
        // Half-transparent premultiplied red -> straight red, kept alpha.
        let mut half = vec![128u8, 0, 0, 128];
        assert!(straighten_rgba(&mut half));
        assert_eq!(half, vec![255, 0, 0, 128]);
        // Opaque pixels pass through untouched.
        let mut opaque = vec![10u8, 20, 30, 255];
        assert!(straighten_rgba(&mut opaque));
        assert_eq!(opaque, vec![10, 20, 30, 255]);
        // Fully transparent buffers report invisible.
        let mut empty = vec![0u8, 0, 0, 0];
        assert!(!straighten_rgba(&mut empty));
    }
    #[test]
    fn rasterize_svg_renders_and_rejects() {
        // 16x16 red square scales to fit 96px.
        let square = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect width="16" height="16" fill="red"/></svg>"#;
        assert!(rasterize_svg(square).is_some());
        // Malformed input and empty documents never panic.
        assert!(rasterize_svg(b"not svg at all").is_none());
        assert!(rasterize_svg(b"").is_none());
        assert!(rasterize_svg(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#).is_none());
    }
}
