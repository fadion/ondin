//! Boolean containers through the export path (§7.1).
//!
//! The end-to-end check for the whole feature: a document holding a
//! `NodeKind::Boolean` is evaluated by `Resolved`, and the exporter writes the
//! *result* rather than the operands. Headless, so it is the check that does not
//! need a window — `ondin export --svg` is this same code path.
//!
//! **Four of these are about pixels rather than about markup**, and they are here
//! for a reason `roadmap.md` named: every other assertion in the boolean work is on
//! areas, subpath counts and windings — geometry, which is the right level for the
//! arithmetic and cannot see a **backend** getting the fill rule wrong. `flo_curves`
//! works even-odd where this app fills non-zero (§15 D91), so "a hole is a subpath
//! wound the other way" is a claim two layers below the one that matters, which is
//! whether the hole is *empty on screen*. The same goes for an aligned stroke: the
//! `clipPath` an inside one exports (§15 D93) is markup, and the raster path builds
//! the same intent out of different code. `ondin export --png` is the path all four
//! drive.

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    BoolOp, Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
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

/// **The SVG carries the result, and not the operands.** SVG has no boolean
/// container, so the derived outline is the only faithful thing to write — and the
/// operands must not appear beside it, or a viewer would draw the very shapes the
/// boolean was meant to consume.
#[test]
fn the_svg_carries_the_result_and_not_the_operands() {
    let (doc, res, _) = notched(BoolOp::Subtract);
    let svg = ondin_export::svg::svg(&doc, &res, None);

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

/// The box `Resolved` reports is the **result's**. For this subtraction that is
/// still the full 100x100 — the notch is interior, and the operand hanging outside
/// contributes nothing. An implementation that unioned the operands would say 130.
#[test]
fn the_reported_box_is_the_results() {
    let (_, res, b) = notched(BoolOp::Subtract);
    let bounds = res.world_bounds(b).expect("a boolean has bounds");
    assert!(
        (bounds.width() - 100.0).abs() < 0.5 && (bounds.height() - 100.0).abs() < 0.5,
        "the subtracted operand hangs outside and must not widen the box: {bounds:?}"
    );
}

/// An intersection that comes to nothing has no outline and no bounds — and must
/// not panic, must not export a degenerate element, and must not be measurable.
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
    let svg = ondin_export::svg::svg(&doc, &res, None);
    assert!(
        !svg.contains("<path"),
        "no element for an empty result:\n{svg}"
    );
}

/// **An inside stroke on a boolean is built, not quietly centred.**
///
/// SVG has no stroke alignment, so an aligned stroke is a doubled-width one under a
/// `clipPath` (§6.3) — and the clip has to be *this* shape. Two things had to be
/// wrong together for the export to look right, and both were: the predicate that
/// decides whether alignment applies measured a boolean's outline through
/// `geometry::local_path`, which cannot build one, and the clip was built from the
/// same call. So a boolean answered "open shape, no inside" and its inside stroke
/// exported centred, straddling the edge, while `Resolved` had already sized the node
/// as though the ink were inside.
///
/// Asserted on the clip, because that *is* the construction: no `clipPath` means no
/// alignment, whatever width came out.
#[test]
fn an_aligned_stroke_on_a_boolean_exports_with_its_clip() {
    let (mut doc, _, b) = notched(BoolOp::Subtract);
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: b,
        strokes: vec![ondin_core::Stroke {
            brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                255, 0, 0, 255,
            )),
            width: 4.0,
            align: ondin_core::StrokeAlign::Inside,
            ..Default::default()
        }],
    }]))
    .expect("a stroke on the boolean");
    let res = Resolved::rebuild(&doc);
    let svg = ondin_export::svg::svg(&doc, &res, None);

    assert!(
        svg.contains("<clipPath"),
        "an inside stroke is a clipped double-width one:\n{svg}"
    );
    assert!(
        svg.contains("stroke-width=\"8\""),
        "…at twice the width, with half of it clipped away:\n{svg}"
    );
    // The clip is the boolean's own outline, notch included — not its bounding box
    // and not a rect either operand would have produced.
    assert!(
        svg.matches("70,20").count() >= 2,
        "the clip path carries the notch too:\n{svg}"
    );

    // And the same stroke centred needs none of it, so the construction is not being
    // emitted unconditionally.
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: b,
        strokes: vec![ondin_core::Stroke {
            brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                255, 0, 0, 255,
            )),
            width: 4.0,
            align: ondin_core::StrokeAlign::Center,
            ..Default::default()
        }],
    }]))
    .expect("centre it");
    let res = Resolved::rebuild(&doc);
    let svg = ondin_export::svg::svg(&doc, &res, None);
    assert!(!svg.contains("<clipPath"), "centred needs no clip:\n{svg}");
    assert!(
        svg.contains("stroke-width=\"4\""),
        "at its own width:\n{svg}"
    );
}

