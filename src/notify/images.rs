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
