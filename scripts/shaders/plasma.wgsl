// Animated plasma. params[0].rgb is an optional tint (all zero = default).
fn shade(uv: vec2<f32>) -> vec4<f32> {
    let p = (uv - 0.5) * vec2<f32>(riced.resolution.x / max(riced.resolution.y, 1.0), 1.0);
    let t = riced.time;
    let v = sin(p.x * 10.0 + t) + sin(p.y * 12.0 - t * 0.7)
        + sin(length(p) * 15.0 - t * 1.3);
    let base = 0.5 + 0.5 * cos(v + vec3<f32>(0.0, 2.0, 4.0));
    let tint = riced.params[0].rgb;
    let color = select(base, base * tint, any(tint != vec3<f32>(0.0)));
    return vec4<f32>(color, 1.0);
}
