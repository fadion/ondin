// Vision simulation over a rendered canvas: `vision::apply` on the GPU, which
// `tests/vision.rs` holds it to.
//
// The canvas is straight-alpha sRGB in an 8-bit texture. Inside `region` each
// pixel is decoded to linear light and put through the pass: `tint` an affine
// map, `blur` one direction of a Gaussian, `ghost` a mix with the pixel an offset
// away. A neighbour is read clamped to the region, so nothing outside it bleeds
// in. Every pixel outside passes through.

struct Params {
    // A tint's rows: three coefficients, then an offset.
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
    // x0, y0, x1, y1 in device pixels, the far edges exclusive.
    region: vec4<u32>,
    // A blur's direction in x, y; a ghost's offset in x, y and its mix in z.
    v: vec4<f32>,
    // A blur's taps on each side, then the stride between them.
    taps: vec4<u32>,
    // A blur's weights from the centre out, four to a vector.
    w: array<vec4<f32>, 9>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> p: Params;

fn decode(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn encode(c: f32) -> f32 {
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn linear(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(decode(c.r), decode(c.g), decode(c.b), c.a);
}

fn srgb(c: vec4<f32>) -> vec4<f32> {
    let l = clamp(c, vec4<f32>(0.0), vec4<f32>(1.0));
    return vec4<f32>(encode(l.r), encode(l.g), encode(l.b), l.a);
}

fn inside(id: vec2<u32>) -> bool {
    return id.x >= p.region.x && id.y >= p.region.y && id.x < p.region.z && id.y < p.region.w;
}

// `at` held to the region; an empty region holds it at its corner.
fn held(at: vec2<i32>) -> vec2<i32> {
    let lo = vec2<i32>(p.region.xy);
    let hi = max(vec2<i32>(p.region.zw) - vec2<i32>(1), lo);
    return clamp(at, lo, hi);
}

@compute @workgroup_size(8, 8)
fn tint(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(src);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let at = vec2<i32>(id.xy);
    var c = textureLoad(src, at, 0);
    if inside(id.xy) {
        let lin = linear(c).rgb;
        let out = vec3<f32>(
            dot(p.m0.xyz, lin) + p.m0.w,
            dot(p.m1.xyz, lin) + p.m1.w,
            dot(p.m2.xyz, lin) + p.m2.w,
        );
        c = srgb(vec4<f32>(out, c.a));
    }
    textureStore(dst, at, c);
}

@compute @workgroup_size(8, 8)
fn blur(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(src);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let at = vec2<i32>(id.xy);
    var c = textureLoad(src, at, 0);
    if inside(id.xy) {
        let n = i32(p.taps.x);
        let stride = i32(p.taps.y);
        let dir = vec2<i32>(i32(p.v.x), i32(p.v.y));
        var acc = vec4<f32>(0.0);
        for (var k = -n; k <= n; k++) {
            let j = u32(abs(k));
            let wk = p.w[j / 4u][j % 4u];
            acc += wk * linear(textureLoad(src, held(at + dir * (k * stride)), 0));
        }
        c = srgb(acc);
    }
    textureStore(dst, at, c);
}

@compute @workgroup_size(8, 8)
fn ghost(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(src);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let at = vec2<i32>(id.xy);
    var c = textureLoad(src, at, 0);
    if inside(id.xy) {
        let off = vec2<i32>(i32(p.v.x), i32(p.v.y));
        let a = linear(c);
        let b = linear(textureLoad(src, held(at - off), 0));
        c = srgb(mix(a, b, p.v.z));
    }
    textureStore(dst, at, c);
}
