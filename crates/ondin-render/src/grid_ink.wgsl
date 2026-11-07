// The pixel grid's adaptive ink over a rendered canvas: `grid_ink::adapt_rgba8`
// on the texels `grid_ink::on_line` picks, which `tests/grid_ink.rs` holds it to.
//
// The canvas is straight-alpha sRGB in an 8-bit texture. Inside `region`, a texel
// on a line of either axis moves STEP in CIE L*: darkened by scaling its linear
// channels above DARKEN_ABOVE, lightened toward white at or below. Alpha and
// every other texel pass through.

struct Params {
    // Per axis, a line's position and the texels to the next, as `Lattice` holds them.
    origin: vec2<f32>,
    pitch: vec2<f32>,
    // x0, y0, x1, y1 in texels, the far edges exclusive.
    region: vec4<u32>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> p: Params;

const STEP: f32 = 10.0;
const DARKEN_ABOVE: f32 = 45.0;
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
const CIE_EPSILON: f32 = 0.008856452;
const CIE_KAPPA: f32 = 903.2963;

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

fn lightness(y: f32) -> f32 {
    if y > CIE_EPSILON {
        return 116.0 * pow(y, 1.0 / 3.0) - 16.0;
    }
    return CIE_KAPPA * y;
}

fn luminance_at(l_in: f32) -> f32 {
    let l = clamp(l_in, 0.0, 100.0);
    if l > CIE_KAPPA * CIE_EPSILON {
        let f = (l + 16.0) / 116.0;
        return f * f * f;
    }
    return l / CIE_KAPPA;
}

fn on_line(origin: f32, pitch: f32, at: u32) -> bool {
    let x = f32(at);
    let k = round((x - origin) / pitch);
    return floor(origin + k * pitch + 0.5) == x;
}

fn adapt(c: vec3<f32>) -> vec3<f32> {
    let lin = vec3<f32>(decode(c.r), decode(c.g), decode(c.b));
    let y = dot(LUMA, lin);
    let l = lightness(y);
    var out: vec3<f32>;
    if l > DARKEN_ABOVE {
        out = lin * (luminance_at(l - STEP) / y);
    } else {
        let m = (luminance_at(l + STEP) - y) / (1.0 - y);
        out = lin + m * (vec3<f32>(1.0) - lin);
    }
    out = clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec3<f32>(encode(out.r), encode(out.g), encode(out.b));
}

@compute @workgroup_size(8, 8)
fn shade(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(src);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let at = vec2<i32>(id.xy);
    var c = textureLoad(src, at, 0);
    let inside = id.x >= p.region.x && id.y >= p.region.y && id.x < p.region.z && id.y < p.region.w;
    if inside && (on_line(p.origin.x, p.pitch.x, id.x) || on_line(p.origin.y, p.pitch.y, id.y)) {
        c = vec4<f32>(adapt(c.rgb), c.a);
    }
    textureStore(dst, at, c);
}
