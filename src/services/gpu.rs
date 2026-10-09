//! GPU sampling belongs to service ingestion, not popup layout.
pub(crate) fn gpu_usage_percent() -> Option<f32> {
    gpu_usage_in(std::path::Path::new("/sys/class/drm"))
}
pub(crate) fn gpu_usage_in(drm: &std::path::Path) -> Option<f32> {
    std::fs::read_dir(drm)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.strip_prefix("card").is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                })
            })
        })
        .filter_map(|entry| {
            std::fs::read_to_string(entry.path().join("device/gpu_busy_percent")).ok()
        })
        .filter_map(|text| text.trim().parse::<f32>().ok())
        .fold(None, |busiest: Option<f32>, usage| {
            Some(busiest.map_or(usage, |peak| peak.max(usage)))
        })
}
