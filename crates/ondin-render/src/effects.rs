//! The effect passes — the pixel arithmetic behind `ondin_core::effect` (§6.4).
//!
//! One implementation, read by both backends. That is not a stylistic
//! preference: `vello` 0.9 has no filter of any kind beyond
//! `draw_blurred_rounded_rect`, so the GPU canvas has to grow its own passes
//! whatever happens, and `vello_cpu` 0.2.0 implements only two of the four we
//! need (`GaussianBlur` and `DropShadow`, single-primitive graphs only — no
//! `ColorMatrix`, no `Composite`, so neither colour filters nor an inner shadow
//! can be expressed). ⚠️ **Re-checked at the 0.0.9 → 0.2.0 bump and unchanged
//! where it matters** (§15 D414): the filter module grew `flood`, `offset` and
//! `shift`, and neither of the two missing kinds is among them, so the argument
//! below holds for the same reason it did. Leaning on the library for the two it has would mean two
//! implementations of blur, on two backends the whole of §3 exists to keep
//! identical. So these functions are the specification, the CPU backend is the
//! reference, and the GPU shaders are written to match it pixel for pixel.
//!
//! ## Two conventions that are easy to get wrong and invisible when you do
//!
//! **Pixels are premultiplied.** Blurring straight (non-premultiplied) alpha
//! mixes the colour of fully transparent pixels into their neighbours, which
//! draws a dark or white halo around every soft edge — the classic symptom, and
//! one that reads as "the blur looks dirty" rather than as a bug in a convolution.
//! Invariant 7 governs what the *document* stores; this module is downstream of
//! `color`, where the conversion has already happened.
//!
//! **Filters run in sRGB, not linear.** SVG's `color-interpolation-filters`
//! defaults to `linearRGB`, so the SVG writer must say `sRGB` explicitly on every
//! filter it emits — which `svg::adjust_def` already does for image adjustments,
//! and which the effect filters have to do for the same reason. Blurring in
//! linear light is arguably more correct and is emphatically not what any design
//! tool does; matching Figma here matters more than matching physics, and the
//! three writers agreeing matters more than either.

use ondin_core::effect::{Effect, EffectKind, Filters};

/// A premultiplied sRGB RGBA8 image the passes work over.
///
/// Deliberately not `vello_cpu::Pixmap` or a `wgpu::Texture`: this is the seam
/// where the two backends meet, so the type has to be one neither owns. Both can
/// hand over a `&mut [u8]` of the right length.
pub struct Surface<'a> {
    pub px: &'a mut [u8],
    pub w: usize,
    pub h: usize,
}

impl Surface<'_> {
    /// Panics if the buffer is not exactly `w * h * 4` bytes.
    ///
    /// A debug assertion would be the softer choice and the wrong one: the whole
    /// class of bug here is an off-by-one on a stride, which in release silently
    /// shears the image instead of failing.
    pub fn new<'a>(px: &'a mut [u8], w: usize, h: usize) -> Surface<'a> {
        assert_eq!(px.len(), w * h * 4, "surface buffer is w*h*4 bytes");
        Surface { px, w, h }
    }
}

/// The 4×5 colour matrix for a [`Filters`] row, in `feColorMatrix` order.
///
/// Twenty coefficients, row-major, each row `[r g b a offset]` — the same layout
/// SVG takes and the same one `ondin_core::AdjustPipeline` already uses for image
/// adjustments, so the SVG writer can emit this array with no rearranging.
///
/// **Composed in the order brightness → contrast → saturation → hue**, which is
/// the order the panel lists them in. Matrix composition does not commute, so
/// this order is part of the specification rather than an implementation detail:
/// saturating a brightened colour is not the same as brightening a saturated one,
/// and the canvas, the raster export and the SVG file have to pick the same one.
pub fn filter_matrix(f: &Filters) -> [f32; 20] {
    let mut m = IDENTITY;
    // Brightness and contrast are per-channel and independent of the others, so
    // they are a scale and a scale-about-the-midpoint respectively.
    let b = f.brightness as f32;
    m = compose(
        &[
            b, 0.0, 0.0, 0.0, 0.0, //
            0.0, b, 0.0, 0.0, 0.0, //
            0.0, 0.0, b, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, 0.0,
        ],
        &m,
    );
    let c = f.contrast as f32;
    // The offset that keeps 0.5 fixed: `c * x + (0.5 - 0.5 * c)`.
    let o = 0.5 - 0.5 * c;
    m = compose(
        &[
            c, 0.0, 0.0, 0.0, o, //
            0.0, c, 0.0, 0.0, o, //
            0.0, 0.0, c, 0.0, o, //
            0.0, 0.0, 0.0, 1.0, 0.0,
        ],
        &m,
    );
    // Saturation and hue are the two `feColorMatrix` shorthands written out. The
    // luminance weights are the sRGB ones the spec fixes — 0.213/0.715/0.072 —
    // and not the 0.299/0.587/0.114 of the older NTSC formula, which is the
    // likelier thing to type from memory and is a visibly different grey.
    let s = f.saturation as f32;
    m = compose(&saturate(s), &m);
    m = compose(&hue_rotate(f.hue as f32), &m);
    m
}

const IDENTITY: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// sRGB luminance weights, per the Filter Effects spec.
const LUMA: [f32; 3] = [0.213, 0.715, 0.072];

