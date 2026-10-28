// Rings follow the cursor. With no cursor they originate at the center.
fn shade(uv: vec2<f32>) -> vec4<f32> {
    let center = select(vec2<f32>(0.5), riced.mouse / max(riced.resolution, vec2<f32>(1.0)), riced.mouse.x >= 0.0);
    let p = (uv - center) * vec2<f32>(riced.resolution.x / max(riced.resolution.y, 1.0), 1.0);
    let ring = 0.5 + 0.5 * sin(length(p) * 48.0 - riced.time * 4.0);
    return vec4<f32>(mix(vec3<f32>(0.04, 0.08, 0.16), vec3<f32>(0.20, 0.70, 0.85), ring), 1.0);
}
