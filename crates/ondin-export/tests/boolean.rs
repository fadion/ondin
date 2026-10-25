//! Boolean containers through the export path (§7.1).
//!
//! A document holding a `NodeKind::Boolean` is evaluated by `Resolved`, and the
//! exporter writes the result rather than the operands (`ondin export --svg` is
//! this path).
//!
//! The pixel tests exist because geometry assertions cannot see a backend getting
//! the fill rule wrong: `flo_curves` works even-odd where this app fills non-zero
//! (§15 D91), and an aligned stroke's `clipPath` (§15 D93) is markup while the
//! raster path builds the same intent from different code.

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    BoolOp, Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    keyed_by_position,
};
use ondin_render::{ImageStore, VelloCpuRenderer, Viewport};

fn rect(w: f64, h: f64) -> NodeKind {
    NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
    }
}

/// A boolean of a 100x100 square and a 60x60 square placed at (70, 20) — so a
/// subtraction is a square with a notch out of its right edge.
fn notched(op: BoolOp) -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0xB0);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (b, a1, a2) = (ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: b,
            parent: root,
            index: 0,
            kind: NodeKind::Boolean { op },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a1,
            parent: b,
            index: 0,
            kind: rect(100.0, 100.0),
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a2,
            parent: b,
            index: 1,
            kind: rect(60.0, 60.0),
            transform: Some(Affine::translate((70.0, 20.0))),
            name: None,
        },
    ]))
    .expect("build the boolean");
    let res = Resolved::rebuild(&doc);
    (doc, res, b)
}

/// The SVG carries the derived outline and not the operands, which a viewer would
/// otherwise draw.
#[test]
fn the_svg_carries_the_result_and_not_the_operands() {
    let (doc, res, _) = notched(BoolOp::Subtract);
    let svg = ondin_export::svg::svg(&doc, &res, doc.first_page(), None);

    assert_eq!(svg.matches("<path").count(), 1, "one element:\n{svg}");
    assert!(
        !svg.contains("<rect"),
        "the two operand rects are inputs, not output:\n{svg}"
    );
    // The notch: the outline visits the inner corner at (70, 20) and rejoins the
    // edge at (100, 80). Neither point is on a plain 100x100 square.
    assert!(svg.contains("70,20"), "the notch's inner corner:\n{svg}");
    assert!(
        svg.contains("100,80"),
        "and where it rejoins the edge:\n{svg}"
    );
}

/// The reported box is the result's: still 100x100 here, where unioning the
/// operands would say 130.
#[test]
fn the_reported_box_is_the_results() {
    let (_, res, b) = notched(BoolOp::Subtract);
    let bounds = res.world_bounds(b).expect("a boolean has bounds");
    assert!(
        (bounds.width() - 100.0).abs() < 0.5 && (bounds.height() - 100.0).abs() < 0.5,
        "the subtracted operand hangs outside and must not widen the box: {bounds:?}"
    );
}

/// An intersection that comes to nothing has no outline, no bounds and no element.
#[test]
fn a_boolean_with_no_result_exports_nothing() {
    let mut ids = IdSource::new(0xB1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (b, a1, a2) = (ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: b,
            parent: root,
            index: 0,
            kind: NodeKind::Boolean {
                op: BoolOp::Intersect,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a1,
            parent: b,
            index: 0,
            kind: rect(10.0, 10.0),
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a2,
            parent: b,
            index: 1,
            kind: rect(10.0, 10.0),
            transform: Some(Affine::translate((500.0, 500.0))),
            name: None,
        },
    ]))
    .expect("build the boolean");
    let res = Resolved::rebuild(&doc);

    assert!(res.boolean_path(b).is_none(), "nothing to draw");
    assert!(res.world_bounds(b).is_none(), "and nothing to measure");
    let svg = ondin_export::svg::svg(&doc, &res, doc.first_page(), None);
    assert!(
        !svg.contains("<path"),
        "no element for an empty result:\n{svg}"
    );
}

/// An inside stroke on a boolean exports as a doubled-width stroke under a
/// `clipPath` of the boolean's own outline (§6.3), not centred. Asserted on the
/// clip: no `clipPath` means no alignment, whatever width came out.
#[test]
fn an_aligned_stroke_on_a_boolean_exports_with_its_clip() {
    let (mut doc, _, b) = notched(BoolOp::Subtract);
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: b,
        strokes: keyed_by_position([ondin_core::Stroke {
            brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                255, 0, 0, 255,
            ))
            .into(),
            width: 4.0,
            align: ondin_core::StrokeAlign::Inside,
            ..Default::default()
        }]),
    }]))
    .expect("a stroke on the boolean");
    let res = Resolved::rebuild(&doc);
    let svg = ondin_export::svg::svg(&doc, &res, doc.first_page(), None);

    assert!(
        svg.contains("<clipPath"),
        "an inside stroke is a clipped double-width one:\n{svg}"
    );
    assert!(
        svg.contains("stroke-width=\"8\""),
        "…at twice the width, with half of it clipped away:\n{svg}"
    );
    // The clip is the outline, notch included, not a bounding box or an operand.
    assert!(
        svg.matches("70,20").count() >= 2,
        "the clip path carries the notch too:\n{svg}"
    );

    // Centred needs no clip, so it is not emitted unconditionally.
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: b,
        strokes: keyed_by_position([ondin_core::Stroke {
            brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                255, 0, 0, 255,
            ))
            .into(),
            width: 4.0,
            align: ondin_core::StrokeAlign::Center,
            ..Default::default()
        }]),
    }]))
    .expect("centre it");
    let res = Resolved::rebuild(&doc);
    let svg = ondin_export::svg::svg(&doc, &res, doc.first_page(), None);
    assert!(!svg.contains("<clipPath"), "centred needs no clip:\n{svg}");
    assert!(
        svg.contains("stroke-width=\"4\""),
        "at its own width:\n{svg}"
    );
}

