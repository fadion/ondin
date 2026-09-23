//! PNG export via the CPU renderer (§7.2). No GPU anywhere in this path.
//!
//! Renders the viewport with `vello_cpu` to a straight-alpha RGBA8 buffer, then
//! encodes it as PNG. Shapes, artboard backgrounds, images **and text** are all
//! rasterized: the scene walk hands a `TextRun` to `ScenePainter::draw_text` and
//! `cpu.rs` fills its glyphs through `vello_cpu`, so this path draws the same ink
//! the GPU canvas does.
//!
//! This said text was "deferred with the rest of shaping" until 2026-08-20, which
//! had stopped being true long before anyone read it — `png.rs`'s own
//! `text_rasterizes_ink_pixels` asserts the parley→glyph→`vello_cpu` path inks
//! pixels, and was written for exactly this claim. Worth knowing as a shape: a
//! module doc naming something as *not built* is the kind that rots silently,
//! because nothing recompiles when the thing arrives.

use ondin_core::{Document, Resolved};
use ondin_render::{ImageStore, VelloCpuRenderer, Viewport};

/// Rasterize `vp` and encode a PNG. Returns the encoded bytes.
///
/// **The image store is built here and thrown away**, which is right for a
/// one-shot export and wrong for anything else: a cache exists to be reused
/// across renders, and this path renders once. An interactive caller holds its
/// own and should gain an entry point that takes it rather than paying a decode
/// per export.
///
/// **`ImageStore::new`, never `interactive`**, and it matters more than it looks:
/// the interactive store may answer a freshly adjusted picture with a *reduced*
/// one while the value is still moving, and an export that took that would write
/// a soft photograph with nothing on screen to say so. The default is the exact
/// one so that reaching for the fast answer has to be deliberate.
pub fn png(doc: &Document, res: &Resolved, vp: &Viewport) -> Vec<u8> {
    let mut images = ImageStore::new();
    images.prepare(doc);
    let (rgba, width, height) = VelloCpuRenderer::new().render_to_rgba(doc, res, vp, &images);
    encode_rgba8(&rgba, width, height)
}

/// Rasterize **these subtrees** on their own, framed on their combined bounds at
/// the document's own resolution. `None` when the set has no bounds to frame.
///
/// The raster mirror of [`crate::svg::svg_of`], and it exists because a viewport
/// cannot say "only these layers": framed on a selection's box, [`fn@png`] would
/// quietly include every sibling that overlapped it, plus the artboard background
/// behind. `scene::build_of` starts the walk at each origin instead, so what is not
/// asked for is not painted — which is also what puts the result on transparency
/// without anyone having to ask for that either.
///
/// **One device pixel per world unit** — the framing that needs nothing decided,
/// since world units are px by implication everywhere else in the app, so the file
/// comes out exactly the pixel size the inspector's W and H report. This used to be
/// the *only* framing, and said so: a multiplier was "a control that needs somewhere
/// to live before it needs a parameter". It has one now — the Export panel — so the
/// parameter is real and lives on [`RasterOpts`].
///
/// ⚠️ **No production caller, as of 2026-08-22.** The list here used to read "the
/// CLI, *Export as…*, the tests" and was wrong about two of the three: the CLI calls
/// [`png()`] with a viewport, and *Export as…* was removed (§15 D264) — which took the
/// only one that was ever true. `ondin-export` is a library and this is `pub`, so
/// `dead_code` will never say so and every gate stays green; the single call left in
/// the workspace is in `tests/png.rs`. Kept on purpose as [`raster_of`]'s 1:1
/// shortcut with the framing argument above attached to it, **but that is a
/// judgement rather than a fact about the code**, and the next reader should know
/// they are looking at an API with no user rather than a load-bearing one.
///
/// **`build::in_document_order` is the caller's**, as it is for `svg_of`, and for
/// both of its halves: a set holding a group and something inside it would paint that
/// child twice, and a set in *pick* order would paint it upside down (§15 D267). This
/// cannot check either — it does not know whether a repetition was meant, and a walk
/// has no way to tell a deliberate order from an accidental one.
pub fn png_of(doc: &Document, res: &Resolved, origins: &[ondin_core::NodeId]) -> Option<Vec<u8>> {
    let r = raster_of(doc, res, origins, &RasterOpts::default())?;
    Some(encode_rgba8(&r.rgba, r.width, r.height))
}

