//! **The writer and the reader are inverse functions in two crates, and this is the
//! only thing that binds them.**
//!
//! `ondin_export::svg` writes the markup and `ondin_core::svg_in` reads it; nothing
//! the compiler does will notice when one grows an attribute the other does not
//! know, and neither crate's own tests can — the writer's assert on substrings and
//! the reader's on hand-written fixtures, so both stay green while they drift apart.
//! What fails here is exactly that drift.
//!
//! **Asserted as geometry rather than as bytes**, and relative to the set's own
//! bounding box rather than absolutely: the writer frames its output on a `viewBox`
//! computed from world bounds, so the whole drawing may legitimately arrive at a
//! different origin. What must survive is every shape's size and its position
//! *relative to the others* — which is what a user would call "the same picture".
//!
//! ⚠️ **One asymmetry is deliberate and is asserted here rather than excused
//! elsewhere**: a `Polygon` or `Star` is written as `<polygon>` and comes back as a
//! `Path`. `NodeKind::Polygon` is a regular *n*-gon with a radius, so reading every
//! point list back as one would turn an arbitrary outline into a hexagon. The
//! geometry round-trips; the kind does not.
//!
//! 🚨 **One test in this file is not a binding, and the count is what made that
//! invisible** (§15 D650, `[A5-L6-05]`).
//! `an_imported_clip_path_is_empty_outside_the_clip` takes **hand-authored**
//! markup and never calls `svg_of`; it is a *renderer* test, and honest as one —
//! its own doc says so — but it was being read as this suite's clip coverage,
//! which it cannot be. **Both sides of a hand-written fixture agree about what SVG
//! looks like**, so the only thing that can disagree with you is the writer's own
//! output. `the_writers_own_clip_spelling_round_trips` is the binding it was
//! standing in for. The lesson generalises past clips: *a test in this file that
//! does not call `svg_of` is not one of the things that binds*, however much its
//! name reads like it.
//!
//! **Flipped twice, and each flip bit exactly one test.** ⚠️ *Recorded when this
//! file held four tests, and left in the tense it was written in rather than
//! re-run over sixteen.* Making the reader ignore every element `transform` fails
//! the arrangement test at shape 0 — the rect arrives at (-10, -10) instead of
//! (10, 10) — and left the other three green. Making it ignore `fill-rule` failed
//! the exclusion test and nothing else. *Neither flip was caught by the group
//! test*, which asserts opacity and no placement at all, and that is the shape to
//! remember: four round-trip tests overlapped much less than four round-trip
//! tests sound like they would.

mod common;

use ondin_core::kurbo::{Affine, Point, Rect, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    BoolOp, Document, FillRule, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
};
use ondin_export::svg::svg_of;

/// Import `markup` into a fresh document and return it with every node under the
/// import's root, in document order.
fn reimport(markup: &str) -> (Document, Vec<NodeId>) {
    let mut ids = IdSource::new(0xBEEF);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let out =
        ondin_core::svg_in::import(markup, &mut ids, root, 0, None).expect("our own writer's SVG");
    doc.apply(&out.tx).expect("the import applies");

    fn walk(doc: &Document, id: NodeId, into: &mut Vec<NodeId>) {
        for c in doc
            .get(id)
            .map(|n| n.children().to_vec())
            .unwrap_or_default()
        {
            into.push(c);
            walk(doc, c, into);
        }
    }
    let mut all = Vec::new();
    walk(&doc, out.root, &mut all);
    (doc, all)
}

/// Every id's world box, and the union of them — the two things a comparison
/// relative to the set needs.
fn boxes(doc: &Document, ids: &[NodeId]) -> (Vec<Rect>, Rect) {
    let res = Resolved::rebuild(doc);
    let all: Vec<Rect> = ids
        .iter()
        .map(|id| res.world_bounds(*id).expect("a live node has bounds"))
        .collect();
    let union = all
        .iter()
        .copied()
        .reduce(|a, b| a.union(b))
        .expect("a non-empty set");
    (all, union)
}

fn relative(r: Rect, origin: Point) -> Rect {
    Rect::new(
        r.x0 - origin.x,
        r.y0 - origin.y,
        r.x1 - origin.x,
        r.y1 - origin.y,
    )
}

fn close(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() < 0.02
        && (a.y0 - b.y0).abs() < 0.02
        && (a.x1 - b.x1).abs() < 0.02
        && (a.y1 - b.y1).abs() < 0.02
}

/// Four shapes at four different places, one per emitter in the writer: a rounded
/// rect, a circle, a diagonal line and a closed triangle.
///
/// **Built here rather than taken from `common::fixture`**, and the reason is what
/// the first draft of this file got wrong: exporting the shared fixture's *artboard*
/// brings its background `<rect>` and its `<text>` along, so the imported set is
/// neither the same length nor in the same order as the original, and the comparison
/// quietly softens into "some shapes arrived somewhere". A set exported by id has no
/// extras in it.
fn four_shapes() -> (Document, Resolved, Vec<NodeId>) {
    let mut ids = IdSource::new(0x4444);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (rect, circle, line, tri) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());

    let mut p = ondin_core::kurbo::BezPath::new();
    p.move_to(Point::new(0.0, 0.0));
    p.line_to(Point::new(40.0, 0.0));
    p.line_to(Point::new(20.0, 30.0));
    p.close_path();

    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 40.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::from_single_radius(6.0),
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        },
        Operation::CreateNode {
            id: circle,
            parent: root,
            index: 1,
            kind: NodeKind::Ellipse {
                size: Size::new(50.0, 50.0),
            },
            transform: Some(Affine::translate((200.0, 15.0))),
            name: None,
        },
        Operation::CreateNode {
            id: line,
            parent: root,
            index: 2,
            kind: NodeKind::Line {
                end: Point::new(60.0, 45.0),
            },
            transform: Some(Affine::translate((20.0, 120.0))),
            name: None,
        },
        Operation::CreateNode {
            id: tri,
            parent: root,
            index: 3,
            kind: NodeKind::Path {
                path: p,
                corner_radii: Vec::new(),
            },
            transform: Some(Affine::translate((180.0, 140.0))),
            name: None,
        },
        // A line with no stroke has no ink and no bounds worth comparing.
        Operation::SetStrokes {
            id: line,
            strokes: vec![ondin_core::Stroke {
                brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(0, 0, 0, 255)),
                width: 2.0,
                join: ondin_core::kurbo::Join::Miter,
                cap: ondin_core::kurbo::Cap::Butt,
                dashes: Vec::new(),
                dash_offset: 0.0,
                miter_limit: 4.0,
                dash_fit: false,
                align: ondin_core::StrokeAlign::Center,
                sides: ondin_core::StrokeSides::All,
                visible: true,
            }],
        },
    ]))
    .expect("the fixture applies");
    let res = Resolved::rebuild(&doc);
    (doc, res, vec![rect, circle, line, tri])
}

