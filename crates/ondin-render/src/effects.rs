//! The effect passes — the pixel arithmetic behind `ondin_core::effect` (§6.4).
//!
//! One implementation, read by both backends: `vello` has no filters beyond
//! `draw_blurred_rounded_rect`, and `vello_cpu` has only `GaussianBlur` and
//! `DropShadow` (no `ColorMatrix`, no `Composite`), so using the library for those
//! two would mean two blurs on backends that must stay identical. These functions
//! are the specification, the CPU backend is the reference, and the GPU shaders
//! match it pixel for pixel.
//!
//! **Pixels are premultiplied.** Blurring straight alpha mixes the colour of
//! transparent pixels into their neighbours and draws a halo around soft edges.
//!
//! **Filters run in sRGB, not linear.** SVG defaults `color-interpolation-filters`
//! to `linearRGB`, so the SVG writer says `sRGB` on every filter it emits; the
//! three writers agreeing matters more than physical accuracy.

use ondin_core::Keyed;
use ondin_core::effect::{Effect, EffectKind, Filters};

/// A premultiplied sRGB RGBA8 image the passes work over.
///
/// Not `vello_cpu::Pixmap` or a `wgpu::Texture`: this is the seam between the
/// backends, so the type is one neither owns.
pub struct Surface<'a> {
    pub px: &'a mut [u8],
    pub w: usize,
    pub h: usize,
}

impl Surface<'_> {
    /// Panics if the buffer is not exactly `w * h * 4` bytes; a stride error would
    /// silently shear the image in release, so this is not a debug assertion.
    pub fn new<'a>(px: &'a mut [u8], w: usize, h: usize) -> Surface<'a> {
        assert_eq!(px.len(), w * h * 4, "surface buffer is w*h*4 bytes");
        Surface { px, w, h }
    }
}

