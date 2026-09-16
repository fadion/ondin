//! M2: the resolved layer — world transforms, bounds, spatial queries, and the
//! central invariant that incremental `update` equals a full `rebuild`.

use ondin_core::kurbo::Vec2;
use ondin_core::kurbo::{Affine, Point, Rect, RoundedRectRadii, Size};
use ondin_core::{
    Document, Effect, EffectKind, GeometryPatch, History, IdSource, NodeId, NodeKind, Operation,
    Resolved, Shadow, TextSizing, TextStyle, Transaction, hit_test, is_effectively_locked,
    nodes_in_view,
};

fn rect(w: f64, h: f64) -> NodeKind {
    NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
    }
}

fn text_style(size: f64) -> TextStyle {
    TextStyle {
        font_family: "Inter".into(),
        font_size: size,
        weight: 400,
        italic: false,
        line_height: Some(ondin_core::Length::Em(1.2)),
        ..Default::default()
    }
}

fn text(content: &str, size: f64) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(text_style(size)),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

fn artboard() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(800.0, 600.0),
    }
}

fn create(doc: &mut Document, id: NodeId, parent: NodeId, index: usize, kind: NodeKind, t: Affine) {
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: Some(t),
        name: None,
    }]))
    .unwrap();
}

#[test]
fn world_transform_composes_through_nested_groups() {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let group = ids.mint();
    let r = ids.mint();
    create(
        &mut doc,
        ab,
        root,
        0,
        artboard(),
        Affine::translate((100.0, 20.0)),
    );
    create(
        &mut doc,
        group,
        ab,
        0,
        NodeKind::Group,
        Affine::translate((5.0, 7.0)),
    );
    create(
        &mut doc,
        r,
        group,
        0,
        rect(50.0, 30.0),
        Affine::translate((3.0, 4.0)),
    );

    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_transform(r).unwrap(),
        Affine::translate((108.0, 31.0))
    );
    // Rect world bounds = transformed local box (no stroke).
    assert_eq!(
        res.world_bounds(r).unwrap(),
        Rect::new(108.0, 31.0, 158.0, 61.0)
    );
    // Group bounds = union of children = the rect's bounds.
    assert_eq!(
        res.world_bounds(group).unwrap(),
        res.world_bounds(r).unwrap()
    );
    // Artboard bounds = its own transformed frame.
    assert_eq!(
        res.world_bounds(ab).unwrap(),
        Rect::new(100.0, 20.0, 900.0, 620.0)
    );
}