/// How an export frames and finishes a raster — everything an [`ExportSpec`]
/// asks of the pixels, resolved to numbers this module can act on.
///
/// **Resolved, not the spec itself.** `ExportBackground::Frame` needs the tree to
/// answer and `ExportScale::Width` needs the subject's box; both are decided in
/// `crate::plan`, which has the document in hand, so this crate's raster path
/// takes a colour and a multiplier and has no opinions left to form. That is also
/// what lets the CLI and the panel produce identical bytes — there is one place
/// the spec is interpreted.
///
/// [`ExportSpec`]: ondin_core::ExportSpec
#[derive(Clone, Copy, Debug)]
pub struct RasterOpts {
    /// Device pixels per world unit.
    pub scale: f64,
    /// Composited *under* the artwork, or `None` to leave it on transparency.
    pub background: Option<ondin_core::peniko::Color>,
    /// Drop fully transparent rows and columns from the edges.
    pub trim: bool,
    /// Pad the result out to a square.
    pub pad_square: bool,
}

impl Default for RasterOpts {
    fn default() -> Self {
        Self {
            scale: 1.0,
            background: None,
            trim: false,
            pad_square: false,
        }
    }
}

/// A rasterized export before it is encoded: straight-alpha RGBA8 and its size.
///
/// Returned rather than encoded so the two encoders can share every step in front
/// of them. A JPEG that framed, trimmed or padded even slightly differently from
/// the PNG beside it would be a difference nobody could see until the two files
/// were laid over each other — the same reasoning `crate::extent` carries for the
/// viewBox and the pixel grid.
pub struct Raster {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Rasterize **these subtrees** on their own, framed on their combined bounds and
/// finished as `opts` asks. `None` when the set has no bounds to frame.
///
/// The raster mirror of [`crate::svg::svg_of`], and it exists because a viewport
/// cannot say "only these layers": framed on a selection's box, [`fn@png`] would
/// quietly include every sibling that overlapped it, plus the artboard background
/// behind. `scene::build_of` starts the walk at each origin instead, so what is not
/// asked for is not painted — which is also what puts the result on transparency
/// when no background is asked for.
///
/// **The three finishing steps run trim → pad → composite, and the order is the
/// whole of their meaning.** Trim reads alpha, so it has to come before a
/// background fills that alpha in — composite first and there is nothing
/// transparent left to trim, which is a silent no-op rather than an error. Pad
/// then squares what trim left, since padding first would only have its padding
/// trimmed straight back off. Composite is last so that one pass covers the
/// artwork and the padding alike: filling the pad as it was added would be the
/// same colour written by two pieces of code that had to agree.
pub fn raster_of(
    doc: &Document,
    res: &Resolved,
    origins: &[ondin_core::NodeId],
    opts: &RasterOpts,
) -> Option<Raster> {
    let vp = viewport_at(crate::extent(res, origins)?, opts.scale);
    let mut images = ImageStore::new();
    images.prepare(doc);
    let (rgba, width, height) =
        VelloCpuRenderer::new().render_subtrees_to_rgba(doc, res, &vp, &images, origins);
    let mut r = Raster {
        rgba,
        width,
        height,
    };
    if opts.trim {
        r = trimmed(r);
    }
    if opts.pad_square {
        r = padded_square(r, opts.background);
    }
    if let Some(bg) = opts.background {
        composite_under(&mut r, bg);
    }
    Some(r)
}

/// Drop the fully transparent rows and columns round the edges.
///
/// **An image with no ink at all comes back untouched**, rather than as the 1×1
/// or 0×0 the loop would otherwise produce. A layer whose export is empty is a
/// state worth being able to see — a hidden child, a shape whose fill was turned
/// off — and a file that is one transparent pixel says nothing about which of
/// those happened, where one the size of the layer at least shows the framing was
/// right.
fn trimmed(r: Raster) -> Raster {
    let (w, h) = (r.width as usize, r.height as usize);
    let alpha = |x: usize, y: usize| r.rgba[(y * w + x) * 4 + 3];
    let mut top = 0;
    while top < h && (0..w).all(|x| alpha(x, top) == 0) {
        top += 1;
    }
    if top == h {
        return r; // nothing drawn at all
    }
    let mut bottom = h - 1;
    while bottom > top && (0..w).all(|x| alpha(x, bottom) == 0) {
        bottom -= 1;
    }
    let mut left = 0;
    while left < w && (top..=bottom).all(|y| alpha(left, y) == 0) {
        left += 1;
    }
    let mut right = w - 1;
    while right > left && (top..=bottom).all(|y| alpha(right, y) == 0) {
        right -= 1;
    }
    let (nw, nh) = (right - left + 1, bottom - top + 1);
    let mut out = Vec::with_capacity(nw * nh * 4);
    for y in top..=bottom {
        let row = (y * w + left) * 4;
        out.extend_from_slice(&r.rgba[row..row + nw * 4]);
    }
    Raster {
        rgba: out,
        width: nw as u32,
        height: nh as u32,
    }
}

/// Centre the raster in a square of its longer side.
///
/// **Centred, and biased to the left/top on an odd remainder**, which is the only
/// choice here that could surprise: an icon set padded this way is consistent with
/// itself, which is the point, and a half-pixel has to go somewhere.
fn padded_square(r: Raster, _fill: Option<ondin_core::peniko::Color>) -> Raster {
    let side = r.width.max(r.height);
    if side == r.width && side == r.height {
        return r;
    }
    // Transparent, always — the background composite runs after this and fills the
    // whole buffer, so painting the pad here would be doing it twice and would
    // have to agree with the other one exactly.
    // ⚠️ **Widened before the multiply, not after.** `side * side * 4` in `u32`
    // overflows at `side == 32768`, which is well inside what the clamp permits
    // and nowhere near large: a 4096×64 banner at 8× is a 32768×512 raster of
    // 131 KB, and *Pad to square* on it killed the whole `--all` run before a
    // single file was written. In debug that was an overflow panic here; in
    // release the product wrapped to 0, `vec![0u8; 0]` was allocated, and the
    // copy below panicked one line down with a slice message naming neither the
    // export nor the size. Every other size computation in this module widens
    // first (`trimmed`, `jpeg::encode`); this was the one that did not.
    //
    // ⚠️ **What is still unbounded is the result.** A square of `MAX_RASTER_SIDE`
    // is 17 GB, and nothing here refuses it — the allocation simply fails. That
    // is a *size* decision rather than an arithmetic one, it is the same decision
    // `[S8.2-L5-05]` asks for about the raster path as a whole, and it is better
    // taken once for both than guessed at here.
    let (w, side_us) = (r.width as usize, side as usize);
    let mut out = vec![0u8; square_bytes(side)];
    let dx = ((side - r.width) / 2) as usize;
    let dy = ((side - r.height) / 2) as usize;
    for y in 0..r.height as usize {
        let src = y * w * 4;
        let dst = ((y + dy) * side_us + dx) * 4;
        out[dst..dst + w * 4].copy_from_slice(&r.rgba[src..src + w * 4]);
    }
    Raster {
        rgba: out,
        width: side,
        height: side,
    }
}

/// How many bytes a square RGBA8 buffer of `side` needs.
///
/// **A named function for one multiply**, because the multiply is the bug: in
/// `u32` it overflows at `side == 32768` and there is no size in this module big
/// enough to make that obvious from the call site. Here it can be asserted
/// against a number, which is what `size_tests` below does.
fn square_bytes(side: u32) -> usize {
    side as usize * side as usize * 4
}

/// Paint `bg` behind the artwork, in place, leaving the result fully opaque.
///
/// Straight alpha in and straight alpha out, so this is the plain `src-over`
/// arithmetic and not the premultiplied shortcut — `render_to_rgba`'s own doc
/// comment is what pins which of the two this buffer is.
fn composite_under(r: &mut Raster, bg: ondin_core::peniko::Color) {
    let c = bg.to_rgba8();
    let (br, bgc, bb) = (c.r as u32, c.g as u32, c.b as u32);
    for px in r.rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a == 255 {
            continue;
        }
        let inv = 255 - a;
        px[0] = ((px[0] as u32 * a + br * inv) / 255) as u8;
        px[1] = ((px[1] as u32 * a + bgc * inv) / 255) as u8;
        px[2] = ((px[2] as u32 * a + bb * inv) / 255) as u8;
        px[3] = 255;
    }
}