// --- pixels ---------------------------------------------------------------

/// A boolean of two rects, filled opaque red, in a 100×100 world with no artboard,
/// so a hole is literally `alpha == 0`. The operands keep their default fill on
/// purpose: painting them beside the result (§15 D88) would fail every "empty"
/// assertion.
fn filled(
    op: BoolOp,
    outer: (f64, f64, f64, f64),
    inner: (f64, f64, f64, f64),
) -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0xB2);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (b, a1, a2) = (ids.mint(), ids.mint(), ids.mint());
    let at = |(x, y, w, h): (f64, f64, f64, f64)| (rect(w, h), Affine::translate((x, y)));
    let (k1, t1) = at(outer);
    let (k2, t2) = at(inner);
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: b,
            parent: root,
            index: 0,
            kind: NodeKind::Boolean { op },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a1,
            parent: b,
            index: 0,
            kind: k1,
            transform: Some(t1),
            name: None,
        },
        Operation::CreateNode {
            id: a2,
            parent: b,
            index: 1,
            kind: k2,
            transform: Some(t2),
            name: None,
        },
    ]))
    .expect("build the boolean");
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: b,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(220, 30, 40, 255)).into(),
            visible: true,
        }]),
    }]))
    .expect("fill the result");
    let res = Resolved::rebuild(&doc);
    (doc, res, b)
}

/// One pixel of a 100×100 render of the whole world, as RGBA.
fn px(doc: &Document, res: &Resolved, x: u32, y: u32) -> [u8; 4] {
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    };
    let (rgba, w, _) =
        VelloCpuRenderer::new().render_to_rgba(doc, res, &vp, &ImageStore::new(), doc.first_page());
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// A subtracted boolean's hole is empty on screen, not merely wound the other way
/// (§15 D91). The ring is asserted inked first, or the empty middle would pass
/// over a boolean that came to nothing.
///
/// flip: in `boolean::from_flo`, `let want_positive = depth % 2 == 0;` →
/// `let want_positive = true;` → fails at the hole assertion (middle comes back
/// `[220, 30, 40, 255]`); the Exclude test stays green through it.
#[test]
fn a_subtracted_boolean_leaves_a_hole_that_is_actually_empty() {
    let (doc, res, _) = filled(
        BoolOp::Subtract,
        (10.0, 10.0, 80.0, 80.0),
        (30.0, 30.0, 40.0, 40.0),
    );

    // The fixture: the ring is inked on all four sides.
    for (x, y, side) in [
        (50, 20, "top"),
        (50, 80, "bottom"),
        (20, 50, "left"),
        (80, 50, "right"),
    ] {
        let p = px(&doc, &res, x, y);
        assert_eq!(p[3], 255, "the {side} of the ring is not painted: {p:?}");
        assert!(
            (i32::from(p[0]) - 220).abs() <= 2,
            "and it is the fill colour: {p:?}"
        );
    }

    // The claim.
    let hole = px(&doc, &res, 50, 50);
    assert_eq!(
        hole[3], 0,
        "the subtracted middle must be empty, not filled: {hole:?}"
    );
    // And nothing outside the outer rect.
    assert_eq!(px(&doc, &res, 4, 4)[3], 0, "outside the result");
}

/// `Exclude`'s overlap is a hole too (§15 D91). `path_full_intersect` returns the
/// two disjoint exteriors, so the hole is an absent subpath rather than a reversed
/// one, independent of the winding correction. The inked corners are the fixture.
///
/// flip: replace the `Exclude` arm with `path_add` → fails at the overlap
/// assertion (it comes back the fill colour).
#[test]
fn an_excluded_boolean_leaves_its_overlap_empty() {
    let (doc, res, _) = filled(
        BoolOp::Exclude,
        (10.0, 10.0, 60.0, 60.0),
        (40.0, 40.0, 60.0, 60.0),
    );

    // The fixture: the two squares each keep the part the other does not cover.
    assert_eq!(px(&doc, &res, 20, 20)[3], 255, "the first square's corner");
    assert_eq!(px(&doc, &res, 90, 90)[3], 255, "the second square's corner");

    let overlap = px(&doc, &res, 55, 55);
    assert_eq!(
        overlap[3], 0,
        "the overlap must be excluded, not unioned: {overlap:?}"
    );
}

