//! ICO and ICNS: one file holding a layer at several square pixel sizes (§7).
//!
//! Each size is its own render of the vector, never a resample of another: the
//! layer's box is centred in a square of its longer side and that square is drawn
//! at exactly N × N pixels. A size can come from a frame of its own
//! (`‹layer›-‹size›`), or from a copy of the layer pixel-fitted to that size's grid.
//!
//! Both containers are written here rather than through a crate: each is a short
//! header and a table of PNG entries, and the `icns` crate would bring a second,
//! older `png` into the tree.

use crate::png::Raster;
use ondin_core::kurbo::{Affine, Rect};
use ondin_core::{Document, ExportFormat, ExportSpec, NodeId, NodeKind, Resolved};
use ondin_render::{ImageStore, VelloCpuRenderer, Viewport};

/// One size of an icon file, rendered.
pub struct Entry {
    pub size: u32,
    /// Straight-alpha RGBA8, `size` × `size`.
    pub raster: Raster,
    /// The frame this size was drawn from, when it had one of its own.
    pub frame: Option<NodeId>,
}

/// The longest side the trim pass measures at: enough to place an edge within a
/// fraction of the smallest size's pixel, without rendering a page-sized layer big.
const MEASURE_MAX: f64 = 2048.0;

/// The frames that stand in for sizes of `node`: for each of `sizes`, the first
/// frame on `node`'s page, in document order, named `‹name›-‹size›`. Only sizes
/// with such a frame are answered, smallest first.
pub fn size_frames(doc: &Document, node: NodeId, sizes: &[u32]) -> Vec<(u32, NodeId)> {
    let (Some(n), Some(page)) = (doc.get(node), doc.page_of(node)) else {
        return Vec::new();
    };
    let name = n.name();
    let frames: Vec<NodeId> = ondin_core::build::subtree_nodes(doc, &[page])
        .into_iter()
        .filter(|id| {
            *id != node
                && doc
                    .get(*id)
                    .is_some_and(|f| matches!(f.kind(), NodeKind::Artboard { .. }))
        })
        .collect();
    sizes
        .iter()
        .filter_map(|&size| {
            let want = format!("{name}-{size}");
            let id = frames
                .iter()
                .copied()
                .find(|id| doc.get(*id).is_some_and(|f| f.name() == want))?;
            Some((size, id))
        })
        .collect()
}

/// Every size `spec` asks of `node`, rendered. `None` when the layer has no box.
pub fn entries(
    doc: &Document,
    res: &Resolved,
    node: NodeId,
    spec: &ExportSpec,
) -> Option<Vec<Entry>> {
    let sizes = spec.icon_sizes();
    let frames = match spec.size_frames {
        true => size_frames(doc, node, &sizes),
        false => Vec::new(),
    };
    let mut images = ImageStore::new();
    images.prepare(doc);
    let background = crate::plan::background_color(doc, node, spec);
    // A slice draws the page through its box, so its square cannot widen the view
    // (the page beyond the slice would show): it is drawn at its own box and padded.
    if let Some((view, origins)) = crate::plan::slice_region(doc, res, node) {
        return Some(
            sizes
                .iter()
                .map(|&size| {
                    let longer = view.width().max(view.height()).max(1.0);
                    let opts = crate::png::RasterOpts {
                        // A hair under, so `ceil` cannot reach one past the size.
                        scale: (f64::from(size) - 1e-6) / longer,
                        background: None,
                        trim: false,
                        pad_square: true,
                    };
                    let r = crate::png::raster_region(doc, res, &origins, view, &opts);
                    Entry {
                        size,
                        raster: finished(squared(r, size), background),
                        frame: None,
                    }
                })
                .collect(),
        );
    }
    let mut view = res.ink_bounds(node)?;
    if spec.trim {
        view = trimmed_view(doc, res, node, view, &images, &sizes).unwrap_or(view);
    }
    let square = square_of(view);
    Some(
        sizes
            .iter()
            .map(|&size| {
                let frame = frames.iter().find(|(s, _)| *s == size).map(|(_, id)| *id);
                let raster = match frame {
                    // Drawn as it is, on its own box: it was drawn at this size.
                    Some(f) => {
                        let fv = res.world_bounds(f).unwrap_or(square);
                        draw(doc, res, &[f], square_of(fv), size, &images)
                    }
                    None if spec.pixel_fit => fitted(doc, res, node, square, size, &images)
                        .unwrap_or_else(|| draw(doc, res, &[node], square, size, &images)),
                    None => draw(doc, res, &[node], square, size, &images),
                };
                Entry {
                    size,
                    raster: finished(raster, background),
                    frame,
                }
            })
            .collect(),
    )
}

