// The GPU effect passes — one entry point per step of `crate::effects::run`,
// which is the specification these are written against (§6.4, §15 D333).
//
// **Premultiplied throughout, straight at the two ends.** vello's GPU output is
// *straight* alpha and its image atlas takes straight alpha too, so `premul` and
// `unpremul` bracket the stack; everything between them works on premultiplied
// pixels exactly as `effects.rs` does. Blurring straight alpha mixes the colour
// of fully transparent pixels into their neighbours and haloes every soft edge,
// which is the whole reason the CPU reference is premultiplied.
//
// **Every store goes through `q`.** Writing an `rgba8unorm` storage texture
// quantises to 8 bits with rounding the spec leaves to the implementation, and
// the CPU reference rounds half-up. Doing the quantisation explicitly makes the
// value already representable, so the store cannot round it a second time and
// the two backends agree byte for byte rather than within a tolerance nobody
// measured.

struct Params {
    size: vec2<u32>,
    // A shadow's offset in device pixels, already rounded by the host.
    off: vec2<i32>,
    // Kernel half-width for a blur, box half-width for a spread.
    radius: i32,
    // bit 0: the pass runs horizontally. bit 1: inner (invert the silhouette).
    // bit 2: the spread grows rather than shrinks.
    flags: u32,
    // Where this layer sits inside the texture it is read from. Sibling effect
    // layers are rasterized into **one** packed texture and one vello pass (see
    // `fx_gpu::run`), so the ingest reads from an offset; every pass after it
    // works in the layer's own buffer and ignores this.
    src_origin: vec2<i32>,
    // A shadow's colour, straight, alpha carrying its opacity.
    tint: vec4<f32>,
    // `effects::filter_matrix`'s twenty coefficients, row-major, packed five to
    // a row of four.
    m: array<vec4<f32>, 5>,
    // The domain being *read*, where that is a different grid from the one being
    // written: the fine size for `coarsen`, the coarse size for `upsample`. Every
    // other pass reads and writes the same grid and ignores it.
    //
    // **Last rather than beside `size`, which is where it belongs by meaning.**
    // The struct's offsets are a hand-written contract with `fx_gpu::Params`, and
    // inserting a field in the middle moves every one of them — a change that
    // compiles on both sides and draws nonsense.
    src_size: vec2<u32>,
};

@group(0) @binding(0) var src_a: texture_2d<f32>;
@group(0) @binding(1) var src_b: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var<uniform> p: Params;
@group(0) @binding(4) var<storage, read> kernel: array<f32>;

const HORIZONTAL: u32 = 1u;
const INNER: u32 = 2u;
const GROW: u32 = 4u;

fn inside(c: vec2<i32>) -> bool {
    return c.x >= 0 && c.y >= 0 && c.x < i32(p.size.x) && c.y < i32(p.size.y);
}

