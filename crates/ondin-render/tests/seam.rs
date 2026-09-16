//! Two shapes that share an edge must not show a seam between them.
//!
//! Reported as "a line shows between them ... when zoomed out it shows the most":
//! an 800×100 rect above an 800×500 rect on a white 800×600 frame, both the same
//! grey, drew a **lighter** line along `y = 100`. Not a geometry bug — the rects
//! abut exactly. It is vello's conflation artifact (its open issue #49): coverage
//! is computed per path and alpha-blended independently, so where the shared edge
//! falls at fraction `f` across a device pixel, `f(1-f)` of the white behind both
//! leaks between them. That peaks at a quarter of the fill→backdrop contrast —
//! `102` grey on white measured **140** at `f = 0.5`.
//!
//! No antialiasing setting touches it: `Area`, `Msaa8` and `Msaa16` each measured
//! 140 on device, because vello's MSAA sharpens coverage and then collapses it to
//! a scalar before blending, exactly as `Area` does. `scene::snapped_box_world`
//! rounds axis-aligned boxes onto the pixel grid instead, so the two shapes round
//! the shared edge to the same integer and meet on a boundary with no coverage to
//! conflate.
//!
//! The phases below are the ones that were *wrong* before the fix. `f = 0` was
//! always clean — it is here so a "fix" that merely moved the artifact somewhere
//! else cannot pass, and so the fixture is pinned to a grid the seam can be
//! absent from for the right reason.

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{Document, Fill, IdSource, NodeKind, Operation, Resolved, Transaction};
use ondin_render::{ImageStore, VelloCpuRenderer, Viewport};

const GREY: Color = Color::from_rgb8(102, 102, 102);
/// The grey's own channel value, and what every pixel down the seam must read.
const FILL: u8 = 102;
const SPLIT: f64 = 100.0;

/// 800×600 white frame, an 800×100 rect on top of an 800×500 rect, sharing `y = 100`.
fn abutting_pair() -> (Document, Resolved) {
    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut d = Document::new(root);
    let ab = ids.mint();
    let top = ids.mint();
    let bot = ids.mint();
    d.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(800.0, 600.0),
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: ab,
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        },
        Operation::CreateNode {
            id: top,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(800.0, SPLIT),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((0.0, 0.0))),
            name: None,
        },
        Operation::CreateNode {
            id: bot,
            parent: ab,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(800.0, 500.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((0.0, SPLIT))),
            name: None,
        },
    ]))
    .unwrap();
    for id in [top, bot] {
        d.apply(&Transaction(vec![Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Solid(GREY),
                visible: true,
            }],
        }]))
        .unwrap();
    }
    let res = Resolved::rebuild(&d);
    (d, res)
}

/// Render a 200×200 window centred on the shared edge, with the edge sitting
/// `phase` device pixels off the grid, and return the column through it.
fn column_through_the_seam(zoom: f64, phase: f64) -> Vec<u8> {
    let (d, res) = abutting_pair();
    let (w, h) = (200u32, 200u32);
    let vh = h as f64 / zoom;
    let y0 = SPLIT - vh / 2.0 + phase / zoom;
    let vp = Viewport {
        view: Rect::new(0.0, y0, w as f64 / zoom, y0 + vh),
        pixel_size: (w, h),
    };
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(&d, &res, &vp, &ImageStore::new());
    // Well inside the 800-wide rects horizontally, so only the shared edge is in play.
    let x = 100usize;
    (0..h as usize)
        .map(|y| rgba[(y * w as usize + x) * 4])
        .collect()
}

#[test]
fn abutting_shapes_leave_no_seam_at_any_sub_pixel_phase() {
    // A quarter, a half and a third of a pixel off the grid, at zooms either side
    // of 1:1. Half is the worst case — it is the phase that measured 140.
    for &(zoom, phase) in &[
        (1.0, 0.5),
        (1.0, 0.25),
        (2.0, 0.25),
        (4.0, 0.13),
        (0.37, 0.5),
        (0.5, 0.33),
        // Already clean before the fix; snapping must not break it.
        (1.0, 0.0),
    ] {
        let col = column_through_the_seam(zoom, phase);
        // A band about the shared edge, not the whole column: the view is centred
        // on the edge, so at the low zooms the window is tall enough to contain the
        // *frame's* own top edge as well, and that one is a real boundary against
        // the page. 20 rows either side is inside both rects at every zoom here.
        //
        // Every value in the band being the fill is what makes this non-vacuous —
        // a window that missed the artwork would read 0 or 255, not 102.
        let mid = col.len() / 2;
        let band = &col[mid - 20..mid + 20];
        if let Some((i, &v)) = band.iter().enumerate().find(|&(_, &v)| v != FILL) {
            let y = mid - 20 + i;
            panic!(
                "zoom {zoom} phase {phase}: seam at y={y} reads {v}, expected {FILL} \
                 (the reported symptom measured 140 against a {FILL} fill)"
            );
        }
    }
}

/// The rounding must not delete a shape that is thinner than a device pixel.
///
/// At 25% a 2-unit bar is half a pixel, and rounding both its edges to the nearest
/// integer sends them to the same place — so the naive spelling of this fix erases
/// hairlines at low zoom instead of snapping them.
#[test]
fn a_sub_pixel_shape_survives_snapping() {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut d = Document::new(root);
    let bar = ids.mint();
    d.apply(&Transaction(vec![Operation::CreateNode {
        id: bar,
        parent: root,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(400.0, 2.0),
            corner_radii: RoundedRectRadii::default(),
        },
        transform: Some(Affine::translate((0.0, 50.0))),
        name: None,
    }]))
    .unwrap();
    d.apply(&Transaction(vec![Operation::SetFills {
        id: bar,
        fills: vec![Fill {
            brush: Brush::Solid(Color::BLACK),
            visible: true,
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&d);

    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 400.0, 400.0),
        pixel_size: (100, 100), // 0.25x: the 2-unit bar is half a device pixel
    };
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(&d, &res, &vp, &ImageStore::new());
    let ink = (0..100usize)
        .filter(|y| rgba[(y * 100 + 50) * 4 + 3] > 0)
        .count();
    assert!(ink > 0, "the bar rounded away to nothing at 25% zoom");
}