/// A 1:1 viewport over `view`, with both extents floored at one world unit.
///
/// **The floor is not defensive padding, it is a division.** `scene::device_base`
/// scales by `pixel_size / view.width()`, so a box with no width at all — which is
/// exactly what a horizontal `Line` has, and what any single point has — makes that
/// an infinity and the render meaningless rather than merely wrong. Flooring the
/// *view* rather than only the pixel count is what keeps the two in step: raising
/// the pixel count alone leaves the divisor at zero.
pub fn viewport_over(view: ondin_core::kurbo::Rect) -> Viewport {
    viewport_at(view, 1.0)
}

/// The same viewport at `scale` device pixels per world unit.
///
/// **The world box does not move — only the pixel count does.** That is what a
/// multiplier means here: the same drawing, denser. Scaling the *view* instead
/// would frame a different region at the same density, which is a pan, and would
/// put a 2× export somewhere else on the artwork rather than nearer to it.
///
/// The pixel count is capped. A `512w` on a hairline, or a 4× on a poster, can ask
/// for a buffer that is not merely large but impossible — `vello_cpu` takes a
/// `u16` per side — and the honest failure is a smaller file than was asked for
/// rather than a panic inside the renderer. `plan` reports what it clamped.
///
/// **The cap is on the scale, not on each side**, and the difference is the whole
/// of whether the result is still the drawing. Clamping the axes independently
/// squashes a 100×50 asked for at 2000× into a 65535 square — the same picture at
/// the wrong aspect ratio, which is a worse answer than a smaller one and one
/// nobody would read as "too big". Backing the multiplier off until both sides fit
/// keeps the proportions the layer was drawn at.
pub fn viewport_at(view: ondin_core::kurbo::Rect, scale: f64) -> Viewport {
    let w = view.width().max(1.0);
    let h = view.height().max(1.0);
    let scale = if scale.is_finite() {
        scale.max(1e-4)
    } else {
        1.0
    };
    let ceiling = (MAX_RASTER_SIDE as f64 / w.max(h)).max(1e-4);
    let scale = scale.min(ceiling);
    Viewport {
        view: ondin_core::kurbo::Rect::new(
            view.min_x(),
            view.min_y(),
            view.min_x() + w,
            view.min_y() + h,
        ),
        pixel_size: (px(w * scale), px(h * scale)),
    }
}