// Round half-up to the nearest 8-bit level, as the CPU reference does.
fn q(v: vec4<f32>) -> vec4<f32> {
    return floor(clamp(v, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0 + 0.5) / 255.0;
}

fn load_a(c: vec2<i32>) -> vec4<f32> {
    return textureLoad(src_a, c, 0);
}

// **Zero outside the buffer, never the edge pixel.** A layer's blur has nothing
// outside it, so clamping the edge would smear the outermost row outwards
// forever — `effects::convolve` says the same in its own words.
fn load_a_edge(c: vec2<i32>) -> vec4<f32> {
    if (!inside(c)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(src_a, c, 0);
}

fn mval(i: u32) -> f32 {
    return p.m[i / 4u][i % 4u];
}

@compute @workgroup_size(8, 8)
fn premul(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = load_a(c + p.src_origin);
    textureStore(dst, c, q(vec4<f32>(s.rgb * s.a, s.a)));
}

@compute @workgroup_size(8, 8)
fn unpremul(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = load_a(c);
    if (s.a <= 0.0) {
        // Colour under a zero alpha is not a colour; dividing by it would make a
        // NaN that the clamp then turns into white.
        textureStore(dst, c, vec4<f32>(0.0));
        return;
    }
    textureStore(dst, c, q(vec4<f32>(s.rgb / s.a, s.a)));
}

@compute @workgroup_size(8, 8)
fn copy_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    textureStore(dst, c, load_a(c));
}

// `effects::apply_matrix`: un-premultiply, transform, re-premultiply. A colour
// matrix is defined on straight colour, and running it on premultiplied data
// makes every coefficient depend on the alpha beside it — a hue shift confined
// to soft edges, i.e. exactly where nobody looks for it.
@compute @workgroup_size(8, 8)
fn matrix(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = load_a(c);
    if (s.a <= 0.0) {
        textureStore(dst, c, vec4<f32>(0.0));
        return;
    }
    let straight = vec4<f32>(s.rgb / s.a, s.a);
    var o: vec4<f32>;
    for (var row = 0u; row < 4u; row = row + 1u) {
        o[row] = mval(row * 5u + 0u) * straight.r
            + mval(row * 5u + 1u) * straight.g
            + mval(row * 5u + 2u) * straight.b
            + mval(row * 5u + 3u) * straight.a
            + mval(row * 5u + 4u);
    }
    let oa = clamp(o.a, 0.0, 1.0);
    textureStore(dst, c, q(vec4<f32>(clamp(o.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * oa, oa)));
}

// One axis of a separable gaussian. The kernel comes from `effects::kernel` on
// the host, so the weights are the same numbers the CPU convolves with —
// including its truncated-then-normalized sum, which is what keeps a large flat
// fill from stepping down by the 0.3% of the tail beyond 3σ.
@compute @workgroup_size(8, 8)
fn blur(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let horizontal = (p.flags & HORIZONTAL) != 0u;
    let step = select(vec2<i32>(0, 1), vec2<i32>(1, 0), horizontal);
    var acc = vec4<f32>(0.0);
    let n = 2 * p.radius;
    for (var t = 0; t <= n; t = t + 1) {
        acc = acc + load_a_edge(c + step * (t - p.radius)) * kernel[u32(t)];
    }
    textureStore(dst, c, q(acc));
}

// `effects::coarsen`: one thread per `radius`×`radius` block, averaging it into
// a single texel of the coarse grid that lives in the destination's top-left
// corner. Dispatched over the coarse grid, so the total work is one read per
// source pixel however large the block is — a full-size dispatch reading k²
// texels each would be k² times the work and is the obvious way to write this.
//
// **Off the right and bottom edges reads as transparent and still divides by
// k²**, matching the reference: dividing a partial block by its covered count
// instead leaves a bright rim down two sides of every resampled shadow.
//
// 🚨 **The loop is bounded by the source, and that is the whole of D403's
// promise** (§15 D743, `[S10.2-L4-02]`). The sentence above — one read per source
// pixel however large the block is — holds only while `k ≤ min(w, h)`. Past that
// the coarse grid is 1×1, one thread iterates `k²` times over a buffer of `w·h`,
// and the pass **becomes** the full-size spelling D403 says it was not writing:
// **116.3 ms** measured on a 4070 Ti for one shadow at `k = 1334`, against
// **0.45 ms** with the bound — 258× — and the figure does not move when the
// buffer does. Running to
// `hi` skips exactly the taps the old `if` skipped, so no value changes — the
// divisor stays `k²`, which is what the transparent-off-edge rule requires.
//
// `min(p.radius, src)` before the addition rather than after: `base + p.radius`
// on a block factor near `i32`'s ceiling is a wrap, and the clamped form cannot
// exceed twice the buffer. The divisor is computed in `f32` for the same reason —
// `p.radius * p.radius` in `i32` wraps above 46 341, which `MAX_SHADOW_BLOCK`
// keeps the host from sending today and this shader should not depend on.
@compute @workgroup_size(8, 8)
fn coarsen(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let src = vec2<i32>(p.src_size);
    let base = c * p.radius;
    let hi = min(src, base + min(vec2<i32>(p.radius), src));
    var acc = vec4<f32>(0.0);
    for (var y = base.y; y < hi.y; y = y + 1) {
        for (var x = base.x; x < hi.x; x = x + 1) {
            acc = acc + textureLoad(src_a, vec2<i32>(x, y), 0);
        }
    }
    textureStore(dst, c, q(acc / (f32(p.radius) * f32(p.radius))));
}

// `effects::split`: the coarse index below `v` and the fraction past it, clamped
// into `0..n`. Written as its own function for the same reason the reference has
// one — the clamping at *both* ends is what stops the last row of a shadow
// stretching, and it is three lines that are easy to get subtly different.
fn split(v: f32, n: i32) -> vec2<f32> {
    if (v <= 0.0) { return vec2<f32>(0.0, 0.0); }
    let i = floor(v);
    if (i >= f32(n - 1)) { return vec2<f32>(f32(n - 1), 0.0); }
    return vec2<f32>(i, v - i);
}

// `effects::upsample`: read the coarse grid back at full resolution, bilinearly.
// A coarse sample stands at the centre of the block it averaged, `(k-1)/2` pixels
// in — the half-block the `- half` accounts for, and the one thing here that
// would show up as a shadow creeping sideways as the zoom changes rather than as
// anything obviously wrong.
@compute @workgroup_size(8, 8)
fn upsample(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let k = f32(p.radius);
    let half = (k - 1.0) / 2.0;
    let sx = split((f32(c.x) - half) / k, i32(p.src_size.x));
    let sy = split((f32(c.y) - half) / k, i32(p.src_size.y));
    let i0 = vec2<i32>(i32(sx.x), i32(sy.x));
    let i1 = min(i0 + vec2<i32>(1), vec2<i32>(p.src_size) - vec2<i32>(1));
    let t = vec2<f32>(sx.y, sy.y);
    let a = textureLoad(src_a, vec2<i32>(i0.x, i0.y), 0);
    let b = textureLoad(src_a, vec2<i32>(i1.x, i0.y), 0);
    let cc = textureLoad(src_a, vec2<i32>(i0.x, i1.y), 0);
    let d = textureLoad(src_a, vec2<i32>(i1.x, i1.y), 0);
    let top = a + (b - a) * t.x;
    let bot = cc + (d - cc) * t.x;
    textureStore(dst, c, q(top + (bot - top) * t.y));
}

// The silhouette a shadow is cast from: the graphic's alpha, shifted by the
// offset. Colour is discarded — a shadow is its own colour everywhere, and
// `tint` supplies it.
//
// 🚨 **An inner shadow is complemented at the `tint`, not here** (§15 D742,
// `[S10.2-L1-01]`) — `effects::shadow_layer` says why at length, and this is the
// half that has to agree with it. Inverting here made this pass's edge rule
// ("outside the buffer is fully casting") the opposite of `blur`'s, `spread`'s
// and `coarsen`'s, which all read outside as transparent; on a shape whose ink is
// its own bounding box the inverted silhouette is zero everywhere the buffer
// holds, so at offset `(0, 0)` an inner shadow drew nothing at all. Both backends
// agreed, which is why `fx_gpu`'s comparison stayed green over two
// implementations of the same wrong picture.
@compute @workgroup_size(8, 8)
fn silhouette(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = c - p.off;
    var a = 0.0;
    if (inside(s)) {
        a = textureLoad(src_a, s, 0).a;
    }
    textureStore(dst, c, q(vec4<f32>(0.0, 0.0, 0.0, a)));
}

// One axis of the box dilation `spread` performs. Boxy on purpose: SVG's
// `feMorphology` is boxy too, so the export and the canvas are boxy in the
// *same* way rather than one of them being round.
@compute @workgroup_size(8, 8)
fn spread(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let horizontal = (p.flags & HORIZONTAL) != 0u;
    let grow = (p.flags & GROW) != 0u;
    let step = select(vec2<i32>(0, 1), vec2<i32>(1, 0), horizontal);
    var best = select(1.0, 0.0, grow);
    for (var d = -p.radius; d <= p.radius; d = d + 1) {
        let t = c + step * d;
        // Outside the buffer reads as empty, matching the blur's transparent
        // edge: a silhouette does not continue past the region it was
        // rasterized into.
        var v = 0.0;
        if (inside(t)) {
            v = textureLoad(src_a, t, 0).a;
        }
        best = select(min(best, v), max(best, v), grow);
    }
    // Quantised through the same 8-bit ladder the CPU walks, so a min/max over
    // one backend's rounded values cannot pick a different neighbour.
    textureStore(dst, c, q(vec4<f32>(0.0, 0.0, 0.0, best)));
}

// The shadow's colour at the silhouette's alpha, times the colour's own — and
// **the complement for an inner shadow**, which is the step `silhouette` above no
// longer performs. `INNER` reaches this pass rather than that one now.
@compute @workgroup_size(8, 8)
fn tint(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let inner = (p.flags & INNER) != 0u;
    let v = load_a(c).a;
    let a = select(v, 1.0 - v, inner) * p.tint.a;
    textureStore(dst, c, q(vec4<f32>(p.tint.rgb * a, a)));
}

// Confine an inner shadow to the layer, or the inverted silhouette paints the
// whole buffer outside the shape. `src_b` is the graphic and its alpha is the
// shape.
@compute @workgroup_size(8, 8)
fn mask_in(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let k = textureLoad(src_b, c, 0).a;
    textureStore(dst, c, q(load_a(c) * k));
}

// Source-over, both sides premultiplied: `src_a` over `src_b`.
@compute @workgroup_size(8, 8)
fn over(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = load_a(c);
    let d = textureLoad(src_b, c, 0);
    textureStore(dst, c, q(s + d * (1.0 - s.a)));
}