/// **Four shapes out, four shapes back, each the same size in the same place
/// relative to the others.**
///
/// Four emitters in the writer and four arms in the reader, so a break in any one
/// of them fails here alone — and the *arrangement* claim is what a single-shape
/// test cannot make: the four are at four corners of the drawing, so an importer
/// that dropped a translation would put them on top of each other while every size
/// still matched.
///
/// ⚠️ **The tolerance is 0.02 and it is doing real work.** The writer formats
/// coordinates as decimal text, so the trip is lossy by construction — an exact
/// comparison would be a test of the number formatter rather than of the mapping.
#[test]
fn the_four_shape_emitters_round_trip_with_their_arrangement() {
    let (doc, res, ids) = four_shapes();
    let out = svg_of(&doc, &res, &ids);
    let (back, got) = reimport(&out);

    assert_eq!(got.len(), 4, "four shapes and no stray groups: {out}");
    let (before, before_union) = boxes(&doc, &ids);
    let (after, after_union) = boxes(&back, &got);
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        assert!(
            close(
                relative(*a, after_union.origin()),
                relative(*b, before_union.origin())
            ),
            "shape {i} moved or resized: {a:?} against {b:?}"
        );
    }
    // And the kinds that *can* survive do. A rect that came back as a path would
    // pass every assertion above and be a worse layer to edit.
    assert!(matches!(
        back.get(got[0]).unwrap().kind(),
        NodeKind::Rect { .. }
    ));
    assert!(matches!(
        back.get(got[1]).unwrap().kind(),
        NodeKind::Ellipse { .. }
    ));
    assert!(matches!(
        back.get(got[2]).unwrap().kind(),
        NodeKind::Line { .. }
    ));
    assert!(matches!(
        back.get(got[3]).unwrap().kind(),
        NodeKind::Path { .. }
    ));
}

/// **A group's opacity and transform survive as a group's**, which is the reason the
/// writer nests at all: `opacity` on a `<g>` composites the group as a unit, and a
/// reader that flattened it would fade each child separately and draw the overlap
/// twice as dark.
#[test]
fn a_group_keeps_its_opacity_rather_than_fading_each_child() {
    let mut ids = IdSource::new(0x9999);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (group, a, b) = (ids.mint(), ids.mint(), ids.mint());
    let rect = |id: NodeId, parent: NodeId, index: usize, at: f64| Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Rect {
            size: Size::new(40.0, 40.0),
            corner_radii: Default::default(),
        },
        transform: Some(Affine::translate((at, 0.0))),
        name: None,
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: group,
            parent: root,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((30.0, 40.0))),
            name: None,
        },
        rect(a, group, 0, 0.0),
        rect(b, group, 1, 20.0),
        Operation::SetOpacity {
            id: group,
            opacity: 0.5,
        },
    ]))
    .expect("the fixture applies");
    let res = Resolved::rebuild(&doc);

    let (back, got) = reimport(&svg_of(&doc, &res, &[group]));
    let groups: Vec<NodeId> = got
        .iter()
        .copied()
        .filter(|id| matches!(back.get(*id).unwrap().kind(), NodeKind::Group))
        .collect();
    assert_eq!(groups.len(), 1, "one group, still a group");
    assert!(
        (back.get(groups[0]).unwrap().opacity() - 0.5).abs() < 0.01,
        "and it is the group that is half-transparent, not its children: {}",
        back.get(groups[0]).unwrap().opacity()
    );
    for child in back.get(groups[0]).unwrap().children() {
        assert!(
            (back.get(*child).unwrap().opacity() - 1.0).abs() < 0.01,
            "a child must not have inherited the fade as its own"
        );
    }
}

/// **An exclusion round-trips as an even-odd path**, which is the newest thing in
/// this pair and the one most likely to be lost on either side (§15 D239).
///
/// The writer gained `fill-rule="evenodd"` hours before the reader existed, and a
/// reader that ignored the attribute would produce a *union* — a filled square where
/// the file says there is a hole. Asserted as the hole, not as the attribute.
#[test]
fn an_exclusion_comes_back_with_its_hole() {
    let mut ids = IdSource::new(0x0DD);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (a, b, bool_id) = (ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: bool_id,
            parent: root,
            index: 0,
            kind: NodeKind::Boolean {
                op: BoolOp::Exclude,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a,
            parent: bool_id,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: b,
            parent: bool_id,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((50.0, 0.0))),
            name: None,
        },
        Operation::SetFills {
            id: bool_id,
            fills: vec![ondin_core::Fill {
                brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(255, 0, 0, 255)),
                visible: true,
            }],
        },
    ]))
    .expect("the fixture applies");
    let res = Resolved::rebuild(&doc);

    let out = svg_of(&doc, &res, &[bool_id]);
    assert!(
        out.contains("evenodd"),
        "the writer says the rule out loud: {out}"
    );

    let (back, ids) = reimport(&out);
    let id = *ids.first().expect("one path");
    assert_eq!(
        back.get(id).unwrap().fill_rule(),
        FillRule::EvenOdd,
        "and the reader carried it"
    );
    let NodeKind::Path { path, .. } = back.get(id).unwrap().kind() else {
        panic!("an exclusion is a path once it is written out")
    };
    // The overlap is the hole. Sampled in the path's own space, which the writer
    // wrote in local coordinates and the reader read back the same way.
    let bounds = ondin_core::kurbo::Shape::bounding_box(path);
    let mid = Point::new(bounds.center().x, bounds.center().y);
    assert!(
        !FillRule::EvenOdd.contains(path, mid),
        "the shared band is still a hole after the trip"
    );
    assert!(
        FillRule::EvenOdd.contains(path, Point::new(bounds.x0 + 5.0, bounds.center().y)),
        "and the left rect is still solid"
    );
}

/// **An imported `clip-path` is empty outside the clip, on screen.**
///
/// The core tests assert the *structure* — a wrapper group with a mask node under
/// its content — and structure cannot see the renderer ignoring the flag, putting
/// the run in the wrong order, or clipping to the wrong box. This is the layer where
/// "there is a mask node in the tree" becomes "the ink stops", and it is the same
/// gap `boolean.rs`'s four pixel tests exist to close.
///
/// **The fixture is asserted before the claim**: the inside has to be red, or "the
/// outside is transparent" passes over an import that produced no ink at all — the
/// way this test would most plausibly be vacuous.
///
/// ⚠️ **Flipped by dropping the `SetMask` operation**, where (60, 60) comes back
/// `[255, 0, 0, 255]` — the fill colour spread over the whole square, which is the
/// unclipped picture in as many words.
#[test]
fn an_imported_clip_path_is_empty_outside_the_clip() {
    let (doc, _) = reimport(
        r##"<svg xmlns="http://www.w3.org/2000/svg">
              <rect width="100" height="100" fill="red" clip-path="url(#c)"/>
              <defs><clipPath id="c"><rect width="40" height="40"/></clipPath></defs>
            </svg>"##,
    );
    let res = Resolved::rebuild(&doc);
    let vp = ondin_render::Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    };
    let (rgba, w, _) = ondin_render::VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &vp,
        &ondin_render::ImageStore::new(),
    );
    let px = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };
    assert_eq!(
        px(10, 10),
        [255, 0, 0, 255],
        "inside the clip is the rect's own red — the fixture has ink in it"
    );
    assert_eq!(
        px(60, 60),
        [0, 0, 0, 0],
        "and outside it there is nothing, which is what a clip is"
    );
}