/// One side of a raster: at least a pixel, and never more than the CPU renderer
/// can take.
///
/// ⚠️ **Not `u16::MAX`, which is the one value it cannot take** (§15 D434, which
/// amends D274). This was
/// `u16::MAX` and the clamp therefore handed the renderer exactly the size that
/// panics it: `ondin export --png --width 65535` on a 100×50 rect aborted inside
/// `vello_common`, and so did a plain 1× export of a document whose one rect is
/// `1e30` wide — no flag involved, only geometry a `.ondin` can carry.
/// `viewport_at`'s own doc promises the opposite (*"the honest failure is a
/// smaller file than was asked for rather than a panic inside the renderer"*), so
/// the promise was true of everything except the number it clamped to.
///
/// **65408 is `511 × 128`, and both factors are vello's.** `vello_cpu`'s depth
/// buffer holds one bucket per `DEPTH_BUCKET_WIDTH = 128` pixels
/// (`coarse/depth.rs`), and `bucket_span` computes a bucket run's pixel span as
/// `index as u16 * DEPTH_BUCKET_WIDTH` — so the highest bucket index that cannot
/// overflow a `u16` is `511`, and the widest buffer that cannot produce a higher
/// one is `511 × 128`. That also clears the second site, `vello_common`'s
/// `snap_to_tile_coordinates`, whose `checked_next_multiple_of(Tile::WIDTH)`
/// needs a side of at most `65532`; 65408 is already a multiple of 4.
///
/// ⚠️ **Neither constant is public**, so this number cannot be derived at compile
/// time and a vello upgrade can silently invalidate it.
/// `a_raster_at_the_cap_renders_on_either_axis` is what re-measures it — it renders at exactly this side on both axes, which
/// is the only thing here that would go red.
///
/// ⚠️ **The number itself moved to [`ondin_render::MAX_RASTER_SIDE`]** (§15 D586)
/// and this is an alias. The limit is the CPU backend's, and `ondin-render` had
/// made the same `as u16` mistake at a second site
/// (`rasterize_glyph_run`, `[S21-L1-05]`) while the argument for the right value
/// lived only here. One number, one argument, two callers.
pub const MAX_RASTER_SIDE: u32 = ondin_render::MAX_RASTER_SIDE;