#[test]
fn hit_test_is_topmost_first_and_respects_visibility() {
    let mut ids = IdSource::new(2);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let bottom = ids.mint();
    let top = ids.mint();
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(
        &mut doc,
        bottom,
        ab,
        0,
        rect(100.0, 100.0),
        Affine::IDENTITY,
    );
    create(&mut doc, top, ab, 1, rect(100.0, 100.0), Affine::IDENTITY);

    let res = Resolved::rebuild(&doc);
    // Both rects (and the artboard) cover (10,10); topmost first.
    let hits = hit_test(&res, &doc, Point::new(10.0, 10.0), 0.0);
    assert_eq!(hits.first(), Some(&top));
    assert_eq!(hits[1], bottom);
    assert!(hits.contains(&ab));

    // Hide the top rect: it drops out.
    doc.apply(&Transaction(vec![Operation::SetVisible {
        id: top,
        visible: false,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let hits = hit_test(&res, &doc, Point::new(10.0, 10.0), 0.0);
    assert!(!hits.contains(&top));
    assert_eq!(hits.first(), Some(&bottom));

    // Lock the bottom rect: it stops receiving clicks too.
    doc.apply(&Transaction(vec![Operation::SetLocked {
        id: bottom,
        locked: true,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let hits = hit_test(&res, &doc, Point::new(10.0, 10.0), 0.0);
    assert!(!hits.contains(&bottom));
}

#[test]
fn hit_test_uses_exact_geometry_not_bounds() {
    let mut ids = IdSource::new(3);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let ellipse = ids.mint();
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(
        &mut doc,
        ellipse,
        ab,
        0,
        NodeKind::Ellipse {
            size: Size::new(100.0, 100.0),
        },
        Affine::IDENTITY,
    );
    let res = Resolved::rebuild(&doc);

    // Center is inside the ellipse.
    assert!(hit_test(&res, &doc, Point::new(50.0, 50.0), 0.0).contains(&ellipse));
    // Corner (5,5) is inside the bounding box but OUTSIDE the ellipse.
    assert!(!hit_test(&res, &doc, Point::new(5.0, 5.0), 0.0).contains(&ellipse));
}

#[test]
fn nodes_in_view_matches_brute_force() {
    let mut ids = IdSource::new(4);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    // Scatter rects across the artboard.
    let mut placed = Vec::new();
    for i in 0..20 {
        let r = ids.mint();
        let x = (i as f64) * 30.0;
        create(
            &mut doc,
            r,
            ab,
            i,
            rect(20.0, 20.0),
            Affine::translate((x, 0.0)),
        );
        placed.push(r);
    }
    let res = Resolved::rebuild(&doc);

    let view = Rect::new(100.0, -10.0, 250.0, 50.0);
    let mut got = nodes_in_view(&res, view);
    got.sort();

    // Brute force over every node with known world bounds.
    let mut expected: Vec<NodeId> = std::iter::once(ab)
        .chain(placed.iter().copied())
        .filter(|id| {
            res.world_bounds(*id)
                .is_some_and(|b| b.intersect(view).area() >= 0.0 && overlaps(b, view))
        })
        .collect();
    expected.sort();

    assert_eq!(got, expected);
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.min_x() <= b.max_x()
        && a.max_x() >= b.min_x()
        && a.min_y() <= b.max_y()
        && a.max_y() >= b.min_y()
}

/// The central M2 invariant: after any sequence of operations, incrementally
/// `update`-ing a live `Resolved` yields the same world transforms and bounds as
/// a fresh `rebuild`. Driven by a deterministic PRNG over many random-but-valid
/// op sequences (a randomized differential test).
#[test]
fn incremental_update_equals_rebuild_over_random_ops() {
    for seed in 0..40u64 {
        run_random_session(seed);
    }
}

fn run_random_session(seed: u64) {
    let mut rng = Lcg::new(seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1));
    let mut ids = IdSource::new(0xF00D);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut hist = History::new();

    // Start with a couple of artboards so there are containers to nest under.
    let mut containers = vec![root]; // things that can legally parent shapes/groups (except root)
    for _ in 0..2 {
        let ab = ids.mint();
        hist.commit(
            &mut doc,
            Transaction(vec![Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: artboard(),
                transform: Some(rand_transform(&mut rng)),
                name: None,
            }]),
        )
        .unwrap();
        containers.push(ab);
    }

    let mut res = Resolved::rebuild(&doc);
    let mut movable: Vec<NodeId> = Vec::new(); // shapes/groups we created (never root/artboards)
    let mut texts: Vec<NodeId> = Vec::new(); // the Text subset, for content/style edits

    for _ in 0..60 {
        // Pick an operation. Bias toward creation early (movable may be empty).
        let choice = rng.next_range(10);
        let tx = match choice {
            // create a rect under a random container (artboard or group)
            0 | 1 => {
                let parent = *pick(&mut rng, &container_targets(&doc, &containers, &movable))
                    .unwrap_or(&containers[1]);
                let id = ids.mint();
                movable.push(id);
                Some(Transaction(vec![Operation::CreateNode {
                    id,
                    parent,
                    index: 0,
                    kind: rect(
                        10.0 + rng.next_range(90) as f64,
                        10.0 + rng.next_range(90) as f64,
                    ),
                    transform: Some(rand_transform(&mut rng)),
                    name: None,
                }]))
            }
            // create a group under a random container
            2 => {
                let parent = *pick(&mut rng, &container_targets(&doc, &containers, &movable))
                    .unwrap_or(&containers[1]);
                let id = ids.mint();
                movable.push(id);
                Some(Transaction(vec![Operation::CreateNode {
                    id,
                    parent,
                    index: 0,
                    kind: NodeKind::Group,
                    transform: Some(rand_transform(&mut rng)),
                    name: None,
                }]))
            }
            // move a random movable node
            3 => pick(&mut rng, &movable).map(|&id| {
                Transaction(vec![Operation::SetTransform {
                    id,
                    transform: rand_transform(&mut rng),
                }])
            }),
            // resize a random movable node (only affects rects here)
            4 => pick(&mut rng, &movable).map(|&id| {
                Transaction(vec![Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Size(Size::new(
                        10.0 + rng.next_range(120) as f64,
                        10.0 + rng.next_range(120) as f64,
                    )),
                }])
            }),
            // create a text node — its bounds come from the cached layout, so
            // this is what exercises the text half of update ≡ rebuild
            5 => {
                let parent = *pick(&mut rng, &container_targets(&doc, &containers, &movable))
                    .unwrap_or(&containers[1]);
                let id = ids.mint();
                movable.push(id);
                texts.push(id);
                Some(Transaction(vec![Operation::CreateNode {
                    id,
                    parent,
                    index: 0,
                    kind: text(
                        WORDS[rng.next_range(WORDS.len() as u64) as usize],
                        8.0 + rng.next_range(40) as f64,
                    ),
                    transform: Some(rand_transform(&mut rng)),
                    name: None,
                }]))
            }
            // re-shape an existing text node (content or style)
            6 => pick(&mut rng, &texts).map(|&id| {
                if rng.next_range(2) == 0 {
                    Transaction(vec![Operation::SetText {
                        id,
                        content: WORDS[rng.next_range(WORDS.len() as u64) as usize].to_string(),
                        spans: Default::default(),
                        para_spans: Default::default(),
                    }])
                } else {
                    Transaction(vec![Operation::SetTextStyle {
                        id,
                        style: text_style(8.0 + rng.next_range(60) as f64),
                        spans: None,
                    }])
                }
            }),
            // make a random movable node a mask, or release it. **The one op here
            // whose effect on a box travels sideways**: a mask does not change its
            // own bounds, it changes what its *siblings* lend their container. The
            // incremental pass measures children before parents and the full one
            // walks the child list in order, so the two arrive at that union by
            // different routes — which is exactly what this test is for.
            7 => pick(&mut rng, &movable).map(|&id| {
                Transaction(vec![Operation::SetMask {
                    id,
                    mask: rng.next_range(2) == 0,
                }])
            }),
            // put an effect stack on a random movable node, or take it off.
            //
            // **The second op here whose effect on a box travels, and it travels
            // the other way**: a mask changes what a node's *siblings* lend their
            // container, while an effect changes what the node itself lends every
            // ancestor above it. `ink_bounds` is unioned children-first in one
            // pass and returned up the recursion in the other, so the two reach
            // that answer by different routes — which is what this arm is for.
            //
            // Deliberately mixes a **rotation** into some stacks via the node's
            // own transform (`rand_transform` already rotates one time in three)
            // and a **spread**, so `effect::escaped`'s two spaces are exercised
            // rather than only its identity case.
            8 => pick(&mut rng, &movable).map(|&id| {
                let effects = match rng.next_range(4) {
                    0 => vec![],
                    1 => vec![Effect::new(EffectKind::LayerBlur {
                        radius: rng.next_range(40) as f64,
                    })],
                    2 => vec![Effect::new(EffectKind::DropShadow(Shadow {
                        offset: Vec2::new(
                            (rng.next_range(40) as f64) - 20.0,
                            (rng.next_range(40) as f64) - 20.0,
                        ),
                        blur: rng.next_range(30) as f64,
                        spread: (rng.next_range(20) as f64) - 10.0,
                        ..Shadow::default()
                    }))],
                    // Two of them, so the union-not-sum rule is exercised where
                    // the two passes could disagree about it.
                    _ => vec![
                        Effect::new(EffectKind::InnerShadow(Shadow::default())),
                        Effect {
                            kind: EffectKind::DropShadow(Shadow {
                                blur: rng.next_range(24) as f64,
                                ..Shadow::default()
                            }),
                            visible: rng.next_range(2) == 0,
                        },
                    ],
                };
                Transaction(vec![Operation::SetEffects { id, effects }])
            }),
            // delete a random movable node (and its subtree)
            _ => {
                pick(&mut rng, &movable).map(|&id| Transaction(vec![Operation::DeleteNode { id }]))
            }
        };

        let Some(tx) = tx else { continue };
        // SetGeometry::Size on a group is invalid; skip failures rather than panic.
        let Ok(dirty) = hist.commit(&mut doc, tx) else {
            continue;
        };
        res.update(&doc, &dirty);

        // Prune movable ids that no longer exist (deleted, possibly as part of a
        // deleted ancestor's subtree).
        movable.retain(|id| doc.contains(*id));
        texts.retain(|id| doc.contains(*id));

        // Differential check against a full rebuild.
        assert_resolved_matches_rebuild(&doc, &res, seed);
    }
}

fn container_targets(doc: &Document, containers: &[NodeId], movable: &[NodeId]) -> Vec<NodeId> {
    // Artboards plus any movable node that is a Group.
    let mut out: Vec<NodeId> = containers.iter().copied().skip(1).collect(); // skip root
    for id in movable {
        if matches!(doc.get(*id).map(|n| n.kind()), Some(NodeKind::Group)) {
            out.push(*id);
        }
    }
    out
}

fn assert_resolved_matches_rebuild(doc: &Document, live: &Resolved, seed: u64) {
    let fresh = Resolved::rebuild(doc);
    // Walk the whole tree from the root via the public API.
    let mut stack = vec![doc.root()];
    while let Some(id) = stack.pop() {
        assert_eq!(
            live.world_transform(id),
            fresh.world_transform(id),
            "world transform mismatch for {id:?} (seed {seed})"
        );
        assert_eq!(
            live.world_bounds(id),
            fresh.world_bounds(id),
            "world bounds mismatch for {id:?} (seed {seed})"
        );
        // The ink box is computed at both sites too, by different arithmetic —
        // one unions the children's cached ink, the other threads it back up the
        // recursion — so it needs its own line here or half of that pair is
        // unchecked.
        assert_eq!(
            live.ink_bounds(id),
            fresh.ink_bounds(id),
            "ink bounds mismatch for {id:?} (seed {seed})"
        );
        assert_eq!(
            live.text_layout(id),
            fresh.text_layout(id),
            "text layout mismatch for {id:?} (seed {seed})"
        );
        for c in doc.get(id).unwrap().children() {
            stack.push(*c);
        }
    }
}

fn pick<'a>(rng: &mut Lcg, items: &'a [NodeId]) -> Option<&'a NodeId> {
    if items.is_empty() {
        None
    } else {
        Some(&items[rng.next_range(items.len() as u64) as usize])
    }
}

fn rand_transform(rng: &mut Lcg) -> Affine {
    let tx = (rng.next_range(400) as f64) - 200.0;
    let ty = (rng.next_range(400) as f64) - 200.0;
    // Occasionally add a rotation to exercise non-commuting composition.
    if rng.next_range(3) == 0 {
        let angle = (rng.next_range(360) as f64).to_radians();
        Affine::translate((tx, ty)) * Affine::rotate(angle)
    } else {
        Affine::translate((tx, ty))
    }
}

/// Content pool for random text nodes — varied lengths and a multi-line case so
/// shaped widths and heights actually differ between edits.
const WORDS: &[&str] = &["a", "hello", "the quick brown fox", "two\nlines", ""];

/// Tiny deterministic PRNG (no external crate, no `Date::now`/`rand`).
struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed ^ 0xDEAD_BEEF_CAFE_F00D)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn next_range(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

/// The text cache (§5.9): `Resolved` shapes each `Text` node once and everything
/// downstream (bounds, hit-testing, the scene walk) reads that layout instead of
/// re-shaping. Editing content must refresh both the layout and the bounds.
#[test]
fn text_layout_is_cached_and_refreshed_on_edit() {
    let mut ids = IdSource::new(0x7E47);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let t = ids.mint();
    let r = ids.mint();
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, t, ab, 0, text("hi", 20.0), Affine::IDENTITY);
    create(&mut doc, r, ab, 1, rect(10.0, 10.0), Affine::IDENTITY);

    let mut res = Resolved::rebuild(&doc);

    // Cached for text, absent for everything else.
    let short = res.text_layout(t).expect("text node is shaped").clone();
    assert!(short.size.width > 0.0 && !short.runs.is_empty());
    assert!(res.text_layout(r).is_none(), "rects have no text layout");
    assert!(res.text_layout(ab).is_none());

    // Bounds are derived from that layout, not re-measured independently.
    let bounds = res.world_bounds(t).unwrap();
    assert!((bounds.width() - short.size.width).abs() < 1e-9);

    // An edit re-shapes and widens both the layout and the world bounds.
    let dirty = doc
        .apply(&Transaction(vec![Operation::SetText {
            id: t,
            content: "a considerably longer string".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]))
        .unwrap()
        .dirty;
    res.update(&doc, &dirty);
    let long = res.text_layout(t).expect("still shaped");
    assert!(
        long.size.width > short.size.width,
        "layout should grow: {} vs {}",
        long.size.width,
        short.size.width
    );
    assert!(res.world_bounds(t).unwrap().width() > bounds.width());

    // Deleting the node drops its cache entry.
    let dirty = doc
        .apply(&Transaction(vec![Operation::DeleteNode { id: t }]))
        .unwrap()
        .dirty;
    res.update(&doc, &dirty);
    assert!(res.text_layout(t).is_none(), "cache entry must be dropped");
}

/// `invalidate_text` is what the app calls when a font finishes loading. With
/// no new fonts registered it must be a no-op — same layouts, same bounds —
/// which is the property that makes it safe to call on every font arrival.
#[test]
fn invalidate_text_is_consistent_with_rebuild() {
    let mut ids = IdSource::new(0x1234);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let t1 = ids.mint();
    let t2 = ids.mint();
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, t1, ab, 0, text("first", 12.0), Affine::IDENTITY);
    create(
        &mut doc,
        t2,
        ab,
        1,
        text("second line", 30.0),
        Affine::translate((40.0, 40.0)),
    );

    let mut res = Resolved::rebuild(&doc);
    res.invalidate_text(&doc);

    let fresh = Resolved::rebuild(&doc);
    for id in [root, ab, t1, t2] {
        assert_eq!(res.text_layout(id), fresh.text_layout(id), "{id:?}");
        assert_eq!(res.world_bounds(id), fresh.world_bounds(id), "{id:?}");
    }
}

/// Bounds must follow stroke alignment, not assume centred.
///
/// It is not cosmetic. Bounds drive artboard clipping, viewport culling and the
/// selection rectangle, so an outside stroke reported as centred gets its outer
/// half clipped by its own frame and culled a frame early at the viewport edge.
#[test]
fn stroke_expansion_follows_alignment() {
    use ondin_core::{Fill, Paint, Stroke, StrokeAlign, geometry};

    let stroke = |align| Paint {
        fills: Vec::<Fill>::new(),
        strokes: vec![Stroke {
            brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::BLACK),
            width: 10.0,
            join: ondin_core::kurbo::Join::Miter,
            cap: ondin_core::kurbo::Cap::Butt,
            dashes: Vec::new(),
            dash_offset: 0.0,
            sides: Default::default(),
            miter_limit: 4.0,
            dash_fit: false,
            align,
            visible: true,
        }],
    };

    assert_eq!(
        geometry::stroke_expansion(&rect(10.0, 10.0), &stroke(StrokeAlign::Center)),
        5.0
    );
    assert_eq!(
        geometry::stroke_expansion(&rect(10.0, 10.0), &stroke(StrokeAlign::Inside)),
        0.0
    );
    assert_eq!(
        geometry::stroke_expansion(&rect(10.0, 10.0), &stroke(StrokeAlign::Outside)),
        10.0
    );
}

/// Alignment needs a closed outline to mean anything. Lines and open paths get
/// centred strokes, and the renderer, the SVG writer and the inspector all ask
/// this one function so they cannot disagree about which shapes qualify.
#[test]
fn stroke_alignment_only_applies_to_closed_shapes() {
    use ondin_core::geometry::stroke_align_applies;
    use ondin_core::kurbo::BezPath;

    assert!(stroke_align_applies(&rect(10.0, 10.0)));
    // An ellipse's path is four cubics with no explicit `ClosePath`; it still
    // has an inside.
    assert!(stroke_align_applies(&NodeKind::Ellipse {
        size: Size::new(10.0, 10.0)
    }));
    assert!(!stroke_align_applies(&NodeKind::Line {
        end: Point::new(10.0, 0.0)
    }));

    let mut open = BezPath::new();
    open.move_to(Point::ZERO);
    open.line_to(Point::new(10.0, 10.0));
    assert!(!stroke_align_applies(&NodeKind::Path {
        path: open.clone(),
        corner_radii: Vec::new(),
    }));

    let mut closed = open;
    closed.close_path();
    assert!(stroke_align_applies(&NodeKind::Path {
        path: closed,
        corner_radii: Vec::new()
    }));
}

// --- transform origin -------------------------------------------------------

/// `pivot_for` picks the variant and `pivot_point` reads it back, so the two have
/// to be exact inverses — otherwise dropping the marker on a corner stores
/// something that resolves a fraction away from it.
#[test]
fn a_pivot_round_trips_through_the_variant_its_kind_gets() {
    use ondin_core::NodeKind;
    use ondin_core::geometry::{pivot_for, pivot_point};
    use ondin_core::kurbo::{Point, Rect, Size};

    let authored = NodeKind::Rect {
        size: Size::new(80.0, 40.0),
        corner_radii: Default::default(),
    };
    let emergent = NodeKind::Group;
    // A group's box need not start at the origin — it is its children's union —
    // which is exactly the case a fraction would get wrong.
    let offset_box = Rect::new(-20.0, 10.0, 60.0, 50.0);

    for (kind, box_) in [
        (&authored, Rect::new(0.0, 0.0, 80.0, 40.0)),
        (&emergent, offset_box),
    ] {
        for at in [
            box_.origin(),
            Point::new(box_.max_x(), box_.max_y()),
            box_.center(),
            Point::new(box_.x0 + box_.width() * 0.25, box_.y0),
        ] {
            let stored = pivot_for(kind, box_, at);
            let back = pivot_point(Some(stored), box_);
            assert!(
                (back.x - at.x).abs() < 1e-9 && (back.y - at.y).abs() < 1e-9,
                "{kind:?} at {at:?} came back as {back:?} via {stored:?}"
            );
        }
    }
}

/// **Normalized where the box is authored, absolute where it is emergent.** A
/// rectangle's extent is a field, so a pivot two thirds along it stays two thirds
/// along it when the field changes. A group's is its children's union, so a
/// fraction there would slide the pivot every time a child moved.
#[test]
fn an_authored_box_keeps_the_pivots_fraction_and_an_emergent_one_its_point() {
    use ondin_core::geometry::{pivot_for, pivot_point};
    use ondin_core::kurbo::{Point, Rect, Size};
    use ondin_core::{NodeKind, Pivot, TextSizing};

    let rect = NodeKind::Rect {
        size: Size::new(80.0, 40.0),
        corner_radii: Default::default(),
    };
    let before = Rect::new(0.0, 0.0, 80.0, 40.0);
    // Right edge, vertically centred.
    let stored = pivot_for(&rect, before, Point::new(80.0, 20.0));
    assert!(matches!(stored, Pivot::Normalized(_)), "{stored:?}");
    // Widened to 200: the pivot is still on the right edge.
    let after = Rect::new(0.0, 0.0, 200.0, 40.0);
    assert_eq!(pivot_point(Some(stored), after), Point::new(200.0, 20.0));

    // A group's pivot is a point, so growing its box leaves it exactly where it is.
    let stored = pivot_for(&NodeKind::Group, before, Point::new(80.0, 20.0));
    assert!(matches!(stored, Pivot::Local(_)), "{stored:?}");
    assert_eq!(pivot_point(Some(stored), after), Point::new(80.0, 20.0));

    // Auto-sized text has no authored extent yet, so it lands in the emergent
    // half — a later content edit must not drag its pivot around with the wrapping.
    let auto = NodeKind::Text {
        content: "hi".into(),
        style: Box::new(ondin_core::TextStyle {
            font_family: "Inter".into(),
            font_size: 12.0,
            weight: 400,
            italic: false,
            line_height: Some(ondin_core::Length::Em(1.2)),
            ..Default::default()
        }),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    };
    assert!(matches!(
        pivot_for(&auto, before, Point::new(10.0, 10.0)),
        Pivot::Local(_)
    ));

    // And `None` is the box's centre — the fixed point every gesture used before
    // the field existed, which is what makes an untouched document behave as it did.
    assert_eq!(pivot_point(None, after), after.center());
}

/// **The centre is the absence of a pivot, not a pivot in the middle.** Both doors
/// — the canvas marker dragged back to the middle and the inspector's origin row
/// typed to 50/50 — go through `pivot_placed_at` for exactly this, because the two
/// states are not interchangeable: `None` is what `build::flip` reads and what the
/// canvas stops drawing a badge for, and a stored half resolves to the same point
/// while behaving differently everywhere that asks whether one was placed.
#[test]
fn the_centre_of_the_box_is_the_absence_of_a_pivot() {
    use ondin_core::NodeKind;
    use ondin_core::geometry::{pivot_placed_at, pivot_point};
    use ondin_core::kurbo::{Point, Rect, Size};

    let rect = NodeKind::Rect {
        size: Size::new(80.0, 40.0),
        corner_radii: Default::default(),
    };
    // Offset, so "the centre" cannot be confused with the local origin.
    let box_ = Rect::new(-20.0, 10.0, 60.0, 50.0);
    assert_eq!(pivot_placed_at(&rect, box_, box_.center()), None);
    // A hair off it is a placement, and it resolves back to where it was asked for.
    let just_off = Point::new(box_.center().x + 0.01, box_.center().y);
    let placed = pivot_placed_at(&rect, box_, just_off).expect("a placement");
    let back = pivot_point(Some(placed), box_);
    assert!(
        (back.x - just_off.x).abs() < 1e-9 && (back.y - just_off.y).abs() < 1e-9,
        "{just_off:?} came back as {back:?}"
    );
}

/// A box with no extent on an axis has **one** position on it — its minimum, which
/// is also its centre — so the other axis decides on its own whether the origin has
/// been placed.
///
/// This is the case the rule used to get wrong. `canvas::pivot_at` accumulated a
/// `centred` flag by asking whether each axis had snapped to its *middle* third; a
/// zero extent puts all three candidates on the same coordinate and the loop
/// reports the first, which is labelled `0.0`. So a line drawn flat — anything
/// drawn with Shift held — could never have its origin dragged back to nothing.
#[test]
fn a_degenerate_axis_is_centred_by_itself() {
    use ondin_core::geometry::pivot_placed_at;
    use ondin_core::kurbo::{Point, Rect};
    use ondin_core::{NodeKind, Pivot};

    let line = NodeKind::Line {
        end: Point::new(100.0, 0.0),
    };
    let flat = Rect::new(0.0, 0.0, 100.0, 0.0);
    assert_eq!(flat.center(), Point::new(50.0, 0.0));
    // Halfway along the live axis, anywhere at all on the dead one: cleared.
    assert_eq!(pivot_placed_at(&line, flat, Point::new(50.0, 0.0)), None);
    // Off-centre along the live axis: a placement, and an absolute one, a line
    // having no authored box for a fraction to be measured against.
    assert!(matches!(
        pivot_placed_at(&line, flat, Point::new(10.0, 0.0)),
        Some(Pivot::Local(_))
    ));
}

// --- per-side strokes, dashes and mitre ------------------------------------

/// A square-cornered rect's four sides are its four edges, each running the full
/// span with no corner arc to share.
///
/// The measurement that matters is that they **tile the outline**: four sides
/// whose lengths sum to the perimeter and whose ends meet are a border, and four
/// that overlap or leave gaps are the "some weird lines" bug in the making.
#[test]
fn the_four_sides_of_a_square_rect_tile_its_outline() {
    use ondin_core::Side;
    use ondin_core::geometry::side_path;
    use ondin_core::kurbo::{PathEl, Shape};

    let kind = rect(40.0, 20.0);
    let ends = |side| {
        let p = side_path(&kind, side).expect("a rect has sides");
        let start = match p.elements().first() {
            Some(PathEl::MoveTo(p)) => *p,
            other => panic!("a side must open with a MoveTo, got {other:?}"),
        };
        (start, p.perimeter(0.01))
    };
    let (top, top_len) = ends(Side::Top);
    let (right, right_len) = ends(Side::Right);
    let (bottom, bottom_len) = ends(Side::Bottom);
    let (left, left_len) = ends(Side::Left);
    assert_eq!(top, Point::new(0.0, 0.0));
    assert_eq!(right, Point::new(40.0, 0.0));
    assert_eq!(bottom, Point::new(40.0, 20.0));
    assert_eq!(left, Point::new(0.0, 20.0));
    assert!((top_len - 40.0).abs() < 1e-9, "top {top_len}");
    assert!((right_len - 20.0).abs() < 1e-9, "right {right_len}");
    assert!((bottom_len - 40.0).abs() < 1e-9, "bottom {bottom_len}");
    assert!((left_len - 20.0).abs() < 1e-9, "left {left_len}");
    let total = top_len + right_len + bottom_len + left_len;
    assert!(
        (total - 120.0).abs() < 1e-9,
        "sides sum to {total}, want 120"
    );
}

/// A rounded corner belongs **half to each of the sides that share it**, which is
/// what stops a rounded rect's border showing four gaps where its corners are.
///
/// Pinned by arithmetic rather than by eye: the top side has to start at the
/// top-left arc's 45 degree point, not at the end of the straight edge, and the
/// four sides still have to sum to the rounded perimeter.
#[test]
fn a_rounded_corner_is_split_between_the_two_sides_that_share_it() {
    use ondin_core::Side;
    use ondin_core::geometry::{local_path, side_path};
    use ondin_core::kurbo::{PathEl, Shape};

    const R: f64 = 8.0;
    let kind = NodeKind::Rect {
        size: Size::new(40.0, 20.0),
        corner_radii: RoundedRectRadii::from_single_radius(R),
    };
    let top = side_path(&kind, Side::Top).expect("a rect has sides");
    let Some(PathEl::MoveTo(start)) = top.elements().first().copied() else {
        panic!("a side must open with a MoveTo");
    };
    // The 45 degree point of the top-left arc: centre (R,R) plus R at 225.
    let off = R * std::f64::consts::FRAC_1_SQRT_2;
    let want = Point::new(R - off, R - off);
    assert!(
        (start - want).hypot() < 1e-6,
        "top starts at {start:?}, want the corner's midpoint {want:?}"
    );
    assert!(
        start.x > 0.0 && start.x < R,
        "the top side has to reach into the corner, not stop at the straight edge"
    );

    let sum: f64 = Side::ALL
        .iter()
        .map(|s| side_path(&kind, *s).expect("sides").perimeter(0.001))
        .sum();
    let whole = local_path(&kind).expect("outline").perimeter(0.001);
    assert!(
        (sum - whole).abs() < 0.05,
        "four sides measure {sum} against an outline of {whole}"
    );
}

/// Only a box has four nameable sides, so only the two boxes answer with them — a
/// rect and, since frames stroke (§15 D144), a frame.
#[test]
fn only_a_box_takes_a_per_side_stroke() {
    use ondin_core::Side;
    use ondin_core::geometry::{side_path, stroke_sides_apply};
    use ondin_core::kurbo::{BezPath, Shape};

    assert!(stroke_sides_apply(&rect(10.0, 10.0)));
    // The widening §15 D68 held open until a frame's stroke actually painted. A
    // frame has no radii, so its sides are four straight edges: the four together
    // tile its outline, which is what `the_four_sides_of_a_square_rect_tile_its_outline`
    // asserts for the square-cornered rect they share a code path with.
    let frame = NodeKind::Artboard {
        size: Size::new(10.0, 20.0),
    };
    assert!(stroke_sides_apply(&frame));
    let sides: f64 = Side::ALL
        .iter()
        .map(|s| {
            side_path(&frame, *s)
                .expect("a frame has four sides")
                .perimeter(0.001)
        })
        .sum();
    assert!(
        (sides - 60.0).abs() < 1e-6,
        "the four sides tile a 10×20 frame's 60 of perimeter, got {sides}"
    );

    for kind in [
        NodeKind::Ellipse {
            size: Size::new(10.0, 10.0),
        },
        NodeKind::Polygon {
            size: Size::new(10.0, 10.0),
            sides: 3,
        },
        NodeKind::Line {
            end: Point::new(10.0, 0.0),
        },
        NodeKind::Path {
            path: BezPath::new(),
            corner_radii: Vec::new(),
        },
        NodeKind::Group,
        // Text is the one worth naming: its outline is its glyphs', which have no
        // four edges between them however boxy the paragraph looks.
        text("hi", 12.0),
    ] {
        assert!(!stroke_sides_apply(&kind), "{kind:?} has no nameable sides");
        assert!(side_path(&kind, Side::Top).is_none(), "{kind:?}");
    }
}

/// `StrokeSides` decides which sides carry paint and how thick each is. `All` is
/// deliberately *not* four full-width sides: it is the one case that keeps the
/// closed outline, so its corners still join.
#[test]
fn stroke_sides_report_a_width_per_side() {
    use ondin_core::{Side, StrokeSides};

    assert_eq!(
        StrokeSides::All.per_side(3.0),
        None,
        "All keeps the outline"
    );
    assert_eq!(StrokeSides::Top.per_side(3.0), Some(vec![(Side::Top, 3.0)]));
    assert_eq!(
        StrokeSides::Custom {
            top: 1.0,
            right: 0.0,
            bottom: 4.0,
            left: 2.0,
        }
        .per_side(9.0),
        // A side of 0 is no stroke at all, and the nominal width is ignored.
        Some(vec![
            (Side::Top, 1.0),
            (Side::Bottom, 4.0),
            (Side::Left, 2.0)
        ])
    );
    // Bounds allow for the thickest side, and for nothing when every side is 0.
    assert_eq!(StrokeSides::All.max_width(3.0), 3.0);
    assert_eq!(StrokeSides::Left.max_width(3.0), 3.0);
    assert_eq!(
        StrokeSides::Custom {
            top: 1.0,
            right: 0.0,
            bottom: 4.0,
            left: 2.0,
        }
        .max_width(9.0),
        4.0
    );
    assert_eq!(
        StrokeSides::Custom {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
        .max_width(9.0),
        0.0
    );
    // Switching to Custom starts from whatever the stroke already drew, so
    // opening the row changes nothing on canvas.
    assert_eq!(
        StrokeSides::All.as_custom(2.0),
        StrokeSides::Custom {
            top: 2.0,
            right: 2.0,
            bottom: 2.0,
            left: 2.0
        }
    );
    assert_eq!(
        StrokeSides::Right.as_custom(2.0),
        StrokeSides::Custom {
            top: 0.0,
            right: 2.0,
            bottom: 0.0,
            left: 0.0
        }
    );
}

/// The dash classification has to be **invariant under width**. Its predecessor
/// compared the first dash against `width * 1.5`, so a typed 12-long dash on a
/// 9px stroke came back as *dotted* and lit the wrong cell in the panel.
#[test]
fn the_dash_style_does_not_depend_on_the_stroke_width() {
    use ondin_core::{DashStyle, Stroke};

    let with = |dashes: Vec<f64>, width: f64| Stroke {
        dashes,
        width,
        ..Default::default()
    };
    assert_eq!(with(vec![], 1.0).dash_style(), DashStyle::Solid);
    // A dot is a zero-length dash, at any width.
    for width in [0.5, 1.0, 9.0, 40.0] {
        assert_eq!(
            with(vec![0.0, 6.0], width).dash_style(),
            DashStyle::Dotted,
            "width {width}"
        );
        assert_eq!(
            with(vec![12.0, 4.0], width).dash_style(),
            DashStyle::Dashed,
            "width {width}: a 12-long dash is a dash whatever the stroke weighs"
        );
    }
    assert_eq!(
        with(vec![5.0, 10.0, 15.0, 5.0], 1.0).dash_style(),
        DashStyle::Custom
    );
    // And the numbers read back out the way the fields put them in.
    assert_eq!(with(vec![0.0, 6.0], 1.0).dot_spacing(), 6.0);
    assert_eq!(
        with(vec![12.0, 4.0], 1.0).dash_length_spacing(),
        (12.0, 4.0)
    );
}

/// A zero-length dash draws **nothing** with a butt or square cap, so the dots
/// would vanish. The substitution is a square dot one width long with the gap
/// shortened to match, which keeps the *period* and so keeps the spacing field
/// meaning centre-to-centre under all three caps.
#[test]
fn a_dotted_stroke_survives_a_cap_that_cannot_draw_a_zero_length_dash() {
    use ondin_core::Stroke;
    use ondin_core::geometry::{local_path, resolved_dashes};
    use ondin_core::kurbo::Cap;

    let path = local_path(&rect(40.0, 20.0)).expect("outline");
    let dotted = |cap| Stroke {
        dashes: vec![0.0, 6.0],
        width: 2.0,
        cap,
        ..Default::default()
    };
    // Round can draw a dot of no length at all, and keeps one.
    assert_eq!(
        resolved_dashes(&dotted(Cap::Round), 2.0, &path),
        vec![0.0, 6.0]
    );
    for cap in [Cap::Butt, Cap::Square] {
        let got = resolved_dashes(&dotted(cap), 2.0, &path);
        assert_eq!(got, vec![2.0, 4.0], "{cap:?}");
        let period: f64 = got.iter().sum();
        assert!(
            (period - 6.0).abs() < 1e-9,
            "{cap:?}: the period moved to {period}, so 'spacing' stopped meaning \
             centre-to-centre"
        );
    }
    // A solid stroke is left solid whatever the cap.
    assert!(
        resolved_dashes(
            &Stroke {
                width: 2.0,
                ..Default::default()
            },
            2.0,
            &path
        )
        .is_empty()
    );
}

/// Fit to corners scales the pattern so a whole number of periods spans the
/// outline, which is what puts ink on the corners rather than a gap.
///
/// A flag honoured at the render boundary rather than a value written back into
/// the fields: this is the assertion that it survives a resize, since the same
/// stroke fits a different rect to a different pattern.
#[test]
fn fitting_a_dash_pattern_divides_the_outline_a_whole_number_of_times() {
    use ondin_core::Stroke;
    use ondin_core::geometry::{fitted_dashes, local_path, resolved_dashes};

    // Perimeter 100. A period of 7 does not divide it, so the fit nudges it.
    let path = local_path(&rect(30.0, 20.0)).expect("outline");
    let stroke = Stroke {
        dashes: vec![5.0, 2.0],
        width: 1.0,
        dash_fit: true,
        ..Default::default()
    };
    let got = resolved_dashes(&stroke, 1.0, &path);
    let period: f64 = got.iter().sum();
    let n = 100.0 / period;
    assert!(
        (n - n.round()).abs() < 1e-9,
        "{got:?} has period {period}, which fits {n} times into 100"
    );
    assert_eq!(n.round(), 14.0, "nearest whole count to 100/7");
    // The ratio between dash and gap is preserved: only the scale moves.
    assert!((got[0] / got[1] - 2.5).abs() < 1e-9, "{got:?}");

    // The same stroke on a bigger rect fits differently, which is exactly why
    // the flag is honoured at render time instead of written back into the fields.
    let bigger = local_path(&rect(60.0, 40.0)).expect("outline");
    assert_ne!(resolved_dashes(&stroke, 1.0, &bigger), got);

    // Degenerate inputs answer rather than divide by zero.
    assert_eq!(fitted_dashes(&[0.0, 0.0], 100.0), vec![0.0, 0.0]);
    assert_eq!(fitted_dashes(&[5.0], 0.0), vec![5.0]);
}

/// Bounds have to allow for the widest side, and for nothing at all when every
/// side is zero: a stroke that paints nowhere must not inflate the box.
#[test]
fn stroke_expansion_follows_the_sides_a_stroke_actually_paints() {
    use ondin_core::geometry::stroke_expansion;
    use ondin_core::{Fill, Paint, Stroke, StrokeAlign, StrokeSides};

    let paint = |sides| Paint {
        fills: Vec::<Fill>::new(),
        strokes: vec![Stroke {
            width: 10.0,
            align: StrokeAlign::Outside,
            sides,
            ..Default::default()
        }],
    };
    assert_eq!(
        stroke_expansion(&rect(10.0, 10.0), &paint(StrokeSides::All)),
        10.0
    );
    assert_eq!(
        stroke_expansion(&rect(10.0, 10.0), &paint(StrokeSides::Top)),
        10.0
    );
    assert_eq!(
        stroke_expansion(
            &rect(10.0, 10.0),
            &paint(StrokeSides::Custom {
                top: 3.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            })
        ),
        3.0,
        "the box grows by the thickest side, not by the nominal width"
    );
    assert_eq!(
        stroke_expansion(
            &rect(10.0, 10.0),
            &paint(StrokeSides::Custom {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            })
        ),
        0.0,
        "a stroke with no side to paint must not widen the bounds"
    );
}

/// The mitre field reads in degrees and stores a ratio. 28.96 degrees is exactly
/// ratio 4 (kurbo's default and SVG's), which is the number the panel has to come
/// back to, and the pair has to round-trip or scrubbing the field would drift it.
#[test]
fn the_mitre_angle_and_its_ratio_convert_both_ways() {
    use ondin_core::{miter_degrees_of_ratio, miter_ratio_of_degrees};

    let default_degrees = miter_degrees_of_ratio(4.0);
    assert!(
        (default_degrees - 28.955).abs() < 0.01,
        "ratio 4 is {default_degrees} degrees, want ~28.96"
    );
    assert!((miter_ratio_of_degrees(default_degrees) - 4.0).abs() < 1e-9);
    for degrees in [1.0, 15.0, 28.955_024_5, 60.0, 90.0, 179.0, 180.0] {
        let back = miter_degrees_of_ratio(miter_ratio_of_degrees(degrees));
        assert!((back - degrees).abs() < 1e-6, "{degrees} -> {back}");
    }
    // 180 degrees is ratio 1 (always bevel) and is the meaningful top end.
    assert!((miter_ratio_of_degrees(180.0) - 1.0).abs() < 1e-12);
    // A rectangle's 90 degree corner is ratio sqrt(2), so the default never bites
    // on one; only a sharp point such as a low-inner-ratio star reaches ratio 4.
    assert!((miter_ratio_of_degrees(90.0) - 2.0_f64.sqrt()).abs() < 1e-12);
    // Out of range clamps rather than running to infinity as the angle -> 0.
    assert!(miter_ratio_of_degrees(0.0).is_finite());
    assert!(miter_ratio_of_degrees(-5.0).is_finite());
}

/// A pill and a circle are rects whose radii are **clamped** — `from_rect` cuts
/// each to half the shorter side — so their "sides" are pure arcs with no straight
/// edge between them. They still have to tile the outline and still have to be real
/// paths: a degenerate side here would show up on the most ordinary shape in any
/// design system, a rounded button.
#[test]
fn sides_survive_a_clamped_radius_and_a_collapsed_box() {
    use ondin_core::Side;
    use ondin_core::geometry::{local_path, side_path};
    use ondin_core::kurbo::{RoundedRectRadii, Shape};

    for (w, h, r) in [
        (20.0, 20.0, 40.0),  // a circle: radius clamped from 40 to 10
        (100.0, 10.0, 50.0), // a pill: clamped to 5
        (1.0, 1.0, 0.5),     // the smallest circle the UI can make
    ] {
        let kind = NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::from_single_radius(r),
        };
        let sides: Vec<_> = Side::ALL
            .iter()
            .map(|s| side_path(&kind, *s).expect("a rect has sides"))
            .collect();
        let sum: f64 = sides.iter().map(|p| p.perimeter(0.001)).sum();
        let whole = local_path(&kind).expect("outline").perimeter(0.001);
        assert!(
            (sum - whole).abs() < 0.05,
            "{w}x{h} r{r}: four sides measure {sum} against an outline of {whole}"
        );
        for (side, path) in Side::ALL.iter().zip(&sides) {
            assert!(
                path.segments().count() > 0,
                "{w}x{h} r{r}: the {side:?} side drew nothing"
            );
        }
    }

    // A box collapsed to nothing answers rather than panicking or dividing by zero —
    // a shape can pass through 0×0 while a handle is being dragged.
    let flat = NodeKind::Rect {
        size: Size::new(0.0, 0.0),
        corner_radii: RoundedRectRadii::from_single_radius(4.0),
    };
    for side in Side::ALL {
        let path = side_path(&flat, side).expect("still a rect");
        assert_eq!(path.perimeter(0.001), 0.0, "{side:?}");
    }
}

/// **Bounds have to allow for the stroke the walk will actually draw.**
///
/// `stroke_expansion` is the one bounds consumer that reads `sides`, and it has to
/// read it the way the scene walk does — through `effective_sides`. Reading the
/// stored field instead was a way for the two to disagree: on a kind with no
/// nameable sides, `Custom { 0, 0, 0, 0 }` reported *no* reach while the walk
/// stroked the whole outline at full width. Ink outside its own reported bounds is
/// a stroke clipped by its own frame and culled a frame early at the viewport edge.
///
/// Stated as the pairing, because either half alone passes with the bug in place.
#[test]
fn bounds_allow_for_a_stroke_the_walk_will_actually_draw() {
    use ondin_core::geometry::{effective_sides, stroke_expansion};
    use ondin_core::{Fill, Paint, Stroke, StrokeAlign, StrokeSides};

    // Every side switched off — the one setting that reaches nowhere at all.
    let nowhere = Stroke {
        width: 10.0,
        align: StrokeAlign::Outside,
        sides: StrokeSides::Custom {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        },
        ..Default::default()
    };
    let paint = Paint {
        fills: Vec::<Fill>::new(),
        strokes: vec![nowhere.clone()],
    };

    // On a rect the field is honoured, so nothing is painted and nothing is
    // allowed for. The two agree.
    let rect = rect(50.0, 50.0);
    assert_eq!(effective_sides(&rect, &nowhere), nowhere.sides);
    assert_eq!(stroke_expansion(&rect, &paint), 0.0);

    // On an ellipse the field cannot be honoured, so the walk strokes the whole
    // outline at full width — and the bounds have to say so.
    let ellipse = NodeKind::Ellipse {
        size: Size::new(50.0, 50.0),
    };
    assert_eq!(
        effective_sides(&ellipse, &nowhere),
        StrokeSides::All,
        "a kind with no nameable sides draws its whole outline"
    );
    assert_eq!(
        stroke_expansion(&ellipse, &paint),
        10.0,
        "the bounds reported no reach for a stroke the walk paints at full width"
    );
}

// --- booleans -------------------------------------------------------------
//
// A boolean's outline is the second thing `Resolved` derives rather than reads
// (after shaped text), and the first that depends on a whole *subtree*. These
// cover the two properties that follow from that: the box is measured from the
// result rather than from the operands, and editing an operand re-derives it.

/// Root → boolean(op) → two 100x100 rects overlapping over a 50x100 band.
fn boolean_pair(op: ondin_core::BoolOp) -> (Document, IdSource, NodeId, NodeId, NodeId) {
    let mut ids = IdSource::new(0xB001);
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
            kind: rect(100.0, 100.0),
            transform: Some(Affine::translate((50.0, 0.0))),
            name: None,
        },
    ]))
    .expect("build the boolean");
    (doc, ids, b, a1, a2)
}

