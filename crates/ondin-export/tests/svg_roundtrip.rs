//! The writer (`ondin_export::svg`) and the reader (`ondin_core::svg_in`) are
//! inverse functions in two crates, and this file is what binds them: neither
//! crate's own tests notice when one grows an attribute the other does not know.
//!
//! Asserted as geometry relative to the set's own bounding box, not as bytes: the
//! writer frames its output on a `viewBox` from world bounds, so the drawing may
//! arrive at a different origin.
//!
//! One asymmetry is deliberate: a `Polygon` or `Star` is written as `<polygon>` and
//! comes back as a `Path`, since reading every point list as a regular *n*-gon would
//! turn an arbitrary outline into a hexagon.
//!
//! A test here that does not call `svg_of` is not a binding: hand-written markup
//! agrees with the reader about what SVG looks like.

mod common;

use ondin_core::kurbo::{Affine, Point, Rect, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    BoolOp, Document, FillRule, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    keyed_by_position,
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
/// rect, a circle, a diagonal line and a closed triangle. Not `common::fixture`,
/// whose artboard export brings a background `<rect>` and a `<text>` along.
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
                corner_smoothing: 0.0,
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
            strokes: keyed_by_position([ondin_core::Stroke {
                brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(0, 0, 0, 255)).into(),
                width: 2.0,
                width_var: None,
                join: ondin_core::kurbo::Join::Miter,
                cap: ondin_core::kurbo::Cap::Butt,
                dashes: Vec::new(),
                dash_offset: 0.0,
                miter_limit: 4.0,
                dash_fit: false,
                align: ondin_core::StrokeAlign::Center,
                sides: ondin_core::StrokeSides::All,
                visible: true,
            }]),
        },
    ]))
    .expect("the fixture applies");
    let res = Resolved::rebuild(&doc);
    (doc, res, vec![rect, circle, line, tri])
}

/// Four shapes out, four back, each the same size in the same place relative to
/// the others. The arrangement claim catches an importer that dropped a
/// translation while every size still matched. The 0.02 tolerance is needed
/// because the writer formats coordinates as decimal text.
///
/// flip: the reader ignoring element `transform` → fails at shape 0 (the rect
/// arrives at (-10, -10) instead of (10, 10)).
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
    // The kinds that can survive do; a rect back as a path would pass the above.
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

/// A rounded polygon and star come back with their rounding: they are written as
/// a `<path>`, since a `<polygon>`'s point list would drop every fillet. Compared
/// by area, which a sharp outline overshoots.
///
/// flip: the writer's rounded arm removed → fails at the no-`<polygon>` assertion.
#[test]
fn a_rounded_polygon_and_star_round_trip_with_their_rounding() {
    let mut ids = IdSource::new(0x5151);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let shapes = [
        NodeKind::Polygon {
            size: Size::new(80.0, 80.0),
            sides: 6,
            corner_radius: 12.0,
        },
        NodeKind::Star {
            size: Size::new(90.0, 90.0),
            points: 5,
            inner_ratio: 0.45,
            corner_radius: 6.0,
        },
    ];
    let mut made = Vec::new();
    for (i, kind) in shapes.iter().enumerate() {
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: i,
            kind: kind.clone(),
            transform: Some(Affine::translate((10.0 + 120.0 * i as f64, 10.0))),
            name: None,
        }]))
        .unwrap();
        made.push(id);
    }
    let res = Resolved::rebuild(&doc);
    let out = svg_of(&doc, &res, &made);
    assert!(!out.contains("<polygon"), "{out}");
    let (back, got) = reimport(&out);
    assert_eq!(got.len(), 2, "{out}");
    for (kind, id) in shapes.iter().zip(&got) {
        use ondin_core::kurbo::Shape;
        let want = ondin_core::geometry::local_path(kind).unwrap().area().abs();
        let NodeKind::Path { path, .. } = back.get(*id).unwrap().kind() else {
            panic!("{kind:?} came back as {:?}", back.get(*id).unwrap().kind());
        };
        let area = path.area().abs();
        assert!(
            (area - want).abs() < want * 1e-3,
            "{kind:?}: area {area}, want {want}"
        );
    }
}

