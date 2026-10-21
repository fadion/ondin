# Effects

The effect model (`ondin_core::effect`, §5.3a) and the pixel passes that draw it (`ondin-render/src/effects.rs`, §6.4). The passes are the single specification read by the CPU backend, the GPU shaders and the SVG writer.

## 5.3a Effects (`effect.rs`)

```rust
pub struct Effect { pub kind: EffectKind, pub visible: bool, pub vars: EffectVars }
pub type EffectVars = [Option<VarId>; 4];   // X, Y, blur, spread (EFFECT_X/_Y/_BLUR/_SPREAD);
                                            // a layer blur's radius is the BLUR slot, others None
impl Effect {
    pub fn numbers(&self) -> [Option<f64>; 4];  // None where the kind has no such number
    pub fn set_number(&mut self, slot: usize, x: f64);  // silent no-op where the kind lacks it
    pub fn number_read(slot: usize, x: f64) -> f64;  // blur within 0..=MAX_BLUR; offset and spread as is
}

pub enum EffectKind {
    DropShadow(Shadow),
    InnerShadow(Shadow),
    LayerBlur { radius: f64 },      // the RADIUS as typed, not the deviation
    Filters(Filters),
}

pub struct Shadow {
    pub offset: kurbo::Vec2,        // layer's own space, so it turns with the layer
    pub blur: f64,                  // radius
    pub spread: f64,                // dilate (or erode) the silhouette before blurring
    pub color: peniko::Color,       // straight sRGB; the ALPHA is the opacity
}

pub struct Filters { pub brightness: f64, pub contrast: f64, pub saturation: f64, pub hue: f64 }
                                    // factors, 1.0 neutral; hue in DEGREES, 0.0 neutral

pub const BLUR_DEVIATION: f64 = 0.5;   // deviation = radius x this
pub const BLUR_CUTOFF: f64 = 3.0;      // deviations worth rasterizing (vello_common's number)
pub fn deviation(radius: f64) -> f64;  // what the three writers take
pub fn reach(radius: f64) -> f64;      // what bounds and culling use
pub fn stack_escape(effects: &[Effect]) -> Insets;
pub fn escaped(bounds: Rect, escape: Insets, world: Affine) -> Rect;   // local escape -> world box
```

- **Four effects**: the ones expressible on all three of the canvas, the raster export and the SVG writer. Background blur and noise are deliberately not built (§1, §15 D333). Per-effect blend modes are a deferred non-goal with a keyboard block reserved.
- **`visible` sits beside the payload** (as on `Fill`) so hiding a shadow does not throw away its tuned numbers. The eye, the shadow colour and the inner shadow's spread are additions to the mockup.
- **A shadow's opacity is its colour's alpha**, with no second field. The rule is "one door": `paint::alpha_of` / `with_alpha` are the single pair that read and write a brush's alpha. (A gradient fill's opacity is its own field because scaling stops is not invertible through zero, §15 D767.)
- **A blur stores the radius the designer types**, not the gaussian deviation; `BLUR_DEVIATION` is the one place the two relate. CSS `blur(12px)` means deviation 6; SVG `feGaussianBlur` and both vello backends take the deviation. A radius is capped at `MAX_BLUR` (1,000), because the CPU backend's buffer grows with it: `Document::apply` refuses past it (`OpError::BlurTooLarge`, at `SetEffects` and `InsertSubtree`), the SVG importer clamps to it, and a bound blur is read within it.
- **`Filters` is factors with 1.0 neutral, hue in degrees**, deliberately unlike `ImageAdjust` (`-1..=1`, zero neutral, see images.md): these numbers go straight into `feColorMatrix` and the shader. `Filters::is_neutral` and `changed` answer "is the row doing anything" and "how many channels are off neutral"; a neutral row must emit no primitive.
- **`escape` is how far an effect's ink lands outside the geometry**, as `Insets` read outward; zero for a hidden effect. A drop shadow reaches `blur + spread` on the side the offset points at, and the same minus the offset on the other. An inner shadow and a `Filters` row escape `ZERO`.
  - `escape` is where ink *lands*, not where passes must be *evaluated*. An inner shadow is cast from everything outside the layer, so its silhouette needs `blur + |spread| + |offset|` out. Both renderers get that free (outside the buffer reads as no layer); the SVG writer pads the filter region (§7), because outside a filter region every primitive is transparent black. Do not "fix" `escape` towards the pad: it would break every bound in §5.9.