/// **The box is the result's, not the operands'.** A union spans both rects; an
/// intersection spans only the overlap — which is the whole reason bounds cannot
/// be the child union a group uses, and is what the selection rect, the snap
/// targets and the R-tree all read.
#[test]
fn a_booleans_bounds_come_from_its_result_not_its_operands() {
    let (doc, _, b, _, _) = boolean_pair(ondin_core::BoolOp::Union);
    let res = Resolved::rebuild(&doc);
    let union = res.world_bounds(b).expect("a union has bounds");
    assert!(
        (union.width() - 150.0).abs() < 0.5 && (union.height() - 100.0).abs() < 0.5,
        "union spans both rects: {union:?}"
    );

    let (doc, _, b, _, _) = boolean_pair(ondin_core::BoolOp::Intersect);
    let res = Resolved::rebuild(&doc);
    let overlap = res.world_bounds(b).expect("an intersection has bounds");
    assert!(
        (overlap.width() - 50.0).abs() < 0.5 && (overlap.height() - 100.0).abs() < 0.5,
        "intersection spans only the overlapping band: {overlap:?}"
    );
    assert!(
        res.boolean_path(b).is_some(),
        "and the outline itself is cached"
    );
    assert!(
        res.boolean_path(doc.root()).is_none(),
        "nothing else gets an entry"
    );
}