/// `entries` encoded as `format` asks; `None` for a format that is no icon file.
pub fn encode(format: ExportFormat, entries: &[Entry]) -> Option<Vec<u8>> {
    let pngs: Vec<(u32, Vec<u8>)> = entries
        .iter()
        .map(|e| {
            let r = &e.raster;
            (e.size, crate::png::encode_rgba8(&r.rgba, r.width, r.height))
        })
        .collect();
    match format {
        ExportFormat::Ico => Some(ico(&pngs)),
        ExportFormat::Icns => Some(icns(&pngs)),
        _ => None,
    }
}

/// The square of `view`'s longer side, centred on it.
fn square_of(view: Rect) -> Rect {
    let side = view.width().max(view.height()).max(1.0);
    Rect::from_center_size(view.center(), (side, side))
}

/// `origins` drawn through the world square `square` at exactly `size` pixels.
fn draw(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    square: Rect,
    size: u32,
    images: &ImageStore,
) -> Raster {
    let vp = Viewport {
        view: square,
        pixel_size: (size, size),
    };
    let (rgba, width, height) =
        VelloCpuRenderer::new().render_subtrees_to_rgba(doc, res, &vp, images, origins);
    Raster {
        rgba,
        width,
        height,
    }
}

/// `node` drawn from a copy of the document fitted to `size`'s pixel grid over
/// `square`, which is drawn unmoved so the grid and the pixels agree. `None` when
/// the fit cannot be applied.
fn fitted(
    doc: &Document,
    res: &Resolved,
    node: NodeId,
    square: Rect,
    size: u32,
    images: &ImageStore,
) -> Option<Raster> {
    let grid = Affine::scale(f64::from(size) / square.width())
        * Affine::translate((-square.x0, -square.y0));
    let (tx, _) = ondin_core::pixel_fit::fit_on(doc, res, &[node], grid);
    if tx.0.is_empty() {
        return Some(draw(doc, res, &[node], square, size, images));
    }
    let mut copy = doc.clone();
    copy.apply(&tx).ok()?;
    let res = Resolved::rebuild(&copy);
    Some(draw(&copy, &res, &[node], square, size, images))
}

/// The part of `view` that `node`'s ink reaches, measured on a render: what
/// *Trim transparent edges* keeps, in world units. `None` when nothing is drawn.
fn trimmed_view(
    doc: &Document,
    res: &Resolved,
    node: NodeId,
    view: Rect,
    images: &ImageStore,
    sizes: &[u32],
) -> Option<Rect> {
    let longer = view.width().max(view.height()).max(1.0);
    let largest = f64::from(sizes.iter().copied().max().unwrap_or(256));
    let side = (largest * 2.0).clamp(256.0, MEASURE_MAX);
    let vp = crate::png::viewport_at(view, side / longer);
    let (rgba, w, h) =
        VelloCpuRenderer::new().render_subtrees_to_rgba(doc, res, &vp, images, &[node]);
    let (w, h) = (w as usize, h as usize);
    let mut ink: Option<(usize, usize, usize, usize)> = None;
    for y in 0..h {
        for x in 0..w {
            if rgba[(y * w + x) * 4 + 3] != 0 {
                let (x0, y0, x1, y1) = ink.unwrap_or((x, y, x, y));
                ink = Some((x0.min(x), y0.min(y), x1.max(x), y1.max(y)));
            }
        }
    }
    let (x0, y0, x1, y1) = ink?;
    let (sx, sy) = (
        vp.view.width() / f64::from(vp.pixel_size.0),
        vp.view.height() / f64::from(vp.pixel_size.1),
    );
    Some(Rect::new(
        vp.view.x0 + x0 as f64 * sx,
        vp.view.y0 + y0 as f64 * sy,
        vp.view.x0 + (x1 + 1) as f64 * sx,
        vp.view.y0 + (y1 + 1) as f64 * sy,
    ))
}

/// `r` centred in exactly `size` × `size`: padded where it is short, cut where a
/// rounding left it a pixel long.
fn squared(r: Raster, size: u32) -> Raster {
    if r.width == size && r.height == size {
        return r;
    }
    let n = size as usize;
    let mut out = vec![0u8; n * n * 4];
    let place = |have: u32| -> (usize, usize, usize) {
        // (source start, destination start, length)
        match have >= size {
            true => (((have - size) / 2) as usize, 0, n),
            false => (0, ((size - have) / 2) as usize, have as usize),
        }
    };
    let (sx, dx, w) = place(r.width);
    let (sy, dy, h) = place(r.height);
    let stride = r.width as usize * 4;
    for y in 0..h {
        let src = (sy + y) * stride + sx * 4;
        let dst = ((dy + y) * n + dx) * 4;
        out[dst..dst + w * 4].copy_from_slice(&r.rgba[src..src + w * 4]);
    }
    Raster {
        rgba: out,
        width: size,
        height: size,
    }
}