// --- pixels ---------------------------------------------------------------

/// A boolean of two rects, filled opaque red, in a 100×100 world with nothing
/// behind it — so every non-transparent pixel is the boolean's own result and a
/// hole is literally `alpha == 0`.
///
/// **No artboard.** One would only add a background to see through, and the
/// question here is whether the *result* has a hole in it, not what is under the
/// hole. The operands keep whatever fill they were created with, deliberately: if
/// they were ever painted beside the result — which they were, until §15 D88 —
/// every "empty" assertion below would fail, so these tests carry that regression
/// too for free.
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
        fills: vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(220, 30, 40, 255)),
            visible: true,
        }],
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
    let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(doc, res, &vp, &ImageStore::new());
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// **A subtracted boolean's hole is empty on screen, not merely wound the other
/// way.**
///
/// The gap `roadmap.md` held open for the boolean work: everything else asserts
/// geometry, and geometry cannot see the **backend** filling an inner subpath solid.
/// `flo_curves` works even-odd where this app fills non-zero (§15 D91), so the
/// difference between "there is a reversed subpath in the outline" and "there is a
/// hole in the picture" is exactly one layer of code — this is the layer.
///
/// The fixture is asserted before the claim: a ring has to have ink *in* it, or
/// "the middle is transparent" passes over a boolean that came to nothing at all,
/// which is the way this test would most plausibly be vacuous.
///
/// ⚠️ **Flipped against losing the winding correction** — in `boolean::from_flo`,
/// `let want_positive = depth % 2 == 0;` → `let want_positive = true;`, one word,
/// and the state the code was in before D91 — where
/// the middle comes back `[220, 30, 40, 255]`, the fill colour, which is the
/// reported symptom in as many words. Note what the flip also showed: the
/// *Exclude* test below stayed green through it, so these two are not one test
/// twice. A hole from a nested subpath and a hole from two disjoint ones fail in
/// different places.
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

/// **`Exclude`'s overlap is a hole too, which is D91 seen from the other end.**
///
/// That entry is "Exclude behaved as Union, and holes filled solid" — two faults
/// under one report, and this is the first of them. `path_full_intersect` returns
/// the two *exteriors*, which are disjoint, so this hole is an absence of any
/// subpath rather than a reversed one: the winding correction the test above flips
/// against has nothing to do here, and losing it leaves this one green. Two faults,
/// two tests.
///
/// ⚠️ **Flipped by replacing the arm with `path_add`**, which is exactly what D91
/// reports it doing — the overlap comes back the fill colour. The corners being
/// inked is the fixture: without it, "the overlap is empty" passes over an exclude
/// that produced nothing at all.
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
        strokes: vec![ondin_core::Stroke {
            brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(30, 90, 220, 255)),
            width,
            align,
            ..Default::default()
        }],
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

