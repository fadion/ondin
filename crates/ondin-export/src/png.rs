//! PNG export via the CPU renderer (§7.2). No GPU anywhere in this path.
//!
//! Renders the viewport with `vello_cpu` to a straight-alpha RGBA8 buffer, then
//! encodes it as PNG. Shapes, artboard backgrounds, images **and text** are all
//! rasterized: the scene walk hands a `TextRun` to `ScenePainter::draw_text` and
//! `cpu.rs` fills its glyphs through `vello_cpu`, so this path draws the same ink
//! the GPU canvas does.

use ondin_core::{Document, Resolved};
use ondin_render::{ImageStore, VelloCpuRenderer, Viewport};

/// Rasterize `vp` and encode a PNG. Returns the encoded bytes.
///
/// The image store is built here and thrown away, fine for a one-shot export.
/// It is `ImageStore::new`, never `interactive`: the interactive store may answer
/// a moving picture with a reduced one, and an export would write it soft.
pub fn png(doc: &Document, res: &Resolved, vp: &Viewport) -> Vec<u8> {
    let mut images = ImageStore::new();
    images.prepare(doc);
    let (rgba, width, height) = VelloCpuRenderer::new().render_to_rgba(doc, res, vp, &images);
    encode_rgba8(&rgba, width, height)
}

/// Rasterize **these subtrees** on their own, framed on their combined bounds at
/// the document's own resolution. `None` when the set has no bounds to frame.
///
/// The raster mirror of [`crate::svg::svg_of`]: a viewport cannot say "only these
/// layers", so `scene::build_of` starts the walk at each origin and the result is
/// on transparency. One device pixel per world unit, so the file is the size the
/// inspector's W and H report; other multipliers go through [`RasterOpts`].
///
/// No production caller: only `tests/png.rs` calls it, kept as [`raster_of`]'s 1:1
/// shortcut. `dead_code` cannot say so on a `pub` item.
///
/// `build::in_document_order` is the caller's job, as for `svg_of`: a group plus
/// its child paints the child twice, and pick order paints upside down
/// (§15 D267). This cannot check either.
pub fn png_of(doc: &Document, res: &Resolved, origins: &[ondin_core::NodeId]) -> Option<Vec<u8>> {
    let r = raster_of(doc, res, origins, &RasterOpts::default())?;
    Some(encode_rgba8(&r.rgba, r.width, r.height))
}

/// How an export frames and finishes a raster: what an [`ExportSpec`] asks of the
/// pixels, resolved to numbers.
///
/// Resolved rather than the spec itself: `Frame` and `Width` need the tree and the
/// subject's box, which `crate::plan` has, so the CLI and the panel share one
/// interpretation.
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
/// Returned rather than encoded so the PNG and JPEG encoders share every step in
/// front of them.
pub struct Raster {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Rasterize **these subtrees** on their own, framed on their combined bounds and
/// finished as `opts` asks. `None` when the set has no bounds to frame.
///
/// The raster mirror of [`crate::svg::svg_of`]: [`fn@png`] framed on a selection
/// would include overlapping siblings and the artboard, so `scene::build_of`
/// starts the walk at each origin.
///
/// The finishing steps run trim, pad, composite, and the order matters: trim reads
/// alpha, so a background first leaves nothing to trim; padding before trim would
/// be trimmed back off; composite last covers artwork and padding in one pass.
pub fn raster_of(
    doc: &Document,
    res: &Resolved,
    origins: &[ondin_core::NodeId],
    opts: &RasterOpts,
) -> Option<Raster> {
    let mut images = ImageStore::new();
    images.prepare(doc);
    raster_of_with(doc, res, origins, opts, &images)
}

/// [`raster_of`] with the pictures already decoded — `images` prepared for
/// `doc` by the caller, once, for as many rasters as it makes.
///
/// For anything that rasters more than once over one document: each
/// [`raster_of`] decodes every embedded picture, used or not.
pub fn raster_of_with(
    doc: &Document,
    res: &Resolved,
    origins: &[ondin_core::NodeId],
    opts: &RasterOpts,
    images: &ImageStore,
) -> Option<Raster> {
    let vp = viewport_at(crate::extent(res, origins)?, opts.scale);
    let (rgba, width, height) =
        VelloCpuRenderer::new().render_subtrees_to_rgba(doc, res, &vp, images, origins);
    let mut r = Raster {
        rgba,
        width,
        height,
    };
    if opts.trim {
        r = trimmed(r);
    }
    if opts.pad_square {
        r = padded_square(r);
    }
    if let Some(bg) = opts.background {
        composite_under(&mut r, bg);
    }
    Some(r)
}

/// Drop the fully transparent rows and columns round the edges.
///
/// An image with no ink comes back untouched, not as a 1×1 or 0×0 file, so an
/// empty export still shows the framing.
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
/// Biased to the left/top on an odd remainder.
fn padded_square(r: Raster) -> Raster {
    let side = r.width.max(r.height);
    if side == r.width && side == r.height {
        return r;
    }
    // Transparent, always: the background composite runs after and fills the
    // whole buffer. The size is widened to usize before the multiply (see
    // `square_bytes`). The result itself is unbounded: a `MAX_RASTER_SIDE` square
    // is 17 GB and the allocation simply fails.
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
/// A named function so the multiply can be asserted: in `u32` it overflows at
/// `side == 32768`, which `size_tests` checks.
fn square_bytes(side: u32) -> usize {
    side as usize * side as usize * 4
}

/// Paint `bg` behind the artwork, in place, leaving the result fully opaque.
///
/// Straight alpha in and out, so plain `src-over` arithmetic, not the
/// premultiplied shortcut.
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
/// The floor avoids a division by zero: `scene::device_base` scales by
/// `pixel_size / view.width()`, and a horizontal `Line` or a point has no width.
/// The view is floored, not only the pixel count, which would leave the divisor 0.
pub fn viewport_over(view: ondin_core::kurbo::Rect) -> Viewport {
    viewport_at(view, 1.0)
}

/// The same viewport at `scale` device pixels per world unit.
///
/// The world box does not move, only the pixel count: scaling the view would be a
/// pan.
///
/// The pixel count is capped, since `vello_cpu` takes a `u16` per side; the result
/// is a smaller file rather than a panic, and `plan` reports the clamp. The cap is
/// on the scale, not on each side: clamping axes independently would squash the
/// drawing's aspect ratio.
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
/// Not `u16::MAX`, the one value `vello_cpu` cannot take (§15 D434): it panics
/// inside `vello_common`.
///
/// 65408 is `511 × 128`: the depth buffer holds one bucket per
/// `DEPTH_BUCKET_WIDTH = 128` pixels and `bucket_span` computes
/// `index as u16 * DEPTH_BUCKET_WIDTH`, so 511 is the highest safe bucket index. It
/// also satisfies `snap_to_tile_coordinates` (at most 65532, a multiple of 4).
/// Neither vello constant is public, so a vello upgrade can silently invalidate
/// it; `a_raster_at_the_cap_renders_on_either_axis` re-measures it.
///
/// An alias of [`ondin_render::MAX_RASTER_SIDE`] (§15 D586), shared with the
/// glyph-run rasterizer.
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

    /// `square_bytes` sizes in `usize`: asserted directly, since a real pad at the
    /// overflowing side allocates 4 GB.
    ///
    /// flip: `(side * side * 4) as usize` → fails at the `4_294_967_296` assertion
    /// in release (wraps to 0) and by overflow panic in debug.
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