/// A smoothed rect is written as its outline, not as a `<rect rx>`, whose
/// corners are circular, and comes back with the smoothed area.
///
/// flip: the writer's `filter` on smoothing removed → fails at the no-`<rect`
/// assertion.
#[test]
fn a_smoothed_rect_round_trips_as_its_outline() {
    use ondin_core::kurbo::Shape;
    let mut ids = IdSource::new(0x5252);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let id = ids.mint();
    let kind = NodeKind::Rect {
        size: Size::new(120.0, 80.0),
        corner_radii: ondin_core::kurbo::RoundedRectRadii::from_single_radius(20.0),
        corner_smoothing: 0.6,
    };
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id,
        parent: root,
        index: 0,
        kind: kind.clone(),
        transform: None,
        name: None,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let out = svg_of(&doc, &res, &[id]);
    assert!(!out.contains("<rect"), "{out}");
    let (back, got) = reimport(&out);
    let NodeKind::Path { path, .. } = back.get(got[0]).unwrap().kind() else {
        panic!("{out}");
    };
    let want = ondin_core::geometry::local_path(&kind)
        .unwrap()
        .area()
        .abs();
    assert!((path.area().abs() - want).abs() < want * 1e-3);
}

/// A group's opacity survives as the group's: `opacity` on a `<g>` composites the
/// group as a unit, and flattening it would draw the overlap twice as dark.
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
            corner_smoothing: 0.0,
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

/// An exclusion round-trips as an even-odd path; a reader ignoring `fill-rule`
/// would fill the hole. Asserted as the hole, not as the attribute.
///
/// flip: the reader ignoring `fill-rule` → fails here and in no other test.
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
                corner_smoothing: 0.0,
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
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((50.0, 0.0))),
            name: None,
        },
        Operation::SetFills {
            id: bool_id,
            fills: keyed_by_position([ondin_core::Fill {
                brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(255, 0, 0, 255)).into(),
                visible: true,
            }]),
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
    // The overlap is the hole, sampled in the path's own local space.
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

/// An imported `clip-path` is empty outside the clip, on screen. This is a renderer
/// test on hand-written markup, not a writer binding; the inside is asserted red
/// first so an import with no ink cannot pass vacuously.
///
/// flip: dropping the `SetMask` operation → fails at the (60, 60) assertion, which
/// comes back `[255, 0, 0, 255]`.
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
        doc.first_page(),
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

/// Text comes back as text with its baseline where the writer put it; a paragraph
/// comes back as lines, since the writer emits one `<text>` per line and nothing
/// in the markup says they were one node. The picture closes; the structure does not.
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
    // Size, not position: the origin legitimately moves, and with one layer the
    // relative-to-union comparison is trivially (0, 0). A dropped `font-size` or a
    // baseline read as a box top shows in the size.
    assert!(
        (union.width() - before.width()).abs() < 2.0
            && (union.height() - before.height()).abs() < 2.0,
        "the text came back the same size: {union:?} against {before:?}"
    );
}

/// Two different weights settle on SVG's initial 400 in one cycle, not on either
/// run's weight: `svg::run_attrs` puts every font property on the `<tspan>`s and
/// none on the `<text>`, so on re-import both runs differ from 400, both mint spans,
/// neither covers the content and neither is hoisted. The markup is not byte-stable
/// across cycles (`viewBox`, `transform`, `fill` on tspans move), so the assertion
/// is on the node's weight and span boundaries. If the writer ever states a font
/// property on `<text>`, cycle 1 becomes an identity and this name is wrong.
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

    // Cycle 2: the styling is a fixed point from here (same weight, same spans),
    // though the markup is not byte-identical.
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

/// A two-colour line comes back as one node with its colour span intact: the
/// writer emits one `<tspan>` per style run and the reader folds its fill back into
/// `CharSpans`. The content is `Hi <there>` so the escaped `&lt;`/`&gt;` make
/// content offsets differ from markup offsets; a reader counting markup bytes would
/// colour `t;there`, so the range is asserted exactly.
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