fn saturate(s: f32) -> [f32; 20] {
    let [lr, lg, lb] = LUMA;
    [
        lr + (1.0 - lr) * s,
        lg * (1.0 - s),
        lb * (1.0 - s),
        0.0,
        0.0,
        lr * (1.0 - s),
        lg + (1.0 - lg) * s,
        lb * (1.0 - s),
        0.0,
        0.0,
        lr * (1.0 - s),
        lg * (1.0 - s),
        lb + (1.0 - lb) * s,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}

fn hue_rotate(degrees: f32) -> [f32; 20] {
    let (s, c) = degrees.to_radians().sin_cos();
    let [lr, lg, lb] = LUMA;
    // The spec's matrix, written with the luminance weights named rather than
    // inlined so the two shorthands above cannot drift apart.
    [
        lr + c * (1.0 - lr) + s * -lr,
        lg + c * -lg + s * -lg,
        lb + c * -lb + s * (1.0 - lb),
        0.0,
        0.0,
        lr + c * -lr + s * 0.143,
        lg + c * (1.0 - lg) + s * 0.140,
        lb + c * -lb + s * -0.283,
        0.0,
        0.0,
        lr + c * -lr + s * -(1.0 - lr),
        lg + c * -lg + s * lg,
        lb + c * (1.0 - lb) + s * lb,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}

/// `b` applied after `a`, i.e. the matrix for "do `a`, then do `b`".
fn compose(b: &[f32; 20], a: &[f32; 20]) -> [f32; 20] {
    let mut out = [0.0f32; 20];
    for row in 0..4 {
        for col in 0..4 {
            let mut v = 0.0;
            for k in 0..4 {
                v += b[row * 5 + k] * a[k * 5 + col];
            }
            out[row * 5 + col] = v;
        }
        // The translation column picks up `b`'s own offset plus `b`'s linear part
        // applied to `a`'s offsets.
        let mut t = b[row * 5 + 4];
        for k in 0..4 {
            t += b[row * 5 + k] * a[k * 5 + 4];
        }
        out[row * 5 + 4] = t;
    }
    out
}

/// Apply a colour matrix in place.
///
/// **Un-premultiplies, transforms, re-premultiplies.** A colour matrix is
/// defined on straight colour, and running it on premultiplied data makes every
/// coefficient depend on the alpha it happens to sit next to — which shows up as
/// a hue shift confined to soft edges, i.e. exactly where nobody looks for it.
pub fn apply_matrix(s: &mut Surface<'_>, m: &[f32; 20]) {
    for px in s.px.chunks_exact_mut(4) {
        let a = px[3] as f32 / 255.0;
        if a == 0.0 {
            // Nothing to transform, and dividing by it would produce a colour
            // the re-multiply then throws away — but not before it has become a
            // NaN and poisoned the clamp.
            continue;
        }
        let r = px[0] as f32 / 255.0 / a;
        let g = px[1] as f32 / 255.0 / a;
        let b = px[2] as f32 / 255.0 / a;
        let out = [
            m[0] * r + m[1] * g + m[2] * b + m[3] * a + m[4],
            m[5] * r + m[6] * g + m[7] * b + m[8] * a + m[9],
            m[10] * r + m[11] * g + m[12] * b + m[13] * a + m[14],
            m[15] * r + m[16] * g + m[17] * b + m[18] * a + m[19],
        ];
        let oa = out[3].clamp(0.0, 1.0);
        for i in 0..3 {
            px[i] = ((out[i].clamp(0.0, 1.0) * oa) * 255.0 + 0.5) as u8;
        }
        px[3] = (oa * 255.0 + 0.5) as u8;
    }
}

/// The half-width of the kernel a deviation of `sigma` needs, in pixels.
///
/// `ceil(3σ)`, matching `ondin_core::effect::BLUR_CUTOFF` and `vello_common`'s
/// own `bounds_expansion`. Three places, one number, and this is the only one
/// that turns it into an integer.
///
/// ⚠️ **Capped at `ondin_core::effect::MAX_KERNEL_RADIUS`, and this is the one
/// place both backends' blurs go through** (§15 D454). `[S10.2-L1-03]`: the
/// `.ceil() as usize` saturates at `usize::MAX` above σ ≈ 6.1e18, so
/// [`kernel`]'s `Vec::with_capacity(2 * r + 1)` was a capacity-overflow panic;
/// below that it was simply an allocation nobody has — **24 GB at σ = 1e9**, from
/// a `<feDropShadow stdDeviation="1e30">` that imports with both report lists
/// empty and reloads cleanly. `NaN` and the infinities fall out of the `<= 0.0`
/// test as `0` and out of `min` as the cap respectively, so neither reaches the
/// multiply.
///
/// **Here rather than at either caller, and that is the whole reason it closes
/// the GPU too.** The CPU path reaches the allocation behind `coarsen`'s `Θ(k)`
/// walk — a three-minute hang first; the GPU issues coarsen as a *dispatch* and
/// calls this on the host almost immediately, so the same document panicked one
/// backend at once and hung the other. One guard, one function, both.
pub fn kernel_radius(sigma: f32) -> usize {
    if sigma.is_nan() || sigma <= 0.0 {
        return 0;
    }
    let r = (f64::from(sigma) * ondin_core::effect::BLUR_CUTOFF).ceil();
    r.min(ondin_core::effect::MAX_KERNEL_RADIUS as f64) as usize
}

/// A normalized 1-D gaussian of the given deviation, `2 * radius + 1` long.
///
/// **Public because the GPU passes convolve with these exact numbers** — the host
/// builds the kernel and hands it to the shader as a buffer, rather than the
/// shader evaluating the same formula a second time. Two implementations of a
/// gaussian is precisely what `crate::fx_gpu` exists not to be.
pub fn kernel(sigma: f32) -> Vec<f32> {
    let r = kernel_radius(sigma);
    let mut k = Vec::with_capacity(2 * r + 1);
    let denom = 2.0 * sigma * sigma;
    for i in 0..=2 * r {
        let x = i as f32 - r as f32;
        k.push((-(x * x) / denom).exp());
    }
    // Normalized *after* truncation, not analytically. The tail beyond 3σ is
    // ~0.3% of the area, and a kernel that sums to 0.997 dims everything it
    // touches by that much — which on a large flat fill is a visible step at the
    // blur's edge rather than a rounding error.
    let sum: f32 = k.iter().sum();
    for v in &mut k {
        *v /= sum;
    }
    k
}

/// Gaussian blur, in place, with independent deviations per axis.
///
/// Separable: a horizontal pass then a vertical one, which is O(w·h·r) rather
/// than O(w·h·r²) and is exact for a gaussian (the only kernel for which
/// separating is not an approximation).
///
/// **Edges are transparent-black, not clamped.** A layer's blur has nothing
/// outside it, so extending the edge pixel would smear the outermost row of the
/// artwork outwards forever. The caller owes the surface enough padding to hold
/// the reach — [`ondin_core::effect::reach`] is that number — and gets a blur
/// that fades to nothing at the buffer's own border if it does not.
pub fn blur(s: &mut Surface<'_>, sigma_x: f32, sigma_y: f32) {
    if sigma_x > 0.0 {
        let k = kernel(sigma_x);
        convolve(s, &k, true);
    }
    if sigma_y > 0.0 {
        let k = kernel(sigma_y);
        convolve(s, &k, false);
    }
}

/// The widest or tallest scratch buffer an effect layer may be rasterized into.
///
/// **One fixed number rather than the machine's own maximum**, which is the
/// maintainer's decision and the only spelling that keeps §15 D395's requirement:
/// the two backends must choose the **same** buffer for one drawing, or the same
/// shadow is blurred by different amounts on the canvas and in the export. A
/// device limit read from `wgpu` is a property of whoever opened the file, so a
/// document would look different on two machines and neither would be wrong.
///
/// **8192 because it is the floor, not because it is this card's ceiling.** It is
/// `wgpu::Limits::default()`'s `max_texture_dimension_2d`, which is what the app
/// requests and therefore what `device.limits()` reports here whatever the card
/// could manage; it is also vello's `MAX_ATLAS_SIZE`, so the one number bounds
/// both the effect scratch textures and the image atlas. A machine that could do
/// 16384 resamples very slightly sooner than it must, which is the price of the
/// two backends agreeing.
///
/// ⚠️ **Not `u16::MAX`, which is what both `push_effect_layer`s used to say**
/// (§15 D744, `[S11.2-L1-01]`). That number bounds nothing either backend can
/// build: on the GPU it is eight times the texture limit, so an ordinary **wide,
/// thin** layer — a 1600 × 12 banner at 6×, whose buffer measures 9672 × 144 —
/// allocated scratch textures past the limit and **panicked in `wgpu` validation**
/// where the CPU drew it correctly; on the CPU it is 127 pixels past
/// [`crate::cpu::MAX_RASTER_SIDE`], whose own doc says *"the one value the backend
/// cannot take"* twenty lines above the site that took it.
///
/// ⚠️ **Not the finding's own case, which does not reproduce**: at
/// `offset = 1000` and 8× the buffer comes back at exactly 8192, the viewport to
/// the pixel, because `effect::grow_to_escape` grows by the *opposite* side's
/// escape and intersects with the layer's own ink, and `buffer_box_within` then
/// trims by **area**. `gpu_effects.rs`'s
/// `a_layer_needing_more_than_the_texture_limit_still_draws` has the measurements.
pub const MAX_EFFECT_BUFFER_SIDE: u32 = 8192;

/// The scratch buffer an effect layer is rasterized into: its size in pixels and
/// the downscale that got it there.
///
/// `extent` is the layer's device-space box after
/// `ondin_core::effect::buffer_box_within` has clipped it, and `k` is
/// `buffer_downscale`'s answer for the stack. **Both backends call this rather
/// than doing the arithmetic twice**, which is the requirement §15 D395 turns on
/// and which the two copies of it had already drifted from in the same direction.
///
/// **Too big is answered by resampling, never by truncating** — the maintainer's
/// decision, and it is the mechanism D395 already uses for a wide blur rather
/// than a new one: `k` grows until both sides fit, the layer is rasterized coarse
/// and scaled back up on the way out. Truncating is what the old `.min()` did, and
/// it is wrong in a way that reads as a rendering bug rather than a bound — the
/// silhouette a shadow is cast from is read out of this buffer, so a truncated
/// one cuts the shape off and casts a shadow from the stump.
///
/// The returned `k` is the one the caller must use for its own `base` transform;
/// dropping it and keeping `buffer_downscale`'s puts the artwork at a different
/// scale from the buffer it is being drawn into.
pub fn effect_buffer(extent: (f64, f64), k: f64) -> ((u32, u32), f64) {
    let cap = f64::from(MAX_EFFECT_BUFFER_SIDE);
    // `max(1.0)` never shrinks `k`: a layer already inside the cap is rasterized
    // at exactly the resolution `buffer_downscale` asked for, so this changes
    // nothing at all for every document that worked before.
    //
    // ⚠️ **A non-finite extent falls through every branch safely and on purpose.**
    // `f64::max` returns the other operand against a `NaN`, so `fit` is 1.0; an
    // infinite extent gives an infinite `fit` and the `min(cap)` below answers the
    // cap. Both are the conservative direction — a buffer, not a crash.
    let fit = (extent.0 / cap).max(extent.1 / cap).max(1.0);
    let k = if fit.is_finite() { k * fit } else { k };
    let side = |e: f64| (e / k).ceil().max(1.0).min(cap) as u32;
    ((side(extent.0), side(extent.1)), k)
}

/// The dimensions of the coarse grid a `k`-pixel block resample of `w`×`h` has.
///
/// **Public because the GPU dispatches over exactly this grid** — one thread per
/// block — and a backend that rounded the last partial block the other way would
/// blur a different number of samples.
pub fn coarse_size(w: usize, h: usize, k: usize) -> (usize, usize) {
    (w.div_ceil(k), h.div_ceil(k))
}

/// Box-average `s` into `k`×`k` blocks, one pixel per block.
///
/// **Pixels off the right and bottom edges count as transparent**, so the divisor
/// is `k²` for every block including a partial one. Dividing by the *covered*
/// count instead would make the last block as bright as a full one, which draws a
/// bright rim along two edges of every heavily blurred shadow.
///
/// ⚠️ **Both loops stop at the source rather than at the block** (§15 D743,
/// `[S10.2-L4-02]`). The `if … continue` this replaces was correct and cost `k²`
/// iterations per block whatever the buffer held; once `k` passes `min(w, h)`
/// there is one block, so a 64×48 surface at `k = 1334` walked 1.78 M iterations
/// to read 3 072 pixels. The optimizer hoisted most of *that* away — the finding
/// measured the **old** loop here at 41.6 ns per unit of `k`, i.e. Θ(k) rather
/// than Θ(k²), where the code below is neither and costs `w · h` — but **the
/// shader had no optimizer and paid the whole square**,
/// and a reference whose cost model differs from the implementation it is a
/// reference *for* is worth less. Skipping exactly the same taps, so no value
/// moves.
///
/// `saturating_mul` because `k` is this function's parameter rather than
/// `shadow_downscale`'s output: the caller in `shadow_layer` is capped at
/// `MAX_SHADOW_BLOCK`, and a `pub fn` is not.
pub fn coarsen(s: &Surface<'_>, k: usize) -> Vec<u8> {
    let (cw, ch) = coarse_size(s.w, s.h, k);
    let mut out = vec![0u8; cw * ch * 4];
    let n = (k as f32) * (k as f32);
    for cy in 0..ch {
        for cx in 0..cw {
            let mut acc = [0.0f32; 4];
            for y in cy * k..(cy + 1).saturating_mul(k).min(s.h) {
                for x in cx * k..(cx + 1).saturating_mul(k).min(s.w) {
                    let p = (y * s.w + x) * 4;
                    for (a, v) in acc.iter_mut().zip(&s.px[p..p + 4]) {
                        *a += f32::from(*v);
                    }
                }
            }
            let p = (cy * cw + cx) * 4;
            for (o, a) in out[p..p + 4].iter_mut().zip(acc) {
                *o = (a / n + 0.5) as u8;
            }
        }
    }
    out
}

/// Read a `k`-block coarse grid back at full resolution, bilinearly.
///
/// A coarse sample stands at the **centre** of the block it averaged, `(k-1)/2`
/// pixels in, which is what the `- (k - 1) / 2` here is; getting that wrong
/// shifts every shadow by half a block, and a block is large exactly when the
/// shadow is soft enough to hide it — so the error would show up as a shadow that
/// creeps as you zoom rather than as anything obviously broken.
///
/// Outside the outermost centres the sample is clamped, which is the right edge
/// rule here for the opposite reason [`convolve`]'s is: this is reading back a
/// field that has already been blurred to nothing at its own border, so clamping
/// extends transparency rather than smearing ink.
pub fn upsample(s: &mut Surface<'_>, coarse: &[u8], k: usize) {
    let (cw, ch) = coarse_size(s.w, s.h, k);
    let half = (k as f32 - 1.0) / 2.0;
    for y in 0..s.h {
        let v = (y as f32 - half) / k as f32;
        let (y0, ty) = split(v, ch);
        let y1 = (y0 + 1).min(ch - 1);
        for x in 0..s.w {
            let u = (x as f32 - half) / k as f32;
            let (x0, tx) = split(u, cw);
            let x1 = (x0 + 1).min(cw - 1);
            let p = (y * s.w + x) * 4;
            for c in 0..4 {
                let at = |cx: usize, cy: usize| coarse[(cy * cw + cx) * 4 + c] as f32;
                let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
                let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
                s.px[p + c] = (top + (bot - top) * ty + 0.5) as u8;
            }
        }
    }
}

/// The coarse index below `v` and the fraction past it, clamped into `0..n`.
fn split(v: f32, n: usize) -> (usize, f32) {
    if v <= 0.0 {
        return (0, 0.0);
    }
    let i = v.floor();
    if i >= (n - 1) as f32 {
        return (n - 1, 0.0);
    }
    (i as usize, v - i)
}

/// A gaussian blur computed on a `k`-pixel grid: coarsen, blur at `σ/k`, read
/// back bilinearly.
///
/// `sigma_x`/`sigma_y` are the **coarse** deviations — already divided — because
/// the caller is the one that chose `k` from them
/// (`ondin_core::effect::shadow_downscale`).
///
/// This is what pins a shadow's cost: the convolution runs over `w·h/k²` pixels
/// with a kernel that no longer grows, and the two resamples are one pass each
/// (§15 D403).
pub fn blur_coarse(s: &mut Surface<'_>, sigma_x: f32, sigma_y: f32, k: usize) {
    let (cw, ch) = coarse_size(s.w, s.h, k);
    let mut coarse = coarsen(s, k);
    blur(&mut Surface::new(&mut coarse, cw, ch), sigma_x, sigma_y);
    upsample(s, &coarse, k);
}

fn convolve(s: &mut Surface<'_>, k: &[f32], horizontal: bool) {
    let r = (k.len() - 1) / 2;
    let (w, h) = (s.w, s.h);
    let mut out = vec![0u8; s.px.len()];
    // The outer axis is the one the kernel does *not* run along, so each inner
    // run walks contiguous memory in the horizontal pass.
    let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
    for o in 0..outer {
        for i in 0..inner {
            let mut acc = [0.0f32; 4];
            for (t, weight) in k.iter().enumerate() {
                let d = i as isize + t as isize - r as isize;
                if d < 0 || d >= inner as isize {
                    continue;
                }
                let (x, y) = if horizontal {
                    (d as usize, o)
                } else {
                    (o, d as usize)
                };
                let p = (y * w + x) * 4;
                for (a, v) in acc.iter_mut().zip(&s.px[p..p + 4]) {
                    *a += *v as f32 * weight;
                }
            }
            let (x, y) = if horizontal { (i, o) } else { (o, i) };
            let p = (y * w + x) * 4;
            for (o, a) in out[p..p + 4].iter_mut().zip(acc) {
                *o = (a + 0.5).min(255.0) as u8;
            }
        }
    }
    s.px.copy_from_slice(&out);
}

/// The device-space deviations a blur of `radius` document units needs, given the
/// scale the surface was rasterized at.
///
/// **Two numbers, because a non-uniform scale blurs unevenly** — and this is
/// exactly where the project's transform rule bites in an unexpected direction.
/// A node's transform carries no scale (§5.6), but the *viewport* does, and so
/// does an export at 2×; a skew in an ancestor's transform contributes too.
/// Passing one averaged sigma is the obvious shortcut and it makes a blur on a
/// skewed layer round rather than lean.
pub fn device_sigma(radius: f64, scale_x: f64, scale_y: f64) -> (f32, f32) {
    let d = ondin_core::effect::deviation(radius);
    ((d * scale_x.abs()) as f32, (d * scale_y.abs()) as f32)
}

/// Apply a whole effect stack to a rasterized layer, in place.
///
/// `scale` is device pixels per document unit along each axis, so an authored
/// radius becomes a kernel and an authored offset becomes pixels.
///
/// **Two stages, matching the SVG writer exactly** (`svg::effect_def`): `Filters`
/// and `LayerBlur` transform the layer's own appearance in list order, then every
/// shadow is cast from *that* and merged behind or in front. The one-fold
/// version, where each effect reads whatever the previous produced, makes an
/// inner shadow following a drop shadow take its silhouette from the layer plus
/// the shadow — and the two writers would then disagree about a file, which is
/// the failure this whole module is arranged to prevent.
pub fn run(s: &mut Surface<'_>, effects: &[Effect], scale: (f64, f64)) {
    for e in effects.iter().filter(|e| e.visible) {
        match &e.kind {
            EffectKind::Filters(f) if !f.is_neutral() => apply_matrix(s, &filter_matrix(f)),
            EffectKind::LayerBlur { radius } if *radius > 0.0 => {
                let (sx, sy) = device_sigma(*radius, scale.0, scale.1);
                blur(s, sx, sy);
            }
            _ => {}
        }
    }

    // The layer as the shadows will see it, kept aside before anything is
    // composited over or under it.
    let graphic = s.px.to_vec();
    let mut out = graphic.clone();
    for e in effects.iter().filter(|e| e.visible) {
        let (sh, inner) = match &e.kind {
            EffectKind::DropShadow(sh) => (sh, false),
            EffectKind::InnerShadow(sh) => (sh, true),
            _ => continue,
        };
        if sh.color.components[3] <= 0.0 {
            continue;
        }
        let layer = shadow_layer(&graphic, s.w, s.h, sh, scale, inner);
        if inner {
            // Over the accumulated result: an inner shadow is drawn on top of the
            // layer, and a second one on top of the first.
            over(&mut out, &layer);
        } else {
            // Behind it: the shadow is the destination and what is already there
            // is composited onto it, then the pair becomes the new result.
            let mut behind = layer;
            over(&mut behind, &out);
            out = behind;
        }
    }
    s.px.copy_from_slice(&out);
}

/// One shadow's own premultiplied layer, the size of the surface.
fn shadow_layer(
    graphic: &[u8],
    w: usize,
    h: usize,
    sh: &ondin_core::effect::Shadow,
    scale: (f64, f64),
    inner: bool,
) -> Vec<u8> {
    // The silhouette: the layer's alpha, shifted by the offset. Colour is
    // discarded — a shadow is its own colour everywhere.
    //
    // 🚨 **The inversion an inner shadow needs happens at the *tint*, not here**
    // (§15 D742, `[S10.2-L1-01]`). Inverting first put the buffer's edge rule and
    // the offset lookup's edge rule in contradiction: the lookup called outside
    // the buffer *fully casting* while [`convolve`], [`spread_alpha`] and
    // [`coarsen`] all read it as *transparent*, which for an inverted silhouette
    // means the opposite thing. On any shape whose ink is its own bounding box —
    // a rect, a frame, an image, a text node — the inverted silhouette is zero in
    // every pixel the buffer holds, so at offset `(0, 0)` the lookup branch never
    // fired and the render was pixel-identical to no effect at all.
    //
    // Inverting last needs no edge flag anywhere, because every pass between here
    // and the tint is **linear or a max/min with a matching identity**, and each
    // commutes with the complement: `blur(255−v)` padded with 255 is exactly
    // `255 − blur(v)` padded with 0 for a normalized kernel, `coarsen` is a box
    // average of the same shape (partial blocks included, which is the `k²`
    // divisor the record argued about), `upsample` is bilinear with weights
    // summing to one, and a dilation of the complement is the erosion of the
    // original. So the four passes keep the one edge rule they document, and the
    // shape's own alpha is what travels through them.
    //
    // ⚠️ It also puts [`spread_alpha`]'s `grow` the right way up. That flag reads
    // `(spread > 0) != inner`, which is the rule for the **shape's** alpha — grow
    // it for a drop shadow, erode it for an inner one — and it was being applied
    // to an already-inverted buffer, i.e. backwards. Nothing pinned the direction;
    // `a_positive_spread_thickens_an_inner_shadow_inwards` does now.
    let (dx, dy) = (
        (sh.offset.x * scale.0).round() as isize,
        (sh.offset.y * scale.1).round() as isize,
    );
    let mut mask = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x as isize - dx, y as isize - dy);
            let a = if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize {
                // Outside the buffer there is no layer, for either kind of
                // shadow. For an inner one the complement below turns that into
                // "outside the shape, i.e. fully casting" — which is the same
                // answer the lookup used to give here, now given once for every
                // pass rather than only for this one.
                0
            } else {
                graphic[(sy as usize * w + sx as usize) * 4 + 3]
            };
            let p = (y * w + x) * 4;
            // Premultiplied black: colour comes from the flood below, so only the
            // alpha channel carries anything here.
            mask[p + 3] = a;
        }
    }
    let mut surface = Surface {
        px: &mut mask,
        w,
        h,
    };
    // Spread dilates or erodes the silhouette before it is blurred. Skipped
    // rather than approximated: see the note on `spread_alpha`.
    if sh.spread != 0.0 {
        spread_alpha(
            &mut surface,
            (sh.spread * scale.0, sh.spread * scale.1),
            inner,
        );
    }
    if sh.blur > 0.0 {
        let (sx, sy) = device_sigma(sh.blur, scale.0, scale.1);
        // **The silhouette is resampled where the layer's buffer may not be**
        // (§15 D403). `shadow_downscale` is the only thing bounding a shadow's
        // kernel, since D395's buffer downscale is forbidden to a shadow — the
        // artwork over it stays sharp, this mask does not.
        let k = ondin_core::effect::shadow_downscale(sx.max(sy) as f64) as usize;
        if k > 1 {
            blur_coarse(&mut surface, sx / k as f32, sy / k as f32, k);
        } else {
            blur(&mut surface, sx, sy);
        }
    }
    // Tint: the shadow's colour at the silhouette's alpha, times the colour's own
    // — **complementing the silhouette first for an inner shadow**, which is the
    // step the buffer build above no longer does. `255 - v` on a `u8` is exact, so
    // this costs no precision the old spelling had.
    let [r, g, b, ca] = sh.color.components;
    for px in mask.chunks_exact_mut(4) {
        let v = if inner { 255 - px[3] } else { px[3] };
        let a = (v as f32 / 255.0) * ca;
        px[0] = (r * a * 255.0 + 0.5) as u8;
        px[1] = (g * a * 255.0 + 0.5) as u8;
        px[2] = (b * a * 255.0 + 0.5) as u8;
        px[3] = (a * 255.0 + 0.5) as u8;
    }
    if inner {
        // Confine it to the layer, or the inverted silhouette paints the whole
        // buffer outside the shape. `graphic`'s alpha is the shape.
        for (px, src) in mask.chunks_exact_mut(4).zip(graphic.chunks_exact(4)) {
            let k = src[3] as f32 / 255.0;
            for v in px.iter_mut() {
                *v = (*v as f32 * k + 0.5) as u8;
            }
        }
    }
    mask
}

