//! Compositor geometry has one source of truth and no window/UI dependencies.
use iced_wayland_subscriber::OutputInfo;
pub(crate) fn oriented_pixels(
    size: (i32, i32),
    transform: wayland_client::protocol::wl_output::Transform,
) -> (u32, u32) {
    use wayland_client::protocol::wl_output::Transform;
    let (width, height) = (size.0.max(1) as u32, size.1.max(1) as u32);
    if matches!(
        transform,
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270
    ) {
        (height, width)
    } else {
        (width, height)
    }
}
pub(crate) fn output_geometry(info: &OutputInfo) -> (f32, f32, f32, f32) {
    let (sx, sy) = info
        .logical_position
        .unwrap_or((info.location.0, info.location.1));
    let (sw, sh) = info.logical_size.unwrap_or_else(|| {
        let mode = info
            .modes
            .iter()
            .find(|m| m.current)
            .map(|m| m.dimensions)
            .unwrap_or((1920, 1080));
        let (width, height) = oriented_pixels(mode, info.transform);
        let scale = info.scale_factor.max(1) as u32;
        (
            (width / scale).max(1) as i32,
            (height / scale).max(1) as i32,
        )
    });
    (sx as f32, sy as f32, sw as f32, sh as f32)
}