/// The 4×5 colour matrix for a [`Filters`] row, in `feColorMatrix` order.
///
/// Twenty coefficients, row-major, each row `[r g b a offset]` — the SVG layout,
/// so the SVG writer emits the array unchanged.
///
/// **Composed in the order brightness → contrast → saturation → hue.** Matrix
/// composition does not commute, so the canvas, raster export and SVG file must
/// all pick this order.
pub fn filter_matrix(f: &Filters) -> [f32; 20] {
    let mut m = IDENTITY;
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
    // The luminance weights are the spec's sRGB ones, not NTSC 0.299/0.587/0.114.
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
    // Luminance weights are named so this and `saturate` cannot drift apart.
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
/// **Un-premultiplies, transforms, re-premultiplies**: a colour matrix is defined
/// on straight colour, and premultiplied input shifts hue on soft edges.
pub fn apply_matrix(s: &mut Surface<'_>, m: &[f32; 20]) {
    for px in s.px.as_chunks_mut::<4>().0 {
        let a = px[3] as f32 / 255.0;
        if a == 0.0 {
            // Dividing by zero alpha would produce a NaN that poisons the clamp.
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
/// `bounds_expansion`; the only place that turns it into an integer.
///
/// **Capped at `ondin_core::effect::MAX_KERNEL_RADIUS`, and both backends' blurs
/// go through here**, so one guard bounds [`kernel`]'s allocation and `coarsen`'s
/// walk on both (a `stdDeviation="1e30"` import would otherwise panic or hang).
/// `NaN` and the infinities come out as `0` and the cap respectively.
pub fn kernel_radius(sigma: f32) -> usize {
    if sigma.is_nan() || sigma <= 0.0 {
        return 0;
    }
    let r = (f64::from(sigma) * ondin_core::effect::BLUR_CUTOFF).ceil();
    r.min(ondin_core::effect::MAX_KERNEL_RADIUS as f64) as usize
}

/// A normalized 1-D gaussian of the given deviation, `2 * radius + 1` long.
///
/// **Public because the GPU passes convolve with these exact numbers**: the host
/// hands the kernel to the shader as a buffer instead of the shader re-evaluating
/// the formula.
pub fn kernel(sigma: f32) -> Vec<f32> {
    let r = kernel_radius(sigma);
    let mut k = Vec::with_capacity(2 * r + 1);
    let denom = 2.0 * sigma * sigma;
    for i in 0..=2 * r {
        let x = i as f32 - r as f32;
        k.push((-(x * x) / denom).exp());
    }
    // Normalized after truncation: a kernel summing to 0.997 would dim a flat fill
    // into a visible step at the blur's edge.
    let sum: f32 = k.iter().sum();
    for v in &mut k {
        *v /= sum;
    }
    k
}

/// Gaussian blur, in place, with independent deviations per axis.
///
/// Separable: a horizontal pass then a vertical one, O(w·h·r) and exact for a
/// gaussian.
///
/// **Edges are transparent-black, not clamped**, so the edge row is not smeared
/// outwards. The caller owes the surface padding for [`ondin_core::effect::reach`];
/// without it the blur fades to nothing at the buffer's border.
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
/// **One fixed number rather than the machine's own maximum** (§15 D395): both
/// backends must choose the same buffer for one drawing, and a device limit would
/// make a document blur differently on two machines.
///
/// **8192 is the floor**: `wgpu::Limits::default()`'s `max_texture_dimension_2d`,
/// which is what the app requests, and vello's `MAX_ATLAS_SIZE`. A card that could
/// do more resamples slightly sooner than it must.
///
/// Not `u16::MAX`: that bounds nothing a backend can build, and a wide thin layer
/// panicked `wgpu` validation past the texture limit while the CPU drew it.
/// `gpu_effects.rs`'s `a_layer_needing_more_than_the_texture_limit_still_draws`
/// covers the case.
pub const MAX_EFFECT_BUFFER_SIDE: u32 = 8192;

/// The scratch buffer an effect layer is rasterized into: its size in pixels and
/// the downscale that got it there.
///
/// `extent` is the layer's device-space box after
/// `ondin_core::effect::buffer_box_within` has clipped it, and `k` is
/// `buffer_downscale`'s answer for the stack. Both backends call this so the
/// arithmetic exists once (§15 D395).
///
/// **Too big is answered by resampling, never by truncating**: `k` grows until both
/// sides fit and the layer is scaled back up on the way out. Truncation cuts off
/// the silhouette a shadow is cast from.
///
/// The returned `k` is the one the caller must use for its own `base` transform.
pub fn effect_buffer(extent: (f64, f64), k: f64) -> ((u32, u32), f64) {
    let cap = f64::from(MAX_EFFECT_BUFFER_SIDE);
    // `max(1.0)` never shrinks `k`. A non-finite extent falls through safely:
    // `f64::max` against `NaN` gives `fit` 1.0, and an infinite one is capped below.
    let fit = (extent.0 / cap).max(extent.1 / cap).max(1.0);
    let k = if fit.is_finite() { k * fit } else { k };
    let side = |e: f64| (e / k).ceil().max(1.0).min(cap) as u32;
    ((side(extent.0), side(extent.1)), k)
}

/// The dimensions of the coarse grid a `k`-pixel block resample of `w`×`h` has.
///
/// **Public because the GPU dispatches over exactly this grid**, one thread per
/// block.
pub fn coarse_size(w: usize, h: usize, k: usize) -> (usize, usize) {
    (w.div_ceil(k), h.div_ceil(k))
}

/// Box-average `s` into `k`×`k` blocks, one pixel per block.
///
/// **Pixels off the right and bottom edges count as transparent**, so the divisor
/// is `k²` for every block including a partial one; dividing by the covered count
/// would draw a bright rim along two edges of a heavily blurred shadow.
///
/// Both loops stop at the source rather than at the block, so the cost is `w · h`
/// however large `k` is; the shader has no optimizer to hoist the empty taps and
/// the reference must have the same cost model.
///
/// `saturating_mul` because `k` is a parameter of a `pub fn`, not
/// `shadow_downscale`'s capped output.
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
/// A coarse sample stands at the **centre** of its block, `(k-1)/2` pixels in;
/// getting that wrong shifts every shadow by half a block, which shows as a shadow
/// that creeps with zoom.
///
/// Outside the outermost centres the sample is clamped: the field has already been
/// blurred to nothing at its border, so clamping extends transparency (the
/// opposite reason from [`convolve`]'s edge rule).
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
/// `sigma_x`/`sigma_y` are the **coarse** deviations, already divided by the `k`
/// the caller chose (`ondin_core::effect::shadow_downscale`). This pins a shadow's
/// cost: the convolution runs over `w·h/k²` pixels with a kernel that no longer
/// grows (§15 D403).
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
    // The outer axis is the one the kernel does not run along.
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
/// **Two numbers, because a non-uniform scale blurs unevenly**: a node's transform
/// carries no scale (§5.6), but the viewport, an export scale and an ancestor's
/// skew do. One averaged sigma would make a blur on a skewed layer round.
pub fn device_sigma(radius: f64, scale_x: f64, scale_y: f64) -> (f32, f32) {
    let d = ondin_core::effect::deviation(radius);
    ((d * scale_x.abs()) as f32, (d * scale_y.abs()) as f32)
}

/// Apply a whole effect stack to a rasterized layer, in place.
///
/// `scale` is device pixels per document unit along each axis.
///
/// **The statement of §6.4's two stages**, which the SVG writer (`svg::effect_def`)
/// and the bounds (`effect::stack_escape`) follow: `Filters` and `LayerBlur`
/// transform the layer's own appearance in list order, then every shadow is cast
/// from *that* and merged behind or in front. Not one fold: an inner shadow after
/// a drop shadow would take its silhouette from the layer plus the shadow. Reorder
/// the stages and both followers must move too; nothing fails if only one does.
pub fn run(s: &mut Surface<'_>, effects: &[Keyed<Effect>], scale: (f64, f64)) {
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

    // The layer as the shadows see it, before anything is composited over or under.
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
            // On top of the accumulated result.
            over(&mut out, &layer);
        } else {
            // Behind it: the shadow is the destination.
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
    // The silhouette: the layer's alpha shifted by the offset; colour comes later.
    //
    // **The inversion an inner shadow needs happens at the tint, not here**
    // (§15 D742). Every pass between here and the tint (blur, coarsen, upsample,
    // spread) is linear or a max/min that commutes with the complement, so they
    // keep one edge rule (outside the buffer is transparent) and the shape's own
    // alpha travels through them. Inverting first would contradict that rule at
    // the buffer edge. It also keeps [`spread_alpha`]'s `grow` the right way up.
    let (dx, dy) = (
        (sh.offset.x * scale.0).round() as isize,
        (sh.offset.y * scale.1).round() as isize,
    );
    let mut mask = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x as isize - dx, y as isize - dy);
            let a = if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize {
                // No layer outside the buffer; the inner tint's complement makes
                // that "fully casting".
                0
            } else {
                graphic[(sy as usize * w + sx as usize) * 4 + 3]
            };
            let p = (y * w + x) * 4;
            mask[p + 3] = a;
        }
    }
    let mut surface = Surface {
        px: &mut mask,
        w,
        h,
    };
    // Spread dilates or erodes the silhouette before it is blurred.
    if sh.spread != 0.0 {
        spread_alpha(
            &mut surface,
            (sh.spread * scale.0, sh.spread * scale.1),
            inner,
        );
    }
    if sh.blur > 0.0 {
        let (sx, sy) = device_sigma(sh.blur, scale.0, scale.1);
        // The silhouette may be resampled where the layer's buffer may not be
        // (§15 D403): `shadow_downscale` is the only bound on a shadow's kernel.
        let k = ondin_core::effect::shadow_downscale(sx.max(sy) as f64) as usize;
        if k > 1 {
            blur_coarse(&mut surface, sx / k as f32, sy / k as f32, k);
        } else {
            blur(&mut surface, sx, sy);
        }
    }
    // Tint: the shadow's colour at the silhouette's alpha, **complementing the
    // silhouette first for an inner shadow**.
    let [r, g, b, ca] = sh.color.components;
    for px in mask.as_chunks_mut::<4>().0 {
        let v = if inner { 255 - px[3] } else { px[3] };
        let a = (v as f32 / 255.0) * ca;
        px[0] = (r * a * 255.0 + 0.5) as u8;
        px[1] = (g * a * 255.0 + 0.5) as u8;
        px[2] = (b * a * 255.0 + 0.5) as u8;
        px[3] = (a * 255.0 + 0.5) as u8;
    }
    if inner {
        // Confine it to the layer's alpha, or it paints the buffer outside the shape.
        for (px, src) in mask
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(graphic.as_chunks::<4>().0)
        {
            let k = src[3] as f32 / 255.0;
            for v in px.iter_mut() {
                *v = (*v as f32 * k + 0.5) as u8;
            }
        }
    }
    mask
}

/// A spread's per-axis half-width in device pixels, **bounded by the buffer it
/// runs in** — the pair both backends' dilation loops count to (§15 D615).
///
/// `shadow_layer` passes `sh.spread × scale`, and the only other bound is in
/// document units (`inspector::MAX_OFFSET`), so without this cap the per-pixel
/// cost of a spread tracks `spread × zoom` without limit.
///
/// **The cap is exactly lossless.** Both loops read outside the buffer as empty,
/// so once the window covers the whole line a dilation finds nothing further and
/// an erosion has already met a zero (`n ≥ extent − 1`). The cost is still
/// Θ(w·h·n) up to the buffer's size; running the dilation on the coarse grid would
/// be a separate approximation.
pub(crate) fn spread_steps(r: (f64, f64), w: usize, h: usize) -> (i32, i32) {
    let step = |v: f64, extent: usize| {
        let n = v.abs().round();
        // `saturating_sub`: a zero-width buffer must not underflow.
        n.min(extent.saturating_sub(1) as f64) as i32
    };
    (step(r.0, w), step(r.1, h))
}

/// Grow (or shrink) an alpha silhouette by `r` device pixels — **`r.0` across,
/// `r.1` down**.
///
/// A box dilation, like SVG's `feMorphology`, so export and canvas are boxy in the
/// same way.
///
/// **Two half-widths, for the reason [`device_sigma`] takes two deviations**
/// (§15 D601): a non-uniform pair is real (`push_effect_layer` takes the two column
/// norms, and skew is a shipped gesture), and one averaged width draws the spread
/// too thick on the short axis, outside the bounds `ondin_core::effect::escape`
/// allocated.
fn spread_alpha(s: &mut Surface<'_>, r: (f64, f64), inner: bool) {
    let n = spread_steps(r, s.w, s.h);
    let n = (n.0 as isize, n.1 as isize);
    if n.0 == 0 && n.1 == 0 {
        return;
    }
    // Positive spread grows a drop shadow and thickens an inner one, which are
    // opposite operations on the shape's alpha. **The sum, not either one**: a
    // zero scale on an axis zeroes that radius, and the sign comes from the other.
    let grow = (r.0 + r.1 > 0.0) != inner;
    // Separable like the gaussian: a box min/max is the 1-D one along each axis,
    // O(r) per pixel instead of O(r²).
    for horizontal in [true, false] {
        // Zero is the identity for dilation and erosion, so the pass is skipped.
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
                    // Outside the buffer reads as empty, like the blur's edge.
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
    for (d, s) in dst
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src.as_chunks::<4>().0)
    {
        let ia = 1.0 - s[3] as f32 / 255.0;
        for i in 0..4 {
            d[i] = (s[i] as f32 + d[i] as f32 * ia + 0.5).min(255.0) as u8;
        }
    }
}

/// Whether `effects` contains anything that would change a pixel.
///
/// **Not `!effects.is_empty()`**: hidden entries and neutral `Filters` rows must
/// answer `false`, or the layer pays for an offscreen buffer every frame.
pub fn any_ink(effects: &[Keyed<Effect>]) -> bool {
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
    use ondin_core::keyed_by_position;
    use ondin_core::peniko::Color;

    fn solid(w: usize, h: usize, rgba: [u8; 4]) -> Vec<u8> {
        rgba.iter().copied().cycle().take(w * h * 4).collect()
    }

    /// A neutral row composes to the identity, guarding the hand-typed coefficients.
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

    /// Desaturating pure red gives the sRGB luminance grey (54/255), not NTSC's 76.
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

    /// Brightness then contrast differs from the reverse (contrast pivots about 0.5).
    #[test]
    fn brightness_is_applied_before_contrast_not_after() {
        // 0.4 grey: brightness first gives 1.1, clamped to 1; the other order gives 0.6.
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

    /// Half-alpha grey, fully desaturated, stays the same grey; skipping the
    /// un-premultiply darkens it to 27. The partial alpha is the whole test.
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

    /// A blur conserves total alpha; catches an un-normalized kernel (0.3% tail).
    #[test]
    fn a_blur_conserves_total_alpha() {
        let (w, h) = (64, 64);
        let mut px = vec![0u8; w * h * 4];
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

    /// A zero blur leaves the buffer exactly untouched, not re-quantized.
    #[test]
    fn a_zero_blur_is_a_no_op() {
        let mut px = solid(8, 8, [200, 100, 50, 255]);
        let want = px.clone();
        let mut s = Surface::new(&mut px, 8, 8);
        blur(&mut s, 0.0, 0.0);
        assert_eq!(s.px, &want[..]);
    }

    /// Both separable passes run: omitting the second `convolve` in `blur` leaves
    /// vertical edges sharp.
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

    /// The spread follows the scale on each axis, as the blur does (§15 D601); the
    /// `(1, 1)` row is the control. `blur: 0` and `offset: 0` isolate the dilation.
    ///
    /// flip: `spread_alpha` with one radius (`scale.0.max(scale.1)`) → fails at the
    /// `(4, 1)` row, `(40, 40)` against `(40, 10)`.
    #[test]
    fn a_spread_follows_the_scale_on_each_axis_the_way_the_blur_does() {
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
            run(
                &mut Surface::new(&mut px, w, h),
                &keyed_by_position(effects),
                scale,
            );
            let a = |x: usize, y: usize| px[(y * w + x) * 4 + 3];
            let right = (hi..w).take_while(|&x| a(x, (lo + hi) / 2) > 0).count();
            let down = (hi..h).take_while(|&y| a((lo + hi) / 2, y) > 0).count();
            (right, down)
        };
        assert_eq!(reach((1.0, 1.0)), (10, 10), "the control: 1:1 is unchanged");
        assert_eq!(reach((4.0, 1.0)), (40, 10), "four times as dense across");
        assert_eq!(reach((1.0, 4.0)), (10, 40), "and the other way round");
    }

    /// A spread past the buffer's edge draws the same picture and stops the work
    /// (§15 D615). The timing assertion is a loose 20× ratio (the real gap is
    /// hundreds), so it is not a clock flake.
    ///
    /// flip: `spread_steps` uncapped (`v.abs().round() as i32`) → fails at the
    /// timing assertion; the picture assertion stays green.
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
            let effects = keyed_by_position(effects);
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

    /// The cap at its boundary: `extent - 1`, since the window `[i − n, i + n]`
    /// already spans the line there (§15 D615).
    #[test]
    fn a_spreads_reach_is_capped_at_the_axis_it_runs_along() {
        assert_eq!(spread_steps((3.0, 4.0), 120, 120), (3, 4), "under the cap");
        assert_eq!(spread_steps((119.0, 5.0), 120, 120), (119, 5), "at it");
        assert_eq!(spread_steps((120.0, 5.0), 120, 120), (119, 5), "over it");
        // Per axis; an erosion is capped identically; an empty axis asks for none.
        assert_eq!(spread_steps((999.0, 999.0), 40, 200), (39, 199));
        assert_eq!(spread_steps((-999.0, -1.0), 40, 200), (39, 1));
        assert_eq!(spread_steps((5.0, 5.0), 0, 1), (0, 0));
    }

    /// A switched-off stack reports no ink; each arm is a state the panel produces.
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
                    vars: Default::default(),
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
            assert!(
                !any_ink(&keyed_by_position(stack)),
                "{label} must not cost a buffer"
            );
        }
        assert!(
            any_ink(&keyed_by_position([Effect::new(EffectKind::LayerBlur {
                radius: 1.0
            })])),
            "and a real one must"
        );
    }

    /// A disc: a curved edge shows a resample where a straight one would not.
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

    /// A shadow blurred on a `k`-pixel grid is visually the exact one (§15 D403);
    /// σ is 60 against a budget of 24, so `k` is 3. The tolerance is `<= 2` because
    /// a resample cannot be exact.
    ///
    /// flip: blurring the coarse grid at the full σ rather than σ/k → fails at the
    /// tolerance assertion (45/255 worst).
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

    /// The deadband: a shadow under twice the budget is blurred exactly as authored.
    #[test]
    fn a_shadow_under_twice_the_budget_is_not_resampled_at_all() {
        assert_eq!(ondin_core::effect::shadow_downscale(47.0), 1);
        assert_eq!(ondin_core::effect::shadow_downscale(48.0), 2);
        // `ceil`, so the coarse σ lands under the budget: `round` would give 4.
        assert_eq!(ondin_core::effect::shadow_downscale(100.0), 5);
    }

    /// A resample does not move the shadow: a half-block error in `upsample`'s
    /// `(k-1)/2` shifts the centre of mass, which the tolerance above is too coarse
    /// to catch.
    #[test]
    fn the_resample_does_not_shift_the_shadow() {
        let (w, h) = (200, 200);
        for k in [2usize, 5, 8] {
            let mut px = disc(w, h, 30.0);
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

    /// An out-of-range blur (`<feDropShadow stdDeviation="1e30">` imports as
    /// `Shadow { blur: 2e30 }`) is a wrong picture, not a hang or a panic. Two
    /// sites bound it and neither cap covers the other: `shadow_downscale`'s block
    /// factor and `kernel_radius`. The clock is a loose 5 s.
    ///
    /// `MAX_SHADOW_BLOCK` is a correctness cap: `k as i32` above `i32::MAX` wraps
    /// negative and the GPU's `coarsen` would draw no shadow.
    ///
    /// flip: removing the `min` in `shadow_downscale` → fails at the
    /// `k <= MAX_SHADOW_BLOCK` assertion; removing the `min` in `kernel_radius` →
    /// panics inside `run` at `kernel`'s `2 * r + 1`.
    #[test]
    fn an_out_of_range_blur_is_bounded_at_both_of_its_two_sites() {
        let (w, h) = (200usize, 220usize);
        let mut px = solid(w, h, [0, 0, 0, 255]);
        let effects = [Effect::new(EffectKind::DropShadow(Shadow {
            blur: 2e30,
            ..Shadow::default()
        }))];
        let effects = keyed_by_position(effects);
        let t = std::time::Instant::now();
        run(&mut Surface::new(&mut px, w, h), &effects, (1.0, 1.0));
        let took = t.elapsed();
        assert!(
            took < std::time::Duration::from_secs(5),
            "a shadow of blur 2e30 took {took:?}; before the two caps it was ~180 s \
             of `coarsen` with a 24 GB allocation waiting behind it"
        );

        // The kernel is bounded and finite: a huge sigma overflows `2σ²` to `inf`,
        // giving a normalized box.
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

        // The block factor is bounded, and an infinite sigma gets the cap, not `1`.
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

    /// An effect layer past the cap is resampled into it, never truncated
    /// (§15 D744): the buffer fits and the returned `k` is the one that produced
    /// it. The under-the-cap row pins that `k` comes back identical.
    ///
    /// flip: `.min(u16::MAX as f64)` in place of the `fit` growth → fails at the
    /// first `<= MAX_EFFECT_BUFFER_SIDE` assertion (20000), then the `k` assertion.
    #[test]
    fn an_effect_buffer_over_the_cap_is_resampled_into_it_rather_than_cut_off() {
        use super::{MAX_EFFECT_BUFFER_SIDE, effect_buffer};
        let cap = f64::from(MAX_EFFECT_BUFFER_SIDE);

        // Under the cap: untouched, both numbers.
        let ((w, h), k) = effect_buffer((400.0, 220.0), 1.0);
        assert_eq!((w, h, k), (400, 220, 1.0), "a small layer is not resampled");

        // 8320 across, 128 past the limit.
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

        // Far past it on the other axis, with a stack that already resamples.
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