/// **Text comes back as text, and its baseline lands where the writer put it.**
///
/// ⚠️ **This test used to assert the opposite** — it was written as
/// `text_is_the_one_thing_the_round_trip_still_loses`, recording the gap where the
/// gap was so that closing it would be a *failing test* rather than an act of
/// memory. It failed the day text import landed, which is the whole return on
/// writing a test for something that does not work yet.
///
/// **What still does not round-trip is the paragraph, not the text.** The writer
/// emits one `<text>` per *line* at a baseline the canvas computed (§15 D81), and
/// nothing in the markup says those lines were one node — so a two-line text layer
/// comes back as two layers in the right places. *The picture closes; the structure
/// does not*, and that is asserted here rather than left to be discovered.
#[test]
fn text_round_trips_as_text_but_a_paragraph_comes_back_as_lines() {
    let f = common::fixture();
    let out = svg_of(&f.doc, &f.res, &[f.text]);
    assert!(out.contains("<text"), "the writer emits it: {out}");

    let (doc, ids) = reimport(&out);
    let texts: Vec<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Text { .. }))
        .collect();
    assert_eq!(
        texts.len(),
        out.matches("<text").count(),
        "one layer per line the writer emitted — the structural loss, stated"
    );

    let before = f.res.world_bounds(f.text).expect("the text has bounds");
    let (_, union) = boxes(&doc, &texts);
    // ⚠️ **Size, not position, and the reason is in this file's own header**: the
    // writer frames its output on a `viewBox` computed from world bounds, so the
    // drawing legitimately arrives at a different origin — and with a *single* layer
    // the "relative to the union" comparison the other tests use is trivially (0, 0)
    // and asserts nothing. What is left that has teeth is the box's own size, which
    // is where a dropped `font-size` or a baseline read as a box top would show.
    assert!(
        (union.width() - before.width()).abs() < 2.0
            && (union.height() - before.height()).abs() < 2.0,
        "the text came back the same size: {union:?} against {before:?}"
    );
}

/// **Two different weights reach a fixed point in one cycle, on SVG's initial 400 —
/// not on the first run's weight** (§15 D398).
///
/// ⚠️ **This was written down as a *reading* first and the reading was wrong**, which
/// is why it is a test. The prediction was "cycle 1 keeps the first run's weight with
/// a span for the other; cycle 2 moves the node to the element default" — two cycles,
/// with the node passing through the authored weight. What the code does is neither:
/// `svg::run_attrs` puts every font property on the `<tspan>`s and states none of them
/// on the `<text>`, so on re-import `run_style.weight` is `Style::default()`'s **400**
/// for our own output; *both* runs differ from it, both mint spans, neither covers the
/// whole content, so neither is hoisted (§15 D398) and the node lands on 400 in **one**
/// cycle.
///
/// It renders identically at every step, and the *styling* is a fixed point from
/// cycle 2. ⚠️ **The markup is not** — I asserted byte-identity here first and it
/// failed on two things that have nothing to do with weights: the `viewBox` and
/// `transform` move, the writer framing on world bounds; and `fill` stops being
/// written on every `<tspan>`, because the run colours were hoisted onto the node.
/// The assertion is on the node's weight and its span boundaries instead, which is
/// the claim this test is actually about.
///
/// *Revisit if the writer ever states a font property on the `<text>` element*, which
/// would make cycle 1 an identity and this test's name wrong.
#[test]
fn two_weights_settle_on_the_element_default_rather_than_on_either_run() {
    use ondin_core::{CharAttr, CharSpans};

    let mut f = common::fixture();
    let NodeKind::Text { style, .. } = f.doc.get(f.text).unwrap().kind() else {
        panic!("a Text node")
    };
    // The fixture's own weight is 500; the tail gets 700, so the two runs disagree
    // and neither matches SVG's initial 400.
    assert_eq!(style.weight, 500, "the fixture is the two-weight case");
    let mut spans = CharSpans::default();
    spans.set(3..10, CharAttr::Weight(700), style);
    f.doc
        .apply(&Transaction(vec![Operation::SetTextSpans {
            id: f.text,
            spans,
        }]))
        .expect("the span applies");
    f.res = Resolved::rebuild(&f.doc);

    let out = svg_of(&f.doc, &f.res, &[f.text]);
    assert!(
        !out.contains("<text font-weight") && out.contains("font-weight=\"500\""),
        "the writer states the weights on the tspans and not on the <text>: {out}"
    );

    let (doc, ids) = reimport(&out);
    let text = ids
        .iter()
        .copied()
        .find(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Text { .. }))
        .expect("a text layer came back");
    let NodeKind::Text { style, spans, .. } = doc.get(text).unwrap().kind() else {
        panic!("a Text node")
    };
    assert_eq!(
        style.weight, 400,
        "one cycle, and it lands on the element default rather than on either run"
    );
    let weights: Vec<(usize, usize)> = spans
        .as_slice()
        .iter()
        .filter(|s| matches!(s.attr, CharAttr::Weight(_)))
        .map(|s| (s.start, s.end))
        .collect();
    assert_eq!(
        weights,
        [(0, 3), (3, 10)],
        "both runs mint a span, neither covers everything, so neither is hoisted"
    );

    // Cycle 2: the *styling* is a fixed point from here — same node weight, same two
    // spans. ⚠️ **The markup is not byte-identical, and asserting that it was is what
    // this comment is for.** Two things legitimately move and neither is about
    // weights: the `viewBox` and the `transform`, because the writer frames on world
    // bounds and the re-imported node sits at a different origin (this file's own
    // header); and `fill`, which the first pass emits on the `<text>` *and* on every
    // `<tspan>` and the second emits only on the `<text>` — the run colours having
    // been hoisted onto the node (§15 D398), which is the hoist doing its job one
    // cycle later on a different attribute than the one it was written for.
    let res2 = Resolved::rebuild(&doc);
    let (doc3, ids3) = reimport(&svg_of(&doc, &res2, &[text]));
    let text3 = ids3
        .iter()
        .copied()
        .find(|id| matches!(doc3.get(*id).unwrap().kind(), NodeKind::Text { .. }))
        .expect("a text layer came back again");
    let NodeKind::Text { style, spans, .. } = doc3.get(text3).unwrap().kind() else {
        panic!("a Text node")
    };
    assert_eq!(style.weight, 400, "the weight is stable");
    assert_eq!(
        spans
            .as_slice()
            .iter()
            .filter(|s| matches!(s.attr, CharAttr::Weight(_)))
            .map(|s| (s.start, s.end))
            .collect::<Vec<_>>(),
        [(0, 3), (3, 10)],
        "and so are the runs that carry it"
    );
}