/// A spread's per-axis half-width in device pixels, **bounded by the buffer it
/// runs in** — the pair both backends' dilation loops count to.
///
/// 🚨 **`n` is device pixels and the only cap on it was in document units**
/// (§15 D615, `[S10.2-L4-04]`, **G10**). `shadow_layer` passes
/// `sh.spread × scale`, so the authored number is multiplied by the zoom, and the
/// one bound anywhere is `inspector::MAX_OFFSET = 10_000` — whose doc calls it
/// *"the reach of an offset or a spread, in both directions"* and which the panel
/// applies to the **Offset** fields as well. An offset costs nothing: it is a
/// shifted texture read, O(1) per pixel. A spread is Θ(n) per pixel. Two fields
/// sharing one range is exactly the G10 shape — *the cap measures how far the
/// shadow goes and what grows is how many samples each pixel takes*. Measured in
/// release on a real adapter, one shadow with `blur: 0`: **23.8 ms** at
/// `spread: 200` zoom 256, **117.1 ms** at 1 000 zoom 256, **277.9 ms** at 10 000
/// zoom 64, and **1 092.7 ms** at 10 000 zoom 256 — one shadow, one frame. ⚠️ The
/// zoom-64 row is here because the pair matters: the cost tracks `spread × zoom`
/// and neither factor alone, which is what makes a document-unit cap the wrong
/// instrument.
///
/// **The cap is exactly lossless, which is what makes it the fix rather than a
/// clamp.** Both loops read outside the buffer as **empty**, so once the window
/// covers the whole line the answer stops moving: a dilation takes the line's
/// maximum and can find nothing further, and an erosion has already met a zero.
/// At `n ≥ extent − 1` that is true of every pixel on the axis. So this changes
/// no picture at all — including the ones that render today — and only stops the
/// work that could not have changed one.
///
/// ⚠️ **What it does not do**, and the record should not read as if it did:
/// the cost is still Θ(w·h·n) for `n` up to the buffer's own size, which is the
/// **1 026.9 ms at 960×540, n = 500** the finding measured on the CPU export path.
/// Bounding *that* means running the dilation on `shadow_downscale`'s coarse grid
/// the way `blur_coarse` does — a real approximation, guarded by
/// `fx_gpu::a_resampled_shadow_with_a_spread_matches_the_reference`, and a
/// separate decision. This removes the **unboundedness**; the remaining growth is
/// bounded by the viewport.
pub(crate) fn spread_steps(r: (f64, f64), w: usize, h: usize) -> (i32, i32) {
    let step = |v: f64, extent: usize| {
        let n = v.abs().round();
        // `saturating_sub` rather than `- 1`: a zero-width buffer has no axis to
        // dilate along, and `0 - 1` on a `usize` is the wrong kind of surprise.
        n.min(extent.saturating_sub(1) as f64) as i32
    };
    (step(r.0, w), step(r.1, h))
}