- **`stack_escape` is a union taken per stage of what §6.4 composes.** Stage 1 is one isotropic reach from the convolved deviation of all visible `LayerBlur`s (`sigma = sqrt(s1^2 + s2^2)`); stage 2 is each shadow's escape grown by stage 1 on every side; the answer is the union. Composition is done in deviations and turned into a reach once (squaring reaches instead agrees only while `BLUR_CUTOFF` is constant).
- **`escaped` splits a local escape into an offset and a radius** rather than transforming four corners. A blur's reach is a disc; the rotated AABB of a square is up to sqrt(2) too big. The radius becomes the exact half-extents of the mapped ellipse (row Euclidean norm, not absolute row sum). It never shrinks a box.
- **Variables**: a shadow's X, Y, blur, spread and a layer blur's radius bind to number variables (§5.3f, §15 D1074), bindings in the item (`vars`) and the bound number's literal 0. Every reader (ink bounds, scene walk, ghost, SVG writer, snapshot) takes the stack through `Document::resolved_effects`, never `Node::effects` bare.
- Written by `Operation::SetEffects` (§5.7), cached into `Resolved::ink_bounds` (§5.9), drawn through `ScenePainter::push_effect_layer` (§6.2) by the passes below, exported as one `<filter>` per node (§7), saved as additive `NodeDto::effects` (§5.11), authored in the inspector's Effects card (§9.4).

## 6.4 The effect passes (`effects.rs`)

**One implementation of the pixel arithmetic, read by both backends.** `vello` 0.9 has no filters beyond `draw_blurred_rounded_rect`, and `vello_cpu` 0.2.0 implements only `GaussianBlur` and `DropShadow` as single-primitive graphs (no `ColorMatrix`, no `Composite`, so no colour filters and no inner shadow). Leaning on the library for the two it has would mean two blur implementations on two backends that §3 keeps identical. So `effects.rs` is the specification, the CPU backend is the reference, and `fx.wgsl` has one entry point per step of `effects::run`, in the same order, over the same premultiplied 8-bit values. The gaussian kernel is built on the host by `effects::kernel` and passed to the shader as a buffer. `tests/fx_gpu.rs` runs both over one buffer and compares bytes.

`ondin-export` calls `effects::filter_matrix` (once, from the `<filter>` emitter) and `effects::any_ink` (twice) across the crate boundary; that is why SVG needs `ondin-render`. Moving them into core was declined: it would split `filter_matrix`'s twenty coefficients (in the order `fx.wgsl` packs them) from the backend that holds them to account.

`Surface { px: &mut [u8], w, h }` is the seam: RGBA8, neither `vello_cpu::Pixmap` nor `wgpu::Texture`. Its constructor asserts the length (a stride off-by-one would silently shear the picture in release).

Two conventions:

- **Pixels are premultiplied.** Blurring straight alpha haloes soft edges.
- **Filters run in sRGB, not linear.** SVG defaults `color-interpolation-filters` to `linearRGB`, so every emitted filter says `sRGB` explicitly. Matching design tools matters more than physics, and the three writers agreeing matters more than either.

### Staging

`run` is two stages, and the SVG writer is arranged the same way. `Filters` and `LayerBlur` transform the layer's own appearance in list order; then every shadow is cast from *that* result and merged behind or over it. An inner shadow after a drop shadow must not take its silhouette from layer-plus-shadow. `effect::stack_escape` is the same decision for bounds: reordering the stages here means changing it too, and nothing fails if only one moves.