/// **A two-colour line comes back as one node with its colour span intact**, which
/// is the half of the text round trip that used to be a report rather than a
/// drawing.
///
/// The writer emits one `<tspan>` per *style run* (§15 D81) and the run's colour is
/// what §15 D154 put on it; the reader now folds an unpositioned `<tspan>`'s fill
/// back into `CharSpans`. **This test is the only thing that binds those two**, and
/// it is worth a whole test rather than an assertion on the one above because the
/// two halves live in different crates and neither's own suite can see the seam.
///
/// ⚠️ **The fixture's content is `Hi <there>`, and the angle brackets are the
/// point.** The writer escapes them to `&lt;`/`&gt;`, so the coloured run starts at
/// byte 3 of the *content* and byte 3 of the *markup* is a different character
/// entirely. A reader that counted markup bytes would come back colouring `t;there`
/// and pass every assertion about there being a span at all — so the range is
/// asserted exactly, and the content it indexes is asserted first.
#[test]
fn a_runs_colour_survives_the_round_trip_where_its_paragraph_does_not() {
    use ondin_core::{CharAttr, CharSpans};

    let mut f = common::fixture();
    let red = Color::from_rgba8(220, 30, 40, 255);
    let mut spans = CharSpans::default();
    let NodeKind::Text { style, .. } = f.doc.get(f.text).unwrap().kind() else {
        panic!("a Text node")
    };
    spans.set(3..10, CharAttr::Color(Some(red)), style);
    assert_eq!(spans.as_slice().len(), 1, "the fixture has a coloured run");
    f.doc
        .apply(&Transaction(vec![Operation::SetTextSpans {
            id: f.text,
            spans,
        }]))
        .expect("the span applies");
    f.res = Resolved::rebuild(&f.doc);

    let out = svg_of(&f.doc, &f.res, &[f.text]);
    assert!(
        out.contains("&lt;there&gt;"),
        "the writer escapes the brackets, which is what makes the offsets differ: {out}"
    );

    let (doc, ids) = reimport(&out);
    let text = ids
        .iter()
        .copied()
        .find(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Text { .. }))
        .expect("a text layer came back");
    let NodeKind::Text { content, spans, .. } = doc.get(text).unwrap().kind() else {
        panic!("a Text node")
    };
    // Assert the fixture is in the state the range is about, or the range is
    // about nothing.
    assert_eq!(content, "Hi <there>", "the entities decoded");
    let list = spans.as_slice();
    assert_eq!(list.len(), 1, "one colour span came back: {list:?}");
    assert_eq!(
        (list[0].start, list[0].end),
        (3, 10),
        "over \"<there>\" — content bytes, not markup bytes"
    );
    let CharAttr::Color(Some(c)) = &list[0].attr else {
        panic!("a colour span, got {:?}", list[0].attr)
    };
    assert_eq!(
        c.to_rgba8().to_u8_array(),
        red.to_rgba8().to_u8_array(),
        "and it is the colour that went in"
    );
}

/// **A rail round-trips: type on a path comes back on the same path** (§15 D406).
///
/// **The gap this closes was named on the day the writer opened it** (§15 D405):
/// `<textPath>` was emitted and not read, so type on a curve went out and came back
/// as *nothing*. That is the worse of the two failure modes — worse than coming back
/// straight, because a straight line at least tells you where the layer is.
///
/// Three claims, and they fail in three different ways:
///
/// - **It comes back as one text node carrying a rail.** A reader that skipped
///   `<textPath>` and read the `<text>` around it would produce a straight layer at
///   the origin, which is the plausible wrong answer.
/// - **The picture is the same size *and in the same place*.** The rail is the
///   node's geometry now, so the box is the ribbon it sweeps.
///   ⚠️ **The rect is exported alongside the text for the position half, and it is
///   not decoration**: with a single layer the "relative to the union" comparison
///   this file is built on is trivially `(0, 0)` and asserts nothing, which is
///   exactly what the older text round-trip says about itself. The failure it buys
///   is the one a size assertion cannot see — the baseline translate left on top of
///   a rail, which moves the layer by an ascent and resizes it by nothing.
/// - **The `d` survives exactly.** Asserted on the geometry rather than the markup
///   because the writer's `viewBox` legitimately moves everything, per this file's
///   header.
#[test]
fn type_on_a_rail_round_trips_as_type_on_the_same_rail() {
    use ondin_core::kurbo::{BezPath, Shape};
    use ondin_core::{GeometryPatch, Operation, Transaction};

    // ⚠️ **The rail starts well away from the node's own origin on purpose.** A rail
    // through `(0, 0)` puts the first glyph's baseline at `y ≈ 0`, which is what the
    // *flat* placement arithmetic reads as the ascent — so leaving that arithmetic
    // on top of a rail moves the layer by about one unit and every tolerance in this
    // file swallows it. Measured: with the bug in, a rail from the origin drifts by
    // (1.0, 0.4) and this rail by an ascent. **A fixture that does not reach the
    // failure is the commonest way a round-trip test passes for the wrong reason.**
    let mut rail = BezPath::new();
    rail.move_to((10.0, 60.0));
    rail.curve_to((50.0, 30.0), (120.0, 90.0), (160.0, 60.0));

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetGeometry {
            id: f.text,
            geometry: GeometryPatch::TextPath(Some(rail.clone())),
        }]))
        .expect("a text node takes a rail");
    let res = Resolved::rebuild(&f.doc);
    let before = res.world_bounds(f.text).expect("a railed box");

    // Two layers, so position is comparable at all — see the doc.
    let out = svg_of(&f.doc, &res, &[f.text, f.rect]);
    assert!(out.contains("<textPath"), "the writer emits it: {out}");
    let (was, was_union) = boxes(&f.doc, &[f.text, f.rect]);

    let (doc, ids) = reimport(&out);
    let texts: Vec<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Text { .. }))
        .collect();
    assert_eq!(texts.len(), 1, "one layer, not one per line: {out}");

    let NodeKind::Text {
        on_path: Some(back),
        ..
    } = doc.get(texts[0]).unwrap().kind()
    else {
        panic!("the rail did not survive — this is the layer coming back straight");
    };
    let (a, b) = (rail.bounding_box(), back.bounding_box());
    assert!(
        (a.width() - b.width()).abs() < 0.01 && (a.height() - b.height()).abs() < 0.01,
        "the same curve came back: {a:?} against {b:?}"
    );

    let (now, now_union) = boxes(&doc, &ids);
    let (then, this) = (
        relative(was[0], was_union.origin()),
        relative(
            now[ids.iter().position(|i| *i == texts[0]).expect("in the set")],
            now_union.origin(),
        ),
    );
    assert!(
        (this.width() - then.width()).abs() < 2.0 && (this.height() - then.height()).abs() < 2.0,
        "the layer is the same size: {this:?} against {then:?}"
    );
    assert!(
        (this.x0 - then.x0).abs() < 2.0 && (this.y0 - then.y0).abs() < 2.0,
        "and in the same place beside the rect — an ascent's worth of drift here is \
         the baseline translate left on top of a rail: {this:?} against {then:?}"
    );
    // Silence the unused warning without weakening either assertion above.
    let _ = before;
}

/// **A flipped rail round-trips as the same *drawing*, and deliberately not as the
/// same *spelling*** (§15 D406).
///
/// The writer has no way to say "the other side" that every viewer honours — SVG 2's
/// `side="right"` is unevenly implemented — so it writes the rail **reversed**,
/// which is exactly what our own flip means (`text::PathWarp::flip` traverses
/// backwards) and which every SVG renderer since 1.1 gets right. The reader then
/// sees a reversed rail with no flag.
///
/// ⚠️ **So the assertion is on the ink and not on the flag**, and asserting on the
/// flag is the mistake this doc exists to stop the next reader making: `on_path_flip`
/// comes back `false` and that is correct. What must survive is where the letters
/// are, which is what a user would call the same picture.
#[test]
fn a_flipped_rail_round_trips_as_the_same_drawing() {
    use ondin_core::kurbo::BezPath;
    use ondin_core::{GeometryPatch, Operation, Transaction};

    let mut rail = BezPath::new();
    rail.move_to((0.0, 0.0));
    rail.curve_to((40.0, -30.0), (110.0, 30.0), (150.0, 0.0));

    let sized = |flip: bool| -> Rect {
        let mut f = common::fixture();
        f.doc
            .apply(&Transaction(vec![
                Operation::SetGeometry {
                    id: f.text,
                    geometry: GeometryPatch::TextPath(Some(rail.clone())),
                },
                Operation::SetGeometry {
                    id: f.text,
                    geometry: GeometryPatch::TextPathFlip(flip),
                },
            ]))
            .expect("a railed, optionally flipped text node");
        let res = Resolved::rebuild(&f.doc);
        let before = res.world_bounds(f.text).expect("a railed box");
        let out = svg_of(&f.doc, &res, &[f.text]);
        let (doc, ids) = reimport(&out);
        let texts: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Text { .. }))
            .collect();
        assert_eq!(texts.len(), 1, "one layer: {out}");
        // The flag is *not* asserted — see the doc. What is asserted is that the
        // box the reader produces matches the one the writer started from.
        let (_, union) = boxes(&doc, &texts);
        assert!(
            (union.width() - before.width()).abs() < 2.0
                && (union.height() - before.height()).abs() < 2.0,
            "flip={flip}: the layer came back the same size: {union:?} against {before:?}"
        );
        before
    };

    // And the two are genuinely different pictures to begin with, or the assertion
    // above is being made twice about one thing. A flip on this rail moves the type
    // to the other side of a curve that is not symmetric about it, so the boxes
    // differ.
    let (plain, flipped) = (sized(false), sized(true));
    assert!(
        (plain.y0 - flipped.y0).abs() > 1.0 || (plain.y1 - flipped.y1).abs() > 1.0,
        "the fixture must actually be flippable, or this test asserts nothing: \
         {plain:?} against {flipped:?}"
    );
}