/// **An inside stroke on a boolean lands inside it — measured in pixels, on both
/// of the ring's edges.**
///
/// The last gap `roadmap.md` held open for the boolean work: §15 D93 and D97 do
/// the arithmetic for an aligned stroke on a derived outline, and until now
/// nothing looked at the result. `an_aligned_stroke_on_a_boolean_exports_with_its_clip`
/// above asserts the *markup* — that SVG gets a double-width stroke under a
/// `clipPath`, since SVG has no alignment of its own — and markup cannot see the
/// raster backend doing something else entirely. This is that layer, and it is the
/// one `ondin export --png` and the on-screen canvas share.
///
/// **The hole is what makes it worth measuring.** A ring has two contours, and
/// "inside" means inside the *filled region* — so the band on the outer edge goes
/// inward and the band on the hole's edge goes **outward from the hole**, into the
/// ring. Both are asserted, and the second is the one that would catch the clip
/// being taken from the bounding box rather than from the outline: a box-clipped
/// stroke has nothing to stop it painting across the hole.
///
/// Sampled at 6 units wide so every probe sits at least 1.5px clear of a boundary,
/// which keeps the antialiased edge out of the assertions. The bands, scanned:
/// inside puts them at 10..16 and 24..30, centre at 7..13 and 27..33.
///
/// ⚠️ **Flipped two ways, and they fail in different places** — which is the point
/// of having both edges in here.
///
/// Dropping `NodeKind::Boolean` from `geometry::stroke_align_applies` is the bug
/// D93 and D97 report in as many words, a boolean answering "open shape, no
/// inside": the whole stroke re-centres and x=8 comes back `Stroke`.
///
/// Clipping to the bounding box instead of the outline — `outer_bounds(shape,
/// 0.0)` in `scene::…push_layer` — is the one the *hole* is here for, and it was
/// checked rather than assumed: under it x=31 comes back `Stroke`, the band
/// painting straight across the hole, while x=26 and x=20 stay exactly right. A
/// test that sampled only the outer edge would have called that version correct.
#[test]
fn an_inside_stroke_on_a_boolean_lands_inside_both_of_its_edges() {
    let (doc, res) = stroked(ondin_core::StrokeAlign::Inside, 6.0);

    // The fixture first: without a fill band left between the two stroke bands,
    // "the stroke is inside" would be passing over a ring painted stroke-colour
    // edge to edge, which is what a doubled width with no clip would give.
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

    // And a centred stroke paints in exactly the two places the inside one left
    // empty — so neither assertion above is passing because the stroke is missing.
    let (doc, res) = stroked(ondin_core::StrokeAlign::Center, 6.0);
    assert_eq!(
        ink(&doc, &res, 8),
        Ink::Stroke,
        "centred, outside the outline"
    );
    assert_eq!(ink(&doc, &res, 31), Ink::Stroke, "centred, into the hole");
}

/// **An outside stroke on a boolean is the complement, and its mitre survives the
/// clip.**
///
/// The other half of the pair, and the one that reaches D97. `Outside` is not the
/// mirror of `Inside` in the code: `scene.rs` cannot clip to the shape, so it
/// builds "everything but the shape" — a rect big enough to hold anything, with
/// the outline appended, taken **even-odd** — and clips to that. Even-odd rather
/// than a reversed winding because the user's own path direction is unknown, and
/// it gets a shape with holes right, since **the hole is outside**. That last
/// clause is a claim about pixels and is asserted below: on a ring, an outside
/// stroke paints *into* the hole, where the inside one painted away from it.
///
/// **The corner probe is weaker than it looks, and that is a finding about D97
/// rather than about this test.** A mitre on a 90° corner runs out past the
/// shape's own bounds — for a 6-wide outside stroke, 6√2 ≈ 8.5 from the corner
/// along the bisector, landing its tip at (4, 4) on a box starting at (10, 10) —
/// and D97 sizes the enclosing rect by `miter_reach` so the clip cannot cut it
/// off. The probe below confirms the mitre is there in pixels, which nothing else
/// did. What it does **not** do is pin `miter_reach`: `scene::outer_bounds` pads by
/// `(width + height).max(1.0) + reach`, which on this shape is 160 before `reach`
/// contributes its 24 — `miter_reach(6, 4)`, at the default mitre limit — so
/// folding `miter_reach` to zero was tried and changes nothing observable.
/// Every flat-edge probe stays right too. So D97's term is currently
/// unobservable at any realistic size and only starts to matter if that pad is
/// ever tightened — worth knowing before anyone tightens it on the assumption a
/// test is watching. What the corner probe genuinely holds is that the complement
/// is taken over the *whole* plane outside the shape, corners included.
///
/// ⚠️ **Flipped by clipping to `outer_bounds` without appending the shape**, which
/// is the "everything but the shape" construction losing its second half: the
/// even-odd rule then keeps the interior too and x=12 comes back `Stroke`.
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
    // The hole *is* outside, which is the whole reason the complement is taken
    // even-odd rather than by reversing a winding.
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
