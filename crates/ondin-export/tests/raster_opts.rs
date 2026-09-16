//! What the Export panel's controls do to the pixels (§7.2) — the scale
//! multiplier, the background, the trim and the pad.
//!
//! These assert on the **raster**, not on the encoded file, because every one of
//! them is a claim about where the ink landed and a PNG's bytes say nothing about
//! that without a decoder. `raster_of` is the seam both encoders sit behind, so
//! testing it covers JPEG as well.

use ondin_core::kurbo::Size;
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, ExportFormat, ExportScale, ExportSpec, Fill, IdSource, NodeId, NodeKind,
    Operation, Resolved, Transaction,
};
use ondin_export::png::{RasterOpts, raster_of};

const RED: Color = Color::from_rgba8(220, 30, 40, 255);

/// A 400×300 frame with **no background** holding one 100×50 red rect at
/// (150, 125) — centred, with a wide transparent margin all round it.
///
/// The margin is the point: bounds are tight to the ink for a shape, so a frame
/// is the ordinary subject where a trim has anything to do at all.
fn frame_with_a_small_rect() -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, rect) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
            },
            transform: None,
            name: Some("Board".into()),
        },
        Operation::CreateNode {
            id: rect,
            parent: frame,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 50.0),
                corner_radii: Default::default(),
            },
            transform: Some(ondin_core::kurbo::Affine::translate((150.0, 125.0))),
            name: Some("Button".into()),
        },
        Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: Brush::Solid(RED),
                visible: true,
            }],
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, frame, rect)
}

fn px(r: &ondin_export::png::Raster, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * r.width + x) * 4) as usize;
    [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2], r.rgba[i + 3]]
}

/// A multiplier is "the same drawing, denser" — so the pixel grid grows and the
/// framing does not move.
///
/// **The second half is the one worth asserting.** Scaling the *view* instead of
/// the pixel count would produce a buffer of exactly the same size, with the
/// artwork occupying a quarter of it and three quarters empty — a bug that reads
/// as "my 2× export has a huge margin" and is invisible in the arithmetic.
#[test]
fn a_scale_multiplies_the_pixels_and_leaves_the_framing_alone() {
    let (doc, res, _, rect) = frame_with_a_small_rect();
    let one = raster_of(&doc, &res, &[rect], &RasterOpts::default()).expect("bounds");
    assert_eq!((one.width, one.height), (100, 50));

    let two = raster_of(
        &doc,
        &res,
        &[rect],
        &RasterOpts {
            scale: 2.0,
            ..RasterOpts::default()
        },
    )
    .expect("bounds");
    assert_eq!((two.width, two.height), (200, 100));
    // Every corner is still the fill, which is what "the framing did not move"
    // means for a subject that fills its own bounds.
    for (x, y) in [(0, 0), (199, 0), (0, 99), (199, 99)] {
        assert_eq!(px(&two, x, y), [220, 30, 40, 255], "at {x},{y}");
    }
}

/// A background is composited *under* the artwork and leaves the result opaque —
/// which is also the whole of what a JPEG export needs, since its matte is
/// resolved to this same colour before the encoder sees anything.
#[test]
fn a_background_fills_the_transparent_parts_and_nothing_else() {
    let (doc, res, frame, _) = frame_with_a_small_rect();
    let bare = raster_of(&doc, &res, &[frame], &RasterOpts::default()).expect("bounds");
    assert_eq!(
        px(&bare, 0, 0)[3],
        0,
        "the frame has no background of its own"
    );

    let blue = Color::from_rgba8(0, 0, 255, 255);
    let filled = raster_of(
        &doc,
        &res,
        &[frame],
        &RasterOpts {
            background: Some(blue),
            ..RasterOpts::default()
        },
    )
    .expect("bounds");
    assert_eq!((filled.width, filled.height), (bare.width, bare.height));
    assert_eq!(px(&filled, 0, 0), [0, 0, 255, 255], "the empty corner");
    assert_eq!(
        px(&filled, 200, 150),
        [220, 30, 40, 255],
        "and the ink is untouched, not tinted"
    );
}

/// Trim drops the transparent margin a fixed-size subject leaves round its ink.
#[test]
fn a_trim_shrinks_to_the_ink() {
    let (doc, res, frame, _) = frame_with_a_small_rect();
    let whole = raster_of(&doc, &res, &[frame], &RasterOpts::default()).expect("bounds");
    assert_eq!(
        (whole.width, whole.height),
        (400, 300),
        "the fixture must have a margin to trim, or this test is about nothing"
    );

    let tight = raster_of(
        &doc,
        &res,
        &[frame],
        &RasterOpts {
            trim: true,
            ..RasterOpts::default()
        },
    )
    .expect("bounds");
    assert_eq!((tight.width, tight.height), (100, 50));
    assert_eq!(px(&tight, 0, 0), [220, 30, 40, 255]);
}