/// **A rail's start offset survives the round trip, as `startOffset` in percent**
/// (§15 D409).
///
/// **The writer already had a percentage and the reader was throwing it away.**
/// `startOffset` was emitted from the alignment alone and read back only to be
/// checked against `text-anchor` and reported as approximate; now the model has a
/// field for "the type starts a third of the way along", both sides carry it.
///
/// ⚠️ **The anchor is *not* folded in, and asserting both is the point.** They say
/// different things — the anchor is how the text sits about a point on the rail,
/// the offset is which point — so a centred node offset by a quarter writes `75%`
/// with `middle` rather than an absolute start with `text-anchor="start"`. Folding
/// them would give up the one robustness the percentage buys: a viewer whose
/// shaping differs still centres the text on the same spot.
#[test]
fn a_rails_start_offset_round_trips_as_a_percentage() {
    use ondin_core::kurbo::BezPath;
    use ondin_core::{GeometryPatch, Operation, Transaction};

    let mut rail = BezPath::new();
    rail.move_to((10.0, 60.0));
    rail.curve_to((50.0, 30.0), (120.0, 90.0), (160.0, 60.0));

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![
            Operation::SetGeometry {
                id: f.text,
                geometry: GeometryPatch::TextPath(Some(rail)),
            },
            Operation::SetGeometry {
                id: f.text,
                geometry: GeometryPatch::TextPathOffset(0.25),
            },
        ]))
        .expect("a railed node with an offset");
    let res = Resolved::rebuild(&f.doc);
    let out = svg_of(&f.doc, &res, &[f.text]);

    assert!(
        out.contains(r#"startOffset="25%""#),
        "a start-aligned node writes its offset as the whole percentage: {out}"
    );

    let (doc, ids) = reimport(&out);
    let text = ids
        .iter()
        .copied()
        .find(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Text { .. }))
        .expect("one text layer");
    let NodeKind::Text {
        on_path_offset,
        on_path: Some(_),
        ..
    } = doc.get(text).unwrap().kind()
    else {
        panic!("the rail did not survive");
    };
    assert!(
        (on_path_offset - 0.25).abs() < 0.001,
        "and it comes back as the same fraction: {on_path_offset}"
    );
}

/// **A radial gradient's focal point survives the round trip** (§15 D413).
///
/// The focal circle is SVG's `fx`/`fy` and peniko's `start_center` — the point the
/// ramp radiates *from*, which is what makes a sphere read as lit from one side
/// rather than as a flat target. The writer emits it and the reader defaults it to
/// `cx`/`cy` when absent, and **nothing in the workspace asserted either half**
/// until 2026-09-03, which is how `paint::with_radial_centre` came to discard it in
/// the panel with every gate green.
///
/// ⚠️ **Asserted as the offset from the centre, not as a coordinate.** The writer
/// frames its output on a `viewBox` computed from world bounds, so the pair may
/// legitimately arrive somewhere else on the page; what must survive is where the
/// highlight sits *relative to the ramp it belongs to*. That is the same rule this
/// file's head states for shapes, applied inside one paint.
///
/// **Flipped** against the plausible wrong writer — `fx`/`fy` emitted from
/// `end_center`, which is a copy-paste away and is also what someone would write
/// believing the focal point defaults to the centre — and it fails on the offset
/// assertion. Note the `fx=`/`fy=` *presence* check above cannot catch that one: the
/// attributes are still there, carrying the wrong point.
#[test]
fn a_radial_gradients_focal_point_round_trips_where_it_sits() {
    use ondin_core::peniko::{Gradient, GradientKind, RadialGradientPosition};

    let mut ids = IdSource::new(0xF0CA);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let rect = ids.mint();
    let mut gradient = Gradient::new_radial((40.0, 40.0), 30.0).with_stops([
        (0.0_f32, Color::from_rgba8(255, 255, 255, 255)),
        (1.0_f32, Color::from_rgba8(0, 0, 0, 255)),
    ]);
    gradient.kind = GradientKind::Radial(RadialGradientPosition {
        start_center: Point::new(28.0, 24.0),
        start_radius: 0.0,
        end_center: Point::new(40.0, 40.0),
        end_radius: 30.0,
    });
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Ellipse {
                size: Size::new(80.0, 80.0),
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: rect,
            fills: vec![ondin_core::Fill {
                brush: ondin_core::Brush::Gradient(gradient.into()),
                visible: true,
            }],
        },
    ]))
    .expect("a lit sphere");
    let res = Resolved::rebuild(&doc);
    let out = svg_of(&doc, &res, &[rect]);
    assert!(
        out.contains("fx=") && out.contains("fy="),
        "the writer says the focal point at all: {out}"
    );

    let (back, got) = reimport(&out);
    let shape = got
        .iter()
        .copied()
        .find(|id| !matches!(back.get(*id).unwrap().kind(), NodeKind::Group))
        .expect("the shape came back");
    let ondin_core::Brush::Gradient(g) = &back.get(shape).unwrap().paint().fills[0].brush else {
        panic!("still a gradient fill");
    };
    let GradientKind::Radial(p) = g.gradient.kind else {
        panic!("still radial");
    };
    let offset = p.start_center - p.end_center;
    assert!(
        (offset.x + 12.0).abs() < 0.01 && (offset.y + 16.0).abs() < 0.01,
        "and the highlight is still up and to the left by the same amount: {offset:?}"
    );
}

