// Stable riced fragment-shader ABI. Coordinates are local logical pixels.
struct Riced {
    time: f32,
    opacity: f32,
    resolution: vec2<f32>,
    mouse: vec2<f32>,
    _padding: vec2<f32>,
    params: array<vec4<f32>, 4>,
    _uv_rect: vec4<f32>,
}
@group(0) @binding(0) var<uniform> riced: Riced;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Vertex {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: Vertex;
    out.position = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = riced._uv_rect.xy + uv * riced._uv_rect.zw;
    return out;
}

@fragment
fn fs_main(in: Vertex) -> @location(0) vec4<f32> {
    let color = shade(in.uv);
    return vec4<f32>(color.rgb, color.a * riced.opacity);
}