/// A subject with nothing drawn in it comes back whole rather than as a 1×1 — a
/// file the size of the layer says the framing was right and the layer was empty,
/// where a single pixel says neither.
#[test]
fn a_trim_of_an_empty_subject_is_a_no_op() {
    let (doc, res, frame, rect) = frame_with_a_small_rect();
    let mut doc = doc;
    doc.apply(&Transaction(vec![Operation::SetVisible {
        id: rect,
        visible: false,
    }]))
    .unwrap();
    let res2 = Resolved::rebuild(&doc);
    let _ = res;

    let r = raster_of(
        &doc,
        &res2,
        &[frame],
        &RasterOpts {
            trim: true,
            ..RasterOpts::default()
        },
    )
    .expect("bounds");
    assert_eq!((r.width, r.height), (400, 300));
}

/// Pad squares the result on its longer side, centred, and the padding takes the
/// background like everything else — one composite pass covers both.
///
/// 🚨 **Both orientations, and only one of them was here** (§15 D655,
/// `[S8.3-L6-11]`). The fixture is 100 × 50, so `side == r.width` and
/// `padded_square`'s `dx` is **0 in every run** — the three samples are one column
/// at three rows, which pins `dy` and says nothing whatever about `dx`. Replacing
/// `let dx = ((side - r.width) / 2) as usize` with `let dx = 0` left this test
/// green, and `trim_runs_before_pad` too, since that one asserts only the size. A
/// **tall** subject then pads left-aligned instead of centred, which is exactly
/// the property `padded_square`'s own doc claims: *"an icon set padded this way is
/// consistent with itself, which is the point"*. CLAUDE.md's first vacuity shape —
/// *it asserted one of two boundaries* — the two boundaries here being the two
/// orientations a pad can have.
///
/// ⚠️ **Flip run**, `dx = 0`, and the predicted site was right: the tall case's
/// *padding to the left*, red where grey was wanted — the first sample the
/// mutation moves ink into. The eight other tests in the file stay green.
#[test]
fn a_pad_squares_the_result_and_takes_the_background() {
    let (doc, res, _, rect) = frame_with_a_small_rect();
    let grey = Color::from_rgba8(128, 128, 128, 255);
    let r = raster_of(
        &doc,
        &res,
        &[rect],
        &RasterOpts {
            pad_square: true,
            background: Some(grey),
            ..RasterOpts::default()
        },
    )
    .expect("bounds");
    assert_eq!(
        (r.width, r.height),
        (100, 100),
        "100×50 padded to its width"
    );
    assert_eq!(px(&r, 50, 0), [128, 128, 128, 255], "padding above");
    assert_eq!(px(&r, 50, 99), [128, 128, 128, 255], "padding below");
    assert_eq!(
        px(&r, 50, 50),
        [220, 30, 40, 255],
        "the artwork in the middle"
    );

    // The other orientation: the same rect turned on its side, so the padding is
    // now horizontal and `dx` is what is being read.
    let mut doc = doc;
    doc.apply(&Transaction(vec![Operation::SetGeometry {
        id: rect,
        geometry: ondin_core::GeometryPatch::Size(Size::new(50.0, 100.0)),
    }]))
    .unwrap();
    let tall = raster_of(
        &doc,
        &Resolved::rebuild(&doc),
        &[rect],
        &RasterOpts {
            pad_square: true,
            background: Some(grey),
            ..RasterOpts::default()
        },
    )
    .expect("bounds");
    assert_eq!(
        (tall.width, tall.height),
        (100, 100),
        "50×100 padded to its height"
    );
    assert_eq!(
        px(&tall, 0, 50),
        [128, 128, 128, 255],
        "padding to the left"
    );
    assert_eq!(
        px(&tall, 99, 50),
        [128, 128, 128, 255],
        "padding to the right"
    );
    assert_eq!(
        px(&tall, 50, 50),
        [220, 30, 40, 255],
        "and the artwork centred between them — `dx` is 25, not 0"
    );
}

/// The two finishing steps compose in the order the doc comment claims: trim
/// first, then pad. The other order would square the untrimmed 400×300 and then
/// trim it straight back to 100×50 — so the sizes tell the two apart.
#[test]
fn trim_runs_before_pad() {
    let (doc, res, frame, _) = frame_with_a_small_rect();
    let r = raster_of(
        &doc,
        &res,
        &[frame],
        &RasterOpts {
            trim: true,
            pad_square: true,
            ..RasterOpts::default()
        },
    )
    .expect("bounds");
    assert_eq!((r.width, r.height), (100, 100));
}

