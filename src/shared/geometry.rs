//! Compositor geometry has one source of truth and no window/UI dependencies.
use iced_wayland_subscriber::OutputInfo;
pub(crate) fn output_geometry(info: &OutputInfo) -> (f32, f32, f32, f32) {
    let (sx, sy) = info
        .logical_position
        .unwrap_or((info.location.0, info.location.1));
    let (sw, sh) = info.logical_size.unwrap_or_else(|| {
        info.modes
            .iter()
            .find(|m| m.current)
            .map(|m| m.dimensions)
            .unwrap_or((1920, 1080))
    });
    (sx as f32, sy as f32, sw as f32, sh as f32)
}