### Colour and geometry details

- `filter_matrix` composes a `Filters` row into the 4x5 `feColorMatrix` array in the order **brightness -> contrast -> saturation -> hue** (the panel's order; composition does not commute). Luminance weights are SVG's sRGB 0.213/0.715/0.072, not NTSC 0.299/0.587/0.114.
- `device_sigma` returns two deviations: a non-uniform scale (viewport, 2x export, skewed ancestor) blurs unevenly, and one averaged sigma would make a blur on a skewed layer round.
- `spread_alpha` is a box dilation (what `feMorphology` does) and takes two half-widths for the same reason. The factor is the ratio of the two column norms `push_effect_layer` computes, so it is unbounded; the bounds side (`escaped`) already maps the spread as an ellipse.
- `effects::spread_steps` caps the half-width at `extent - 1` per axis. Lossless, since both backends read outside the buffer as empty. Cost is still O(w*h*n); running the dilation on the coarse grid is an open decision.

### Inner shadow: complement at the tint

The silhouette both shadow kinds are cast from is the graphic's own alpha. `255 - v` happens once, in the last pass before the confine, never at the silhouette. Inverting first puts two edge rules in contradiction (the offset lookup treats outside as casting while convolve, spread and coarsen treat it as transparent), and on a shape whose ink equals its box at offset (0,0) the render equals no effect at all. The late complement needs no edge flag because every pass between commutes with it: blur with a normalized kernel, `coarsen` (box average), `upsample` (bilinear, weights sum to 1), and dilation-of-complement = erosion. A pass inserted between that is neither linear nor a max/min with matching identity breaks this. `spread_alpha`'s `grow` (`(spread > 0) != inner`) relies on it. The GPU sends `inner` to `Pass::Tint`, not `Pass::Silhouette`.

### Bounding the shadow blur

- `effect::shadow_downscale` / `effects::blur_coarse`: a shadow's blur sigma in device pixels is the deviation times zoom. The layer buffer may not be downscaled for a shadow (the artwork over it stays sharp), but the silhouette alpha field is: past twice the budget it is box-averaged into `ceil(sigma/24)`-pixel blocks, blurred at sigma/k, and read back bilinearly. The half-block offset is the whole read-back; get it wrong and shadows creep with zoom. Both backends take `k` from the one function, and the buffer size from `effects::effect_buffer`. The GPU spells it `coarsen`, a blur over the coarse grid, `upsample`, with the coarse grid in the corner of a full-size scratch texture.
- `coarsen` dispatches one thread per block, so total work is one read per source pixel only while `k <= min(w, h)`; both loops stop at the source edge, not the block edge, and the divisor stays `k^2` (transparent-off-edge).
- Caps: `effect::MAX_SHADOW_BLOCK` = 4096 and `effect::MAX_KERNEL_RADIUS` = 512, asked in `effects::kernel_radius` (the one function both blurs use). A document can hold an absurd deviation (`stdDeviation="1e30"` imports without report); unbounded, it hangs the cover worker and anything that joins it (library settings Save). `MAX_SHADOW_BLOCK` is now a correctness cap (`k as i32` would wrap negative and draw no shadow).

### Tests

CPU pixel tests in `ondin-render/tests/effects.rs` (shadow colour/position, alpha falloff, blur past the layer edge, filters change colour not extent, spread, inner shadow within the ink and on all four edges at (0,0), no-ink stack equals no stack). The SVG suite reads the filter graph. Neither would notice if only the other were right.

## Traps

- `escape` (where ink lands) and the evaluation extent (where passes run) differ for inner shadows; do not unify them.
- `stack_escape` and the staging in `run` are one decision in two places.
- Compositing order of the complement: any new pass between silhouette and tint must commute with `255 - v`.
- Do not store the blur deviation; store the radius.
- A lone alpha door (`paint::alpha_of`) is the invariant, not "no opacity field".
- `filter_matrix` coefficient order is shared with `fx.wgsl`.