/// A request past what the CPU rasterizer can allocate comes back smaller and
/// **says so**, rather than panicking inside the renderer or quietly succeeding
/// at a size nobody asked for.
///
/// Planned, not rendered: the point is that the arithmetic refuses before
/// anything tries to allocate the buffer.
#[test]
fn an_impossible_size_is_clamped_and_reported() {
    let (_doc, res, _, rect) = frame_with_a_small_rect();
    let mut spec = ExportSpec::new(ExportFormat::Png, ExportScale::Width(200_000));
    let (size, clamped) = ondin_export::plan::raster_size(&res, rect, &spec);
    // 65408 is `MAX_RASTER_SIDE`, and it is **not** `u16::MAX` — see the
    // constant, and `a_raster_at_the_cap_renders_on_either_axis` below, which is
    // what says the difference is load-bearing rather than cosmetic.
    assert_eq!(size, Some((65408, 32705)));
    assert!(clamped, "and the panel is told");

    spec.scale = ExportScale::Times(2.0);
    let (size, clamped) = ondin_export::plan::raster_size(&res, rect, &spec);
    assert_eq!(size, Some((200, 100)));
    assert!(!clamped, "an ordinary request is not flagged");
}

/// A rect `w × h` at the origin, on its own, as a subject to export.
fn thin_rect(w: f64, h: f64) -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(23);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let rect = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(w, h),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: Brush::Solid(RED),
                visible: true,
            }],
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, rect)
}

/// **`[S8.2-L1-01]`'s loss: the clamp handed the renderer the one size it panics
/// at.** `MAX_RASTER_SIDE` was `u16::MAX`, and `vello_common`'s
/// `snap_to_tile_coordinates` carries a comment saying in as many words that
/// `u16::MAX` is the viewport size it cannot take. So `ondin export --png
/// --width 65535` aborted, and so did a plain 1× export of a document whose one
/// rect is `1e30` wide — no flag involved, only geometry a `.ondin` can carry.
///
/// ⚠️ **This test is also the only thing that re-measures the constant.** Neither
/// `DEPTH_BUCKET_WIDTH` nor `Tile::WIDTH` is public, so `511 * 128` is derived by
/// reading vello's source and cannot be checked at compile time: a vello upgrade
/// that changed either would leave the number stale and nothing else would say
/// so. Rendering **at** the cap is what would go red.
///
/// **A thin subject on each axis**, because a square at the cap is 17 GB. The
/// subject fills its own bounds, so the ink runs to the very edge of the buffer,
/// which is the case the panicking site is about.
///
/// Flip-check, run: `MAX_RASTER_SIDE = u16::MAX as u32` fails here on the width
/// arm, inside `vello_common-0.2.0/src/util.rs:174` — the library's own
/// `unwrap`, in the failure message, which is the whole point of rendering
/// rather than asserting a number.
///
/// ⚠️ **`an_impossible_size_is_clamped_and_reported` above goes red under that
/// flip too, and it is the *weaker* of the two**: it compares the clamp's
/// arithmetic against a literal, so it says the number changed and nothing about
/// whether either number can be rendered. Before this test existed it was green
/// at `u16::MAX`, which is exactly how the constant came to be the panicking
/// value.
#[test]
fn a_raster_at_the_cap_renders_on_either_axis() {
    let cap = f64::from(ondin_export::png::MAX_RASTER_SIDE);
    for (w, h) in [(cap, 1.0), (1.0, cap)] {
        let (doc, res, rect) = thin_rect(w, h);
        let r = raster_of(&doc, &res, &[rect], &RasterOpts::default()).expect("bounds");
        assert_eq!(
            (f64::from(r.width), f64::from(r.height)),
            (w, h),
            "asked for {w}×{h}"
        );
        // The far corner, which is the pixel the panicking site is reached by.
        assert_eq!(px(&r, r.width - 1, r.height - 1), [220, 30, 40, 255]);
    }
}

/// A pad at an ordinary size still lands where it did — the control for
/// `[S8.2-L1-02]`'s widening, which must not have moved anything.
///
/// ⚠️ **The overflow itself is asserted in `png.rs`, not here.** The smallest
/// raster that reaches it is `32768 × 1`, whose square is 4 GB, so a test that
/// exercised the real pad would be asserting the allocator rather than the
/// arithmetic. `png::size_tests::a_padded_square_sizes_its_buffer_in_usize` is
/// the one with teeth; this is the one that says the ordinary path is unchanged.
#[test]
fn a_pad_at_an_ordinary_size_is_unchanged_by_the_widening() {
    let (doc, res, rect) = thin_rect(40.0, 10.0);
    let r = raster_of(
        &doc,
        &res,
        &[rect],
        &RasterOpts {
            pad_square: true,
            ..RasterOpts::default()
        },
    )
    .expect("bounds");
    assert_eq!((r.width, r.height), (40, 40));
    assert_eq!(r.rgba.len(), 40 * 40 * 4);
    // Centred, biased to the top on an odd remainder: 10 rows of ink starting at
    // row 15.
    assert_eq!(px(&r, 20, 14), [0, 0, 0, 0], "above the ink");
    assert_eq!(px(&r, 20, 15), [220, 30, 40, 255], "the first inked row");
}