/// A ring with a stroke of `align` on it, in a colour the fill is not.
fn stroked(align: ondin_core::StrokeAlign, width: f64) -> (Document, Resolved) {
    let (mut doc, _, b) = filled(
        BoolOp::Subtract,
        (10.0, 10.0, 80.0, 80.0),
        (30.0, 30.0, 40.0, 40.0),
    );
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: b,
        strokes: keyed_by_position([ondin_core::Stroke {
            brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(30, 90, 220, 255)).into(),
            width,
            align,
            ..Default::default()
        }]),
    }]))
    .expect("stroke the boolean");
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// What is at a pixel: nothing, the stroke's blue, or the fill's red.
#[derive(PartialEq, Eq, Debug)]
enum Ink {
    None,
    Stroke,
    Fill,
}

fn ink(doc: &Document, res: &Resolved, x: u32) -> Ink {
    match px(doc, res, x, 50) {
        [_, _, _, 0] => Ink::None,
        [r, _, b, _] if b > r => Ink::Stroke,
        _ => Ink::Fill,
    }
}

/// An inside stroke on a boolean lands inside the filled region on both of the
/// ring's edges, in pixels (§15 D93, D97): the outer band goes inward, the hole's
/// band goes outward from the hole. Probes sit at least 1.5px clear of a boundary
/// at width 6 (inside bands 10..16 and 24..30, centre 7..13 and 27..33).
///
/// flip: drop `NodeKind::Boolean` from `geometry::stroke_align_applies` → fails at
/// x=8 (`Stroke`). flip: clip to `outer_bounds(shape, 0.0)` in `scene::…push_layer`
/// → fails at x=31 (`Stroke` across the hole); only the hole edge catches it.
#[test]
fn an_inside_stroke_on_a_boolean_lands_inside_both_of_its_edges() {
    let (doc, res) = stroked(ondin_core::StrokeAlign::Inside, 6.0);

    // Fixture: a fill band must remain between the stroke bands, or a ring painted
    // stroke-colour edge to edge (doubled width, no clip) would pass.
    assert_eq!(
        ink(&doc, &res, 20),
        Ink::Fill,
        "no fill left between the bands"
    );

    assert_eq!(
        ink(&doc, &res, 8),
        Ink::None,
        "the stroke spills outside the outline"
    );
    assert_eq!(
        ink(&doc, &res, 12),
        Ink::Stroke,
        "the outer edge is not stroked inward"
    );
    assert_eq!(
        ink(&doc, &res, 26),
        Ink::Stroke,
        "the hole's edge is not stroked at all"
    );
    assert_eq!(
        ink(&doc, &res, 31),
        Ink::None,
        "the stroke painted across the hole"
    );

    // A centred stroke paints where the inside one left empty, so the stroke is not
    // simply missing.
    let (doc, res) = stroked(ondin_core::StrokeAlign::Center, 6.0);
    assert_eq!(
        ink(&doc, &res, 8),
        Ink::Stroke,
        "centred, outside the outline"
    );
    assert_eq!(ink(&doc, &res, 31), Ink::Stroke, "centred, into the hole");
}

/// An outside stroke on a boolean is the complement of the shape, taken even-odd
/// (the user's path direction is unknown, and the hole is outside), and its mitre
/// survives the clip (§15 D97). On a ring it paints into the hole.
///
/// The corner probe does not pin `miter_reach`: `scene::outer_bounds` pads by
/// `(width + height).max(1.0) + reach`, 160 here before `reach` adds 24, so
/// zeroing `miter_reach` changes nothing observable. Anyone tightening that pad
/// should not assume a test watches the term. The probe holds that the complement
/// covers the whole plane outside, corners included.
///
/// flip: clip to `outer_bounds` without appending the shape → fails at x=12
/// (`Stroke`).
#[test]
fn an_outside_stroke_on_a_boolean_is_the_complement_mitre_and_all() {
    let (doc, res) = stroked(ondin_core::StrokeAlign::Outside, 6.0);

    assert_eq!(ink(&doc, &res, 6), Ink::Stroke, "the band is not outside");
    assert_eq!(
        ink(&doc, &res, 12),
        Ink::Fill,
        "the stroke reaches inside the outline"
    );
    assert_eq!(
        ink(&doc, &res, 26),
        Ink::Fill,
        "and inside the ring by the hole"
    );
    // The hole is outside, hence even-odd.
    assert_eq!(ink(&doc, &res, 32), Ink::Stroke, "the hole is not stroked");

    // The corner's mitre, on the bisector: ink at (5, 5) and clear air at (2, 2).
    let diag = |d: u32| match px(&doc, &res, d, d) {
        [_, _, _, 0] => Ink::None,
        [r, _, b, _] if b > r => Ink::Stroke,
        _ => Ink::Fill,
    };
    assert_eq!(
        diag(5),
        Ink::Stroke,
        "the mitre was clipped off at the corner"
    );
    assert_eq!(diag(2), Ink::None, "and it reaches further than it should");
}
