// Colour-vision simulation over a rendered canvas: `vision::simulate_rgba8`
// on the GPU, which `tests/vision.rs` holds it to.
//
// The canvas is straight-alpha sRGB in an 8-bit texture. Inside `region` each
// pixel is decoded to linear light, put through the deficiency's matrix, clamped
// and encoded again; alpha and every pixel outside pass through.

struct Params {
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
    // x0, y0, x1, y1 in device pixels, the far edges exclusive.
    region: vec4<u32>,
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

@compute @workgroup_size(8, 8)
fn simulate(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(src);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let at = vec2<i32>(id.xy);
    var c = textureLoad(src, at, 0);
    let inside = id.x >= p.region.x && id.y >= p.region.y && id.x < p.region.z && id.y < p.region.w;
    if inside {
        let lin = vec3<f32>(decode(c.r), decode(c.g), decode(c.b));
        let out = clamp(
            vec3<f32>(dot(p.m0.xyz, lin), dot(p.m1.xyz, lin), dot(p.m2.xyz, lin)),
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        c = vec4<f32>(encode(out.r), encode(out.g), encode(out.b), c.a);
    }
    textureStore(dst, at, c);
}