/// Grow (or shrink) an alpha silhouette by `r` device pixels — **`r.0` across,
/// `r.1` down**.
///
/// A box dilation, which is what SVG's `feMorphology` does too — so the export
/// and the canvas are boxy in the *same* way rather than one of them being round.
/// Matching the writer that cannot do better is worth more here than being right
/// alone.
///
/// 🚨 **Two half-widths, for the reason [`device_sigma`] takes two deviations**
/// (§15 D601). This took one — `sh.spread * scale.0.max(scale.1)` — while the
/// blur beside it took the pair on purpose, so a spread on a non-uniformly scaled
/// layer was drawn *scale-ratio* times too thick on the short axis.
/// `[S10.2-L1-05]`'s own measurement — **the finding's numbers rather than this
/// session's**, which are `a_spread_follows_the_scale_on_each_axis_the_way_the_blur_does`'s
/// below — a 40×40 square with `spread: 10, blur: 0, offset: 0` through
/// `effects::run`:
///
/// | `scale` | drawn right | drawn down | correct |
/// | --- | --- | --- | --- |
/// | (1, 1) | 10 px | 10 px | 10 / 10 |
/// | (4, 1) | **40 px** | **40 px** | 40 / **10** |
/// | (1, 4) | **40 px** | **40 px** | **10** / 40 |
///
/// ⚠️ **And the excess was outside the bounds the model had allocated for it.**
/// `ondin_core::effect::escape` adds the spread in *document* units per side, so
/// it maps to `spread × scale` per axis and said 40/10 for the middle row —
/// correct, and disagreeing with the ink. `grow_to_escape` then intersects the
/// difference away, giving a ruled-line clip on the short axis. §5.9's *"too small
/// is the one error these bounds may not make"* was kept by the bounds and broken
/// by the drawing.
///
/// ⚠️ **A non-uniform pair is not hypothetical**: `push_effect_layer` takes the
/// two *column norms* of the composed device transform, on purpose and with a
/// comment saying why, and a skew is a shipped gesture (`tools::side_shear`).
/// `[S10.2-L1-05]`.
///
/// The separable two-pass loop below already had the structure for this — each
/// pass reads one axis — so the change is which number each pass takes.
fn spread_alpha(s: &mut Surface<'_>, r: (f64, f64), inner: bool) {
    let n = spread_steps(r, s.w, s.h);
    let n = (n.0 as isize, n.1 as isize);
    if n.0 == 0 && n.1 == 0 {
        return;
    }
    // Positive spread grows a drop shadow and thickens an inner one, which are
    // opposite operations on the silhouette each is cast from. **The sum, not
    // either one**: a scale of zero on an axis zeroes that radius, and the sign
    // has to come from whichever half is still carrying it.
    let grow = (r.0 + r.1 > 0.0) != inner;
    // **Separable, like the gaussian above.** A box min/max over a square is the
    // 1-D one along each axis in turn, which turns O(r²) per pixel into O(r) —
    // and the naive version is not a theoretical cost: a 20px spread is 1,681
    // samples per pixel, so a frame-sized layer runs into hundreds of millions of
    // reads for one shadow.
    for horizontal in [true, false] {
        // The pass's own half-width. Zero is the identity for both a dilation and
        // an erosion, so an axis with no spread on it skips the copy as well.
        let n = if horizontal { n.0 } else { n.1 };
        if n == 0 {
            continue;
        }
        let src: Vec<u8> = s.px.to_vec();
        let (outer, inner_len) = if horizontal { (s.h, s.w) } else { (s.w, s.h) };
        for o in 0..outer {
            for i in 0..inner_len {
                let mut best: u8 = if grow { 0 } else { 255 };
                for d in -n..=n {
                    let t = i as isize + d;
                    // Outside the buffer reads as empty, matching the blur's
                    // transparent-black edge: a silhouette does not continue past
                    // the region it was rasterized into.
                    let v = if t < 0 || t >= inner_len as isize {
                        0
                    } else {
                        let (x, y) = if horizontal {
                            (t as usize, o)
                        } else {
                            (o, t as usize)
                        };
                        src[(y * s.w + x) * 4 + 3]
                    };
                    best = if grow { best.max(v) } else { best.min(v) };
                }
                let (x, y) = if horizontal { (i, o) } else { (o, i) };
                s.px[(y * s.w + x) * 4 + 3] = best;
            }
        }
    }
}