fn px(units: f64) -> u32 {
    (units.ceil() as u32).clamp(1, MAX_RASTER_SIDE)
}

/// Encode a straight-alpha RGBA8 buffer as PNG bytes.
pub fn encode_rgba8(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(rgba).expect("png data");
    }
    out
}

#[cfg(test)]
mod size_tests {
    use super::*;

    /// **`[S8.2-L1-02]`'s loss, at the one line it lives on.**
    ///
    /// `side * side * 4` was `u32` arithmetic and overflowed at `side == 32768` —
    /// which a 4096×64 banner at 8× reaches, a 131 KB raster and an ordinary
    /// export set. In debug that was a multiply-overflow panic; in release the
    /// product wrapped to **exactly 0**, so `vec![0u8; 0]` was allocated and the
    /// copy ran off the end one line later with a slice message naming neither
    /// the file nor the size. `ondin export --all` renders everything before it
    /// writes anything, so one such layer lost the whole run.
    ///
    /// ⚠️ **Asserted here rather than through a real pad**, because the smallest
    /// raster that reaches the overflow squares to 4 GB: a test that padded it
    /// would be measuring the allocator. `MAX_RASTER_SIDE` is included so the
    /// widening is checked at the largest side the clamp can hand this function,
    /// not only at the one that used to wrap.
    ///
    /// Flip-check, run: `(side * side * 4) as usize` fails here in debug with
    /// *"attempt to multiply with overflow"* and in release at `left: 0 right:
    /// 4294967296` — the wrapped value, which is the release bug stated as a
    /// number.
    #[test]
    fn a_padded_square_sizes_its_buffer_in_usize() {
        assert_eq!(square_bytes(2), 16);
        assert_eq!(square_bytes(32_768), 4_294_967_296);
        assert_eq!(
            square_bytes(MAX_RASTER_SIDE),
            MAX_RASTER_SIDE as usize * MAX_RASTER_SIDE as usize * 4
        );
    }
}