/// `r` with the row's background under it, the padding included.
fn finished(mut r: Raster, background: Option<ondin_core::peniko::Color>) -> Raster {
    if let Some(bg) = background {
        crate::png::composite_under(&mut r, bg);
    }
    r
}

/// An ICO of PNG entries: a six-byte header, a sixteen-byte record per entry, then
/// the PNGs. A side of 256 is written as 0, the format's spelling of it.
fn ico(pngs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (size, png) in pngs {
        let side = match *size {
            256.. => 0u8,
            s => s as u8,
        };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in pngs {
        out.extend_from_slice(png);
    }
    out
}

/// The ICNS type codes a PNG of `size` is filed under: its own, and the Retina
/// code of the half size where there is one, as `iconutil` writes them.
fn icns_types(size: u32) -> &'static [&'static [u8; 4]] {
    match size {
        16 => &[b"icp4"],
        32 => &[b"icp5", b"ic11"],
        64 => &[b"icp6", b"ic12"],
        128 => &[b"ic07"],
        256 => &[b"ic08", b"ic13"],
        512 => &[b"ic09", b"ic14"],
        1024 => &[b"ic10"],
        _ => &[],
    }
}

/// An ICNS of PNG entries: `icns` and the file's length, then a type code, a
/// length and the PNG per entry, big-endian throughout.
fn icns(pngs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (size, png) in pngs {
        for code in icns_types(*size) {
            body.extend_from_slice(*code);
            body.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
            body.extend_from_slice(png);
        }
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_of(size: u32) -> Vec<u8> {
        crate::png::encode_rgba8(&vec![0; (size * size * 4) as usize], size, size)
    }

    /// The ICO header counts its entries, each record's offset lands on its PNG,
    /// and 256 is written as 0.
    #[test]
    fn an_ico_records_where_each_png_starts() {
        let pngs: Vec<(u32, Vec<u8>)> = [16, 256].map(|s| (s, png_of(s))).into();
        let out = ico(&pngs);
        assert_eq!(&out[..6], &[0, 0, 1, 0, 2, 0]);
        let record = |i: usize| &out[6 + 16 * i..6 + 16 * (i + 1)];
        assert_eq!(record(0)[0], 16);
        assert_eq!(record(1)[0], 0, "256 is spelled 0");
        for (i, (_, png)) in pngs.iter().enumerate() {
            let len = u32::from_le_bytes(record(i)[8..12].try_into().unwrap()) as usize;
            let at = u32::from_le_bytes(record(i)[12..16].try_into().unwrap()) as usize;
            assert_eq!(&out[at..at + len], png.as_slice());
        }
    }

    /// An ICNS is its declared length, and files a 32 under both its codes.
    #[test]
    fn an_icns_files_each_size_under_its_codes() {
        let pngs: Vec<(u32, Vec<u8>)> = [16, 32].map(|s| (s, png_of(s))).into();
        let out = icns(&pngs);
        assert_eq!(&out[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(out[4..8].try_into().unwrap()) as usize,
            out.len()
        );
        let mut codes = Vec::new();
        let mut at = 8;
        while at < out.len() {
            codes.push(String::from_utf8_lossy(&out[at..at + 4]).into_owned());
            at += u32::from_be_bytes(out[at + 4..at + 8].try_into().unwrap()) as usize;
        }
        assert_eq!(codes, ["icp4", "icp5", "ic11"]);
        assert_eq!(at, out.len());
    }

    /// A raster a pixel long is cut evenly and one short is padded, both to size.
    #[test]
    fn a_raster_squares_to_exactly_its_size() {
        let r = |w: u32, h: u32| Raster {
            rgba: vec![255; (w * h * 4) as usize],
            width: w,
            height: h,
        };
        let s = squared(r(17, 8), 16);
        assert_eq!((s.width, s.height), (16, 16));
        assert_eq!(s.rgba.len(), 16 * 16 * 4);
        // Rows 4..12 are ink, the rest padding.
        assert_eq!(s.rgba[(3 * 16) * 4 + 3], 0);
        assert_eq!(s.rgba[(4 * 16) * 4 + 3], 255);
        assert_eq!(s.rgba[(12 * 16) * 4 + 3], 0);
    }
}
