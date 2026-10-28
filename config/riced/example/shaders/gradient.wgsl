// Static gradient; use :animated(false) to avoid animation redraws.
fn shade(uv: vec2<f32>) -> vec4<f32> {
    let a = vec3<f32>(0.12, 0.16, 0.35);
    let b = vec3<f32>(0.65, 0.30, 0.70);
    return vec4<f32>(mix(a, b, uv.x * 0.6 + uv.y * 0.4), 1.0);
}