/// **Non-destructive: editing an operand re-derives the result.** This is the
/// property the whole container exists for — and it runs through the *incremental*
/// path, which is what a drag uses, not through a rebuild.
#[test]
fn moving_an_operand_re_evaluates_the_boolean_above_it() {
    let (mut doc, _, b, _, a2) = boolean_pair(ondin_core::BoolOp::Intersect);
    let mut res = Resolved::rebuild(&doc);
    let before = res.world_bounds(b).expect("bounds");

    // Slide the second rect right by 25: the overlap narrows from 50 to 25.
    let tx = Transaction(vec![Operation::SetTransform {
        id: a2,
        transform: Affine::translate((75.0, 0.0)),
    }]);
    let outcome = doc.apply(&tx).expect("applies");
    res.update(&doc, &outcome.dirty);

    let after = res.world_bounds(b).expect("bounds");
    assert!(
        (before.width() - 50.0).abs() < 0.5 && (after.width() - 25.0).abs() < 0.5,
        "the intersection narrowed with its operand: {before:?} then {after:?}"
    );
}

/// The invariant this file exists for, extended to the new cache: an incremental
/// `update` after an operand edit must leave the same state a `rebuild` would —
/// including through a **nested** boolean, where the inner result is an operand of
/// the outer one and the evaluation order is what makes it work.
#[test]
fn update_equals_rebuild_for_a_nested_boolean() {
    let (mut doc, mut ids, inner, _, a2) = boolean_pair(ondin_core::BoolOp::Union);
    // Wrap the existing boolean in another one, alongside a third rect.
    let (outer, a3) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: outer,
            parent: doc.root(),
            index: 1,
            kind: NodeKind::Boolean {
                op: ondin_core::BoolOp::Subtract,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a3,
            parent: outer,
            index: 0,
            kind: rect(200.0, 40.0),
            transform: None,
            name: None,
        },
        Operation::Reparent {
            id: inner,
            new_parent: outer,
            index: 1,
        },
    ]))
    .expect("nest the boolean");

    let mut res = Resolved::rebuild(&doc);
    let tx = Transaction(vec![Operation::SetTransform {
        id: a2,
        transform: Affine::translate((10.0, 10.0)),
    }]);
    let outcome = doc.apply(&tx).expect("applies");
    res.update(&doc, &outcome.dirty);

    let fresh = Resolved::rebuild(&doc);
    for id in [outer, inner, a2, a3] {
        assert_eq!(
            res.world_bounds(id).map(|b| format!("{b:?}")),
            fresh.world_bounds(id).map(|b| format!("{b:?}")),
            "bounds diverged for {id:?}"
        );
        assert_eq!(
            res.boolean_path(id).map(|p| p.to_svg()),
            fresh.boolean_path(id).map(|p| p.to_svg()),
            "outline diverged for {id:?} — the incremental pass and the rebuild \
             must agree, which is what the children-first ordering buys"
        );
    }
}

/// **The box a boolean reports is its result's, in its own local space.**
///
/// `local_box` is what the selection outline, the dimensions badge and every resize
/// handle are placed from, and for a container it unions the children — which for a
/// boolean measures the shapes the operation *consumed*. Reported from a screenshot:
/// a subtraction's outline reached out to wherever the subtracted circle ended, and
/// the badge read that rectangle rather than the visible shape.
#[test]
fn a_booleans_local_box_is_its_result_not_its_operands() {
    // A 100x100 square with a 100x100 square subtracted from its right half: the
    // result is the left 50x100, but the operands span 150 wide.
    let (doc, _, b, _, _) = boolean_pair(ondin_core::BoolOp::Subtract);
    let res = Resolved::rebuild(&doc);

    let boxed = ondin_core::local_box(&doc, &res, b).expect("a boolean has a box");
    assert!(
        (boxed.width() - 50.0).abs() < 0.5,
        "the result is 50 wide, not the operands' 150: {boxed:?}"
    );
    assert!((boxed.height() - 100.0).abs() < 0.5, "{boxed:?}");

    // A union, where the two answers coincide — so the test above cannot pass by
    // accidentally measuring something else.
    let (doc, _, b, _, _) = boolean_pair(ondin_core::BoolOp::Union);
    let res = Resolved::rebuild(&doc);
    let boxed = ondin_core::local_box(&doc, &res, b).unwrap();
    assert!((boxed.width() - 150.0).abs() < 0.5, "{boxed:?}");
}

/// **Every kind is grabbable a few units either side of its ink, and no further.**
///
/// Reported twice, and the second report is what widened it. First: "there's a very
/// narrow window to click and select a line with a thin stroke" — a line has no interior
/// to aim at, so its whole target is the width of its own ink. Then: "selecting a shape
/// is a bit tricky, I need to move the mouse within the stroke" — which is the same
/// complaint about a *closed* shape with no fill, where the visible ink is the outline
/// and the interior is empty air nobody aims at.
///
/// `slop` is the allowance either way; the caller sets it from a screen distance and the
/// zoom, so the target is the same size under the pointer at any magnification.
///
/// **The half worth guarding is now the bound, not the exemption.** The old version
/// asserted a filled shape grew *no* halo at all; what has to be true instead is that the
/// halo is a few units and stops — a shape shadowing everything near it would be worse
/// than the problem either report described.
#[test]
fn a_picking_allowance_widens_every_kind_by_a_bounded_band() {
    let mut ids = IdSource::new(0x51F);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (line, box_) = (ids.mint(), ids.mint());
    create(
        &mut doc,
        line,
        root,
        0,
        NodeKind::Line {
            end: Point::new(100.0, 0.0),
        },
        Affine::IDENTITY,
    );
    create(
        &mut doc,
        box_,
        root,
        1,
        rect(40.0, 40.0),
        Affine::translate((200.0, 0.0)),
    );
    let res = Resolved::rebuild(&doc);

    // 3 units off a line with no stroke at all: missed exactly, hit with an allowance.
    let off = Point::new(50.0, 3.0);
    assert!(
        !hit_test(&res, &doc, off, 0.0).contains(&line),
        "the exact test still means exact"
    );
    assert!(
        hit_test(&res, &doc, off, 4.0).contains(&line),
        "an allowance of 4 reaches 3 units off the segment"
    );
    assert!(
        !hit_test(&res, &doc, Point::new(50.0, 9.0), 4.0).contains(&line),
        "…and not 9, so the band is bounded rather than a whole-bbox test"
    );
    // Off the end, not just off the side: the distance is to the *segment*.
    assert!(
        !hit_test(&res, &doc, Point::new(130.0, 0.0), 4.0).contains(&line),
        "past the endpoint is a miss"
    );

    // **A closed shape spends the allowance too, and this used to assert the
    // opposite.** It read "3 units outside a rect is still outside it — only open
    // shapes spend the allowance", which was the rule until a shape with a stroke
    // and no fill showed why it was wrong: the ink you can see is the outline, so
    // the outline is where you aim, and a pixel past it hit nothing. The band is a
    // superset of the old test, so what has to stay pinned is that it is a *band*
    // and not a halo — the far assertion below is the one carrying that now.
    assert!(
        hit_test(&res, &doc, Point::new(210.0, 10.0), 4.0).contains(&box_),
        "inside is inside"
    );
    assert!(
        hit_test(&res, &doc, Point::new(197.0, 10.0), 4.0).contains(&box_),
        "3 units outside the edge is within a 4-unit allowance"
    );
    assert!(
        !hit_test(&res, &doc, Point::new(191.0, 10.0), 4.0).contains(&box_),
        "9 units outside is not: a few units of comfort, not a halo"
    );
    assert!(
        !hit_test(&res, &doc, Point::new(197.0, 10.0), 0.0).contains(&box_),
        "and asking for no allowance still means the exact interior"
    );

    // **Text spends it too, and did not until §15 D483** (`[S4.2-L2-03]`). D115
    // rewrote every arm of `contains_local` to add the tolerance and its own
    // enumeration of what it had rewritten — Rect/Artboard, Ellipse,
    // Polygon/Star/Path — never names Text, so the sweep skipped the kind users
    // click most and this test, which D115 renamed for exactly this rule, had no
    // Text case to notice with. Added here rather than in a test of its own so
    // that "every kind" is asserted in the one place that claims it.
    let words = ids.mint();
    create(
        &mut doc,
        words,
        root,
        2,
        text("Hi", 16.0),
        Affine::translate((0.0, 200.0)),
    );
    let res = Resolved::rebuild(&doc);
    let b = ondin_core::local_box(&doc, &res, words).expect("the text has a box");
    let just_outside = Point::new(b.x1 + 3.0, b.y0 + 200.0 + b.height() / 2.0);
    let well_outside = Point::new(b.x1 + 9.0, b.y0 + 200.0 + b.height() / 2.0);
    assert!(
        hit_test(&res, &doc, just_outside, 4.0).contains(&words),
        "3 units past a text node's box is within a 4-unit allowance: {b:?}"
    );
    assert!(
        !hit_test(&res, &doc, well_outside, 4.0).contains(&words),
        "…and 9 is not, so text gets the same bounded band as everything else"
    );
    assert!(
        !hit_test(&res, &doc, just_outside, 0.0).contains(&words),
        "and no allowance still means the box"
    );
}

/// **A boolean's box corner and its transform's translation are two different points**,
/// which is why the inspector reports the former.
///
/// `build::boolean` creates its container with no transform of its own and leaves the
/// operands where they were, so the translation is the *parent's* origin however far across
/// the page the artwork sits. The panel used to read the translation, and reported X: 0
/// Y: 0 for a boolean anywhere in a frame — "the moment I apply a boolean to shapes, its
/// resulting coordinates reset to X: 0 and Y: 0", with the operands inside still correct
/// because a shape's local origin *is* its box corner.
///
/// This pins the cause rather than the panel (which needs a GPU context to run): the two
/// points differ for a boolean and agree for a rect, so a reader can see which one the
/// field has to use.
#[test]
fn a_booleans_box_corner_is_not_its_transforms_origin() {
    let mut ids = IdSource::new(0x0B0);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    // Two overlapping squares well away from the frame's origin.
    create(
        &mut doc,
        a,
        ab,
        0,
        rect(80.0, 80.0),
        Affine::translate((150.0, 90.0)),
    );
    create(
        &mut doc,
        b,
        ab,
        1,
        rect(80.0, 80.0),
        Affine::translate((190.0, 90.0)),
    );

    let (tx, node) =
        ondin_core::build::boolean(&doc, &mut ids, &[a, b], ondin_core::BoolOp::Union, None)
            .unwrap();
    doc.apply(&tx).unwrap();
    let res = Resolved::rebuild(&doc);
    let world = res.world_transform(node).unwrap();

    let translation = world * Point::ZERO;
    assert_eq!(
        (translation.x, translation.y),
        (0.0, 0.0),
        "the container has no transform of its own — this is the number that used to be shown"
    );
    let local = ondin_core::local_box(&doc, &res, node).expect("a derived box");
    let corner = world * Point::new(local.x0, local.y0);
    assert_eq!(
        (corner.x, corner.y),
        (150.0, 90.0),
        "and this is where the artwork actually starts"
    );

    // A rect is the case that made the old reading look correct: for anything with an
    // authored size the box starts at the local origin, so the two agree exactly.
    let rect_local = ondin_core::local_box(&doc, &res, a).expect("a box");
    assert_eq!((rect_local.x0, rect_local.y0), (0.0, 0.0));
    let _ = res.world_transform(a).unwrap();
}