/// **An elliptical radial gradient survives the round trip** (§15 D412), which is
/// the two-sided half of that change: the writer has to emit `gradientTransform`
/// and the reader has to keep the squash off the two circles.
///
/// ⚠️ **It closes a loop that was previously *reported* as lossy in one direction
/// and silently lossy in the other.** `svg_in` used to approximate an ellipse to a
/// circle and say so; the writer had no `gradientTransform` at all, so a gradient
/// that had somehow acquired a squash would have gone out round with nothing said.
/// Only a round trip can fail on the second half.
///
/// The document is built with the transform directly rather than imported, so this
/// is about the writer and the reader rather than about `svg_in`'s baking rule —
/// `a_squashed_radial_gradient_is_exact_rather_than_drawn_round` covers that.
#[test]
fn an_elliptical_gradient_round_trips_as_the_same_ellipse() {
    use ondin_core::GradientBrush;
    use ondin_core::peniko::Gradient;

    let mut ids = IdSource::new(0xE0E0);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let rect = ids.mint();
    let squash = Affine::scale_non_uniform(1.0, 6.0);
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 80.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: rect,
            fills: vec![ondin_core::Fill {
                brush: ondin_core::Brush::Gradient(GradientBrush {
                    gradient: Gradient::new_radial((40.0, 40.0), 20.0).with_stops([
                        (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
                        (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
                    ]),
                    transform: squash,
                    opacity: 1.0,
                }),
                visible: true,
            }],
        },
    ]))
    .expect("a rect with an elliptical gradient");
    let res = Resolved::rebuild(&doc);
    let out = svg_of(&doc, &res, &[rect]);

    assert!(
        out.contains("gradientTransform=\"matrix(1 0 0 6 0 0)\""),
        "the writer says the squash in SVG's own spelling: {out}"
    );

    let (back, got) = reimport(&out);
    let shape = got
        .iter()
        .copied()
        .find(|id| !matches!(back.get(*id).unwrap().kind(), NodeKind::Group))
        .expect("the rect came back");
    let ondin_core::Brush::Gradient(g) = &back.get(shape).unwrap().paint().fills[0].brush else {
        panic!("still a gradient fill");
    };
    // The two circles must be *unmapped*: a reader that folded the squash into
    // them would give back a round gradient of some averaged radius, which is
    // exactly what the import used to do and report.
    let ondin_core::peniko::GradientKind::Radial(r) = g.gradient.kind else {
        panic!("still radial");
    };
    assert!(
        (r.end_radius - 20.0).abs() < 0.01,
        "the circle is the one that was written: {}",
        r.end_radius
    );
    let (a, b) = (g.transform.as_coeffs(), squash.as_coeffs());
    assert!(
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6),
        "and the squash came back as itself: {a:?} against {b:?}"
    );
}

/// **An effect stack survives the round trip**, which nothing in this file asserted
/// until 2026-09-03 and which no gate could have noticed (§15 D411).
///
/// The writer emits a whole `<filter>` graph for every stack — a `feColorMatrix`
/// silhouette, an optional `feMorphology`, an `feOffset`, an optional
/// `feGaussianBlur`, then `feFlood` + `feComposite` — and the reader accepted a
/// `<filter>` that was exactly one `<feGaussianBlur>` and nothing else. So every
/// shadow and every adjustment this app exported came back as nothing, reported as
/// `skipped: ["filter"]` and therefore honest, but *lost*. **The round trip was the
/// only thing that could have said so, and it had no effect coverage at all.**
///
/// Asserted as the model's own stack rather than as markup: what a user means by
/// "the shadow survived" is a `DropShadow` with their offset, blur and colour on it.
///
/// ⚠️ **The effect comes back on a *group*, not on the shape, and that asymmetry is
/// deliberate on both sides** — the same shape as the `<polygon>`-becomes-`Path` one
/// this file's head already records. The writer wraps a filtered node in
/// `<g filter="url(#…)">` rather than putting the attribute on the element, because
/// a stack applies to a container exactly as it does to a shape and one spelling
/// serves both; the reader then faithfully reads the group it is given. So the
/// picture round-trips and the tree gains a level. Asserted as *which* node carries
/// it — exactly one — rather than as the shape's own stack, which would demand a
/// collapse the reader has no business doing.
#[test]
fn an_effect_stack_round_trips_as_the_same_stack() {
    use ondin_core::{Effect, EffectKind, Shadow};

    let mut ids = IdSource::new(0x5A5A);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let rect = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 40.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((20.0, 20.0))),
            name: None,
        },
        Operation::SetEffects {
            id: rect,
            effects: vec![Effect::new(EffectKind::DropShadow(Shadow {
                offset: ondin_core::kurbo::Vec2::new(4.0, 6.0),
                blur: 8.0,
                spread: 0.0,
                color: Color::from_rgba8(0, 0, 0, 128),
            }))],
        },
    ]))
    .expect("a rect with one drop shadow");
    let res = Resolved::rebuild(&doc);

    let (back, got) = reimport(&svg_of(&doc, &res, &[rect]));
    let carrying: Vec<NodeId> = got
        .iter()
        .copied()
        .filter(|id| !back.get(*id).unwrap().effects().is_empty())
        .collect();
    assert_eq!(
        carrying.len(),
        1,
        "exactly one node came back carrying an effect, not {}",
        carrying.len()
    );
    let effects = back.get(carrying[0]).unwrap().effects();
    assert_eq!(effects.len(), 1, "one effect back, not {}", effects.len());
    let EffectKind::DropShadow(s) = &effects[0].kind else {
        panic!("it is still a drop shadow, not {:?}", effects[0].kind);
    };
    assert!(
        (s.offset.x - 4.0).abs() < 0.01 && (s.offset.y - 6.0).abs() < 0.01,
        "at the offset it was written with: {:?}",
        s.offset
    );
    assert!(
        (s.blur - 8.0).abs() < 0.01,
        "and the blur radius, in the model's units rather than the deviation: {}",
        s.blur
    );
    assert!(
        (s.color.components[3] - 128.0 / 255.0).abs() < 0.01,
        "and the colour's alpha, which the writer splits into flood-opacity: {:?}",
        s.color
    );
}