/// A rail round-trips: type on a path comes back on the same path, as one text
/// node carrying the rail, the same size, in the same place and with the same curve.
/// The rect is exported alongside so the position comparison is not trivially
/// `(0, 0)`; it catches the baseline translate left on top of a rail, which moves
/// the layer by an ascent and resizes it by nothing.
#[test]
fn type_on_a_rail_round_trips_as_type_on_the_same_rail() {
    use ondin_core::kurbo::{BezPath, Shape};
    use ondin_core::{GeometryPatch, Operation, Transaction};

    // The rail starts well away from the node's origin on purpose: through `(0, 0)`
    // the flat placement arithmetic's ascent drifts by about one unit, which every
    // tolerance here swallows.
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

    // Two layers, so position is comparable at all.
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

/// A flipped rail round-trips as the same drawing, not the same spelling: SVG 2's
/// `side="right"` is unevenly implemented, so the writer writes the rail reversed
/// (what `text::PathWarp::flip` means) and the reader sees a reversed rail with no
/// flag. `on_path_flip` comes back `false` and the assertion is on the box.
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
        let (_, union) = boxes(&doc, &texts);
        assert!(
            (union.width() - before.width()).abs() < 2.0
                && (union.height() - before.height()).abs() < 2.0,
            "flip={flip}: the layer came back the same size: {union:?} against {before:?}"
        );
        before
    };

    // The two must be different pictures to begin with: the rail is not symmetric,
    // so a flip moves the boxes.
    let (plain, flipped) = (sized(false), sized(true));
    assert!(
        (plain.y0 - flipped.y0).abs() > 1.0 || (plain.y1 - flipped.y1).abs() > 1.0,
        "the fixture must actually be flippable, or this test asserts nothing: \
         {plain:?} against {flipped:?}"
    );
}

/// A rail's start offset survives the round trip as `startOffset` in percent. The
/// anchor is not folded in: it is how the text sits about a point, the offset is
/// which point, so a viewer whose shaping differs still centres on the same spot.
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

/// A radial gradient's focal point (SVG `fx`/`fy`, peniko `start_center`) survives
/// the round trip, asserted as the offset from the centre rather than a coordinate.
///
/// flip: the writer emitting `fx`/`fy` from `end_center` → fails at the offset
/// assertion (the `fx=`/`fy=` presence check cannot see it).
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
            fills: keyed_by_position([ondin_core::Fill {
                brush: ondin_core::Brush::Gradient(gradient.into()).into(),
                visible: true,
            }]),
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
    let Some(ondin_core::Brush::Gradient(g)) =
        back.get(shape).unwrap().paint().fills[0].brush.literal()
    else {
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

/// An elliptical radial gradient survives the round trip: the writer emits
/// `gradientTransform` and the reader keeps the squash off the two circles. The
/// transform is built directly, not imported;
/// `a_squashed_radial_gradient_is_exact_rather_than_drawn_round` covers `svg_in`'s
/// baking rule.
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
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: rect,
            fills: keyed_by_position([ondin_core::Fill {
                brush: ondin_core::Brush::Gradient(GradientBrush {
                    gradient: Gradient::new_radial((40.0, 40.0), 20.0).with_stops([
                        (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
                        (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
                    ]),
                    transform: squash,
                    opacity: 1.0,
                })
                .into(),
                visible: true,
            }]),
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
    let Some(ondin_core::Brush::Gradient(g)) =
        back.get(shape).unwrap().paint().fills[0].brush.literal()
    else {
        panic!("still a gradient fill");
    };
    // The two circles must be unmapped, not averaged into a round gradient.
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

/// An effect stack survives the round trip, asserted as the model's own stack (a
/// `DropShadow` with its offset, blur and colour) rather than as markup. The effect
/// comes back on a group, not the shape, deliberately on both sides: the writer
/// wraps a filtered node in `<g filter="url(#…)">` and the reader reads the group
/// it is given, so the assertion is that exactly one node carries it.
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
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((20.0, 20.0))),
            name: None,
        },
        Operation::SetEffects {
            id: rect,
            effects: keyed_by_position([Effect::new(EffectKind::DropShadow(Shadow {
                offset: ondin_core::kurbo::Vec2::new(4.0, 6.0),
                blur: 8.0,
                spread: 0.0,
                color: Color::from_rgba8(0, 0, 0, 128),
            }))]),
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

/// A picture round-trips into nothing, and the loss is reported: the writer emits
/// an image fill as a `<pattern>` and the reader has no pattern support, so this
/// pins that `skipped` names the pattern, that the artboard's own `<clipPath>` is
/// not falsely listed as skipped, and that the shape is unpainted rather than an
/// opaque black rectangle. It does not assert the picture survives.
///
/// The artboard `<clipPath>` comes back as a mask node carrying SVG's initial
/// black, harmless as a mask paints nothing, so the picture's rect is found by
/// size rather than by sweeping every node.
///
/// flip: dropping the `patterns` report → fails at the `skipped` assertion;
/// putting `clipPath` back on the walked list → fails at the false-positive one.
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
                corner_smoothing: 0.0,
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
            fills: keyed_by_position([Fill {
                brush: Brush::Image(ondin_core::ImageBrush {
                    image: ImageRef::new(pic),
                    sampler: Default::default(),
                })
                .into(),
                visible: true,
            }]),
        },
    ]))
    .expect("a rect with a picture on it");
    let res = Resolved::rebuild(&doc);
    let markup = svg_of(&doc, &res, &[ab]);

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

    // Not a black rectangle: an unresolvable pattern paints nothing.
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
            .any(|f| matches!(f.brush.literal(), Some(Brush::Solid(c)) if *c == black)),
        "a picture that could not be read must not become an opaque black \
         rectangle — the reference needs its `none` fallback: {fills:?}"
    );
    assert!(
        fills.is_empty(),
        "and `none` means unpainted, not some other colour: {fills:?}"
    );
}