/// **A fitted dash pattern is fitted per subpath, and that is the assertion.**
///
/// A fit scale is a property of one closed run: the point of `dash_fit` is that ink
/// lands where the shape turns, and two subpaths of different lengths need two
/// different scales to each land on their own corners. Fitting the total instead —
/// which is what this replaced — hands both subpaths one pattern, so the shorter one
/// drifts by up to half a period.
///
/// The two rings below have perimeters 100 and 40 against a period of 7, so the
/// per-subpath answer is *two different* patterns fitting 14 and 6 times; the
/// whole-path answer is one pattern fitting 20 times into 140. That the two disagree
/// is the whole content of the change.
#[test]
fn a_fitted_dash_is_fitted_to_each_subpath_it_is_laid_along() {
    use ondin_core::Stroke;
    use ondin_core::geometry::{dash_fit_pieces, resolved_dashes};
    use ondin_core::kurbo::Shape;

    let mut path = Rect::new(0.0, 0.0, 30.0, 20.0).to_path(0.01);
    path.extend(Rect::new(100.0, 0.0, 115.0, 5.0).to_path(0.01).iter());
    let stroke = Stroke {
        dashes: vec![5.0, 2.0],
        width: 1.0,
        dash_fit: true,
        ..Default::default()
    };

    let pieces = dash_fit_pieces(&stroke, 1.0, &path).expect("two subpaths, fitted");
    assert_eq!(pieces.len(), 2, "one piece per subpath");
    let periods: Vec<f64> = pieces
        .iter()
        .map(|(_, dashes)| dashes.iter().sum())
        .collect();
    for (i, (sub, _)) in pieces.iter().enumerate() {
        let n = sub.perimeter(0.001) / periods[i];
        assert!(
            (n - n.round()).abs() < 1e-9,
            "subpath {i} has perimeter {} against period {}: {n} periods",
            sub.perimeter(0.001),
            periods[i]
        );
    }
    assert_eq!(
        (periods[0] * 14.0).round(),
        100.0,
        "the 100-perimeter ring takes 14 periods"
    );
    assert_eq!(
        (periods[1] * 6.0).round(),
        40.0,
        "the 40-perimeter one takes 6, which is a different scale"
    );
    assert!(
        (periods[0] - periods[1]).abs() > 0.1,
        "the two patterns must differ — one fitted to the total ({periods:?}) is \
         exactly the approximation this replaces"
    );

    // And the ratio inside each is untouched: only the scale moves.
    for (_, dashes) in &pieces {
        assert!((dashes[0] / dashes[1] - 2.5).abs() < 1e-9, "{dashes:?}");
    }

    // Nothing else splits. A single-subpath path, an unfitted pattern and a solid
    // stroke all answer `None`, so the common case clones no path and measures no
    // perimeter twice.
    let one = Rect::new(0.0, 0.0, 30.0, 20.0).to_path(0.01);
    assert!(dash_fit_pieces(&stroke, 1.0, &one).is_none(), "one subpath");
    assert!(
        dash_fit_pieces(
            &Stroke {
                dash_fit: false,
                ..stroke.clone()
            },
            1.0,
            &path
        )
        .is_none(),
        "not fitted"
    );
    assert!(
        dash_fit_pieces(
            &Stroke {
                dashes: Vec::new(),
                ..stroke.clone()
            },
            1.0,
            &path
        )
        .is_none(),
        "solid"
    );
    // The single-pattern route still answers for the whole path, which is what the
    // pieces are compared against above.
    assert!(!resolved_dashes(&stroke, 1.0, &path).is_empty());
}

/// **A text node's outline is its glyphs', mirrored about the baseline.**
///
/// The sign is the thing worth pinning, because getting it wrong compiles and draws
/// a legible-looking wrong picture: font outlines are y-up and text-local space is
/// y-down, so each point is `(g.x + x, g.y − y)`. Flip it and every glyph reflects
/// about its own baseline — the letters stay in reading order, so it reads as a font
/// bug rather than as a sign error.
///
/// "Inter" has no descender, so its outline must sit above the baseline — bar the
/// **optical overshoot** of its round letters, which is a real font fact and not
/// slack in the test: the `e`'s bowl measures 0.47 units below the baseline at 40pt,
/// which is 1.2% of the em, and the allowance below is 2%. A flip is nowhere near
/// that: it would put the lowest ink a cap height *below* the baseline (66 against
/// 38) and the highest half a unit above it, so both bounds here reject it.
#[test]
fn a_text_outline_sits_above_its_baseline_and_closes() {
    use ondin_core::kurbo::{PathEl, Shape};

    let mut ids = IdSource::new(4242);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let t = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: t,
        parent: root,
        index: 0,
        kind: text("Inter", 40.0),
        transform: None,
        name: None,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let layout = res
        .text_layout(t)
        .expect("Inter is bundled, so this shapes");
    let baseline = f64::from(layout.runs[0].glyphs[0].y);
    assert!(
        baseline > 0.0,
        "the baseline sits below the node's own origin"
    );

    let outline = ondin_core::text::outline(layout);
    let box_ = outline.bounding_box();
    assert!(!outline.is_empty(), "five letters leave some ink");
    let overshoot = 40.0 * 0.02;
    assert!(
        box_.y1 <= baseline + overshoot,
        "no part of 'Inter' descends past its round letters' overshoot: its lowest \
         ink is at {} against a baseline at {baseline} — anything approaching a cap \
         height below is the mirror applied the wrong way",
        box_.y1
    );
    assert!(
        box_.y0 < baseline - 10.0,
        "and it must reach up: cap height at 40pt is far more than 10 units, got a \
         top of {}",
        box_.y0
    );
    // Within the node's own box, horizontally — the glyphs are placed by the layout
    // and this only translates them.
    assert!(
        box_.x0 >= -1.0 && box_.x1 <= layout.size.width + 1.0,
        "{box_:?} against a box {}, wide",
        layout.size.width
    );

    // Closed contours throughout, which is what `stroke_align_applies` promises for
    // text without measuring — inside and outside alignment need an interior.
    assert!(matches!(outline.elements().last(), Some(PathEl::ClosePath)));
    let moves = outline
        .elements()
        .iter()
        .filter(|el| matches!(el, PathEl::MoveTo(_)))
        .count();
    let closes = outline
        .elements()
        .iter()
        .filter(|el| matches!(el, PathEl::ClosePath))
        .count();
    assert_eq!(
        moves, closes,
        "every contour opened is a contour closed ({moves} moves, {closes} closes)"
    );
    assert!(
        moves >= 5,
        "at least one contour per letter, plus the counters in 'e': got {moves}"
    );
}

/// Alignment is offered on text and per-side widths are not, and both answers are
/// about the same fact: a text node's outline is its glyphs'. Every contour of it is
/// closed, so inside and outside mean something — outlined type is `Outside` on a
/// text node — while five letters have no top edge between them.
#[test]
fn text_takes_a_stroke_alignment_but_not_a_side() {
    use ondin_core::geometry::{local_path, stroke_align_applies, stroke_sides_apply};

    let kind = text("hi", 12.0);
    assert!(
        stroke_align_applies(&kind),
        "answered by fiat, like a boolean's: `local_path` cannot build the outline"
    );
    assert!(local_path(&kind).is_none(), "and this is why");
    assert!(!stroke_sides_apply(&kind));
}

/// Root → artboard → group → [mask, masked], the mask 50×50 and the layer over
/// it 500×400, both at the origin. Everything is at identity, so world space and
/// the group's local space are the same numbers and an assertion reads as the
/// picture.
fn mask_fixture() -> (Document, NodeId, NodeId, NodeId) {
    let mut ids = IdSource::new(0x5A5A);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, group, mask, masked) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, group, ab, 0, NodeKind::Group, Affine::IDENTITY);
    // Bottom-first: a mask clips what is drawn *above* it, so it is child 0.
    create(&mut doc, mask, group, 0, rect(50.0, 50.0), Affine::IDENTITY);
    create(
        &mut doc,
        masked,
        group,
        1,
        rect(500.0, 400.0),
        Affine::IDENTITY,
    );
    (doc, group, mask, masked)
}

/// A mask bounds its **container** and leaves the layer it masks alone.
///
/// The split is the decision, not an accident of where the code went: the box a
/// container reports is what you *see* of it, so zooming to a mask group frames
/// the picture rather than the photograph behind it — while the masked layer
/// keeps the box you drag it about by, which would otherwise shrink to the
/// visible sliver and leave nothing to aim at.
#[test]
fn a_mask_bounds_its_container_and_not_the_layer_it_masks() {
    let (mut doc, group, mask, masked) = mask_fixture();
    let big = Rect::new(0.0, 0.0, 500.0, 400.0);
    assert_eq!(
        Resolved::rebuild(&doc).world_bounds(group),
        Some(big),
        "fixture: until the flag is set the group is the big layer's size"
    );

    doc.apply(&Transaction(vec![Operation::SetMask {
        id: mask,
        mask: true,
    }]))
    .unwrap();

    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(group),
        Some(Rect::new(0.0, 0.0, 50.0, 50.0)),
        "the group stops at what the mask lets through"
    );
    assert_eq!(
        res.world_bounds(masked),
        Some(big),
        "and the masked layer keeps its own box, which is what a drag moves"
    );
    assert_eq!(
        ondin_core::local_box(&doc, &res, group),
        Some(Rect::new(0.0, 0.0, 50.0, 50.0)),
        "the local box agrees, or the handles would be drawn off the outline"
    );
}

/// **A mask larger than what it passes lends its container nothing.**
///
/// `[S4.1-L2-04]`, §15 D494. `mask_fixture`'s mask is *smaller* than the layer
/// beneath it, so `mask ∪ (layer ∩ mask)` is the mask's own square either way and
/// `a_mask_bounds_its_container_and_not_the_layer_it_masks` holds whether or not
/// the mask's rectangle is unioned into its container. **Turn the fixture round**
/// — a 1000×1000 mask over a 100×100 picture — and the two answers separate. The
/// mask passes all of the picture, so the group shows a 100-unit square; it used
/// to report a box ten times that side, framing emptiness on *zoom to selection*,
/// catching a marquee 900 units from anything drawn, and reporting W 1000 H 1000
/// in the inspector.
///
/// A mask contributes the **clip**, not an area, so its own box is not a
/// contribution at all — which is what §5.9 says in words: *"A masked layer keeps
/// its own world bounds; its container gets only what the mask lets through."*
///
/// All four unions are asserted, because they are four separate arms and each is
/// reached by a different caller: `resolve_subtree`'s box and ink (the full
/// `rebuild`), and `recompute_bounds`' box and ink (the incremental `update`).
/// The `update` assertion is what catches a fix applied to one pass and not the
/// other — the shape `a_mask_group_that_masks_internally_still_clips_with_its_whole_union`
/// records below, where the incremental flip left `rebuild` right.
///
/// **Five flips run, one per arm, and every predicted site was right:**
///
/// - `resolve_subtree`'s box → *"the group is what the mask lets through"*,
///   1000×1000 against 100×100.
/// - `resolve_subtree`'s ink → *"and so is the ink the walk culls on"*.
/// - `recompute_bounds`' box → **`rebuild` stays right**, so it fails on the
///   `update` assertion. Without that assertion this flip would not have bitten.
/// - `recompute_bounds`' ink → the same, on the `update` assertion's ink twin,
///   which had **no message** until the flip showed what a bare `assert_eq!`
///   reports when it fires.
/// - `query::local_box` → *"and the local box"*.
#[test]
fn a_mask_larger_than_what_it_passes_lends_its_container_nothing() {
    let mut ids = IdSource::new(0x5A5A);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, group, mask, masked) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, group, ab, 0, NodeKind::Group, Affine::IDENTITY);
    // Bottom-first, as in `mask_fixture`: a mask clips what is drawn above it.
    create(
        &mut doc,
        mask,
        group,
        0,
        rect(1000.0, 1000.0),
        Affine::IDENTITY,
    );
    create(
        &mut doc,
        masked,
        group,
        1,
        rect(100.0, 100.0),
        Affine::IDENTITY,
    );

    let small = Rect::new(0.0, 0.0, 100.0, 100.0);
    let big = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    assert_eq!(
        Resolved::rebuild(&doc).world_bounds(group),
        Some(big),
        "fixture: until the flag is set the group really is the big rectangle"
    );

    doc.apply(&Transaction(vec![Operation::SetMask {
        id: mask,
        mask: true,
    }]))
    .unwrap();

    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(group),
        Some(small),
        "the group is what the mask lets through, not the mask"
    );
    assert_eq!(
        res.ink_bounds(group),
        Some(small),
        "and so is the ink the walk culls on"
    );
    assert_eq!(
        ondin_core::local_box(&doc, &res, group),
        Some(small),
        "and the local box, or the handles are drawn round emptiness"
    );
    assert_eq!(
        res.world_bounds(masked),
        Some(small),
        "the masked layer keeps its own box, which is what a drag moves"
    );

    let mut inc = Resolved::rebuild(&doc);
    inc.update(&doc, &ondin_core::DirtySet([mask].into_iter().collect()));
    assert_eq!(
        inc.world_bounds(group),
        res.world_bounds(group),
        "the incremental pass agrees with the full one, or an edit inside the \
         mask group silently changes the container's box"
    );
    assert_eq!(
        inc.ink_bounds(group),
        res.ink_bounds(group),
        "and so does its ink, which is the box the walk prunes a subtree on"
    );

    // **The consequence, asserted rather than discovered.** A group holding
    // nothing but a mask now has no box at all — it draws nothing, a mask never
    // being painted, so *zoom to selection* and the marquee have nothing to
    // measure and that is the honest answer. It is the one behaviour change
    // outside the too-large case, and it is here so a later reader meets it as a
    // decision.
    doc.apply(&Transaction(vec![Operation::DeleteNode { id: masked }]))
        .unwrap();
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(group),
        None,
        "a group whose only child is a mask draws nothing and measures nothing"
    );
    assert_eq!(
        ondin_core::local_box(&doc, &res, group),
        None,
        "and the local box says the same, or the handles disagree with the outline"
    );
}

/// **One layer whose extent is not a number no longer poisons the document's**
/// (`[S4.2-L1-01]`, §15 D495).
///
/// ⚠️ **`SetTransform` accepts this transform and is right to.** D421 guards the
/// operation with `build::affine_is_finite`, and every coefficient of
/// `scale(1e308)` **is** finite — the overflow happens later, in `transform_rect`'s
/// corner multiply. So this is not a hole in D421's guard; it is the third shape
/// of the same class, where a check on the *operands* cannot see what the
/// *derived* value does. It is what `[S4.2-L1-01]` names and why the guard had to
/// go at the bounds rather than at the op.
///
/// The failure it prevents is the whole session's, not one layer's: an infinite
/// box unions all the way to the root, so `world_bounds(root)` — which
/// `plan::raster_size` and `plan::raster_opts` read (§15 D334) — was infinite, and
/// the spatial index answered a query 5,000 units away from anything drawn.
///
/// **Reachable without hand-editing a file**: `transform="scale(1e308)"` in an
/// imported SVG arrives as exactly this affine.
///
/// The untouched sibling is the in-fixture control: if the guard were backwards it
/// would take that one out of the index too, and the assertion would say so.
#[test]
fn a_layer_whose_box_overflows_is_left_out_rather_than_spread_up_the_tree() {
    let mut ids = IdSource::new(0x1E30);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, group, big, small) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, group, ab, 0, NodeKind::Group, Affine::IDENTITY);
    create(
        &mut doc,
        big,
        group,
        0,
        rect(100.0, 100.0),
        Affine::IDENTITY,
    );
    create(
        &mut doc,
        small,
        group,
        1,
        rect(10.0, 10.0),
        Affine::IDENTITY,
    );

    let overflowing = Affine::new([1e308, 0.0, 0.0, 1e308, 0.0, 0.0]);
    assert!(
        ondin_core::build::affine_is_finite(overflowing),
        "the fixture's own claim: D421's guard accepts this, which is the finding"
    );
    doc.apply(&Transaction(vec![Operation::SetTransform {
        id: big,
        transform: overflowing,
    }]))
    .unwrap();

    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(big),
        None,
        "the layer itself has no measurable extent"
    );
    assert_eq!(
        res.world_bounds(small),
        Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
        "control: the untouched sibling is unaffected"
    );
    assert_eq!(
        res.world_bounds(group),
        res.world_bounds(small),
        "so the container is the sibling, not infinity"
    );
    for id in [group, ab, root] {
        assert!(
            res.world_bounds(id)
                .is_none_or(|b| b.x1.is_finite() && b.y1.is_finite()),
            "no ancestor's box is infinite, and the root's is what the export plan reads"
        );
    }

    // The cull, which is the consequence a user meets: an empty region far from
    // the artwork used to answer with the overflowing layer in it.
    let far = Rect::new(5000.0, 5000.0, 5100.0, 5100.0);
    assert!(
        !nodes_in_view(&res, far).contains(&big),
        "and the spatial index does not answer with it 5,000 units from anything"
    );

    // `update` agrees with `rebuild` — asserted because the two passes were wrong
    // *together* before this, which is why `resolve.rs`' own differential test was
    // structurally blind to the defect.
    let mut inc = Resolved::rebuild(&doc);
    inc.update(&doc, &ondin_core::DirtySet([big].into_iter().collect()));
    assert_eq!(inc.world_bounds(group), res.world_bounds(group));
}