/// Source-over, both sides premultiplied.
fn over(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let ia = 1.0 - s[3] as f32 / 255.0;
        for i in 0..4 {
            d[i] = (s[i] as f32 + d[i] as f32 * ia + 0.5).min(255.0) as u8;
        }
    }
}

/// Whether `effects` contains anything that would change a pixel.
///
/// **Not `!effects.is_empty()`.** A stack of hidden entries, or a `Filters` row
/// sitting on its neutral values, has to answer `false` here — or every layer
/// that has ever had an effect added and turned off pays for an offscreen buffer
/// and a composite on every frame, forever.
pub fn any_ink(effects: &[Effect]) -> bool {
    effects.iter().any(|e| {
        e.visible
            && match &e.kind {
                EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => {
                    s.color.components[3] > 0.0
                }
                EffectKind::LayerBlur { radius } => *radius > 0.0,
                EffectKind::Filters(f) => !f.is_neutral(),
            }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::effect::Shadow;
    use ondin_core::peniko::Color;

    fn solid(w: usize, h: usize, rgba: [u8; 4]) -> Vec<u8> {
        rgba.iter().copied().cycle().take(w * h * 4).collect()
    }

    /// A neutral row must compose to the identity. Any of the four shorthands
    /// written with a sign error fails this, and it is the cheapest possible
    /// guard on twenty hand-typed coefficients.
    #[test]
    fn a_neutral_filters_row_is_the_identity_matrix() {
        let m = filter_matrix(&Filters::default());
        for (i, (got, want)) in m.iter().zip(IDENTITY.iter()).enumerate() {
            assert!(
                (got - want).abs() < 1e-5,
                "coefficient {i}: {got} should be {want}\nwhole matrix: {m:?}"
            );
        }
    }

    /// ⚠️ **The luminance weights are the spec's, not NTSC's.** Fully desaturating
    /// pure red must give the sRGB luminance grey — 0.213, i.e. 54/255 — and not
    /// the 0.299 (76/255) of the older formula that is far likelier to be typed
    /// from memory. A 22-unit difference in grey is plainly visible side by side,
    /// and both numbers look equally plausible in the source.
    #[test]
    fn desaturating_red_gives_the_srgb_luminance_grey() {
        let mut px = solid(2, 2, [255, 0, 0, 255]);
        let mut s = Surface::new(&mut px, 2, 2);
        apply_matrix(
            &mut s,
            &filter_matrix(&Filters {
                saturation: 0.0,
                ..Filters::default()
            }),
        );
        assert_eq!(&s.px[0..4], &[54, 54, 54, 255]);
    }

    /// Composition order is part of the spec, so it needs pinning by a case where
    /// the two orders differ. Brightness 2 then desaturate is a mid grey;
    /// desaturate then brightness 2 is the same grey — those commute. Contrast
    /// does not: it pivots about 0.5, so doubling brightness first moves the
    /// pixel to the other side of the pivot.
    #[test]
    fn brightness_is_applied_before_contrast_not_after() {
        // 0.4 grey. Brightness 2 → 0.8, then contrast 2 → 0.5 + 2*(0.8-0.5) = 1.1 → clamps to 1.
        // The other order: contrast 2 → 0.5 + 2*(0.4-0.5) = 0.3, then brightness 2 → 0.6.
        let mut px = solid(1, 1, [102, 102, 102, 255]);
        let mut s = Surface::new(&mut px, 1, 1);
        apply_matrix(
            &mut s,
            &filter_matrix(&Filters {
                brightness: 2.0,
                contrast: 2.0,
                ..Filters::default()
            }),
        );
        assert_eq!(
            s.px[0], 255,
            "brightness first saturates; the other order lands near 153"
        );
    }

    /// ⚠️ The pass that made this test worth writing. A colour matrix on
    /// premultiplied data shifts colour only where alpha is partial, so a fully
    /// opaque fixture proves nothing at all — the half-transparent pixel is the
    /// whole test.
    ///
    /// Half-alpha mid-grey, fully desaturated, must stay the same grey. Written
    /// against the version that skips the un-premultiply, which darkens it to 27.
    #[test]
    fn a_colour_matrix_reads_straight_colour_not_premultiplied() {
        // Premultiplied: 0.5 alpha over a 108/255 straight grey → 54 stored.
        let mut px = solid(1, 1, [54, 54, 54, 128]);
        let mut s = Surface::new(&mut px, 1, 1);
        apply_matrix(
            &mut s,
            &filter_matrix(&Filters {
                saturation: 0.0,
                ..Filters::default()
            }),
        );
        assert_eq!(
            s.px[3], 128,
            "alpha is untouched by a saturation-only matrix"
        );
        assert!(
            (s.px[0] as i32 - 54).abs() <= 1,
            "a grey desaturates to itself; got {} (27 means it read premultiplied)",
            s.px[0]
        );
    }

    /// A blur must conserve alpha: what spreads out of a region has to arrive
    /// somewhere else in the buffer, or the layer quietly fades.
    ///
    /// This is the assertion that catches an un-normalized kernel — which is off
    /// by only the 0.3% tail beyond 3σ, invisible in any single pixel and a
    /// visible step across a large flat fill.
    #[test]
    fn a_blur_conserves_total_alpha() {
        let (w, h) = (64, 64);
        let mut px = vec![0u8; w * h * 4];
        // One opaque white block, well clear of the edges so nothing falls off.
        for y in 24..40 {
            for x in 24..40 {
                let p = (y * w + x) * 4;
                px[p..p + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let before: u64 = px.iter().skip(3).step_by(4).map(|v| *v as u64).sum();
        let mut s = Surface::new(&mut px, w, h);
        blur(&mut s, 3.0, 3.0);
        let after: u64 = s.px.iter().skip(3).step_by(4).map(|v| *v as u64).sum();
        let drift = (after as i64 - before as i64).abs();
        assert!(
            drift * 1000 < before as i64,
            "alpha drifted {drift} out of {before}, i.e. more than 0.1%"
        );
    }

    /// A blur with no radius must not touch the buffer at all — not "round back
    /// to about the same", exactly untouched. Otherwise adding an effect row and
    /// leaving it at zero re-quantizes the whole layer.
    #[test]
    fn a_zero_blur_is_a_no_op() {
        let mut px = solid(8, 8, [200, 100, 50, 255]);
        let want = px.clone();
        let mut s = Surface::new(&mut px, 8, 8);
        blur(&mut s, 0.0, 0.0);
        assert_eq!(s.px, &want[..]);
    }

    /// Separability is exact for a gaussian, but only if both passes run. A
    /// horizontal-only blur leaves vertical edges sharp, which is what this
    /// catches — written against `blur` with the second `convolve` omitted.
    #[test]
    fn a_blur_softens_both_axes() {
        let (w, h) = (32, 32);
        let mut px = vec![0u8; w * h * 4];
        let p = (16 * w + 16) * 4;
        px[p..p + 4].copy_from_slice(&[255, 255, 255, 255]);
        let mut s = Surface::new(&mut px, w, h);
        blur(&mut s, 2.0, 2.0);
        let at = |x: usize, y: usize| s.px[(y * w + x) * 4 + 3];
        assert!(at(18, 16) > 0, "spread sideways");
        assert!(at(16, 18) > 0, "and vertically");
        assert_eq!(
            at(18, 16),
            at(16, 18),
            "equal deviations must be isotropic to the byte"
        );
    }

    /// 🚨 **The spread follows the scale on each axis, as the blur beside it
    /// already did** (§15 D601, `[S10.2-L1-05]`).
    ///
    /// **The loss first**: at `scale = (4, 1)` a 10-unit spread drew 40 device
    /// pixels *down* as well as across — 4× too thick on the short axis, and 30 px
    /// past the box `ondin_core::effect::escape` had computed for it, which
    /// `grow_to_escape` then clips to a ruled line. The `(1, 1)` row is the
    /// anti-vacuity control and is green either way, which is the point of having
    /// it: this is a test about the *ratio*, so a fixture at one scale proves
    /// nothing.
    ///
    /// **`blur: 0` and `offset: 0` on purpose.** A blur would soften the edge this
    /// counts and an offset would move it, and the quantity under test is the
    /// dilation alone.
    ///
    /// Flip-check, run: `spread_alpha` back to one radius —
    /// `sh.spread * scale.0.max(scale.1)` on both passes — fails at the `(4, 1)`
    /// row, `left: (40, 40)` against `right: (40, 10)`: the *across* number is
    /// right in both versions and the *down* number is the loss, which is the
    /// predicted site. ⚠️ The `(1, 1)` row stays green under that flip, and it is
    /// the *first* row: a test that stopped there is exactly the test that shipped
    /// this.
    #[test]
    fn a_spread_follows_the_scale_on_each_axis_the_way_the_blur_does() {
        // A 40×40 opaque square in the middle of a 400×400 buffer, so a 40-pixel
        // reach has somewhere to go on every side.
        let (w, h) = (400usize, 400usize);
        let (lo, hi) = (180usize, 220usize);
        let reach = |scale: (f64, f64)| {
            let mut px = vec![0u8; w * h * 4];
            for y in lo..hi {
                for x in lo..hi {
                    px[(y * w + x) * 4..][..4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
            let effects = [Effect::new(EffectKind::DropShadow(Shadow {
                blur: 0.0,
                spread: 10.0,
                offset: ondin_core::kurbo::Vec2::ZERO,
                color: Color::BLACK,
            }))];
            run(&mut Surface::new(&mut px, w, h), &effects, scale);
            let a = |x: usize, y: usize| px[(y * w + x) * 4 + 3];
            let right = (hi..w).take_while(|&x| a(x, (lo + hi) / 2) > 0).count();
            let down = (hi..h).take_while(|&y| a((lo + hi) / 2, y) > 0).count();
            (right, down)
        };
        assert_eq!(reach((1.0, 1.0)), (10, 10), "the control: 1:1 is unchanged");
        assert_eq!(reach((4.0, 1.0)), (40, 10), "four times as dense across");
        assert_eq!(reach((1.0, 4.0)), (10, 40), "and the other way round");
    }

    /// **A spread past the buffer's own edge costs nothing more and draws the
    /// same picture** (§15 D615, `[S10.2-L4-04]`, **G10**).
    ///
    /// 🚨 **`n` is device pixels and the only cap on it was in document units.**
    /// `shadow_layer` passes `sh.spread × scale`, so the zoom multiplies the
    /// authored number without limit, and the one bound anywhere is
    /// `inspector::MAX_OFFSET = 10_000` — a range the panel shares with the
    /// **Offset** fields, which cost O(1) per pixel because an offset is a shifted
    /// read. Measured on a real adapter, one shadow with `blur: 0`: **23.8 ms** at
    /// `spread: 200` zoom 256, **117.1 ms** at 1 000, **1 092.7 ms** at 10 000.
    ///
    /// **Two assertions, and the first is the one that matters.** The picture must
    /// not move — the cap is only defensible because both loops read outside the
    /// buffer as *empty*, so a window already covering the line cannot find
    /// anything by growing — and the work must actually stop, or the first
    /// assertion is satisfied by doing nothing at all.
    ///
    /// ⚠️ **The timing assertion is a ratio and a very loose one**, deliberately.
    /// A wall clock in a test suite is a flake waiting to happen; what is being
    /// pinned is Θ(n) versus Θ(1), which at these two counts is a factor of
    /// hundreds, so a bound of 20× is far outside any noise and still fails
    /// immediately if the cap goes. Debug rather than release, which only widens
    /// the gap.
    ///
    /// ⚠️ **Flip-check, run**: `spread_steps` returning `v.abs().round() as i32`
    /// uncapped fails the **timing** assertion and leaves the picture one green —
    /// which is the whole claim, so the predicted site was right. **12.3 ms at
    /// spread 200 against 9.24 s at 200 000**, a ratio of about 754 rather than
    /// the "over 300" the write-up guessed before running it. That is also the
    /// measurement to read the 20× bound against: the margin is nearly two orders
    /// of magnitude, so nothing about this assertion is close to a clock.
    #[test]
    fn a_spread_wider_than_the_buffer_is_capped_without_changing_the_picture() {
        let (w, h) = (120usize, 120usize);
        let shadow = |spread: f64| {
            let mut px = vec![0u8; w * h * 4];
            for y in 50..70 {
                for x in 50..70 {
                    px[(y * w + x) * 4..][..4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
            let effects = [Effect::new(EffectKind::DropShadow(Shadow {
                blur: 0.0,
                spread,
                offset: ondin_core::kurbo::Vec2::ZERO,
                color: Color::BLACK,
            }))];
            let started = std::time::Instant::now();
            run(&mut Surface::new(&mut px, w, h), &effects, (1.0, 1.0));
            (px, started.elapsed())
        };

        // 200 already saturates a 120-wide buffer; 200_000 is the zoomed case.
        let (saturating, quick) = shadow(200.0);
        let (absurd, slow) = shadow(200_000.0);
        assert_eq!(
            saturating, absurd,
            "a dilation that already covers the buffer cannot reach further, so \
             capping it must change no pixel"
        );
        assert!(
            slow < quick * 20,
            "the cap is not bounding the work: {quick:?} at spread 200 against \
             {slow:?} at 200000, which is a thousand times the radius"
        );
    }

    /// The cap's own arithmetic, at the boundary rather than in the middle — a
    /// range being wrong only at its ends (§15 D615).
    ///
    /// ⚠️ **`extent - 1` and not `extent`**, and the off-by-one is the whole
    /// question: the window is `[i − n, i + n]`, so at `n = extent − 1` it already
    /// spans the line for every `i`. One lower and the pixel at each end is still
    /// short of the far edge; one higher is work with nowhere to go.
    #[test]
    fn a_spreads_reach_is_capped_at_the_axis_it_runs_along() {
        assert_eq!(spread_steps((3.0, 4.0), 120, 120), (3, 4), "under the cap");
        assert_eq!(spread_steps((119.0, 5.0), 120, 120), (119, 5), "at it");
        assert_eq!(spread_steps((120.0, 5.0), 120, 120), (119, 5), "over it");
        // Per axis, because the buffer is not square and neither is the pair.
        assert_eq!(spread_steps((999.0, 999.0), 40, 200), (39, 199));
        // A sign is a direction and not a size: an erosion is capped identically.
        assert_eq!(spread_steps((-999.0, -1.0), 40, 200), (39, 1));
        // And an axis with no room at all asks for no passes rather than -1.
        assert_eq!(spread_steps((5.0, 5.0), 0, 1), (0, 0));
    }

    /// The gate that keeps a switched-off stack free. Each arm is a state the
    /// panel can actually produce, and each one answered `true` under
    /// `!effects.is_empty()`.
    #[test]
    fn a_stack_that_cannot_change_a_pixel_reports_no_ink() {
        let clear = Shadow {
            color: Color::TRANSPARENT,
            ..Shadow::default()
        };
        for (label, stack) in [
            ("empty", vec![]),
            (
                "hidden",
                vec![Effect {
                    kind: EffectKind::LayerBlur { radius: 20.0 },
                    visible: false,
                }],
            ),
            (
                "zero radius",
                vec![Effect::new(EffectKind::LayerBlur { radius: 0.0 })],
            ),
            (
                "neutral filters",
                vec![Effect::new(EffectKind::Filters(Filters::default()))],
            ),
            (
                "fully transparent shadow",
                vec![Effect::new(EffectKind::DropShadow(clear))],
            ),
        ] {
            assert!(!any_ink(&stack), "{label} must not cost a buffer");
        }
        assert!(
            any_ink(&[Effect::new(EffectKind::LayerBlur { radius: 1.0 })]),
            "and a real one must"
        );
    }

    /// A hard-edged silhouette: a disc, which is the shape a resample is most
    /// likely to show up on — a straight edge survives box-averaging almost
    /// intact, a curved one does not.
    fn disc(w: usize, h: usize, r: f64) -> Vec<u8> {
        let mut px = vec![0u8; w * h * 4];
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
                if d <= r {
                    px[(y * w + x) * 4 + 3] = 255;
                }
            }
        }
        px
    }

    /// **The whole claim of §15 D403 in one assertion**: a shadow blurred on a
    /// `k`-pixel grid is the shadow, not an approximation of it that anyone could
    /// see. σ is 60 here against a budget of 24, so `k` is 3.
    ///
    /// ⚠️ **The tolerance is a measurement, not a guess, and the guess was three
    /// times too pessimistic.** Worst channel is **1/255** and the mean is
    /// **0.032** over the whole 240×240 field. It is written as `<= 2` rather than
    /// `== 0` because a resample cannot be exact and pretending otherwise would
    /// make this a tripwire for arithmetic rather than for the picture.
    ///
    /// Flip: blurring the coarse grid at the *full* σ rather than σ/k — the
    /// mistake of dividing in one place and not the other — gives **45/255 worst,
    /// mean 3.65**, so this bites hard on the one error the scheme invites. (That
    /// flip is also *not* the disaster it looks like at a glance, which is worth
    /// knowing: a k-times-too-wide blur on a k-times-coarser grid is a blur k²
    /// too wide, and it still looks like a shadow. Only the numbers say so.)
    #[test]
    fn a_coarse_shadow_blur_is_the_same_picture_as_the_exact_one() {
        let (w, h) = (240, 240);
        let (sigma, k) = (60.0f32, 3usize);
        assert_eq!(ondin_core::effect::shadow_downscale(sigma as f64), k as u32);

        let mut exact = disc(w, h, 40.0);
        blur(&mut Surface::new(&mut exact, w, h), sigma, sigma);
        let mut coarse = disc(w, h, 40.0);
        blur_coarse(
            &mut Surface::new(&mut coarse, w, h),
            sigma / k as f32,
            sigma / k as f32,
            k,
        );

        let (mut worst, mut total) = (0u8, 0u64);
        for (a, b) in exact.iter().zip(coarse.iter()) {
            let d = a.abs_diff(*b);
            worst = worst.max(d);
            total += u64::from(d);
        }
        let mean = total as f64 / exact.len() as f64;
        assert!(
            worst <= 2 && mean < 0.1,
            "coarse blur differs by up to {worst}/255, mean {mean:.3}"
        );
    }

    /// The deadband, and the reason it is not caution: a shadow nobody has zoomed
    /// into is blurred exactly as authored, so the ordinary frame is untouched by
    /// any of this.
    #[test]
    fn a_shadow_under_twice_the_budget_is_not_resampled_at_all() {
        assert_eq!(ondin_core::effect::shadow_downscale(47.0), 1);
        assert_eq!(ondin_core::effect::shadow_downscale(48.0), 2);
        // And it is `ceil`, so the coarse deviation lands under the budget rather
        // than near it: 100/24 is 4.17, and a `round` here would give 4 and a
        // coarse σ of 25.
        assert_eq!(ondin_core::effect::shadow_downscale(100.0), 5);
    }

    /// A resample must not move the shadow. The centre of mass of a blurred disc
    /// is the disc's centre; a half-block error in `upsample`'s `(k-1)/2` shows
    /// up here as a shift that grows with `k`, which is the failure mode the
    /// tolerance above is too coarse to catch.
    #[test]
    fn the_resample_does_not_shift_the_shadow() {
        let (w, h) = (200, 200);
        for k in [2usize, 5, 8] {
            let mut px = disc(w, h, 30.0);
            // The coarse deviation is the budget itself, which is the state every
            // resampled shadow ends up in.
            blur_coarse(&mut Surface::new(&mut px, w, h), 24.0, 24.0, k);
            let (mut mx, mut my, mut m) = (0.0f64, 0.0f64, 0.0f64);
            for y in 0..h {
                for x in 0..w {
                    let a = f64::from(px[(y * w + x) * 4 + 3]);
                    mx += a * x as f64;
                    my += a * y as f64;
                    m += a;
                }
            }
            let (cx, cy) = (mx / m, my / m);
            assert!(
                (cx - 99.5).abs() < 0.5 && (cy - 99.5).abs() < 0.5,
                "k={k}: centre of mass at ({cx:.2}, {cy:.2}), not (99.5, 99.5)"
            );
        }
    }

    /// **An out-of-range blur is a wrong picture, not a hang and not a panic.**
    ///
    /// `[S10.1-L1-03]` and `[S10.2-L1-03]`, which are one defect with two
    /// unbounded sites and had to be fixed together — the first finding's own fix
    /// sketch was refuted by the second, because capping the block factor alone
    /// *"leaves the failure in place and only changes which line it happens on"*.
    ///
    /// The input is one SVG attribute: `<feDropShadow stdDeviation="1e30">`
    /// imports with `Import::skipped` and `Import::approximated` **both empty**,
    /// lands `Shadow { blur: 2e30 }`, and `io::load(io::save(doc))` is `Ok`. So it
    /// is on disk, in the library, and `library::cover::rasterize` renders every
    /// document in the base folder to draw the dashboard.
    ///
    /// ⚠️ **The two sites, and why neither cap covers the other.**
    /// `shadow_downscale`'s `k.ceil() as u32` saturated, so σ/k — the one thing
    /// that function promises — went 24.0 → 46.6 → 2.33e20 above σ ≈ 1.03e11, and
    /// `coarsen` **was** `Θ(k)` regardless of the surface: **≈180 s** at
    /// `u32::MAX`, which is why two 100-second kills saw no return. Behind that,
    /// `kernel_radius` had no bound at all: **24 GB** of taps at σ = 1e9 and a
    /// capacity-overflow panic above σ ≈ 6.1e18. Capping `k` alone hands
    /// `1e30 / 4096 = 2.44e26` to `kernel` and dies faster; capping the kernel
    /// alone **then** still walked `coarsen` for three minutes.
    ///
    /// 🚨 **Two of the sentences above are history rather than description, and
    /// re-running the flip is what said so** (§15 D743, `[S10.2-L4-02]`).
    /// `coarsen`'s loops stop at the source now, so it costs `w · h` however
    /// large `k` is and the three-minute walk is gone. **`MAX_SHADOW_BLOCK` is
    /// therefore a correctness cap and no longer a cost one** — what it still buys
    /// is that `k as i32` above `i32::MAX` wraps **negative**, at which point the
    /// GPU's `coarsen` runs neither loop and draws no shadow at all, which is the
    /// arm that is silent rather than slow.
    ///
    /// ⚠️ **The loss is asserted before either mechanism.** The whole pass runs
    /// first, against a wall clock, because what the user loses is a frame that
    /// never comes back; the two length assertions after it say *which* number
    /// bounds it. The clock is deliberately loose — 5 s against a measured
    /// ~0.2 ms — because it stands in for *"does not take three minutes"*, and a
    /// tight bound on a shared machine is a flake.
    ///
    /// ⚠️ **Flipped at each site separately with the other cap in place, and both
    /// flips bite on the clock assertion's own line — which is the refutation
    /// stated as an experiment rather than as an argument.**
    ///
    /// - Removing the `min` in `shadow_downscale`, release: **177.8 s**, and the
    ///   failure prints it. The kernel cap was in place the whole time, so this is
    ///   *"capping the kernel alone still walks `coarsen` for three minutes"*,
    ///   measured rather than extrapolated — the finding predicted ≈180 s from
    ///   41.6 ns per unit of `k`. 🚨 **Re-run after D743 and the teeth moved.**
    ///   The same flip now fails **in 0.00 s, on the `k <= MAX_SHADOW_BLOCK`
    ///   assertion below** — `sigma 1000000: k = 41667` — because the whole pass
    ///   is fast enough to clear the clock. So the clock assertion **no longer
    ///   distinguishes the capped version from the uncapped one** on this arm, and
    ///   the paragraph below saying no flip reaches a length assertion is true of
    ///   the second flip only. The test still bites; what it bites with changed,
    ///   and nothing would have said so without running it again.
    /// - Removing the `min` in `kernel_radius`, debug: a panic **inside `run`**,
    ///   at `kernel`'s `2 * r + 1` — *"attempt to multiply with overflow"*, which
    ///   in release is the `Vec::with_capacity` capacity overflow. `k` was capped
    ///   at 4096 the whole time, so σ/k is 2.4e26 and this is the other half:
    ///   *"capping `k` alone leaves the failure in place and only changes which
    ///   line it happens on"*.
    ///
    /// ⚠️ **Neither flip reached a length assertion** — *when this was written*,
    /// which is worth saying because the length assertions look like the point of
    /// the test and were not: they pin the two contracts, and the clock was what
    /// said the contracts are the right two. **After D743 that holds for the
    /// second flip only**; see the ⚠️ on the first. The clock still earns its
    /// place, because it is what fails if either bound is removed *and* something
    /// else on the path becomes unbounded — but it is no longer what separates
    /// `MAX_SHADOW_BLOCK` from its absence.
    #[test]
    fn an_out_of_range_blur_is_bounded_at_both_of_its_two_sites() {
        // **The loss first.** The whole pass, on the document the finding
        // describes: `<feDropShadow stdDeviation="1e30">` is `Shadow { blur: 2e30 }`
        // and the layer is the finding's own 200×220. What the user loses is a
        // frame that never comes back, so that is what this asserts, before any
        // assertion about the two numbers that bound it.
        let (w, h) = (200usize, 220usize);
        let mut px = solid(w, h, [0, 0, 0, 255]);
        let effects = [Effect::new(EffectKind::DropShadow(Shadow {
            blur: 2e30,
            ..Shadow::default()
        }))];
        let t = std::time::Instant::now();
        run(&mut Surface::new(&mut px, w, h), &effects, (1.0, 1.0));
        let took = t.elapsed();
        assert!(
            took < std::time::Duration::from_secs(5),
            "a shadow of blur 2e30 took {took:?}; before the two caps it was ~180 s \
             of `coarsen` with a 24 GB allocation waiting behind it"
        );

        // The kernel: bounded in length, and finite in value at every tap. An
        // enormous sigma makes `2σ²` overflow to `inf`, so every weight is
        // `exp(-0) = 1` and the normalized kernel is a box — wrong, and finite.
        for sigma in [1e6f32, 1e12, 1e30, f32::MAX, f32::INFINITY] {
            let k = kernel(sigma);
            assert!(
                k.len() <= 2 * ondin_core::effect::MAX_KERNEL_RADIUS + 1,
                "sigma {sigma}: {} taps",
                k.len()
            );
            assert!(
                k.iter().all(|w| w.is_finite()) && (k.iter().sum::<f32>() - 1.0).abs() < 1e-3,
                "sigma {sigma}: the kernel must still be a normalized one"
            );
        }
        // NaN and a negative deviation are "no blur", not "the widest one".
        assert_eq!(kernel_radius(f32::NAN), 0);
        assert_eq!(kernel_radius(-1.0), 0);
        assert_eq!(kernel_radius(f32::NEG_INFINITY), 0);

        // The block factor: bounded, and an infinite sigma gets the cap rather
        // than `1`, which is the arm that used to answer "blur it as authored"
        // for the one input where no downscale at all is certainly wrong.
        use ondin_core::effect::{MAX_SHADOW_BLOCK, shadow_downscale};
        for sigma in [1e6f64, 1.03e11, 2e11, 1e30] {
            assert!(
                shadow_downscale(sigma) <= MAX_SHADOW_BLOCK,
                "sigma {sigma}: k = {}",
                shadow_downscale(sigma)
            );
        }
        assert_eq!(shadow_downscale(f64::INFINITY), MAX_SHADOW_BLOCK);
        assert_eq!(shadow_downscale(f64::NAN), 1);
        assert_eq!(shadow_downscale(f64::NEG_INFINITY), 1);
        assert_eq!(shadow_downscale(24.0), 1, "the deadband is untouched");
    }

    /// **An effect layer past the cap is *resampled* into it, never truncated**
    /// (§15 D744, `[S11.2-L1-01]`).
    ///
    /// The old spelling was `.min(u16::MAX as f64)` at both backends, which is
    /// eight times what the GPU can build: an ordinary wide, thin layer asked for
    /// 9672 pixels across and panicked in `wgpu` validation — see
    /// `gpu_effects.rs`'s
    /// `a_layer_needing_more_than_the_texture_limit_still_draws` for why it is
    /// **not** the drop shadow at `offset = 1000` the finding names.
    /// The two assertions that matter are that the buffer **fits**
    /// and that the returned `k` is the one that produced it — a caller keeping
    /// `buffer_downscale`'s `k` while using this `w`/`h` would draw the artwork at
    /// one scale into a buffer at another, which is a picture that is subtly the
    /// wrong size rather than an error.
    ///
    /// ⚠️ **The under-the-cap row is not decoration.** It is what says this
    /// changes nothing for every document that already worked: `k` must come back
    /// *identical*, not merely close, or every existing shadow moves.
    ///
    /// ⚠️ Flipped by restoring `.min(u16::MAX as f64)` in place of the `fit`
    /// growth: the first `<= MAX_EFFECT_BUFFER_SIDE` assertion fails at 20000,
    /// and the `k` assertion below it fails too, which is the pair that separates
    /// "capped" from "capped by resampling".
    #[test]
    fn an_effect_buffer_over_the_cap_is_resampled_into_it_rather_than_cut_off() {
        use super::{MAX_EFFECT_BUFFER_SIDE, effect_buffer};
        let cap = f64::from(MAX_EFFECT_BUFFER_SIDE);

        // Under the cap: untouched, both numbers.
        let ((w, h), k) = effect_buffer((400.0, 220.0), 1.0);
        assert_eq!((w, h, k), (400, 220, 1.0), "a small layer is not resampled");

        // The finding's own *number*, 8320 across — 128 past the limit. Fed here
        // directly, because no shadow at `offset = 1000` actually produces it.
        let ((w, h), k) = effect_buffer((8320.0, 220.0), 1.0);
        assert!(
            w <= MAX_EFFECT_BUFFER_SIDE && h <= MAX_EFFECT_BUFFER_SIDE,
            "the buffer fits: {w}×{h}"
        );
        assert!(k > 1.0, "and it fits because it was resampled: k = {k}");
        assert!(
            (8320.0 / k - cap).abs() < 1.0,
            "resampled to the cap and no further — a factor of {k} gives {}",
            8320.0 / k
        );

        // Far past it, on the other axis, with a stack that already resamples.
        let ((w, h), k) = effect_buffer((300.0, 20000.0), 4.0);
        assert!(
            w <= MAX_EFFECT_BUFFER_SIDE && h <= MAX_EFFECT_BUFFER_SIDE,
            "the tall buffer fits too: {w}×{h}"
        );
        assert!(
            k > 4.0,
            "and the stack's own downscale is grown, not replaced: k = {k}"
        );

        // The conservative direction for both non-finite inputs.
        let ((w, h), _) = effect_buffer((f64::INFINITY, f64::NAN), 1.0);
        assert!(
            w <= MAX_EFFECT_BUFFER_SIDE && h >= 1,
            "an unrepresentable extent answers a buffer, not a panic: {w}×{h}"
        );
    }
}