/// **A picture round-trips into nothing, and the suite is where that gets
/// recorded** (§15 D477).
///
/// `[A5-L6-01]`. This module's header calls itself *"the only thing that binds"*
/// the writer and the reader, and it had no image case — each side had its own
/// green test on its own hand-written fixture (`svg::an_image_fill_exports_as_a_
/// pattern_over_the_encoded_original` and `svg_in::an_image_becomes_a_rect_with_
/// an_image_fill`) and **nothing composed them**. `svg_in`'s own module doc names
/// this hazard verbatim: *"the two mappings can drift, and nothing the compiler
/// does will say so — what binds them is the round-trip test in
/// `ondin-export/tests/svg_roundtrip.rs`."*
///
/// The writer emits an image fill as `<pattern><image href="data:…"/></pattern>`
/// and points the shape at it; the reader has **no `<pattern>` support at all** —
/// `Gradients::collect` keys `by_id` on `linearGradient`/`radialGradient` and
/// `clips` on `clipPath`/`mask`, and `parse_paint` has no arm for a pattern. So
/// the picture does not come back, and before this it did not come back
/// **silently**: measured, the re-imported rect held one *solid black* fill, the
/// document held **0 images**, and the image was on neither report list.
///
/// ⚠️ **This test does not assert that the picture survives, because it does
/// not.** Reading a pattern back is a new paint-server kind in the reader and is
/// left to whoever writes it — which is exactly the practice §15 D411 set and
/// `text_round_trips_as_text_…` records: *"recording the gap where the gap was so
/// that closing it would be a failing test rather than an act of memory."* What
/// it asserts is the two things that were also wrong and are cheap:
///
/// 1. **The loss is reported.** `svg_in`'s header forbids the alternative —
///    *"nothing is lost silently — a paste that quietly loses half a drawing is
///    worse than one that refuses"* — and `skipped` now names the pattern.
/// 2. **The shape is unpainted rather than an opaque black rectangle** — which it
///    already was, and that is the finding's one claim that does not survive
///    checking. See the ⚠️ below.
///
/// ⚠️ **And `skipped` carried a false positive**, which is the third thing here:
/// the writer emits its artboard `<clipPath>` at top level rather than inside
/// `<defs>`, so the walk reached it through the unknown-element arm and reported
/// it **skipped** — when `Gradients::collect` had already registered it and the
/// clip round-tripped perfectly. A false positive on that list is worse than a
/// missing entry: the list is the one thing telling a user what a paste lost, and
/// an item on it that was not lost teaches them not to read it. `clipPath` and
/// `mask` are definitions wherever they sit and the walk skips them silently now.
///
/// ⚠️ **The "opaque black rectangle" does not reproduce, and a one-word fix for
/// it was written and taken back out.** `svg_in::parse_paint` returns `None` for
/// a `url(…)` it cannot resolve, so `style.fill` becomes `None` and the shape is
/// **unpainted** — no black anywhere near it. A ` none` fallback after the
/// reference was added to `paint_attr` on the finding's reading, and the flip
/// showed it changing nothing; measured in Chrome over HTTP to be sure the
/// finding was not right about *browsers* where it was wrong about this reader,
/// `<rect fill="url(#nope)"/>` and `<rect fill="url(#nope) none"/>` both paint
/// **0** black pixels. It was reverted rather than kept on "some other renderer
/// might".
///
/// **The black the finding saw is almost certainly the artboard `<clipPath>`'s own
/// rect**, which comes back as a **mask node** carrying SVG's initial black
/// because the writer gives it no `fill` — harmless, since a mask paints nothing.
/// A whole-tree sweep for black finds it, which is why this test looks the
/// picture's rect up by size, and why the first draft of it failed for that
/// reason.
///
/// **Flipped** at each surviving half. Dropping the `patterns` report fails the
/// `skipped` assertion. Putting `clipPath` back on the walked list fails the
/// false-positive assertion.
#[test]
fn an_image_fill_is_reported_lost_rather_than_silently_blackened() {
    use ondin_core::{
        Brush, Fill, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource, peniko::Color,
    };

    let mut ids = IdSource::new(0x1A6E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, rect) = (ids.mint(), ids.mint());
    let pic = ImageId("pic".into());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(200.0, 120.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 40.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        },
        Operation::AddImage {
            id: pic.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(vec![1, 2, 3].into()),
                format: ImageFormat::Png,
                width: 4,
                height: 4,
            },
        },
        Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: Brush::Image(ondin_core::ImageBrush {
                    image: ImageRef::new(pic),
                    sampler: Default::default(),
                }),
                visible: true,
            }],
        },
    ]))
    .expect("a rect with a picture on it");
    let res = Resolved::rebuild(&doc);
    let markup = svg_of(&doc, &res, &[ab]);

    // The fixture is in the state this test is about.
    assert!(
        markup.contains("<pattern id=\"img-"),
        "fixture: the writer emits the picture as a pattern:\n{markup}"
    );

    let mut back_ids = IdSource::new(0xBEEF);
    let back_root = back_ids.mint();
    let mut back = Document::new(back_root);
    let out = ondin_core::svg_in::import(&markup, &mut back_ids, back_root, 0, None)
        .expect("our own writer's SVG");
    back.apply(&out.tx).expect("the import applies");

    assert!(
        out.skipped.iter().any(|s| s.contains("pattern")),
        "the picture is lost and the paste has to say so: skipped {:?}, \
         approximated {:?}",
        out.skipped,
        out.approximated
    );
    assert!(
        !out.skipped.iter().any(|s| s == "clipPath"),
        "and the artboard's own clip round-tripped, so naming it is a false \
         positive on the one list a user reads: {:?}",
        out.skipped
    );

    // **Not a black rectangle.** The reference carries `none` as its fallback, so
    // a reader that cannot resolve the pattern paints nothing at all.
    //
    // ⚠️ **Found by node size rather than over every node**, and the first draft
    // swept them all and failed for the wrong reason. The artboard's `<clipPath>`
    // comes back as a **mask node** — that is the clip round-tripping, which is
    // the good half — and its `<rect>` carries no `fill` attribute, so it inherits
    // SVG's initial black like anything else. A mask's fill paints nothing, so
    // that black is harmless and pre-existing; sweeping for it measures the clip
    // rather than the picture.
    let (back2, all) = reimport(&markup);
    let black = Color::from_rgba8(0, 0, 0, 255);
    let picture = all
        .iter()
        .find(|id| {
            matches!(
                back2.get(**id).map(|n| n.kind()),
                Some(NodeKind::Rect { size, .. }) if (size.width - 80.0).abs() < 0.01
            )
        })
        .copied()
        .expect("the 80×40 rect that carried the picture came back");
    let fills = &back2.get(picture).expect("live").paint().fills;
    assert!(
        !fills
            .iter()
            .any(|f| matches!(&f.brush, Brush::Solid(c) if *c == black)),
        "a picture that could not be read must not become an opaque black \
         rectangle — the reference needs its `none` fallback: {fills:?}"
    );
    assert!(
        fills.is_empty(),
        "and `none` means unpainted, not some other colour: {fills:?}"
    );
}