/// **A mask group's box is the box it clips with, and it used to be a tenth of
/// it.**
///
/// `[S4.1-L2-01]`, §15 D460. `Resolved::mask_path`'s `Group | Root` arm skips a
/// mask *inside* a mask group and unions the rest **un-narrowed** — deliberately,
/// since the exact answer needs a path intersection and *"a clip that is too
/// generous"* is the safe direction. The three bounds computations did the
/// opposite: they applied the inner mask. So the outline that clips kept 400×400
/// and the box that culls kept 40×40, and nothing reconciled them.
///
/// ⚠️ **The consequence is a layer that is on screen and clickable and is not
/// painted.** `scene::paint_node` sets `bounds_cover_subtree` for a `Group` and
/// returns early when `ink_bounds` misses the view, so `G`'s whole subtree was
/// pruned on a box a tenth of the clip its own children draw under. Measured
/// through the real walk with a recording painter: 1 fill with the page in view,
/// **0** scrolled onto the artwork, 1 for the same group with no inner mask.
/// Everything else reading the container's box was wrong by the same 10× — *zoom
/// to selection*, the selection outline, the marquee, the inspector's W/H, and
/// `ondin_export::plan`'s exported region.
///
/// ⚠️ **Two `Use as mask` commands on shapes a user assembled by hand.**
/// `NodeKind::can_mask` names `Group` explicitly, and a mask built inside a group
/// is the ordinary way to build one. No hostile input and no import.
///
/// ⚠️ **The differential test did not catch it and could not**, which is worth
/// recording beside a fix that changes both passes:
/// `incremental_update_equals_rebuild_over_random_ops` already generates
/// `SetMask`, and **both passes were wrong in the same way** — so it guards this
/// change and was never going to find the defect.
///
/// ⚠️ **Flipped at each pass separately, and the predicted site was wrong.** The
/// guess was that both flips would fail on `world_bounds(G)`; they fail in
/// different places, and the difference is the reason both had to change:
///
/// - **`resolve_subtree` reverted** — the full pass, which is all `rebuild` runs
///   for a tree with no booleans in it: fails on `world_bounds(G)` at `40×40`
///   against `400×400`, and the render half fails too, at 0 fills against 1.
/// - **`recompute_bounds` reverted** — the incremental pass: `rebuild` is
///   **still right**, so `world_bounds(G)` passes and the render half stays
///   green; it fails on the `update` assertion four lines down. Without that
///   assertion this flip would not have bitten at all, and `update` would have
///   silently disagreed with `rebuild` the first time anything in the mask group
///   was edited.
///
/// The **control** — the same tree with the inner mask flag off, where every box
/// has always agreed — stays green under both.
#[test]
fn a_mask_group_that_masks_internally_still_clips_with_its_whole_union() {
    let mut ids = IdSource::new(0x5A5A);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, g, m, inner_mask, inner_big, outer_big) = (
        ids.mint(),
        ids.mint(),
        ids.mint(),
        ids.mint(),
        ids.mint(),
        ids.mint(),
    );
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, g, ab, 0, NodeKind::Group, Affine::IDENTITY);
    // Bottom-first inside `G`: the mask group, then the artwork it masks.
    create(&mut doc, m, g, 0, NodeKind::Group, Affine::IDENTITY);
    create(
        &mut doc,
        outer_big,
        g,
        1,
        rect(400.0, 400.0),
        Affine::IDENTITY,
    );
    // And inside `M`, the same shape one level down.
    create(
        &mut doc,
        inner_mask,
        m,
        0,
        rect(40.0, 40.0),
        Affine::IDENTITY,
    );
    create(
        &mut doc,
        inner_big,
        m,
        1,
        rect(400.0, 400.0),
        Affine::IDENTITY,
    );
    doc.apply(&Transaction(vec![
        Operation::SetMask { id: m, mask: true },
        Operation::SetMask {
            id: inner_mask,
            mask: true,
        },
    ]))
    .unwrap();

    let res = Resolved::rebuild(&doc);
    let whole = Rect::new(0.0, 0.0, 400.0, 400.0);

    // The fixture is in the state this test is about: the clip really is the
    // whole union, which is what makes the boxes below wrong rather than merely
    // different.
    assert_eq!(
        res.mask_path(&doc, m)
            .map(|p| <kurbo::BezPath as kurbo::Shape>::bounding_box(&p)),
        Some(whole),
        "fixture: `mask_path` unions the mask group's non-mask children un-narrowed"
    );

    assert_eq!(
        res.world_bounds(g),
        Some(whole),
        "the container's box must be the box its mask clips with"
    );
    assert_eq!(
        res.ink_bounds(g),
        Some(whole),
        "and so must the ink box, which is the one the scene walk culls a whole \
         subtree on"
    );
    assert_eq!(
        ondin_core::local_box(&doc, &res, g),
        Some(whole),
        "and the local box, which is where the handles and the snap are drawn from"
    );

    // `update` must agree with `rebuild`, since the fix changes both passes.
    let mut inc = Resolved::rebuild(&doc);
    inc.update(&doc, &ondin_core::DirtySet([m].into_iter().collect()));
    assert_eq!(
        inc.world_bounds(g),
        res.world_bounds(g),
        "the incremental pass takes the same correction"
    );

    // **The control**: with the inner flag off, `M` is an ordinary group and
    // every box has always agreed — so nothing here has widened a box that was
    // right.
    doc.apply(&Transaction(vec![Operation::SetMask {
        id: inner_mask,
        mask: false,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    assert_eq!(res.world_bounds(g), Some(whole));
    assert_eq!(
        res.world_bounds(m),
        Some(whole),
        "and the mask group's own box is the union it always was"
    );
}

/// Masking answers the hit test at both ends: the mask itself is never hit, and
/// what it masks is hit only where the mask lets it through.
///
/// Two assertions rather than one because two different pieces of code answer
/// them, and each passes the other's case. Dropping the mask's own exclusion
/// leaves an invisible layer on top of everything it clips; dropping the point
/// test leaves the clipped-away half of a photograph clickable over empty canvas.
#[test]
fn a_mask_is_unclickable_and_what_it_masks_is_clickable_only_inside_it() {
    let (mut doc, _group, mask, masked) = mask_fixture();
    doc.apply(&Transaction(vec![Operation::SetMask {
        id: mask,
        mask: true,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);

    let inside = hit_test(&res, &doc, Point::new(25.0, 25.0), 0.0);
    assert!(
        inside.contains(&masked),
        "inside the mask the layer is there to be picked: {inside:?}"
    );
    assert!(
        !inside.contains(&mask),
        "and the mask is not, having drawn nothing to aim at"
    );

    let outside = hit_test(&res, &doc, Point::new(300.0, 200.0), 0.0);
    assert!(
        !outside.contains(&masked),
        "outside it the ink is gone, so the click passes through: {outside:?}"
    );
}

/// A **text** layer masks with its glyphs.
///
/// This is the case §15 D191 named as one of the two the fill model cannot
/// cover, and it is the arm a boolean operand deliberately does not have — so it
/// is also the one that would quietly fall through to `geometry::local_path`,
/// which answers `None` for text and would leave the mask clipping nothing at
/// all. Hence both assertions: that there is a path, and that it is the *ink*
/// rather than the line box it sits in.
#[test]
fn a_text_mask_clips_to_its_glyphs_rather_than_to_its_box() {
    let mut ids = IdSource::new(0x7E47);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, t) = (ids.mint(), ids.mint());
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, t, ab, 0, text("Mask", 64.0), Affine::IDENTITY);
    doc.apply(&Transaction(vec![Operation::SetMask { id: t, mask: true }]))
        .unwrap();

    let res = Resolved::rebuild(&doc);
    let path = res
        .mask_path(&doc, t)
        .expect("a text layer masks with its glyph outlines");
    assert!(!path.is_empty(), "and they are actually in it");
    let ink = ondin_core::kurbo::Shape::bounding_box(&path);
    let box_ = ondin_core::local_box(&doc, &res, t).expect("the text layer has a box");
    assert!(
        ink.height() < box_.height(),
        "glyph ink sits inside the line box it was laid out in: {ink:?} vs {box_:?}"
    );
}

/// **Nothing masks inside a boolean**, and the hit test is the only place that
/// can tell.
///
/// The scene walk stops at the container and draws the result, so a mask flag on
/// an operand changes nothing about how the boolean *draws* whatever
/// `governing_mask` answers — which is exactly why the bug this pins is
/// invisible to every rendering assertion in the suite. The hit test does reach
/// the operands (`is_indexed` includes a `Rect`, and a click on one selects the
/// boolean above it), so without the boolean arm the far operand goes
/// unclickable and the boolean can only be picked over half of itself.
#[test]
fn a_mask_among_a_booleans_operands_does_not_clip_them() {
    let mut ids = IdSource::new(0xB001);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, a, ab, 0, rect(40.0, 40.0), Affine::IDENTITY);
    create(
        &mut doc,
        b,
        ab,
        1,
        rect(40.0, 40.0),
        Affine::translate((200.0, 0.0)),
    );
    let mut res = Resolved::rebuild(&doc);
    let (tx, boolean) =
        ondin_core::build::boolean(&doc, &mut ids, &[a, b], ondin_core::BoolOp::Union, None)
            .expect("two rects union");
    let dirty = doc.apply(&tx).expect("the boolean applies").dirty;
    res.update(&doc, &dirty);
    assert_eq!(
        doc.get(boolean).unwrap().children().len(),
        2,
        "fixture: both rects are operands of the boolean"
    );

    // The lower operand claims to be a mask. It is 40×40 at the origin; the other
    // sits 200 to the right, well outside it.
    let lower = doc.get(boolean).unwrap().children()[0];
    doc.apply(&Transaction(vec![Operation::SetMask {
        id: lower,
        mask: true,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);

    assert_eq!(
        doc.governing_mask(doc.get(boolean).unwrap().children()[1]),
        None,
        "an operand is not in a run, so nothing there governs it"
    );
    let hits = hit_test(&res, &doc, Point::new(220.0, 20.0), 0.0);
    assert!(
        hits.contains(&boolean),
        "and the boolean is still clickable over the far half of itself: {hits:?}"
    );
}

/// A **group** masks with the union of what is inside it, not with a
/// concatenation of it.
///
/// **The two members are wound in opposite directions, and that is the whole
/// design of this test.** Appending subpaths and filling the result non-zero —
/// the obvious cheap version — merges two members that happen to wind the same
/// way, so a fixture of two plain rectangles passes it and proves nothing.
/// Checked: concatenation passes that fixture. Wound against each other the
/// overlap sums to zero and the cheap version punches a hole exactly where the
/// two shapes cross, which is the failure `boolean::operand_of` documents for
/// this arm. A user's own paths carry whatever winding they were drawn in, so
/// this is the ordinary case rather than a contrived one.
#[test]
fn a_group_masks_with_the_union_of_its_contents() {
    let mut ids = IdSource::new(0x6209);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, group, a, b) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, group, ab, 0, NodeKind::Group, Affine::IDENTITY);
    create(&mut doc, a, group, 0, rect(60.0, 60.0), Affine::IDENTITY);
    let reversed = NodeKind::Path {
        path: Rect::new(0.0, 0.0, 60.0, 60.0)
            .to_path(0.1)
            .reverse_subpaths(),
        corner_radii: Vec::new(),
    };
    create(
        &mut doc,
        b,
        group,
        1,
        reversed,
        Affine::translate((30.0, 0.0)),
    );
    doc.apply(&Transaction(vec![Operation::SetMask {
        id: group,
        mask: true,
    }]))
    .unwrap();

    let res = Resolved::rebuild(&doc);
    let path = res
        .mask_path(&doc, group)
        .expect("a group masks with what is inside it");
    use ondin_core::kurbo::Shape;
    assert!(
        path.contains(Point::new(45.0, 30.0)),
        "the overlap is kept, not punched out"
    );
    assert!(
        path.contains(Point::new(10.0, 30.0)),
        "and so is one member"
    );
    assert!(path.contains(Point::new(80.0, 30.0)), "and so is the other");
    assert!(
        !path.contains(Point::new(120.0, 30.0)),
        "past both of them there is nothing"
    );
}

/// A boolean of `n` circles of radius 90, centres spaced round a circle of radius
/// 120 — `boolean::tests::ring` built out of real `Ellipse` nodes, which is what
/// makes this a test of the *cache* rather than of the arithmetic.
fn ring_boolean(op: ondin_core::BoolOp, n: usize) -> (Document, NodeId) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let b = ids.mint();
    create(
        &mut doc,
        b,
        root,
        0,
        NodeKind::Boolean { op },
        Affine::IDENTITY,
    );
    for i in 0..n {
        let a = i as f64 / n as f64 * std::f64::consts::TAU;
        let c = Point::new(200.0 + 120.0 * a.cos(), 200.0 + 120.0 * a.sin());
        let e = ids.mint();
        create(
            &mut doc,
            e,
            b,
            i,
            NodeKind::Ellipse {
                size: Size::new(180.0, 180.0),
            },
            Affine::translate((c.x - 90.0, c.y - 90.0)),
        );
    }
    (doc, b)
}

/// An `Intersect` over two 20-unit circles 500 apart: a boolean that draws
/// nothing and is **right** to. The control for every assertion about a failed
/// one — the two are indistinguishable by their outline and must never be
/// indistinguishable to the user.
fn disjoint_intersection() -> (Document, NodeId) {
    let (mut doc, b) = ring_boolean(ondin_core::BoolOp::Intersect, 0);
    let mut ids = IdSource::new(9_000);
    for (i, x) in [0.0, 500.0].into_iter().enumerate() {
        let e = ids.mint();
        create(
            &mut doc,
            e,
            b,
            i,
            NodeKind::Ellipse {
                size: Size::new(20.0, 20.0),
            },
            Affine::translate((x, 0.0)),
        );
    }
    (doc, b)
}

/// **A boolean that draws nothing because the arithmetic gave up is not the same
/// as one that draws nothing because there is nothing there** (§15 D239), and
/// `boolean_path` cannot tell them apart — it answers `None` to both.
///
/// This is the whole of what the layers row's warning colour and the status line
/// rest on, so it is asserted in both directions on purpose. The failing case is
/// still the forty-circle ring, built out of `Ellipse` nodes rather than handed to
/// `evaluate` directly, because the claim being tested is that the mark survives
/// the trip through `operand_of` and lands on the right node. The correct-empty
/// case is the same ring under `Union`, which produces a real shape, and a plain
/// disjoint `Intersect`, which produces nothing at all and **must not** be marked —
/// that is the assertion that fails if `failed` is ever written from "the outline
/// is missing" instead of from `boolean::failures`.
///
/// ⚠️ **The failure is injected since flo_curves 0.8.1** (`boolean::poison_next`,
/// §15 D239): that release fixed the intransitive comparator this ring used to
/// trip, so the ring is now only a plausible *shape* for the fixture and no longer
/// the cause. What still makes this test worth its length is the trip through
/// `operand_of` — the poison lands on the one `evaluate` the rebuild makes for this
/// node, and the mark still has to come out on the right one.
#[test]
fn a_boolean_the_arithmetic_gave_up_on_is_marked_and_an_empty_one_is_not() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let (doc, b) = ring_boolean(ondin_core::BoolOp::Exclude, 40);
    ondin_core::boolean::poison_next(1);
    let res = Resolved::rebuild(&doc);
    std::panic::set_hook(hook);
    assert!(
        res.boolean_path(b).is_none(),
        "the fixture has to be the failing one: this ring is poisoned above"
    );
    assert!(
        res.boolean_failed(b),
        "a boolean that was abandoned mid-arithmetic has to say so"
    );

    let (doc, b) = ring_boolean(ondin_core::BoolOp::Union, 40);
    let res = Resolved::rebuild(&doc);
    assert!(
        res.boolean_path(b).is_some_and(|p| !p.is_empty()),
        "the same ring unions to a real shape"
    );
    assert!(
        !res.boolean_failed(b),
        "a boolean that worked is not marked"
    );

    // Two circles far apart: `Intersect` is correctly empty, and the mark must not
    // follow the emptiness.
    let (doc, b) = disjoint_intersection();
    let res = Resolved::rebuild(&doc);
    assert!(
        res.boolean_path(b).is_none(),
        "disjoint circles have no intersection — the fixture is the point"
    );
    assert!(
        !res.boolean_failed(b),
        "an empty result is an answer, not a failure, and marking it would call \
         correct work broken"
    );
}

/// **The placeholder is ink, so it needs a box and a click target — and the empty
/// boolean beside it must get neither** (§15 D298).
///
/// Three things follow from a failed boolean having no world bounds, and they are
/// asserted together because they are one cause: it is absent from the spatial
/// index, so a click cannot find it; there is nothing for *zoom to selection* to
/// measure; and the scene walk has nothing to cull the drawing against. The fix is
/// the operands' union, which is the only extent the node has left once its own
/// outline is what failed.
///
/// **The disjoint case is the half that makes this a rule rather than a
/// workaround.** Giving *every* pathless boolean a box would put an invisible click
/// target over whatever sits behind a correctly empty one — a shape that cannot be
/// clicked because something you cannot see is in front of it, which is the exact
/// bug this is supposed to be the opposite of.
#[test]
fn a_failed_boolean_has_a_box_and_a_click_target_and_an_empty_one_has_neither() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    // Injected since flo_curves 0.8.1 — see the test above and `boolean::poison_next`.
    let (doc, b) = ring_boolean(ondin_core::BoolOp::Exclude, 40);
    ondin_core::boolean::poison_next(1);
    let res = Resolved::rebuild(&doc);
    std::panic::set_hook(hook);
    assert!(
        res.boolean_failed(b),
        "the fixture has to be the failing one"
    );

    // Centres sit on a circle of radius 120 about (200, 200) and each circle has
    // radius 90, so the operands reach 210 in every direction.
    let bounds = res
        .world_bounds(b)
        .expect("a boolean with a placeholder to draw has a box to draw it in");
    assert!(
        (bounds.x0 + 10.0).abs() < 1.0
            && (bounds.y0 + 10.0).abs() < 1.0
            && (bounds.x1 - 410.0).abs() < 1.0
            && (bounds.y1 - 410.0).abs() < 1.0,
        "the box is the operands' union, not something smaller: {bounds:?}"
    );
    assert!(
        hit_test(&res, &doc, Point::new(200.0, 200.0), 0.0).contains(&b),
        "the placeholder can be seen, so it has to be selectable"
    );
    assert!(
        !hit_test(&res, &doc, Point::new(1000.0, 1000.0), 0.0).contains(&b),
        "and only where it is drawn"
    );

    let (doc, b) = disjoint_intersection();
    let res = Resolved::rebuild(&doc);
    assert!(
        res.world_bounds(b).is_none(),
        "a boolean that is correctly empty draws nothing and measures nothing"
    );
    assert!(
        !hit_test(&res, &doc, Point::new(10.0, 10.0), 0.0).contains(&b),
        "and cannot be clicked in the middle of an operand it consumed"
    );
}

/// **A locked group locks what is inside it** — `is_effectively_locked` walks the
/// ancestors, and `hit_test` refuses a click through it (§15 D321).
///
/// ⚠️ **This was the rule with no test.** `hit_test_is_topmost_first_and_respects_visibility`
/// locks the node it then fails to click, so the *own flag* was covered and the
/// ancestor walk was not — while `context-menus.md` C4 has always claimed the filter
/// works "directly or through an ancestor". Breaking the walk (`cursor = None` in
/// place of `cursor = node.parent()`) left the whole of `ondin-core` green, which is
/// how the app came to have four refusals reading a node's own flag: the weaker
/// reading looks like it works, because the case that separates them is the one
/// nobody wrote down.
///
/// Both halves are asserted because they can fail apart: the query is what the app's
/// editing refusals ask, and `hit_test` is what the canvas asks, and only the second
/// had any coverage at all.
#[test]
fn a_locked_group_locks_its_contents_for_the_query_and_for_a_click() {
    let mut ids = IdSource::new(2);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let group = ids.mint();
    let child = ids.mint();
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    create(&mut doc, group, ab, 0, NodeKind::Group, Affine::IDENTITY);
    create(
        &mut doc,
        child,
        group,
        0,
        rect(100.0, 100.0),
        Affine::IDENTITY,
    );

    // The fixture, asserted before the claim: nothing is locked, and the child is
    // clickable — otherwise every refusal below could be about the geometry.
    let res = Resolved::rebuild(&doc);
    assert!(!is_effectively_locked(&doc, child), "nothing is locked yet");
    assert!(
        hit_test(&res, &doc, Point::new(10.0, 10.0), 0.0).contains(&child),
        "the child covers the point, or this test is about a miss rather than a lock"
    );

    // Lock the **group**. The child's own flag is untouched throughout, which is the
    // whole point: a guard reading `Node::locked` sees nothing change here.
    doc.apply(&Transaction(vec![Operation::SetLocked {
        id: group,
        locked: true,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);

    assert!(
        !doc.get(child).expect("the child").locked(),
        "the child's own flag stays clear — that is what makes this the ancestor case"
    );
    assert!(
        is_effectively_locked(&doc, child),
        "a layer inside a locked group is locked: a group is one thing to the hand \
         that locked it"
    );
    assert!(
        !hit_test(&res, &doc, Point::new(10.0, 10.0), 0.0).contains(&child),
        "and the canvas cannot deliver a click to it either"
    );
    assert!(
        is_effectively_locked(&doc, group),
        "the group itself, for completeness — the walk starts at the node, so a \
         version that only ever looked at the parent would pass everything above"
    );
}

/// The two boxes are two questions, and this pins both answers at once.
///
/// **`world_bounds` must not move.** It is what the selection outline, the
/// inspector's W and H, snapping and the marquee all read, so a shadow moving it
/// would grow the reported height when a blur was softened — the failure that is
/// invisible in code and obvious the first time someone drags a handle.
///
/// **`ink_bounds` must move, and must reach the ancestors.** The shadow is on the
/// *child*; the group above it and the artboard above that have no effect of
/// their own, and both still have to cover the ink or the cull drops it and the
/// export clips it.
#[test]
fn a_shadow_moves_the_ink_box_up_the_tree_and_leaves_the_editing_box_alone() {
    let mut ids = IdSource::new(7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut hist = History::new();
    let ab = ids.mint();
    let group = ids.mint();
    let shape = ids.mint();
    for (id, parent, kind) in [
        (ab, root, artboard()),
        (group, ab, NodeKind::Group),
        (shape, group, rect(100.0, 100.0)),
    ] {
        hist.commit(
            &mut doc,
            Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index: 0,
                kind,
                transform: None,
                name: None,
            }]),
        )
        .unwrap();
    }
    let mut res = Resolved::rebuild(&doc);
    let before: Vec<_> = [shape, group, ab]
        .map(|id| (res.world_bounds(id), res.ink_bounds(id)))
        .into();
    for (b, i) in &before {
        assert_eq!(b, i, "with no effect anywhere, the two boxes are the same");
    }

    // 20 down, no blur and no spread, so the arithmetic is checkable by eye.
    let dirty = hist
        .commit(
            &mut doc,
            Transaction(vec![Operation::SetEffects {
                id: shape,
                effects: vec![Effect::new(EffectKind::DropShadow(Shadow {
                    offset: Vec2::new(0.0, 20.0),
                    blur: 0.0,
                    spread: 0.0,
                    ..Shadow::default()
                }))],
            }]),
        )
        .unwrap();
    res.update(&doc, &dirty);

    for (n, (id, label)) in [
        (shape, "the shape"),
        (group, "its group"),
        (ab, "the artboard"),
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            res.world_bounds(*id),
            before[n].0,
            "{label}: the editing box must not have moved"
        );
    }
    // The artboard is excluded from the ink claim below on purpose: its box is
    // its frame, not its contents, so a shadow inside it is the frame's business
    // only once the frame stops clipping.
    for (n, (id, label)) in [(shape, "the shape"), (group, "its group")]
        .iter()
        .enumerate()
    {
        let ink = res.ink_bounds(*id).expect("has ink");
        let box_ = before[n].0.expect("had a box");
        assert_eq!(
            ink.y1 - box_.y1,
            20.0,
            "{label}: the ink reaches 20 further down"
        );
        assert_eq!(ink.y0, box_.y0, "{label}: and not further up");
    }
}

/// **`outline_at` answers about an *edge*, where `hit_test` answers about an
/// interior** (§15 D408) — which is the whole reason it exists, and the claims
/// below are the ones the Text tool's hover depends on.
///
/// ⚠️ **The distinguishing point is the shape's *middle*, and the first draft of
/// this test asserted the wrong one.** It claimed that an edge is a place
/// `hit_test` does not report — reasoning that an unfilled ellipse has no interior
/// to hit — and that is not true here: this app's hit test does not consult the
/// fill at all, so an unfilled shape is clickable anywhere inside it and the edge
/// is reported by both. What actually separates the two is the centre, where
/// `hit_test` answers with the ring and `outline_at` answers `None`, which is the
/// behaviour that matters: clicking the middle of a circle with the Text tool
/// plants an ordinary text node, and only clicking its edge writes along it.
#[test]
fn outline_at_finds_an_edge_where_hit_test_finds_an_interior() {
    use ondin_core::{NodeKind, outline_at};

    let mut ids = IdSource::new(9);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let ring = ids.mint();
    let label = ids.mint();
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    // An ellipse with no fill at all, at a transform the artboard does not share.
    create(
        &mut doc,
        ring,
        ab,
        0,
        NodeKind::Ellipse {
            size: Size::new(200.0, 200.0),
        },
        Affine::translate((100.0, 100.0)),
    );
    create(&mut doc, label, ab, 1, text("hi", 20.0), Affine::IDENTITY);
    let res = Resolved::rebuild(&doc);

    // On the ring's left edge, in world space: the ellipse spans (100, 100) to
    // (300, 300), so its leftmost point is (100, 200).
    let on_edge = Point::new(100.0, 200.0);
    assert_eq!(
        outline_at(&res, &doc, on_edge, 4.0),
        Some(ring),
        "the edge of an unfilled shape is what this is for"
    );

    // The centre: inside the ring and nowhere near its curve. This is the pair of
    // assertions that separates the two questions — see the note above.
    let inside = Point::new(200.0, 200.0);
    assert_eq!(
        outline_at(&res, &doc, inside, 4.0),
        None,
        "an interior is not an edge"
    );
    assert!(
        hit_test(&res, &doc, inside, 0.0).contains(&ring),
        "while `hit_test` does report it — the fixture must actually separate the \
         two questions"
    );
    // And just outside the tolerance is a miss, or the band means nothing.
    assert_eq!(
        outline_at(&res, &doc, Point::new(80.0, 200.0), 4.0),
        None,
        "20 units away with a 4-unit allowance"
    );

    // A frame is a page and a text node is something to edit — neither is offered
    // as a rail, however close the pointer is to its edge.
    assert_eq!(
        outline_at(&res, &doc, Point::new(0.0, 300.0), 4.0),
        None,
        "the artboard's own left edge is not a rail"
    );
}

/// **A per-segment box reject does not change the answer** (`[S4.1-L4-05]`,
/// §15 D507).
///
/// `outline_at` solved the exact nearest point on **every** segment of every
/// candidate, once per frame the Text or Pen tool is in hand — a quintic
/// root-find per cubic, with no reject in front of it. The candidate query bounds
/// the loop in **nodes** and the work is denominated in **segments**, so one
/// imported path is enough. Re-measured here in release on a ring of cubics, 50
/// calls each at `slop = 4`, before and after:
///
/// ```text
/// segments      before     after
///       10      0.0013     0.0007
///      200      0.0142     0.0042
///    2,000      0.1599     0.0408   ms/call
///   10,000      0.7075     0.2041
/// ```
///
/// **2,000 segments is the number to reason from** — an SVG-imported logo or map
/// reaches it easily — and there it is 0.160 ms/frame down to 0.041.
///
/// The change is a pure early-out, so **the assertion is that nothing moved** —
/// on a path with many segments, which is the shape where the reject actually
/// fires and where a wrong one would not be noticed.
///
/// ⚠️ **The tolerance is where a box test is easiest to get wrong**, and the
/// probes below are placed for it: `2` units off a 100-unit ring, inside a
/// `4`-unit allowance and **not on the ink**. A reject that tested the hull
/// without inflating it by the allowance would still find a point lying exactly
/// on the curve — every such point is inside its own segment's hull at distance
/// zero — so an on-the-outline probe cannot see that mistake and these can.
///
/// **Two flips, and the first is the reason this test is worth its own fixture
/// while the second says the interior case here is not.**
///
/// - `dx² + dy² > cap` narrowed to `> 0.0`, i.e. the reject with no tolerance in
///   it: fails on *"2 units off the outline at 1,0"* at `None` against the ring.
/// - `seg.nearest(…)` replaced by the box distance, i.e. **the reject replacing
///   the solve instead of guarding it**: this test stays **green** and
///   `outline_at_finds_an_edge_where_hit_test_finds_an_interior` fails on
///   *"an interior is not an edge"*. ⚠️ **Which corrects what this fixture was
///   first written believing.** Its draft doc claimed the segments' boxes "cover
///   the whole interior"; they do not — 720 hulls round a ring are each under a
///   unit across, and the centre is 100 units from all of them, so it is rejected
///   trivially. The fixture where hulls *do* swallow the interior is the **4**-
///   segment ellipse in that older test, and it already existed. A many-segment
///   path and a coarse one are different risks, and only running both flips said
///   which fixture answers which.
#[test]
fn a_many_segment_outline_answers_the_same_with_the_segment_reject() {
    use ondin_core::outline_at;

    let mut ids = IdSource::new(0x5E6);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let ring = ids.mint();
    create(&mut doc, ab, root, 0, artboard(), Affine::IDENTITY);
    // A ring of 720 **cubic** segments — the shape an imported drawing has, and
    // the one where a control hull is strictly larger than the curve inside it.
    let mut path = kurbo::BezPath::new();
    let (cx, cy, r) = (200.0_f64, 200.0_f64, 100.0_f64);
    let n = 720;
    for i in 0..n {
        let a = f64::from(i) * std::f64::consts::TAU / f64::from(n);
        let p = Point::new(cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            path.move_to(p);
        } else {
            let mid = (f64::from(i) - 0.5) * std::f64::consts::TAU / f64::from(n);
            let c = Point::new(cx + r * 1.02 * mid.cos(), cy + r * 1.02 * mid.sin());
            path.curve_to(c, c, p);
        }
    }
    path.close_path();
    create(
        &mut doc,
        ring,
        ab,
        0,
        NodeKind::Path {
            path,
            corner_radii: Vec::new(),
        },
        Affine::IDENTITY,
    );
    let res = Resolved::rebuild(&doc);

    assert_eq!(
        outline_at(&res, &doc, Point::new(cx, cy), 4.0),
        None,
        "the centre is 100 units from every segment and is not an edge"
    );
    // **Inside the band and off the curve**, which is the case the tolerance is
    // for. A reject that tested the hull without inflating it by the allowance
    // would answer `None` for every one of these while still finding a point
    // lying exactly on the ink.
    let diag = -std::f64::consts::FRAC_1_SQRT_2;
    for (dx, dy) in [(1.0, 0.0), (0.0, 1.0), (diag, diag)] {
        let near = Point::new(cx + (r + 2.0) * dx, cy + (r + 2.0) * dy);
        assert_eq!(
            outline_at(&res, &doc, near, 4.0),
            Some(ring),
            "2 units off the outline at {dx},{dy} is inside a 4-unit allowance"
        );
    }
    assert_eq!(
        outline_at(&res, &doc, Point::new(cx + r + 20.0, cy), 4.0),
        None,
        "20 units outside with a 4-unit allowance is still a miss"
    );
}

/// **A mask is not a rail, and neither is ink the mask has clipped away.**
///
/// `[S4.1-L1-02]`, §15 D456. `outline_at` asked two of `hit_test`'s three
/// questions and not the third, so the *same two clicks* answered differently
/// through the two doors — and the Text tool acts on this one. Clicking the
/// mask's edge lit the rail affordance on a layer that paints nothing;
/// `text_on_new_path` then **deleted** it, the mask flag did not travel to the
/// `Text` that replaced it, and the group's bounds went `50×50` →
/// `-11,-11..500,400`, the artwork it was hiding fully visible with the status
/// line saying *"the shape became the rail"*. Clicking out in the clipped-away
/// region offered the masked rect where the canvas shows nothing at all.
///
/// ⚠️ **Two halves, and each passes the other's case** — the same split
/// `a_mask_is_unclickable_and_what_it_masks_is_clickable_only_inside_it` records
/// for `hit_test`, which is not a coincidence: this is that function's third
/// question, asked here for the first time.
///
/// ⚠️ **D408's exclusion list is about *kinds*; what governs is a ROLE.** A mask
/// is an ordinary `Rect` — `resolve::is_indexed` says so and `build_index`
/// filters on nothing else — and it must be visible and unlocked to mask at all,
/// so `is_effectively_interactable` passes it too. Nothing in the old rule could
/// have caught it.
///
/// ⚠️ **Flipped** at each guard, with the other in place. Removing the
/// `masked_away` call in `outline_at`: fails on the **first** assertion,
/// answering `Some(5a5a:4)` — the mask. Removing the `mask()` refusal from
/// `text_on_new_path` instead: fails on the `expect_err`, and what it prints is
/// the defect in one line — `CreateNode { parent: <the group>, kind: Text { …
/// on_path: Some(the mask's own 50×50 outline) } }` followed by
/// `DeleteNode { 5a5a:4 }`, with **no mask flag anywhere on the node that
/// replaces it**. That is the door the canvas no longer uses and MCP still does.
/// The two controls stay green under both flips.
///
/// ⚠️ **The first draft aimed at the mask's *left* edge and failed for the wrong
/// reason** — it answered `Some(masked)`, because the mask's left and top edges
/// lie exactly on the 500×400 rect's, and that rect's edge *is* legitimately
/// offered from inside the mask. A point that asks two questions answers neither;
/// the right edge asks one.
#[test]
fn a_mask_is_not_a_rail_and_neither_is_ink_it_has_clipped_away() {
    use ondin_core::{TextParts, build, outline_at};

    let (mut doc, group, mask, masked) = mask_fixture();
    doc.apply(&Transaction(vec![Operation::SetMask {
        id: mask,
        mask: true,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(group),
        Some(Rect::new(0.0, 0.0, 50.0, 50.0)),
        "fixture: the flag is on, so the group is the mask's size"
    );

    // The mask's own **right** edge, at (50, 25). Its left and top edges lie on
    // top of the masked rect's, which are legitimately offered from inside the
    // mask — so aiming there would ask two questions at once and answer neither.
    // Every edge of the 500×400 rect is at least 25 units from here.
    let on_mask = Point::new(50.0, 25.0);
    assert_eq!(
        outline_at(&res, &doc, on_mask, 4.0),
        None,
        "a mask paints nothing, so there is no edge on the canvas to aim at"
    );

    // The masked rect's right edge, at (500, 200) — 450 units outside the mask,
    // where the canvas shows nothing.
    let clipped_away = Point::new(500.0, 200.0);
    assert_eq!(
        outline_at(&res, &doc, clipped_away, 4.0),
        None,
        "and clipped-away ink is not an edge either"
    );

    // The builder refuses the mask outright, for the caller that does not come
    // through the pick.
    let mut ids = IdSource::new(0xB0B);
    let err = build::text_on_new_path(&doc, &res, &mut ids, mask, &TextParts::default(), on_mask)
        .expect_err("a mask may not be consumed as a rail");
    assert!(
        matches!(err, ondin_core::OpError::WrongKindForOp),
        "refused as the wrong kind: {err:?}"
    );

    // **The controls.** Inside the mask the masked layer's own edge is still a
    // rail — nothing here has made a masked group unusable — and the whole thing
    // still works once the flag is off.
    // Two units in from the masked rect's left edge and strictly inside the mask,
    // so the 4-unit band reaches the edge without the point sitting on the mask's
    // own boundary, where `contains` is a coin toss.
    assert_eq!(
        outline_at(&res, &doc, Point::new(2.0, 25.0), 4.0),
        Some(masked),
        "the masked layer's own edge, inside the mask, is still offered"
    );
    doc.apply(&Transaction(vec![Operation::SetMask {
        id: mask,
        mask: false,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        outline_at(&res, &doc, on_mask, 4.0),
        Some(mask),
        "with the flag off it is an ordinary rect and an ordinary rail"
    );
}

/// A chain of `depth` nested groups with `wide` rects hanging off each level,
/// and the id of the topmost group.
fn deep_chain(depth: usize, wide: usize) -> (Document, NodeId) {
    let mut ids = IdSource::new(0xDEE9);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut parent = root;
    let mut top = None;
    for _ in 0..depth {
        let g = ids.mint();
        create(
            &mut doc,
            g,
            parent,
            0,
            NodeKind::Group,
            Affine::translate((1.0, 1.0)),
        );
        top.get_or_insert(g);
        for w in 0..wide {
            create(
                &mut doc,
                ids.mint(),
                g,
                w,
                rect(10.0, 10.0),
                Affine::translate((2.0, 0.0)),
            );
        }
        parent = g;
    }
    (doc, top.expect("depth >= 1"))
}

/// **The incremental `update` is not slower than the full `rebuild` it exists to
/// avoid** (§15 D594, `[S4.1-L4-03]`).
///
/// `update` sorted its affected set by `depth`, with `sort_by_key` — which the
/// standard library documents as calling the key function on **every
/// comparison** — and `depth` walks the parent chain to the root. The set is
/// collected from an `FxHashSet`, so it arrives scrambled and the adaptive sort
/// gets no help. Measured in release with the node count held near constant:
/// `update` was 4.8× `rebuild` at depth 20 and **47×** at depth 200, 93% of it in
/// that one line. Per commit — every arrow-key nudge, every undo, every redo.
///
/// ⚠️ **A ratio against `rebuild`, not a duration**, and it is the property the
/// incremental path exists to have rather than a number about this machine. It is
/// also the honest bound: `update` doing *less work* than `rebuild` is the whole
/// premise, so anything above 1 is already suspicious and the 3.0 here is slack
/// for a debug build's noise on a sub-millisecond measurement.
///
/// ⚠️ **The fixture is deep and narrow on purpose.** Depth is the quantity that
/// made this superlinear and `[A4-MAP]`'s cost model has no depth axis — 400
/// nodes at depth 200 cost more here than 4,000 flat ones, so a wide fixture
/// measures nothing.
///
/// **Three versions measured on this fixture in debug, three runs each and
/// stable to two decimal places** — which is what says where the cost was and
/// that the cheap-looking fix was not enough:
///
/// ```text
/// sort_by_key(depth-walk)         42.9x   <- the finding
/// sort_by_cached_key(depth-walk)   3.2x   <- one word, and still 3x
/// depth recorded on the way down   1.2x   <- this
/// ```
///
/// ⚠️ **The middle row is the one worth carrying.** `sort_by_cached_key` is the
/// obvious one-word fix and it takes 93% of the cost out — and leaves the
/// incremental path three times the price of the thing it exists to avoid,
/// because the walk still runs once per node. A test asserting only *"faster than
/// before"* would have stopped there.
///
/// **Flip run**, back to `sort_by_key` over a `depth` walk: fails at 42.9x,
/// naming both timings.
#[test]
fn an_incremental_update_is_not_slower_than_the_rebuild_it_replaces() {
    let (mut doc, top) = deep_chain(200, 1);
    let mut res = Resolved::rebuild(&doc);

    // The fixture: deep, and the subtree really does reach every node.
    assert!(
        doc.get(top).is_some_and(|n| n.parent() == Some(doc.root())),
        "the fixture: the chain hangs off the root"
    );

    let nudge = |doc: &mut Document, x: f64| {
        doc.apply(&Transaction(vec![Operation::SetTransform {
            id: top,
            transform: Affine::translate((x, 1.0)),
        }]))
        .expect("the nudge")
        .dirty
    };

    // Warm both paths before either is timed.
    let dirty = nudge(&mut doc, 2.0);
    res.update(&doc, &dirty);
    let _ = Resolved::rebuild(&doc);

    let dirty = nudge(&mut doc, 3.0);
    let at = std::time::Instant::now();
    res.update(&doc, &dirty);
    let incremental = at.elapsed().as_secs_f64();

    let at = std::time::Instant::now();
    let full = Resolved::rebuild(&doc);
    let whole = at.elapsed().as_secs_f64();

    // And it still agrees with the thing it is being compared against, which is
    // the assertion that stops this becoming a test about speed alone.
    assert_eq!(
        res.world_transform(top),
        full.world_transform(top),
        "the incremental pass and the full one must agree"
    );

    let ratio = incremental / whole.max(f64::EPSILON);
    assert!(
        ratio < 3.0,
        "the incremental update cost {ratio:.1}x the full rebuild it exists to \
         avoid ({incremental:.5}s vs {whole:.5}s)"
    );
}

/// A group holding `n` text blocks, and the group's id.
fn group_of_text(n: usize) -> (Document, NodeId, Vec<NodeId>) {
    let mut ids = IdSource::new(0x7E57);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let group = ids.mint();
    create(
        &mut doc,
        group,
        root,
        0,
        NodeKind::Group,
        Affine::translate((10.0, 10.0)),
    );
    let blocks: Vec<NodeId> = (0..n)
        .map(|i| {
            let id = ids.mint();
            create(
                &mut doc,
                id,
                group,
                i,
                text("hello world", 16.0),
                Affine::translate((0.0, 20.0 * i as f64)),
            );
            id
        })
        .collect();
    (doc, group, blocks)
}

/// **A translate on a group does not re-shape the text inside it** (§15 D590,
/// `[A4-L4-03]`).
///
/// `Resolved::update` expands the dirty set to whole subtrees — correctly, since
/// a descendant's *world transform* depends on a dirty ancestor — and then ran
/// `reshape_text` over the expansion. A text layout depends on nothing outside
/// its own node, so nudging a group of twenty 4,000-character blocks re-shaped
/// all twenty for a change that cannot move a glyph: about 23 ms per nudge
/// against a key-repeat of ~30/s.
///
/// ⚠️ **Nothing that existed could see this, which is why the counter exists.**
/// The layouts produced are identical either way, so the randomized differential
/// that owns `update`'s correctness is green under the bug and under the fix.
/// `ondin_core::text::shapes()` is the only observable that separates them.
///
/// ⚠️ **The fixture assertion is the first shape count**, and it is not
/// decoration: if the initial `rebuild` shaped nothing — a fixture with no text
/// in it, or text the engine declined — every later assertion would be `0 == 0`
/// and the test would be about nothing.
///
/// 🚨 **Two flips, because the fix has two halves and *either one alone does
/// nothing*.** `op_set_transform` was inserting the whole subtree into the dirty
/// set itself — the finding's brief said `DirtySet` was minimal and it was not —
/// so narrowing `update`'s text pass to the dirty ids changed no count at all
/// until the op stopped doing the expansion `update` already performs. Run
/// separately, each flip fails on *"a translate re-shaped the text"* at **3
/// against 0**:
///
/// - `reshape_text` put back over `affected`;
/// - `op_set_transform` put back to `for d in self.subtree_ids(id)`.
///
/// The `SetText` control below stays green under both, which says the assertion
/// is about the expansion rather than about reshaping having been switched off,
/// and the randomized `update ≡ rebuild` differential passes with the fix in —
/// which is the only thing that can say the narrowed set is still enough.
#[test]
fn a_translate_on_a_group_does_not_reshape_the_text_inside_it() {
    let (mut doc, group, blocks) = group_of_text(3);

    let before = ondin_core::text::shapes();
    let mut res = Resolved::rebuild(&doc);
    let shaped = ondin_core::text::shapes() - before;
    assert_eq!(
        shaped, 3,
        "the fixture: the initial build must shape all three blocks, or nothing \
         below is measuring anything"
    );

    // The translate. Every block is in `affected` and not one of them is dirty.
    let at = ondin_core::text::shapes();
    let dirty = doc
        .apply(&Transaction(vec![Operation::SetTransform {
            id: group,
            transform: Affine::translate((40.0, 0.0)),
        }]))
        .expect("the nudge")
        .dirty;
    res.update(&doc, &dirty);
    assert_eq!(
        ondin_core::text::shapes() - at,
        0,
        "a translate re-shaped the text it moved"
    );
    // The group is *set* to (40, 0), not nudged by it, so the second block —
    // itself at (0, 20) — lands at (40, 20). Asserted because the whole change is
    // to what `update` re-derives for a descendant, and a descendant whose world
    // transform stopped following its parent is the way this fix goes wrong.
    assert_eq!(
        res.world_transform(blocks[1]).unwrap(),
        Affine::translate((40.0, 20.0)),
        "and the world transform it *is* about still moved"
    );

    // The control: an edit that really does change a layout still shapes, and
    // shapes once.
    let at = ondin_core::text::shapes();
    let dirty = doc
        .apply(&Transaction(vec![Operation::SetText {
            id: blocks[1],
            content: "hello world and then some".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]))
        .expect("the edit")
        .dirty;
    res.update(&doc, &dirty);
    assert_eq!(
        ondin_core::text::shapes() - at,
        1,
        "the control: an edit to one block's content must re-shape that block, \
         and only it"
    );
}
