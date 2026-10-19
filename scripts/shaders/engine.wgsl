// Raymarched kaleidoscopic fractal, ported from GLSL. The structure and
// camera rotate forever; mouse input has been removed.
const ITER: i32 = 10;
const RAY_STEPS: i32 = 92;
const SHADOW_STEPS: i32 = 36;
const AO_STEPS: i32 = 5;
const SCALE: f32 = 3.0;

const KEY_LIGHT: vec3<f32> = vec3<f32>(1.00, 0.88, 0.65);
const AMB_COL: vec3<f32> = vec3<f32>(0.04, 0.06, 0.14);
const FOG_COL: vec3<f32> = vec3<f32>(0.00, 0.01, 0.05);
const ENERGY_COL: vec3<f32> = vec3<f32>(0.18, 0.60, 1.00);
const HOT_COL: vec3<f32> = vec3<f32>(1.00, 0.50, 0.10);
const RIM_COL: vec3<f32> = vec3<f32>(0.30, 0.70, 1.00);

const GAMMA: f32 = 1.35;
const BRIGHTNESS: f32 = 0.95;
const SATURATION: f32 = 0.90;

fn rot2(a: f32) -> mat2x2<f32> {
    let c = cos(a);
    let s = sin(a);
    return mat2x2<f32>(c, s, -s, c);
}

fn de(pos: vec3<f32>) -> vec2<f32> {
    var p = pos;
    let xz = rot2(riced.time * 0.04) * p.xz;
    p = vec3<f32>(xz.x, p.y, xz.y);
    let xy = rot2(riced.time * 0.027) * p.xy;
    p = vec3<f32>(xy.x, xy.y, p.z);

    var s: f32 = 1.0;
    var trap: f32 = 1e10;
    let off = vec3<f32>(1.0);

    for (var i: i32 = 0; i < ITER; i = i + 1) {
        p = abs(p);
        if (p.x < p.y) { let t = p.x; p.x = p.y; p.y = t; }
        if (p.x < p.z) { let t = p.x; p.x = p.z; p.z = t; }
        if (p.y < p.z) { let t = p.y; p.y = p.z; p.z = t; }
        p = p * SCALE - off * (SCALE - 1.0);
        s = s * SCALE;
        trap = min(trap, dot(p, p) / (s * s));
    }
    let d = (length(p) - 1.5) / s;
    return vec2<f32>(d, trap);
}

fn calcNormal(p: vec3<f32>) -> vec3<f32> {
    let eps = 0.0003;
    let e = vec2<f32>(eps, 0.0);
    return normalize(vec3<f32>(
        de(p + e.xyy).x - de(p - e.xyy).x,
        de(p + e.yxy).x - de(p - e.yxy).x,
        de(p + e.yyx).x - de(p - e.yyx).x,
    ));
}

fn calcAO(p: vec3<f32>, n: vec3<f32>) -> f32 {
    var ao: f32 = 0.0;
    var sca: f32 = 1.0;
    for (var i: i32 = 0; i < AO_STEPS; i = i + 1) {
        let h = 0.004 + 0.080 * f32(i) / f32(AO_STEPS);
        let d = de(p + n * h).x;
        ao = ao + (h - d) * sca;
        sca = sca * 0.70;
    }
    return clamp(1.0 - 3.0 * ao, 0.0, 1.0);
}

fn softShadow(ro: vec3<f32>, rd: vec3<f32>, k: f32) -> f32 {
    var res: f32 = 1.0;
    var t: f32 = 0.01;
    for (var i: i32 = 0; i < SHADOW_STEPS; i = i + 1) {
        let h = de(ro + rd * t).x;
        if (h < 0.0001) { return 0.0; }
        res = min(res, k * h / t);
        t = t + clamp(h, 0.004, 0.15);
        if (t > 10.0 || res < 0.005) { break; }
    }
    return clamp(res, 0.0, 1.0);
}

fn shade_surface(p: vec3<f32>, rd: vec3<f32>, n: vec3<f32>, trap: f32) -> vec3<f32> {
    let ld = normalize(vec3<f32>(1.0, 1.5, 0.6));
    let sh = softShadow(p + n * 0.004, ld, 14.0);
    let ao = calcAO(p, n);
    let diff = max(0.0, dot(n, ld)) * sh;
    let r = reflect(rd, n);
    let spec = pow(max(0.0, dot(r, ld)) * sh, 55.0) * 0.7;
    let rim = pow(1.0 - max(0.0, dot(n, -rd)), 3.0) * 0.4;

    let eg = exp(-trap * 55.0) * 1.8;
    let hot = exp(-trap * 280.0) * 2.5;

    let grey = mix(0.07, 0.42, ao * 0.6);
    var col = vec3<f32>(grey, grey + 0.02, grey + 0.05);
    col = col * (AMB_COL + diff * KEY_LIGHT);
    col = col + spec * KEY_LIGHT;
    col = col + rim * RIM_COL * 0.35;
    col = col + ENERGY_COL * eg;
    col = col + HOT_COL * hot;
    return col;
}

fn render(ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    var t: f32 = 0.01;
    var glow: f32 = 0.0;
    var d = vec2<f32>(1.0);
    var p = ro;
    var hit = false;

    for (var i: i32 = 0; i < RAY_STEPS; i = i + 1) {
        p = ro + rd * t;
        d = de(p);
        let eps = 0.0001 * (1.0 + t * 10.0);
        if (d.x < eps) { hit = true; break; }
        if (t > 18.0) { break; }
        glow = glow + exp(-max(0.0, d.x) * 14.0) * 0.009;
        t = t + d.x * 0.70;
    }

    let ld = normalize(vec3<f32>(1.0, 1.5, 0.6));
    let lf = dot(rd, ld) * 0.5 + 0.5;
    let bg = mix(FOG_COL, AMB_COL * 2.0 + ENERGY_COL * 0.08, pow(lf, 2.5));
    if (!hit) {
        return bg + ENERGY_COL * glow * 2.5;
    }

    let n = calcNormal(p);
    var col = shade_surface(p, rd, n, d.y);
    col = mix(col, bg, 1.0 - exp(-0.012 * t * t));
    col = col + ENERGY_COL * glow;
    return col;
}

fn shade(uv: vec2<f32>) -> vec4<f32> {
    let frag = uv * riced.resolution;
    let time = riced.time;
    let view = (frag - 0.5 * riced.resolution) / riced.resolution.y;

    let rotY = time * 0.20;
    let rotX = -0.42 + sin(time * 0.09) * 0.18;

    let tgt = vec3<f32>(0.0);
    let radius = 3.5 + sin(time * 0.13) * 0.5;
    let ro = tgt + radius * vec3<f32>(cos(rotX) * sin(rotY), sin(rotX), cos(rotX) * cos(rotY));
    let fwd = normalize(tgt - ro);
    let rgt = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), fwd));
    let up = cross(fwd, rgt);
    let rd = normalize(fwd + view.x * rgt + view.y * up);

    var col = render(ro, rd);
    col = pow(clamp(col * BRIGHTNESS, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / GAMMA));
    col = mix(vec3<f32>(dot(col, vec3<f32>(0.333))), col, SATURATION);
    let q = uv;
    col = col * (0.5 + 0.5 * pow(16.0 * q.x * q.y * (1.0 - q.x) * (1.0 - q.y), 0.3));
    return vec4<f32>(col, 1.0);
}