/// **A named layer keeps its name across the round trip** (§15 D611,
/// `[S8.1-L6-04]`).
///
/// 🚨 **No name reached the file at all, and six reader sites took `id` *as* the
/// name.** So the pair was asymmetric in the one direction that loses data: every
/// layer of an exported drawing came back unnamed, and nothing anywhere said so —
/// this suite calls itself *"the only thing that binds"* the two and asserted
/// geometry, paint and structure and never a name. `Node::name()` was not called
/// once in the whole writer.
///
/// **Four names, chosen for what they do to markup rather than for coverage.**
/// A group and a leaf, because they are written by two different functions
/// (`svg::name_attr`'s three call sites — plain backticks, because nothing checks
/// a link in a `tests/` file and §15 D319 says to say so); a name with a **space**, which is what
/// `naming.rs` produces by default and is not a valid XML `Name` — the reason the
/// attribute is `data-name` and not `id`; and one holding `&`, `<` and a quote,
/// which is the escaping the writer owes and the only one that can make the file
/// unparseable rather than merely wrong.
///
/// ⚠️ **The stacked-paint arm is the third writer of a node's element and is
/// covered here by the two-fill rect**, which is not obvious from the fixture: a
/// single fill takes `emit_shape`'s plain arm and a second one moves the same node
/// to `emit_stacked`, where the `<g>` is the node and its children are its paints.
/// A name written onto one of those children would come back attached to a layer
/// the document does not have.
///
/// **Flip-check, run, and the first one did not bite** — which is a finding about
/// this test rather than about the code, and it is why the second assertion
/// exists. `name_attr` writing ` id="…"` instead of ` data-name="…"` — the other
/// candidate fix, the one the finding sketched first — **round-trips all three
/// names perfectly**, because `roxmltree` hands back an attribute value verbatim
/// and never asks whether an `id` is a valid XML `Name`. So the round trip cannot
/// tell the two spellings apart, and a test that only round-trips would have
/// blessed the wrong one.
///
/// 🚨 **What separates them is the markup, and that is the second assertion.** An
/// `id` must be an XML `Name` and **unique in the document**, and this writer
/// already mints ids from `NodeId::to_wire` for every def it emits — `grad-…`,
/// `clip-…`, `mask-…`, `fx-…`. Writing names as ids therefore puts free text into
/// that namespace: `"Outer group"` is not a `Name` at all, and a layer a user
/// happened to name `clip-601d_2` would shadow a real `<clipPath>` and silently
/// redirect its own frame's `url(#…)`. Asserting that every emitted `id` is a
/// `Name` bites the flip at the group, and removing the writer entirely fails the
/// first assertion with the name missing.
#[test]
fn a_layers_name_survives_the_round_trip() {
    let mut ids = IdSource::new(0x9E9E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (group, leaf, stacked) = (ids.mint(), ids.mint(), ids.mint());

    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: group,
            parent: root,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((10.0, 10.0))),
            name: Some("Outer group".into()),
        },
        Operation::CreateNode {
            id: leaf,
            parent: group,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 30.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: Some(r#"Ampersand & "angle" <bracket>"#.into()),
        },
        Operation::CreateNode {
            id: stacked,
            parent: group,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((60.0, 0.0))),
            name: Some("Two fills".into()),
        },
    ]))
    .expect("the fixture applies");
    // The second fill is what moves `stacked` onto the writer's stacked arm.
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: stacked,
        fills: vec![
            ondin_core::Fill {
                brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(0xC0, 0x20, 0x20, 255)),
                visible: true,
            },
            ondin_core::Fill {
                brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(0x20, 0x20, 0xC0, 0x80)),
                visible: true,
            },
        ],
    }]))
    .expect("two fills apply");

    let res = Resolved::rebuild(&doc);
    let markup = svg_of(&doc, &res, &[group]);
    let (back, got) = reimport(&markup);

    let names: Vec<String> = got
        .iter()
        .filter_map(|id| back.get(*id).map(|n| n.name().to_string()))
        .collect();

    for want in [
        "Outer group",
        r#"Ampersand & "angle" <bracket>"#,
        "Two fills",
    ] {
        assert!(
            names.iter().any(|n| n == want),
            "the name {want:?} did not survive the round trip; got {names:?}\n{markup}"
        );
    }

    // Every `id` the writer emits is still an XML `Name` — which is what says the
    // names went somewhere else. Scanned off the markup rather than off the
    // document, because the claim is about the file.
    for (i, _) in markup.match_indices(r#" id=""#) {
        let from = i + r#" id=""#.len();
        let value = &markup[from..][..markup[from..].find('"').expect("a closed attribute")];
        let name = |first: bool, c: char| {
            c.is_ascii_alphabetic()
                || c == '_'
                || c == ':'
                || (!first && matches!(c, '-' | '.'))
                || (!first && c.is_ascii_digit())
        };
        let ok = value.chars().enumerate().all(|(n, c)| name(n == 0, c));
        assert!(
            ok && !value.is_empty(),
            "the writer emitted id={value:?}, which is not an XML Name — layer names \
             belong in `data-name`, not in the namespace the defs' ids live in\n{markup}"
        );
    }
}
/// **The writer's own `<clipPath>` spelling comes back as a clip and is not
/// reported lost** — §15 D650, `[A5-L6-05]`.
///
/// 🚨 **One of the suite's fifteen tests was not a binding.** This file's header
/// calls itself *"the only thing that binds"* the writer and the reader, and
/// fourteen of the fifteen earned that by calling `svg_of` and feeding the result
/// to `reimport`. (⚠️ **The finding says thirteen and twelve, and this comment
/// did too until `arch-scribe` counted** — the file held thirteen tests when the
/// review pass read it and has grown since. Nothing rests on the number, which is
/// exactly why it went unchecked; `grep -c '^#\[test\]'` is 16 with this one, and
/// the 16 `svg_of(` call sites include two inside
/// `two_weights_settle_on_the_element_default_rather_than_on_either_run`.)
/// `an_imported_clip_path_is_empty_outside_the_clip` — directly above — takes
/// **hand-authored** markup and never touches the writer. It is honest as what it
/// is (a renderer test: *"this is the layer where 'there is a mask node in the
/// tree' becomes 'the ink stops'"*), and it was being counted as the suite's clip
/// coverage, which it cannot be: both sides of a hand-written fixture agree about
/// what SVG looks like, so only the writer's own output disagrees with you.
///
/// **The writer puts the artboard clip at top level rather than inside `<defs>`**,
/// which is legal and is the spelling nothing bound. The reader's
/// `Gradients::collect` registers a `clipPath` wherever it sits, so it works — and
/// its *walk* had no `clipPath` arm, so the same element was also reported
/// **skipped**. That false positive is fixed and asserted by
/// `an_image_fill_is_reported_lost_rather_than_silently_blackened`; what is
/// asserted here is the other half, which nothing held: that the clip is actually
/// **read back as a clip**.
///
/// ⚠️ **The report lists are asserted empty, both of them.** A drawing made of a
/// frame and one rect is the plainest thing this writer can emit, so anything on
/// either list is a divergence by definition — and `approximated` is the half a
/// reader forgets, since an entry there says *"there and slightly wrong"* rather
/// than *"missing"*.
///
/// ⚠️ **Flip run, and the predicted site was wrong** — which is the useful half.
/// The flip is on the *reader*: `Gradients::collect`'s
/// `if name == "clipPath" || name == "mask"` narrowed to `mask` alone, so the
/// writer's markup is byte-identical and only the reading of it changes. The
/// prediction was that the report lists would stay empty and the **mask**
/// assertion would carry it. Both fail, and the reports one goes first, at
/// `skipped: ["clip-path"]` — an unresolvable `url(#…)` is reported by the
/// *attribute* name rather than the element's, which is a spelling neither the
/// finding nor this test's author had in mind. Relaxing that assertion and
/// re-running shows the mask one failing behind it at 0 against 1, so both have
/// teeth and the order is reports-then-structure.
#[test]
fn the_writers_own_clip_spelling_round_trips() {
    let mut ids = IdSource::new(0xC11B);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, rect) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        },
    ]))
    .expect("a frame with one rect in it");
    let res = Resolved::rebuild(&doc);
    let markup = svg_of(&doc, &res, &[ab]);

    // The fixture is in the state this test names: the clip is written, and it is
    // written *outside* `<defs>`, which is the spelling nothing bound.
    let defs = markup.find("<defs>").unwrap_or(markup.len());
    let clip = markup
        .find(r#"<clipPath id="clip-"#)
        .expect("the writer emits the frame's clip");
    assert!(
        clip < defs,
        "fixture: the clip is emitted at top level, not inside <defs>:\n{markup}"
    );
    assert!(
        markup.contains(r#"clip-path="url(#clip-"#),
        "fixture: and the frame's group references it:\n{markup}"
    );

    let mut back_ids = IdSource::new(0xC11C);
    let back_root = back_ids.mint();
    let out = ondin_core::svg_in::import(&markup, &mut back_ids, back_root, 0, None)
        .expect("our own writer's SVG");

    assert!(
        out.skipped.is_empty() && out.approximated.is_empty(),
        "a frame and a rect is the plainest thing this writer emits, so anything \
         on either list is a divergence: skipped {:?}, approximated {:?}",
        out.skipped,
        out.approximated
    );

    // The clip came back *as a clip*: `svg_in` reads `clip-path` as a wrapper
    // group holding a mask node, which is how this model spells one.
    let (back, all) = reimport(&markup);
    let masks: Vec<_> = all
        .iter()
        .filter(|id| back.get(**id).is_some_and(|n| n.mask()))
        .collect();
    assert_eq!(
        masks.len(),
        1,
        "the clip came back as the one mask node the model spells a clip with, \
         out of {} nodes",
        all.len()
    );
}