/// A named layer keeps its name across the round trip. The four names are chosen
/// for what they do to markup: a group and a leaf (two writer functions), a name
/// with a space (not an XML `Name`, why the attribute is `data-name`), and one
/// holding `&`, `<` and a quote (the escaping). The two-fill rect covers the
/// stacked-paint arm (`emit_stacked`), where a name on a child would attach to a
/// layer the document does not have.
///
/// The second assertion exists because the round trip cannot tell `data-name` from
/// `id` (`roxmltree` returns either verbatim): every emitted `id` must be an XML
/// `Name`, since names written as ids would share a namespace with the defs' ids
/// and a layer named `clip-601d_2` would shadow a real `<clipPath>`.
///
/// flip: `name_attr` writing ` id="…"` instead of ` data-name="…"` → fails at the
/// XML `Name` assertion at the group; removing the writer fails the first.
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
                corner_smoothing: 0.0,
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
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((60.0, 0.0))),
            name: Some("Two fills".into()),
        },
    ]))
    .expect("the fixture applies");
    // The second fill is what moves `stacked` onto the writer's stacked arm.
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: stacked,
        fills: keyed_by_position([
            ondin_core::Fill {
                brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(0xC0, 0x20, 0x20, 255))
                    .into(),
                visible: true,
            },
            ondin_core::Fill {
                brush: ondin_core::peniko::Brush::Solid(Color::from_rgba8(0x20, 0x20, 0xC0, 0x80))
                    .into(),
                visible: true,
            },
        ]),
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

    // Every `id` the writer emits is still an XML `Name`, scanned off the markup
    // because the claim is about the file.
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
/// The writer's own `<clipPath>` spelling, emitted at top level rather than inside
/// `<defs>`, comes back as a clip and is not reported lost. Both report lists are
/// asserted empty, since a frame and one rect is the plainest drawing the writer
/// emits and `approximated` means "there and slightly wrong".
///
/// flip: narrowing `Gradients::collect`'s `name == "clipPath" || name == "mask"` to
/// `mask` → fails at the report assertion (`skipped: ["clip-path"]`), and behind it
/// at the mask-count assertion (0 against 1).
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
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        },
    ]))
    .expect("a frame with one rect in it");
    let res = Resolved::rebuild(&doc);
    let markup = svg_of(&doc, &res, &[ab]);

    // The fixture: the clip is written outside `<defs>`.
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

    // `svg_in` reads `clip-path` as a wrapper group holding a mask node.
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
