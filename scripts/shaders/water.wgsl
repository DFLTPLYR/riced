// Wallpaper distortion. params[0].xy optionally set UV amplitude and speed.
fn shade(uv: vec2<f32>) -> vec4<f32> {
    let amplitude = select(0.008, riced.params[0].x, riced.params[0].x != 0.0);
    let frequency = select(2.0, riced.params[0].y, riced.params[0].y != 0.0);
    let time = riced.time * frequency;
    var offset = vec2<f32>(
        sin(uv.y * 24.0 + time),
        cos(uv.x * 20.0 + time * 0.7)
    ) * amplitude;
    if riced.mouse.x >= 0.0 && riced.mouse.y >= 0.0 {
        let delta = (uv - riced.mouse / riced.resolution)
            * vec2<f32>(riced.resolution.x / riced.resolution.y, 1.0);
        let distance = length(delta);
        offset += delta / max(distance, 0.001)
            * sin(distance * 60.0 - time * 3.0)
            * exp(-distance * 8.0) * amplitude;
    }
    return sample_image(uv + offset);
}
