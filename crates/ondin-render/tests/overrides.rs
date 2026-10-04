//! Preview overrides (§6.2, invariant 5).
//!
//! The property that matters is not "overrides draw something" but **"overrides
//! draw exactly what committing would draw"**. A preview that disagrees with
//! its own transaction is worse than no preview: the user aims at what they see
//! and gets something else on release.
//!
//! So the main tests here are differential — build the scene twice, once with
//! the transaction as overrides over the untouched document and once with the
//! transaction actually applied, and require the two draw-call streams to
//! match.

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, BezPath, Point, Rect, RoundedRectRadii, Shape, Size, Vec2};
use ondin_core::peniko::{Color, Gradient};
use ondin_core::{
    Document, Effect, EffectKind, Fill, GeometryPatch, IdSource, NodeId, NodeKind, Operation,
    Resolved, Shadow, TextSizing, TextStyle, Transaction, build, keyed_by_position,
};
use ondin_render::scene::{self, ClipRule, ScenePainter, StrokePaint, TextRun};
use ondin_render::{RenderOverrides, Viewport};

/// Every draw call, in order, in a form that can be compared for equality.
#[derive(Debug, PartialEq)]
enum Call {
    /// **The fill rule is part of the call**, so a preview that reads a node's rule
    /// differently from the commit is a difference this harness can see (§15 D239).
    Fill([f64; 6], String, Brush, ClipRule),
    Stroke([f64; 6], String, Brush, f64),
    Text([f64; 6], usize, Brush),
    /// **The clip path itself, not just whether there was one.** A mask's
    /// geometry is derived twice — once from the committed tree and once through
    /// the overrides (`scene::mask_geometry`) — so recording only `Some`/`None`
    /// here would let the two derivations disagree about the *shape* and still
    /// pass every differential test in this file.
    Push(Option<([f64; 6], String)>, f32),
    /// An alpha mask's composite. Carries nothing because it *is* nothing but a
    /// position in the call stream — what makes it right or wrong is which draws
    /// fall inside it, and those are the `Fill`s and `Text`s that follow.
    MaskPush,
    /// An effect layer, **with the box it asked for** (§5.3a, §15 D341).
    ///
    /// The bounds are the whole reason this variant exists rather than the walk's
    /// effect layers being recorded as a plain `Push`. That is what they were, and
    /// it hid a real bug: the box comes from `Resolved`, which knows only the
    /// *committed* document, so previewing a bigger blur drew it into a buffer
    /// sized for the smaller one and clipped it — visible on the canvas for the
    /// whole of a scrub and gone the instant it committed. A recorder that dropped
    /// the number could not tell the two apart.
    Effect([f64; 6], [f64; 4], Vec<ondin_core::Effect>),
    Pop,
}

#[derive(Default)]
struct Recorder(Vec<Call>);

impl ScenePainter for Recorder {
    // Every picture is available to a recorder: these observe what the walk
    // *emits*, and answering `false` would have the walk emit the missing-image
    // placeholder instead (§15 D179) — a different question from the one each of
    // these tests asks.
    fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        t: Affine,
        path: &BezPath,
        brush: &Brush,
        _framing: Option<ondin_core::Framing>,
        rule: ClipRule,
    ) {
        self.0.push(Call::Fill(
            t.as_coeffs(),
            path.to_svg(),
            brush.clone(),
            rule,
        ));
    }
    fn stroke_path(&mut self, t: Affine, path: &BezPath, s: &StrokePaint<'_>) {
        self.0.push(Call::Stroke(
            t.as_coeffs(),
            path.to_svg(),
            s.brush.clone(),
            s.width,
        ));
    }
    fn draw_text(&mut self, t: Affine, run: &TextRun<'_>) {
        self.0.push(Call::Text(
            t.as_coeffs(),
            run.glyphs.len(),
            run.brush.clone(),
        ));
    }
    fn push_layer(&mut self, t: Affine, clip: Option<&BezPath>, _rule: ClipRule, opacity: f32) {
        self.0.push(Call::Push(
            clip.map(|c| (t.as_coeffs(), c.to_svg())),
            opacity,
        ));
    }
    fn push_mask_layer(&mut self) {
        self.0.push(Call::MaskPush);
    }
    fn push_effect_layer(
        &mut self,
        t: Affine,
        bounds: Rect,
        effects: &[ondin_core::Keyed<ondin_core::Effect>],
    ) {
        self.0.push(Call::Effect(
            t.as_coeffs(),
            [bounds.x0, bounds.y0, bounds.x1, bounds.y1],
            ondin_core::item::values(effects),
        ));
    }
    fn pop_layer(&mut self) {
        self.0.push(Call::Pop);
    }
}

fn viewport() -> Viewport {
    Viewport {
        view: Rect::new(-500.0, -500.0, 1500.0, 1500.0),
        pixel_size: (400, 400),
    }
}

fn record(doc: &Document, res: &Resolved, ov: &RenderOverrides) -> Vec<Call> {
    let mut painter = Recorder::default();
    scene::build(doc, res, &viewport(), ov, &mut painter);
    painter.0
}

/// The central check: previewing `tx` must draw what committing `tx` draws.
fn assert_preview_matches_commit(doc: &Document, tx: &Transaction, what: &str) {
    let res = Resolved::rebuild(doc);
    let overrides = RenderOverrides::from_transaction(doc, &res, tx)
        .unwrap_or_else(|| panic!("{what}: not representable"));
    let previewed = record(doc, &res, &overrides);

    let mut committed_doc = doc.clone();
    committed_doc.apply(tx).expect("transaction applies");
    let committed_res = Resolved::rebuild(&committed_doc);
    let committed = record(&committed_doc, &committed_res, &RenderOverrides::default());

    assert_eq!(previewed, committed, "{what}: preview diverged from commit");
}

/// **`assert_preview_matches_commit` through the doors the app uses** (the
/// release review's `[X4.2-L6-01]`): the preview built from `tx` plus
/// `build::flex_holds`, as `session::set_preview` builds it, and the commit made
/// of `build::keep_flex_sizes(build::keep_insets(tx))`, as `session::commit_inner`
/// makes it. Returns the committed document, so a caller can also ask whether
/// the hold held.
///
/// **The plain harness commits the raw transaction, which the app never does.**
/// The commit door drops an in-flow item's translation, adds the hold that keeps
/// a resize (with `align-` or `justify-self: end` for a start-edge handle, §15
/// D905, D913) and rewrites a pinned layer's placement as insets (§15 D874); the
/// preview takes only the hold. So a change making `flex_holds` and
/// `keep_flex_sizes` disagree, or the preview drawing a resized pinned layer
/// anywhere but where `keep_insets` pins it, passed every test that goes through
/// the plain harness.
///
/// ⚠️ **Not for a plain move of an in-flow item**: the preview draws the drag
/// where the pointer has it and the commit drops its translation, by design
/// (§15 D877) — the landing outline is that gesture's check.
fn assert_preview_matches_commit_through_the_doors(
    doc: &Document,
    tx: &Transaction,
    what: &str,
) -> Document {
    let res = Resolved::rebuild(doc);
    let mut shown = tx.0.clone();
    shown.extend(build::flex_holds(doc, &res, tx));
    let overrides = RenderOverrides::from_transaction(doc, &res, &Transaction(shown))
        .unwrap_or_else(|| panic!("{what}: not representable"));
    let previewed = record(doc, &res, &overrides);

    let landed = build::keep_flex_sizes(doc, &res, build::keep_insets(doc, &res, tx.clone()));
    let mut committed_doc = doc.clone();
    committed_doc.apply(&landed).expect("transaction applies");
    let committed_res = Resolved::rebuild(&committed_doc);
    let committed = record(&committed_doc, &committed_res, &RenderOverrides::default());

    assert_eq!(previewed, committed, "{what}: preview diverged from commit");
    committed_doc
}

/// The transaction a resize handle writes for `id`, unrotated, with its start
/// edges moved `left` across and `top` down and its far edges held: the **used**
/// box shrunk by that much, and the used transform shifted by it — which is what
/// the tools compute from (§15 D874), slot and stretch included.
fn start_edge_resize(doc: &Document, id: NodeId, left: f64, top: f64) -> Transaction {
    let res = Resolved::rebuild(doc);
    let size = res
        .used_kind(doc, id)
        .and_then(|k| ondin_core::geometry::local_bounds(k, None))
        .expect("a sized kind")
        .size();
    let local = res.used_local(doc, id).expect("a placed node");
    Transaction(vec![
        Operation::SetGeometry {
            id,
            geometry: GeometryPatch::Size(Size::new(size.width - left, size.height - top)),
        },
        Operation::SetTransform {
            id,
            transform: local * Affine::translate((left, top)),
        },
    ])
}

struct Fixture {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    artboard: NodeId,
    group: NodeId,
    rect: NodeId,
    inner: NodeId,
    text: NodeId,
}

/// Root → artboard(+30,+20) → { group(+5,+7) → inner, rect, text }, so the
/// world transform of anything interesting is a product of several locals and
/// a missing composition shows up immediately.
fn fixture() -> Fixture {
    let mut ids = IdSource::new(0x0FF5);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let artboard = ids.mint();
    let group = ids.mint();
    let inner = ids.mint();
    let rect = ids.mint();
    let text = ids.mint();

    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: artboard,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(800.0, 600.0),
            },
            transform: Some(Affine::translate((30.0, 20.0))),
            name: None,
        },
        Operation::SetFills {
            id: artboard,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(250, 250, 250, 255)),
                visible: true,
            }]),
        },
        Operation::CreateNode {
            id: group,
            parent: artboard,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((5.0, 7.0))),
            name: None,
        },
        Operation::CreateNode {
            id: inner,
            parent: group,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((3.0, 4.0))),
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: artboard,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 50.0),
                corner_radii: RoundedRectRadii::from_single_radius(6.0),
            },
            transform: Some(Affine::translate((120.0, 90.0))),
            name: None,
        },
        Operation::CreateNode {
            id: text,
            parent: artboard,
            index: 2,
            kind: NodeKind::Text {
                content: "hello".into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: 24.0,
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
            },
            transform: Some(Affine::translate((10.0, 300.0))),
            name: None,
        },
    ]))
    .unwrap();
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: rect,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(70, 130, 220, 255)),
            visible: true,
        }]),
    }]))
    .unwrap();

    Fixture {
        doc,
        ids,
        root,
        artboard,
        group,
        rect,
        inner,
        text,
    }
}

#[test]
fn no_overrides_changes_nothing() {
    // The composed-down-the-tree world transforms must be bit-identical to the
    // ones `Resolved` computed, or every export would shift by a rounding error.
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    assert_preview_matches_commit(&f.doc, &Transaction(Vec::new()), "empty transaction");
    assert!(
        RenderOverrides::default().is_empty(),
        "a default override set is empty"
    );
    assert!(!record(&f.doc, &res, &RenderOverrides::default()).is_empty());
}

#[test]
fn moving_a_leaf_matches_its_commit() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let tx = build::move_by_world(&f.doc, &res, &[f.rect], Vec2::new(37.0, -19.0)).unwrap();
    assert_preview_matches_commit(&f.doc, &tx, "move a leaf");
}

#[test]
fn moving_a_group_carries_its_children() {
    // The case a naive override table gets wrong: only the group is patched,
    // but the child's world transform must move with it.
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let tx = build::move_by_world(&f.doc, &res, &[f.group], Vec2::new(50.0, 60.0)).unwrap();

    let ov = RenderOverrides::from_transaction(&f.doc, &res, &tx).unwrap();
    let before = res.world_transform(f.inner).unwrap() * Point::ZERO;
    let after = ov.world_transform(&f.doc, &res, f.inner).unwrap() * Point::ZERO;
    assert!((after.x - before.x - 50.0).abs() < 1e-9, "{after:?}");
    assert!((after.y - before.y - 60.0).abs() < 1e-9, "{after:?}");

    assert_preview_matches_commit(&f.doc, &tx, "move a group");
}

#[test]
fn moving_an_artboard_matches_its_commit() {
    // Exercises the clip layer's transform as well as the contents'.
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let tx = build::move_by_world(&f.doc, &res, &[f.artboard], Vec2::new(-15.0, 25.0)).unwrap();
    assert_preview_matches_commit(&f.doc, &tx, "move an artboard");
}

#[test]
fn resizing_matches_its_commit() {
    let f = fixture();
    assert_preview_matches_commit(
        &f.doc,
        &Transaction(vec![Operation::SetGeometry {
            id: f.rect,
            geometry: GeometryPatch::Size(Size::new(260.0, 30.0)),
        }]),
        "resize",
    );
}

#[test]
fn a_resize_that_also_moves_matches_its_commit() {
    // What dragging a top-left handle emits: geometry plus a transform.
    let f = fixture();
    assert_preview_matches_commit(
        &f.doc,
        &Transaction(vec![
            Operation::SetGeometry {
                id: f.rect,
                geometry: GeometryPatch::Size(Size::new(60.0, 20.0)),
            },
            Operation::SetTransform {
                id: f.rect,
                transform: Affine::translate((160.0, 120.0)),
            },
        ]),
        "resize from a far handle",
    );
}

#[test]
fn typing_matches_its_commit() {
    let f = fixture();
    assert_preview_matches_commit(
        &f.doc,
        &Transaction(vec![Operation::SetText {
            id: f.text,
            content: "hello, a considerably longer line".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]),
        "typing",
    );
}

#[test]
fn restyling_text_matches_its_commit() {
    let f = fixture();
    let NodeKind::Text { style, .. } = f.doc.get(f.text).unwrap().kind().clone() else {
        panic!("text node");
    };
    let mut bigger = style;
    bigger.font_size = 64.0;
    bigger.weight = 800;
    assert_preview_matches_commit(
        &f.doc,
        &Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            style: *bigger,
            spans: None,
        }]),
        "restyle text",
    );
}

/// **A restyle's preview re-states the span list, exactly as the commit does**
/// (§15 D163).
///
/// `assert_preview_matches_commit` is no use for this and was checked to be: the
/// re-statement only drops spans that have become *echoes* of the new defaults, so
/// what is drawn is identical either way, by construction. The divergence is in the
/// **model** — and the model is read, by the Type panel, through `display_node`, which
/// applies the override. A preview whose kind disagrees with its own commit is what
/// D109 was, so this asserts on the kind.
#[test]
fn a_restyle_preview_restates_the_spans_like_its_commit() {
    let mut f = fixture();
    let NodeKind::Text { style, .. } = f.doc.get(f.text).unwrap().kind().clone() else {
        panic!("text node");
    };
    let mut spans = ondin_core::CharSpans::default();
    spans.set(0..2, ondin_core::CharAttr::Size(40.0), &style);
    f.doc
        .apply(&Transaction(vec![Operation::SetTextSpans {
            id: f.text,
            spans,
        }]))
        .expect("spans apply");

    // The defaults move to exactly what the span was asking for, so the span becomes
    // redundant and the re-statement drops it.
    let mut flattened = style;
    flattened.font_size = 40.0;
    let tx = Transaction(vec![Operation::SetTextStyle {
        id: f.text,
        style: *flattened,
        spans: None,
    }]);

    let res = Resolved::rebuild(&f.doc);
    let overrides =
        RenderOverrides::from_transaction(&f.doc, &res, &tx).expect("a restyle is representable");
    let previewed = overrides
        .get(f.text)
        .and_then(|o| o.kind.clone())
        .expect("the restyle overrides the kind");

    let mut committed = f.doc.clone();
    committed.apply(&tx).expect("transaction applies");

    assert_eq!(
        &previewed,
        committed.get(f.text).unwrap().kind(),
        "the preview's kind should be the commit's, span list included"
    );
    let NodeKind::Text { spans, .. } = &previewed else {
        panic!("text node");
    };
    assert!(
        spans.is_empty(),
        "and the redundant span should be gone from both: {spans:?}"
    );
}

#[test]
fn opacity_matches_its_commit() {
    let f = fixture();
    assert_preview_matches_commit(
        &f.doc,
        &Transaction(vec![Operation::SetOpacity {
            id: f.group,
            opacity: 0.3,
        }]),
        "group opacity",
    );
}

#[test]
fn a_gradient_fill_matches_its_commit() {
    let f = fixture();
    let gradient =
        Gradient::new_linear(Point::new(0.0, 0.0), Point::new(100.0, 50.0)).with_stops([
            (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
            (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
        ]);
    assert_preview_matches_commit(
        &f.doc,
        &Transaction(vec![Operation::SetFills {
            id: f.rect,
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(gradient.into()),
                visible: true,
            }]),
        }]),
        "gradient fill",
    );
}

#[test]
fn a_shape_being_drawn_matches_the_shape_that_lands() {
    // The create gesture: a node that does not exist yet is previewed as a
    // ghost, and must draw exactly where the committed node will.
    let mut f = fixture();
    let new_id = f.ids.mint();
    let index = f.doc.get(f.artboard).unwrap().children().len();
    let tx = Transaction(vec![
        Operation::CreateNode {
            id: new_id,
            parent: f.artboard,
            index,
            kind: NodeKind::Ellipse {
                size: Size::new(80.0, 45.0),
            },
            transform: Some(Affine::translate((200.0, 200.0))),
            name: None,
        },
        Operation::SetFills {
            id: new_id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(217, 217, 217, 255)),
                visible: true,
            }]),
        },
    ]);
    assert_preview_matches_commit(&f.doc, &tx, "drawing a new shape");
}

/// A whole subtree being dropped in previews as the artwork, not as a stand-in.
///
/// This is what an Alt-drag carries, and the reason it is worth a differential
/// test rather than a "something got drawn" one: a ghost that can hold children
/// has to compose their transforms through the parent ghost's, honour a group's
/// opacity as a real layer, and clip a frame's contents — three things the old
/// single-leaf ghost never had to do, and each of which is invisible until it
/// is compared against what actually commits.
#[test]
fn a_dropped_subtree_previews_as_the_artwork_it_will_become() {
    let f = fixture();
    let mut ids = IdSource::new(0xC0FF);

    // The group and the shape inside it, copied and dropped 200 to the right —
    // appended to the artboard, which is where a ghost draws.
    let template = f.doc.capture_subtree(f.group).expect("capture the group");
    let (nodes, new_root) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let base = nodes
        .iter()
        .find(|n| n.id() == new_root)
        .map(|n| n.transform())
        .unwrap();
    let index = f.doc.get(f.artboard).unwrap().children().len();

    let tx = Transaction(vec![
        Operation::InsertSubtree {
            nodes,
            parent: f.artboard,
            index,
        },
        // The op that positions the new root — about a node the document has
        // never seen, which is exactly the case that used to be dropped.
        Operation::SetTransform {
            id: new_root,
            transform: Affine::translate((200.0, 0.0)) * base,
        },
    ]);
    assert_preview_matches_commit(&f.doc, &tx, "dropping a copy of a group");
}

/// A copied *frame* is the case the old ghost could not express at all, and the
/// one that made Alt-drag move the original instead of the copy (§15, D40): a
/// frame is a container, and it clips.
#[test]
fn a_dropped_frame_previews_with_its_clip_and_its_contents() {
    let f = fixture();
    let mut ids = IdSource::new(0xC1FF);

    let template = f
        .doc
        .capture_subtree(f.artboard)
        .expect("capture the frame");
    let (nodes, new_root) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let base = nodes
        .iter()
        .find(|n| n.id() == new_root)
        .map(|n| n.transform())
        .unwrap();
    let index = f.doc.get(f.root).unwrap().children().len();

    let tx = Transaction(vec![
        Operation::InsertSubtree {
            nodes,
            parent: f.root,
            index,
        },
        Operation::SetTransform {
            id: new_root,
            transform: Affine::translate((900.0, 0.0)) * base,
        },
    ]);
    assert_preview_matches_commit(&f.doc, &tx, "dropping a copy of a frame");

    // And it really is drawing the container, not just the frame's own fill: the
    // clip layer the artboard needs is in the stream twice over once the copy is
    // previewed alongside the original.
    let res = Resolved::rebuild(&f.doc);
    let ov = RenderOverrides::from_transaction(&f.doc, &res, &tx).expect("previewable");
    let pushes = record(&f.doc, &res, &ov)
        .iter()
        .filter(|c| matches!(c, Call::Push(Some(_), _)))
        .count();
    assert_eq!(
        pushes, 2,
        "the ghosted frame did not push a clip of its own"
    );
}

/// A subtree whose ids the document already holds is not a copy of anything —
/// `apply` would reject it, so the preview must too.
#[test]
fn a_subtree_of_existing_nodes_is_refused() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let nodes = f.doc.capture_subtree(f.group).expect("capture");
    assert!(
        RenderOverrides::from_transaction(
            &f.doc,
            &res,
            &Transaction(vec![Operation::InsertSubtree {
                nodes,
                parent: f.artboard,
                index: 0,
            }])
        )
        .is_none(),
        "a subtree that collides with the document should not be previewable"
    );
}

#[test]
fn structural_transactions_are_refused() {
    // Refusing is the point: previewing these would need a real tree edit, and
    // guessing would show something other than what commits. A **reorder** was a
    // third row until it became representable as an order override on the parent
    // (§15 D877) — `a_reorder_previews_as_its_commit` has it now — and an index
    // `apply` would refuse still is refused, which is this row.
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    for (tx, what) in [
        (
            Transaction(vec![Operation::DeleteNode { id: f.rect }]),
            "delete",
        ),
        (
            Transaction(vec![Operation::Reparent {
                id: f.rect,
                new_parent: f.group,
                index: 0,
            }]),
            "reparent",
        ),
        (
            Transaction(vec![Operation::Reorder {
                id: f.rect,
                index: 99,
            }]),
            "a reorder past the end",
        ),
    ] {
        assert!(
            RenderOverrides::from_transaction(&f.doc, &res, &tx).is_none(),
            "{what} should not be previewable"
        );
    }
}

#[test]
fn values_the_commit_would_reject_are_refused() {
    // A preview must never show an edit that `apply` will bounce.
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    assert!(
        RenderOverrides::from_transaction(
            &f.doc,
            &res,
            &Transaction(vec![Operation::SetOpacity {
                id: f.rect,
                opacity: 4.0,
            }])
        )
        .is_none(),
        "out-of-range opacity"
    );

    // A layout the commit refuses (§15 D937, the release review's `[R1-L2-05]`):
    // a repeat count past the track cap, which the preview drew as the capped
    // grid the template lays and the commit then refused. Flip run: `absorb`'s
    // `SetDisplay` check deleted fails on "a template past the track cap".
    use ondin_core::container::{Display, Grid, LayoutItem, Track, TrackBreadth, TrackSize};
    let too_many = Grid {
        columns: vec![Track::Repeat {
            repeat: 600,
            tracks: vec![
                TrackSize::Breadth(TrackBreadth::Fr(1.0)),
                TrackSize::Breadth(TrackBreadth::Fr(2.0)),
            ],
        }],
        ..Default::default()
    };
    for (tx, what) in [
        (
            Transaction(vec![Operation::SetDisplay {
                id: f.artboard,
                display: Some(Display::Grid(too_many)),
            }]),
            "a template past the track cap",
        ),
        (
            Transaction(vec![Operation::SetLayoutItem {
                id: f.rect,
                item: LayoutItem {
                    grow: -1.0,
                    ..Default::default()
                },
            }]),
            "a negative grow",
        ),
    ] {
        assert!(
            RenderOverrides::from_transaction(&f.doc, &res, &tx).is_none(),
            "{what}"
        );
    }

    // A frame may be created inside a group (§15 D870) and never under a mask
    // (§15 D876), so its ghost is drawn in the first and not in the second — as a
    // create and as a paste. (Inside another artboard it can — frames nest, §5.3.)
    // Flip run: `CreateNode`'s ancestor check disabled fails on "a frame under a
    // mask", and `frame_barred` disabled on "a paste holding a frame, into a mask".
    let mut ids = IdSource::new(0xBAD);
    let frame_in = |ids: &mut IdSource, parent| {
        Transaction(vec![Operation::CreateNode {
            id: ids.mint(),
            parent,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(10.0, 10.0),
            },
            transform: None,
            name: None,
        }])
    };
    assert!(
        RenderOverrides::from_transaction(&f.doc, &res, &frame_in(&mut ids, f.group)).is_some(),
        "a frame in a group"
    );
    let mut masked = f.doc.clone();
    masked
        .apply(&Transaction(vec![Operation::SetMask {
            id: f.group,
            mask: true,
        }]))
        .expect("the fixture's group can mask");
    let masked_res = Resolved::rebuild(&masked);
    assert!(
        RenderOverrides::from_transaction(&masked, &masked_res, &frame_in(&mut ids, f.group))
            .is_none(),
        "a frame under a mask"
    );
    // The same as a paste: a group holding a frame, into the mask...
    let (g, card) = (ids.mint(), ids.mint());
    let mut holder = masked.clone();
    holder
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: g,
                parent: f.artboard,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: card,
                parent: g,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(10.0, 10.0),
                },
                transform: None,
                name: None,
            },
        ]))
        .unwrap();
    let mut remap = IdSource::new(0xBAE);
    let (nodes, _) =
        ondin_core::remap_subtree(&holder.capture_subtree(g).expect("a capture"), &mut remap)
            .expect("a remap");
    let paste = |parent| {
        Transaction(vec![Operation::InsertSubtree {
            nodes: nodes.clone(),
            parent,
            index: 0,
        }])
    };
    assert!(
        RenderOverrides::from_transaction(&masked, &masked_res, &paste(f.artboard)).is_some(),
        "the control: the paste beside the mask"
    );
    assert!(
        RenderOverrides::from_transaction(&masked, &masked_res, &paste(f.group)).is_none(),
        "a paste holding a frame, into a mask"
    );
    // And the legal one *is* previewable, so the ghost the frame tool draws inside
    // a frame is not quietly suppressed along with it.
    assert!(
        RenderOverrides::from_transaction(
            &f.doc,
            &res,
            &Transaction(vec![Operation::CreateNode {
                id: ids.mint(),
                parent: f.artboard,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(10.0, 10.0),
                },
                transform: None,
                name: None,
            }])
        )
        .is_some(),
        "a frame inside a frame"
    );
    let _ = f.root;
}

#[test]
fn a_mismatched_geometry_patch_is_refused() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    // A corner radius means nothing to a group.
    assert!(
        RenderOverrides::from_transaction(
            &f.doc,
            &res,
            &Transaction(vec![Operation::SetGeometry {
                id: f.group,
                geometry: GeometryPatch::CornerRadius(4.0),
            }])
        )
        .is_none()
    );
}

#[test]
fn a_moving_subtree_is_not_culled_against_stale_bounds() {
    // Dragging an off-screen node into view must draw it; the cached bounds
    // still say it is outside.
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let far = Affine::translate((10_000.0, 10_000.0));
    let mut doc = f.doc.clone();
    doc.apply(&Transaction(vec![Operation::SetTransform {
        id: f.rect,
        transform: far,
    }]))
    .unwrap();
    let far_res = Resolved::rebuild(&doc);

    // Off-screen and culled.
    let culled = record(&doc, &far_res, &RenderOverrides::default());

    // Now drag it back into view.
    let tx = Transaction(vec![Operation::SetTransform {
        id: f.rect,
        transform: Affine::translate((120.0, 90.0)),
    }]);
    let ov = RenderOverrides::from_transaction(&doc, &far_res, &tx).unwrap();
    let previewed = record(&doc, &far_res, &ov);

    assert!(
        previewed.len() > culled.len(),
        "the dragged node should reappear: {} vs {}",
        previewed.len(),
        culled.len()
    );
    let _ = res;
}

/// Where a fill drawn with `transform` sits: how many clip layers deep, its index
/// among all the fills, and how many clip layers the stream pushed at all.
///
/// Keyed on the transform because that is what identifies *one node's* fill in a
/// flat call stream — asking "is any fill clipped" would answer yes for the frame's
/// other children, which is correct and not the question. (It did, on the first
/// attempt at this test.)
fn fill_position(calls: &[Call], transform: [f64; 6]) -> (Option<usize>, Option<usize>, usize) {
    let (mut depth, mut pushes, mut seen) = (0usize, 0usize, 0usize);
    let (mut at_depth, mut index) = (None, None);
    for call in calls {
        match call {
            Call::Push(Some(_), _) => {
                depth += 1;
                pushes += 1;
            }
            Call::Pop => depth = depth.saturating_sub(1),
            Call::Fill(t, ..) => {
                if *t == transform {
                    at_depth = Some(depth);
                    index = Some(seen);
                }
                seen += 1;
            }
            _ => {}
        }
    }
    (at_depth, index, pushes)
}

/// **A layer on its way out of a frame draws in full, in its own z position.**
///
/// The reported symptom: drag a shape out of a frame and only its bounding box
/// leaves — the shape itself stays cut off at the frame's edge and snaps into view
/// the moment the button comes up. A clipping artboard limits its children, and a
/// child being dragged is still a child until the gesture commits, so the clip was
/// the truth about the tree and a lie about the gesture.
///
/// Three things asserted together, because the first version of this fix got the
/// last one wrong: the escaping fill leaves the clip, the frame goes on clipping
/// everything else, and **the fill's position among its siblings does not change**.
/// Drawing it last would have been easier and is what Figma does not do — which
/// shape covers which is what the user is steering by.
#[test]
fn a_layer_leaving_a_frame_escapes_the_clip_without_changing_z_order() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let tx = build::move_by_world(&f.doc, &res, &[f.rect], Vec2::new(700.0, 0.0))
        .expect("a move is representable");

    let inside = RenderOverrides::from_transaction(&f.doc, &res, &tx).expect("representable");
    let moved = inside
        .world_transform(&f.doc, &res, f.rect)
        .expect("the moved rect has a world transform")
        .as_coeffs();
    let (depth, order, pushes) = fill_position(&record(&f.doc, &res, &inside), moved);
    assert!(pushes > 0, "the fixture's artboard does clip");
    assert_eq!(
        depth,
        Some(1),
        "still clipped while the drop would keep it in the frame — and if that \
         stops being true, the rest of this test is moot"
    );

    let mut leaving = RenderOverrides::from_transaction(&f.doc, &res, &tx).unwrap();
    leaving.escape_clip(f.rect);
    let (out_depth, out_order, out_pushes) = fill_position(&record(&f.doc, &res, &leaving), moved);
    assert_eq!(out_depth, Some(0), "drawn outside every clip layer");
    assert!(out_pushes > 0, "the frame still clips its other children");
    assert_eq!(
        out_order, order,
        "and drawn in the same place among its siblings — not lifted to the front"
    );
}

/// Escaping moves *where* a node is drawn, never *what*: same transform, same path,
/// same brush, one fill either way.
#[test]
fn escaping_the_clip_changes_nothing_about_the_ink() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let tx = build::move_by_world(&f.doc, &res, &[f.rect], Vec2::new(700.0, 0.0)).unwrap();

    let inside = RenderOverrides::from_transaction(&f.doc, &res, &tx).unwrap();
    let mut leaving = RenderOverrides::from_transaction(&f.doc, &res, &tx).unwrap();
    leaving.escape_clip(f.rect);

    let fills = |ov: &RenderOverrides| -> Vec<Call> {
        record(&f.doc, &res, ov)
            .into_iter()
            .filter(|c| matches!(c, Call::Fill(..)))
            .collect()
    };
    assert_eq!(
        fills(&inside),
        fills(&leaving),
        "the same fills, in the same order — only the clip around one of them moved"
    );
}

/// The Alt-drag copy leaves a frame exactly as a moved layer does, and was clipped
/// by exactly the same edge — the ghost hangs off the original's parent, so it
/// inherits that parent's clip layer.
#[test]
fn a_ghost_leaving_a_frame_escapes_the_clip_too() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let mut ids = f.ids;
    let template = f.doc.capture_subtree(f.rect).expect("subtree");
    let (nodes, copy) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let tx = Transaction(vec![
        Operation::InsertSubtree {
            parent: f.artboard,
            index: 1,
            nodes,
        },
        Operation::SetTransform {
            id: copy,
            transform: Affine::translate((900.0, 0.0)),
        },
    ]);

    let depths = |escaping: bool| -> Vec<usize> {
        let mut ov = RenderOverrides::from_transaction(&f.doc, &res, &tx).expect("representable");
        if escaping {
            ov.escape_clip_ghosts();
        }
        let (mut depth, mut out) = (0usize, Vec::new());
        for call in &record(&f.doc, &res, &ov) {
            match call {
                Call::Push(Some(_), _) => depth += 1,
                Call::Pop => depth = depth.saturating_sub(1),
                Call::Fill(..) => out.push(depth),
                _ => {}
            }
        }
        out
    };

    let (inside, outside) = (depths(false), depths(true));
    assert_eq!(
        inside.len(),
        outside.len(),
        "the ghost is drawn either way — the question is where"
    );
    assert_eq!(
        outside.iter().filter(|d| **d == 0).count(),
        inside.iter().filter(|d| **d == 0).count() + 1,
        "escaping moves exactly one fill — the ghost's — out of the clip"
    );
}

/// Root → boolean(op) → two overlapping rects, each with its own fill.
fn boolean_fixture(op: ondin_core::BoolOp) -> (Document, NodeId, NodeId, NodeId) {
    let mut ids = IdSource::new(0xB001);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (b, a1, a2) = (ids.mint(), ids.mint(), ids.mint());
    let rect = |w: f64, h: f64| NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
    };
    let fill = |c: Color| {
        keyed_by_position([Fill {
            brush: Brush::Solid(c),
            visible: true,
        }])
    };
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
        Operation::SetFills {
            id: b,
            fills: fill(Color::from_rgba8(0, 0, 255, 255)),
        },
        Operation::SetFills {
            id: a1,
            fills: fill(Color::from_rgba8(255, 0, 0, 255)),
        },
        Operation::SetFills {
            id: a2,
            fills: fill(Color::from_rgba8(0, 255, 0, 255)),
        },
    ]))
    .expect("build the boolean");
    (doc, b, a1, a2)
}

/// **Reordering a `Subtract`'s operands previews the other cut** (§15 D877) —
/// two overlapping squares, the base moved on top, so the result is the *other*
/// square's leftover.
///
/// The third reader of the preview's child order, which `arch-scribe` found
/// missing from the first cut: the walk and the layout view read the new order
/// and the operands did not, so the preview would have drawn the old cut.
///
/// **Flip run**, `operand_children` answering the committed order: fails on the
/// differential, the predicted site. With the reordered parent dropped from
/// `reevaluate_booleans`' seeds instead, it fails there too — nothing asked the
/// boolean again.
#[test]
fn reordering_a_subtracts_operands_previews_the_other_cut() {
    let (doc, _b, a1, _a2) = boolean_fixture(ondin_core::BoolOp::Subtract);
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::Reorder { id: a1, index: 1 }]),
        "the base moved on top",
    );
}

/// **A boolean's operands never reach the canvas.** They are inputs consumed by the
/// operation, and drawing them over the result is what made the whole feature look
/// broken: Subtract, Intersect and Exclude appeared to do nothing (their result was
/// there, hidden under the operands), and dragging an operand looked like it carried
/// the original while the boolean stayed behind.
///
/// Asserted through the *paint*, since that is what the eye reported: the boolean's
/// blue is drawn and the operands' red and green are not.
#[test]
fn a_boolean_draws_its_result_and_never_its_operands() {
    let (doc, _, _, _) = boolean_fixture(ondin_core::BoolOp::Subtract);
    let res = Resolved::rebuild(&doc);
    let calls = record(&doc, &res, &RenderOverrides::default());

    let brushes: Vec<&Brush> = calls
        .iter()
        .filter_map(|c| match c {
            Call::Fill(_, _, brush, _) => Some(brush),
            _ => None,
        })
        .collect();
    let solid = |c: Color| Brush::Solid(c);
    assert_eq!(brushes.len(), 1, "one fill: the result's");
    assert_eq!(
        brushes[0],
        &solid(Color::from_rgba8(0, 0, 255, 255)),
        "and it is the boolean's own paint, not an operand's"
    );
}

/// **The result re-derives while an operand is being dragged**, rather than showing
/// the pre-drag shape until release. Reported as "the boolean is left behind and
/// re-evaluates when I drop".
///
/// The preview owns its own evaluation (`RenderOverrides::reevaluate_booleans`)
/// because `Resolved`'s cache is only refreshed on commit.
#[test]
fn dragging_an_operand_reshapes_the_boolean_in_the_preview() {
    let (doc, b, _, a2) = boolean_fixture(ondin_core::BoolOp::Intersect);
    let res = Resolved::rebuild(&doc);
    let committed = res.boolean_path(b).expect("an overlap").to_svg();

    // Slide the top operand right: the intersection narrows.
    let tx = build::move_by_world(&doc, &res, &[a2], Vec2::new(25.0, 0.0)).unwrap();
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");

    let previewed = ov
        .boolean_path(b)
        .expect("re-derived for the preview")
        .to_svg();
    assert_ne!(
        previewed, committed,
        "the preview outline must differ from the committed one — equal means the \
         drag is showing the shape from before it started"
    );
    // And the scene draws the preview's outline, not the cache's.
    let calls = record(&doc, &res, &ov);
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::Fill(_, d, _, _) if *d == previewed)),
        "the walk drew the re-derived outline"
    );
}

/// **A resize reshapes the boolean live, sizes included.**
///
/// The reported symptom, in three photos: dragging a corner handle moved the
/// subtracted circle around inside the rectangle while the rectangle itself — and
/// the bounding box — stayed put, then everything snapped to the right size on
/// release.
///
/// The cause was that the preview honoured the operands' *transform* overrides and
/// read their geometry from the committed document, so a resize (which is partly a
/// `SetGeometry`) moved each operand without resizing it. Asserted on the box rather
/// than the path: "the outline changed" was already true of the broken version — it
/// was changing in the wrong way — and it is the box the badge and the handles show.
#[test]
fn resizing_a_boolean_grows_its_previewed_outline() {
    let (doc, b, a1, a2) = boolean_fixture(ondin_core::BoolOp::Subtract);
    let res = Resolved::rebuild(&doc);
    let before = res.boolean_path(b).expect("a result").bounding_box();

    // What a corner drag produces: every operand scaled 2x about the box origin —
    // new geometry *and* the transform that repositions it.
    let tx = Transaction(vec![
        Operation::SetGeometry {
            id: a1,
            geometry: GeometryPatch::Size(Size::new(200.0, 200.0)),
        },
        Operation::SetGeometry {
            id: a2,
            geometry: GeometryPatch::Size(Size::new(200.0, 200.0)),
        },
        Operation::SetTransform {
            id: a2,
            transform: Affine::translate((100.0, 0.0)),
        },
    ]);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");
    let after = ov
        .boolean_path(b)
        .expect("re-derived for the preview")
        .bounding_box();

    assert!(
        (after.width() - before.width() * 2.0).abs() < 1.0
            && (after.height() - before.height() * 2.0).abs() < 1.0,
        "the previewed outline should double with its operands: {before:?} then {after:?}"
    );
}

/// **The copy an Alt-drag carries previews as the boolean, not as its operands.**
///
/// Reported as "alt-drag shows a copy of the original shapes on the clone instead of
/// the boolean". Two faults with one symptom, and the ghost walk had both: it had no
/// derived outline to draw, and — because nothing told it to stop at a boolean the way
/// `paint_node` does — it went on to draw the children. So a subtracted bite looked
/// whole and un-cut while it was being dragged and appeared at the instant of release.
///
/// Differential, because that is the property: a ghost drawing *nothing* would also be
/// wrong, and only a comparison against the commit says which of the three it is.
#[test]
fn a_ghost_boolean_previews_its_own_result_and_not_its_operands() {
    let (doc, _, _, _) = boolean_fixture(ondin_core::BoolOp::Subtract);
    let mut ids = IdSource::new(0xC0FE);
    let root = doc.root();
    let original = doc.get(root).unwrap().children()[0];

    // Exactly what Alt-drag emits: the subtree captured, remapped, appended to the
    // same parent, then placed by the pointer.
    let template = doc.capture_subtree(original).expect("capture the boolean");
    let (nodes, new_root) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let base = nodes
        .iter()
        .find(|n| n.id() == new_root)
        .map(|n| n.transform())
        .unwrap();
    let index = doc.get(root).unwrap().children().len();
    let tx = Transaction(vec![
        Operation::InsertSubtree {
            nodes,
            parent: root,
            index,
        },
        Operation::SetTransform {
            id: new_root,
            transform: Affine::translate((200.0, 0.0)) * base,
        },
    ]);
    assert_preview_matches_commit(&doc, &tx, "a copy of a boolean group");

    // And the count in its own right, so a regression names the symptom rather than
    // printing two long call streams: two shapes on the page, two fills, both the
    // boolean's blue. Three or more means the operands are back.
    let res = Resolved::rebuild(&doc);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");
    let fills: Vec<Brush> = record(&doc, &res, &ov)
        .iter()
        .filter_map(|c| match c {
            Call::Fill(_, _, brush, _) => Some(brush),
            _ => None,
        })
        .cloned()
        .collect();
    let blue = Brush::Solid(Color::from_rgba8(0, 0, 255, 255));
    assert_eq!(
        fills.len(),
        2,
        "the original and the ghost, one fill each — more means an operand was drawn"
    );
    assert!(
        fills.iter().all(|b| *b == blue),
        "and both are the boolean's own paint: {fills:?}"
    );
}

/// A boolean **nested inside** the boolean being copied is evaluated too, and the
/// order it happens in is the thing to get wrong: the outer operation reads the
/// inner one's result, so a walk that built the parent first would combine against
/// nothing. `build_ghost` recurses children-first, which is why it works, and this
/// is what says so.
#[test]
fn a_ghost_evaluates_a_boolean_inside_a_boolean() {
    let (mut doc, inner, _, _) = boolean_fixture(ondin_core::BoolOp::Union);
    let mut ids = IdSource::new(0xC0FD);
    let root = doc.root();
    // Wrap the existing boolean and a third rect in an outer Subtract.
    let extra = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: extra,
        parent: root,
        index: 1,
        kind: NodeKind::Rect {
            size: Size::new(60.0, 200.0),
            corner_radii: RoundedRectRadii::default(),
        },
        transform: Some(Affine::translate((0.0, -50.0))),
        name: None,
    }]))
    .expect("a third shape");
    let res = Resolved::rebuild(&doc);
    let (tx, outer) = build::boolean(
        &doc,
        &res,
        &mut ids,
        &[inner, extra],
        ondin_core::BoolOp::Subtract,
        None,
    )
    .unwrap();
    doc.apply(&tx).expect("wrap");

    let template = doc.capture_subtree(outer).expect("capture");
    let (nodes, new_root) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let tx = Transaction(vec![
        Operation::InsertSubtree {
            nodes,
            parent: root,
            index: doc.get(root).unwrap().children().len(),
        },
        Operation::SetTransform {
            id: new_root,
            transform: Affine::translate((300.0, 0.0)),
        },
    ]);
    assert_preview_matches_commit(&doc, &tx, "a copy of a boolean of a boolean");
}

/// **Dragging an operand of the *inner* boolean re-evaluates the outer one too,
/// and in the right order.**
///
/// `[S11.2-L1-02]`, §15 D461. `renderer::reevaluate_booleans` counted its sort
/// key **upward from the seed**, so a boolean nearer the root carried the larger
/// number and `Reverse` evaluated the **outermost** first — the opposite of the
/// rule its own comment states. Measured in release on `Subtract(Union(r1, r2),
/// r3)`, dragging `r2` 260 units right: the inner union's preview followed the
/// pointer to `x1 = 360` while the outer subtract sat at its pre-drag `x1 = 160`
/// for the whole gesture and snapped into place on release.
///
/// ⚠️ **Silent, because the fallback is plausible.** With the outer evaluated
/// first, `self.boolean` holds no entry for the inner one yet, so the `nested:`
/// closure falls through to `Resolved`'s **committed** cache — a real path of the
/// right shape, from before the gesture. §6.2's rule is that a missing preview is
/// recoverable and a lying one is not.
///
/// ⚠️ **This function had no nested-boolean test at all, and the one that looks
/// like it is not one.** `a_ghost_evaluates_a_boolean_inside_a_boolean` seeds
/// from `g.parent`, which is the root and not a boolean, so `affected` comes back
/// empty and what it exercises is `build_ghost`'s own children-first recursion.
/// Checked by breaking the sort and watching that test stay green.
///
/// ⚠️ **Flipped** by restoring the seed-relative key: fails inside
/// `assert_preview_matches_commit` on the two draw-call streams, at the outer
/// boolean's path. The **control** below — dragging an operand of the *outer*
/// boolean, where `affected` holds one entry and no ordering decision is made —
/// stays green under the flip, which is what says the ordering is the cause and
/// not the seeding.
#[test]
fn dragging_an_operand_of_a_nested_boolean_previews_the_outer_one_too() {
    let (mut doc, inner, _r1, r2) = boolean_fixture(ondin_core::BoolOp::Union);
    let mut ids = IdSource::new(0xC0FE);
    let root = doc.root();
    let r3 = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: r3,
        parent: root,
        index: 1,
        kind: NodeKind::Rect {
            size: Size::new(60.0, 200.0),
            corner_radii: RoundedRectRadii::default(),
        },
        transform: Some(Affine::translate((0.0, -50.0))),
        name: None,
    }]))
    .expect("a third shape");
    let res = Resolved::rebuild(&doc);
    let (wrap, _outer) = build::boolean(
        &doc,
        &res,
        &mut ids,
        &[inner, r3],
        ondin_core::BoolOp::Subtract,
        None,
    )
    .expect("Subtract(Union(r1, r2), r3)");
    doc.apply(&wrap).expect("wrap");

    // The gesture: drag `r2`, an operand of the **inner** union, 260 units right.
    // Two booleans are affected and their order is the whole question.
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetTransform {
            id: r2,
            transform: Affine::translate((310.0, 0.0)),
        }]),
        "drag an operand of the inner boolean",
    );

    // **The control**: drag `r3`, an operand of the outer boolean. `affected`
    // holds one entry, so no ordering decision is made at all.
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetTransform {
            id: r3,
            transform: Affine::translate((20.0, -50.0)),
        }]),
        "drag an operand of the outer boolean",
    );
}

/// **A ghost operand joins the operation it is being dropped into.**
///
/// Alt-dragging a copy of a shape that is *inside* a boolean parents the copy to the
/// boolean — that is where `canvas::ClonedSubtree` lands a copy, beside its source —
/// and on release it becomes a third operand. So the preview has to combine it, or the
/// shape sits at its pre-drag outline and jumps at the instant of the drop: the same
/// report `reevaluate_booleans` was written for, one gesture over.
///
/// It needed two things. The seed: a moved node reshapes the booleans *above* it, while
/// a ghost reshapes the one it is parented *to*, so a ghost's walk starts at its parent
/// rather than its parent's parent. And the lookups: `children_of` has to include the
/// ghost, and `local_of` has to be able to answer for a ghost id — it used to return
/// `Affine::IDENTITY`, having looked for an override entry that `absorb` writes into the
/// ghost instead.
#[test]
fn a_ghost_operand_widens_the_boolean_it_is_dropped_into() {
    let (doc, b, _, a2) = boolean_fixture(ondin_core::BoolOp::Union);
    let mut ids = IdSource::new(0xC0FC);
    let res = Resolved::rebuild(&doc);
    let before = res.boolean_path(b).expect("a union").bounding_box();
    assert!(
        (before.width() - 150.0).abs() < 0.5,
        "two 100s over 50: {before:?}"
    );

    // The copy lands in the boolean, 120 to the right — clear of both operands, so the
    // union has to grow to 250 wide rather than stay at 150.
    let template = doc.capture_subtree(a2).expect("capture the operand");
    let (nodes, new_root) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let tx = Transaction(vec![
        Operation::InsertSubtree {
            nodes,
            parent: b,
            index: doc.get(b).unwrap().children().len(),
        },
        Operation::SetTransform {
            id: new_root,
            transform: Affine::translate((170.0, 0.0)),
        },
    ]);

    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");
    let previewed = ov
        .boolean_path(b)
        .expect("re-derived with the ghost as an operand")
        .bounding_box();
    assert!(
        (previewed.width() - 270.0).abs() < 0.5,
        "the ghost is an operand, so the union reaches it: {previewed:?}"
    );

    // The whole property in one line: the ghost is *not drawn* — it is an input, like
    // every other operand — and what changes on screen is the result's own shape.
    assert_preview_matches_commit(&doc, &tx, "a copy of an operand inside a boolean");
}

/// **A ghost operand folds where the commit will fold it, not last** — and this is
/// the test `operand_children`'s own doc said did not exist.
///
/// That doc excused appending on the grounds that `boolean::evaluate` is a left fold
/// where only the first operand is order-sensitive, "the other three commute". ⚠️ **The
/// commuting is true of the mathematics and false of this implementation.** Measured
/// 2026-08-31: moving one operand of a twenty-circle `Exclude` to the end of the fold
/// changes the result by 2,482 area units of 85,850 — about 3%, at an unchanged element
/// count — and the error grows with the count (0.009 at five operands, 1,654 at twelve).
/// `Union` and `Subtract` move by float noise, which is why appending looked safe for as
/// long as nobody tried it on an `Exclude`.
///
/// **Twelve operands and `Exclude`, both load-bearing.** `boolean_fixture`'s two-operand
/// `Union` cannot show this: at two operands there is nothing to reorder, and `Union`'s
/// reordering error is 0.03 area units at twenty, far under any comparison. The ring is
/// the same fixture the cost table is measured on, at the smallest count where the
/// divergence is unmistakable.
///
/// **Index 1, not the end.** The insert has to land *mid-list* or the fix and the bug
/// produce the same fold order and the test is about nothing — which is exactly the hole
/// the old doc named: every existing caller appended.
#[test]
fn a_ghost_operand_folds_in_the_slot_its_insert_names() {
    let mut ids = IdSource::new(0xE0DD);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let b = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: b,
        parent: root,
        index: 0,
        kind: NodeKind::Boolean {
            op: ondin_core::BoolOp::Exclude,
        },
        transform: None,
        name: None,
    }];
    for i in 0..12 {
        let a = f64::from(i) / 12.0 * std::f64::consts::TAU;
        let c = Point::new(200.0 + 120.0 * a.cos(), 200.0 + 120.0 * a.sin());
        ops.push(Operation::CreateNode {
            id: ids.mint(),
            parent: b,
            index: i as usize,
            kind: NodeKind::Ellipse {
                size: Size::new(180.0, 180.0),
            },
            transform: Some(Affine::translate((c.x - 90.0, c.y - 90.0))),
            name: None,
        });
    }
    ops.push(Operation::SetFills {
        id: b,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgb8(0x40, 0x80, 0xC0)),
            visible: true,
        }]),
    });
    doc.apply(&Transaction(ops)).expect("build the ring");

    let first = doc.get(b).unwrap().children()[0];
    let template = doc.capture_subtree(first).expect("capture an operand");
    let (nodes, new_root) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let tx = Transaction(vec![
        Operation::InsertSubtree {
            nodes,
            parent: b,
            index: 1,
        },
        Operation::SetTransform {
            id: new_root,
            transform: Affine::translate((40.0, 25.0)),
        },
    ]);

    assert_preview_matches_commit(&doc, &tx, "a ghost operand landing mid-list in an Exclude");
}

/// **A ghost draws in the slot its insert names, not on top of everything.**
///
/// An Alt-drag copy lands immediately above its original — one step up the stack —
/// so a ghost deferred to the end of the walk previewed a z-order the release would
/// not produce: the copy appeared over siblings it was going to end up *under*.
/// Appending callers (a shape tool dragging out a new node) are unaffected, which is
/// why this went unnoticed for as long as every caller appended.
#[test]
fn a_ghost_draws_at_its_index_not_on_top() {
    let mut ids = IdSource::new(0xE0E0);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let three: Vec<NodeId> = (0..3).map(|_| ids.mint()).collect();
    let mut ops = Vec::new();
    for (i, id) in three.iter().enumerate() {
        ops.push(Operation::CreateNode {
            id: *id,
            parent: root,
            index: i,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((i as f64 * 20.0, 0.0))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: *id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(10 * i as u8, 0, 0, 255)),
                visible: true,
            }]),
        });
    }
    doc.apply(&Transaction(ops))
        .expect("three overlapping rects");

    // A copy of the *bottom* rect, landing just above it — so two siblings remain
    // above the ghost and the difference between "at index 1" and "last" is visible.
    let template = doc.capture_subtree(three[0]).expect("capture");
    let (nodes, new_root) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let tx = Transaction(vec![
        Operation::InsertSubtree {
            nodes,
            parent: root,
            index: 1,
        },
        Operation::SetTransform {
            id: new_root,
            transform: Affine::translate((5.0, 5.0)),
        },
    ]);
    assert_preview_matches_commit(&doc, &tx, "a copy landing above its original");

    // And the order in its own right, so a regression names the symptom: the ghost's
    // fill is the second of four, not the last.
    let res = Resolved::rebuild(&doc);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");
    let order: Vec<[f64; 6]> = record(&doc, &res, &ov)
        .iter()
        .filter_map(|c| match c {
            Call::Fill(t, _, _, _) => Some(*t),
            _ => None,
        })
        .collect();
    assert_eq!(order.len(), 4, "three rects and the copy");
    assert_eq!(
        order[1][4], 5.0,
        "the copy is drawn second — at index 1 — not last: {order:?}"
    );
}

/// **A reorder's preview paints in the order its commit does** (§15 D877) —
/// three overlapping rects at x 0, 20 and 40, the bottom one moved to the top.
///
/// The paint order is the whole of what a reorder changes where nothing lays it
/// out, so it is asserted directly — the fills' x offsets in draw order — as
/// well as by the pixel differential.
///
/// **Flip run**, the scene walk's `children_of` dropped (the committed order
/// read): fails on the differential, the predicted site, before the order
/// assertion is reached — which is there to name the symptom when it is.
#[test]
fn a_reorder_changes_the_paint_order_it_commits() {
    let mut ids = IdSource::new(0xE0E1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let three: Vec<NodeId> = (0..3).map(|_| ids.mint()).collect();
    let mut ops = Vec::new();
    for (i, id) in three.iter().enumerate() {
        ops.push(Operation::CreateNode {
            id: *id,
            parent: root,
            index: i,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((i as f64 * 20.0, 0.0))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: *id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(60 * i as u8 + 40, 0, 0, 255)),
                visible: true,
            }]),
        });
    }
    doc.apply(&Transaction(ops))
        .expect("three overlapping rects");

    let tx = Transaction(vec![Operation::Reorder {
        id: three[0],
        index: 2,
    }]);
    assert_preview_matches_commit(&doc, &tx, "the bottom rect raised to the top");

    let res = Resolved::rebuild(&doc);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");
    let xs: Vec<f64> = record(&doc, &res, &ov)
        .iter()
        .filter_map(|c| match c {
            Call::Fill(t, _, _, _) => Some(t[4]),
            _ => None,
        })
        .collect();
    assert_eq!(xs, vec![20.0, 40.0, 0.0], "drawn in the new order");
}

/// **A chrome ghost draws where its index says, and is not a preview** (§15
/// D268).
///
/// The faded whole photograph behind a layer being cropped is artwork with no
/// `Node` behind it at a chosen depth in one parent's z-order, which is what
/// `Ghost` already is — so it rides that machinery rather than a second copy of
/// it. What it must *not* ride is `ghosts`, and this is the half that would
/// otherwise be found by hand: `is_empty` is how the app asks "is a gesture in
/// flight" (`Session::has_gesture_preview`), so a mode that answered yes for as
/// long as it was open would have every right-click unwinding a drag nobody
/// started. Two claims, and they pull in opposite directions — draw like a
/// ghost, count like nothing — so both are asserted together or the next edit
/// satisfies one by breaking the other.
///
/// **Differential against the same walk without it**, rather than "a fill was
/// emitted": the position is the whole point. Drawn at the layer's own index it
/// lands *under* the layer, which is what leaves the crop at full strength and
/// the offcut faded; appended, as ghosts used to be, it would cover the picture
/// it is explaining.
#[test]
fn a_chrome_ghost_draws_under_its_layer_without_counting_as_a_preview() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let plain = record(&f.doc, &res, &RenderOverrides::default());

    // A colour nothing else in the fixture uses, so the extra call is findable
    // by what it paints rather than by counting.
    let marker = Brush::Solid(Color::from_rgba8(1, 2, 3, 255));
    let mut ov = RenderOverrides::default();
    ov.set_chrome_ghost(Some((
        // The layer the ghost is glued to, which is what exempts the pair from
        // pixel snapping (§15 D272).
        f.rect,
        ondin_render::Ghost {
            // `f.rect` is the artboard's child 1, so this is the crop ghost's own
            // arrangement: same parent, same index, drawn immediately before it.
            parent: f.artboard,
            index: 1,
            node: ondin_render::GhostNode {
                id: NodeId {
                    actor: u64::MAX,
                    seq: u64::MAX,
                },
                transform: Affine::translate((100.0, 70.0)),
                kind: NodeKind::Rect {
                    size: Size::new(140.0, 90.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                paint: ondin_core::Paint {
                    fills: keyed_by_position([Fill {
                        brush: marker.clone(),
                        visible: true,
                    }]),
                    strokes: Vec::new(),
                },
                effects: Vec::new(),
                text: None,
                path: None,
                opacity: 0.3,
                visible: true,
                clip: false,
                mask: false,
                mask_mode: Default::default(),
                children: Vec::new(),
            },
        },
    )));

    assert!(
        ov.is_empty(),
        "a chrome ghost is not a preview — `has_gesture_preview` reads this, and \
         a mode that reported a live gesture would have every right-click \
         cancelling one"
    );

    let with = record(&f.doc, &res, &ov);
    // **Fills only.** A ghost carrying an opacity opens and closes a layer round
    // itself, so the raw call stream gains more than one entry — bookkeeping this
    // test has no claim about. What it does claim is about *ink*: one more piece
    // of it, in one particular place.
    let ink = |calls: &[Call]| -> Vec<Brush> {
        calls
            .iter()
            .filter_map(|c| match c {
                Call::Fill(_, _, b, _) => Some(b.clone()),
                _ => None,
            })
            .collect()
    };
    let (before, after) = (ink(&plain), ink(&with));
    let at = after
        .iter()
        .position(|b| *b == marker)
        .expect("the chrome ghost painted nothing at all");
    // Inserted, not appended and not replacing: everything before it is what the
    // plain walk drew, and everything after it is the rest of that walk.
    assert_eq!(
        after.len(),
        before.len() + 1,
        "the ghost drew {} fills where one was expected",
        after.len() - before.len()
    );
    assert_eq!(
        after[..at],
        before[..at],
        "the ink before the ghost differs"
    );
    assert_eq!(
        after[at + 1..],
        before[at..],
        "the ink after the ghost differs"
    );
    // Ahead of the comparison below rather than left to index off the end, so the
    // one-index-too-late version reports the symptom instead of panicking on a
    // slice.
    assert!(
        at < before.len(),
        "the ghost drew last, over the picture it is explaining rather than under \
         it — the offcut would be covering the crop"
    );
    // And the fill it displaced is the cropped layer's own, which is what "under
    // it" means — an appended ghost, which is what ghosts used to be, covers the
    // very picture it is explaining.
    assert!(
        matches!(&before[at], Brush::Solid(c) if c.to_rgba8().r == 70),
        "the ghost landed above {:?}, not immediately under the layer it explains",
        before[at]
    );
}

/// **The offcut and the crop are one photograph** — the ghost's pixels line up
/// exactly with the layer's, at every frame width (§15 D272).
///
/// `ImageRef::picture_box` is defined so that the whole picture lands where the
/// crop's own mapping says it must, and the ghost is a `Rect` at that box showing
/// the source uncropped. Compose each one's brush transform with the transform the
/// walk paints it under and both should map source pixel → page **identically**:
/// one photograph, drawn twice, at two exposures.
///
/// **Reported as a 1–2pt shift on the ghost while resizing the crop**, with the
/// picture inside the window looking right — which is the signature of the two
/// boxes being rounded independently rather than of the arithmetic being wrong.
/// So this sweeps the frame width across a range of sub-pixel positions instead of
/// checking one: at a single width the two can agree by luck, and the whole
/// complaint was that it drifts *as you drag*.
#[test]
fn the_crop_ghosts_picture_lands_exactly_where_the_layers_does() {
    use ondin_core::{ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource, image_brush};

    #[derive(Default)]
    struct Framings(Vec<(Affine, Option<ondin_core::Framing>)>);
    impl ScenePainter for Framings {
        fn has_image(&self, _id: &ImageId) -> bool {
            true
        }
        fn fill_path(
            &mut self,
            t: Affine,
            _p: &BezPath,
            brush: &Brush,
            framing: Option<ondin_core::Framing>,
            _rule: ClipRule,
        ) {
            if matches!(brush, Brush::Image(_)) {
                self.0.push((t, framing));
            }
        }
        fn stroke_path(&mut self, _t: Affine, _p: &BezPath, _s: &StrokePaint<'_>) {}
        fn draw_text(&mut self, _t: Affine, _r: &TextRun<'_>) {}
        fn push_layer(&mut self, _t: Affine, _c: Option<&BezPath>, _r: ClipRule, _o: f32) {}
        fn push_mask_layer(&mut self) {}
        fn pop_layer(&mut self) {}
    }

    let key = ImageId("sha256:offcut".into());
    let (w, h) = (400u32, 300u32);
    // A crop of a bit under half the source, off centre, so a rule that only holds
    // for a centred or a whole-source rectangle has somewhere to go wrong.
    let crop = Rect::new(0.2, 0.15, 0.6, 0.55);

    // Sub-pixel frame widths, which is what a drag walks through. Any one of them
    // could agree by luck; the complaint was about the walk between them.
    let mut worst = 0.0_f64;
    for step in 0..12 {
        let fw = 120.0 + f64::from(step) * 0.25;
        let f = fixture();
        let mut doc = f.doc;
        let mut img = ImageRef::new(key.clone());
        img.fit = ondin_core::ImageFit::Crop;
        img.crop = crop;
        let Brush::Image(mut brush) = image_brush(key.clone()) else {
            unreachable!()
        };
        brush.image = img.clone();

        doc.apply(&Transaction(vec![
            Operation::AddImage {
                id: key.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(vec![0].into()),
                    format: ImageFormat::Png,
                    width: w,
                    height: h,
                },
            },
            Operation::SetGeometry {
                id: f.rect,
                geometry: GeometryPatch::Size(Size::new(fw, 70.0)),
            },
            Operation::SetFills {
                id: f.rect,
                fills: keyed_by_position([Fill {
                    brush: Brush::Image(brush.clone()),
                    visible: true,
                }]),
            },
        ]))
        .expect("fixture applies");
        let res = Resolved::rebuild(&doc);

        // The chrome ghost exactly as `canvas::image_ghost` builds it.
        let frame = Rect::new(0.0, 0.0, fw, 70.0);
        let limit = img.picture_box(frame, w, h).expect("a picture box");
        let mut whole = brush.clone();
        whole.image.crop = ondin_core::whole_crop();
        let mut ov = RenderOverrides::default();
        // Paired with the layer it continues, which is what exempts both from
        // pixel snapping — the whole subject of this test.
        ov.set_chrome_ghost(Some((
            f.rect,
            ondin_render::Ghost {
                parent: f.artboard,
                index: 1,
                node: ondin_render::GhostNode {
                    id: NodeId {
                        actor: u64::MAX,
                        seq: u64::MAX,
                    },
                    transform: doc.get(f.rect).expect("the rect").transform()
                        * Affine::translate(limit.origin().to_vec2()),
                    kind: NodeKind::Rect {
                        size: limit.size(),
                        corner_radii: RoundedRectRadii::default(),
                    },
                    paint: ondin_core::Paint {
                        fills: keyed_by_position([Fill {
                            brush: Brush::Image(whole),
                            visible: true,
                        }]),
                        strokes: Vec::new(),
                    },
                    effects: Vec::new(),
                    text: None,
                    path: None,
                    opacity: 0.3,
                    visible: true,
                    clip: false,
                    mask: false,
                    mask_mode: Default::default(),
                    children: Vec::new(),
                },
            },
        )));

        // **One device pixel per document unit**, so the number this reports is in
        // the units the report was in. The shared `viewport()` is at 0.2, where a
        // whole pixel of drift reads as five units and "is that a lot" has no
        // answer without arithmetic.
        let vp = Viewport {
            view: Rect::new(0.0, 0.0, 400.0, 400.0),
            pixel_size: (400, 400),
        };
        let mut painter = Framings::default();
        scene::build(&doc, &res, &vp, &ov, &mut painter);
        assert_eq!(
            painter.0.len(),
            2,
            "expected the ghost and the layer, got {} image fills",
            painter.0.len()
        );
        // Source pixel → page, for each. The ghost is painted first (it is under
        // the layer), so index 0 is the offcut.
        let mapped: Vec<Affine> = painter
            .0
            .iter()
            .map(|(t, fr)| *t * fr.expect("an image is framed").transform)
            .collect();
        // **Measured at the source's own corners, not on the matrix.** Comparing
        // coefficients weighs the translation and the scale alike, and the scale is
        // the term that hurts here: `snapped_box_world` rebuilds it from a rounded
        // span, so a half-pixel error over the frame's width is a *proportional*
        // error, and the ghost reaches several frame-widths past the window. The
        // drift the eye sees is at the offcut's far edge, so that is where this
        // looks.
        for corner in [
            Point::ZERO,
            Point::new(f64::from(w), 0.0),
            Point::new(0.0, f64::from(h)),
            Point::new(f64::from(w), f64::from(h)),
        ] {
            worst = worst.max((mapped[0] * corner - mapped[1] * corner).hypot());
        }
    }
    assert!(
        worst < 1e-6,
        "the offcut and the crop map the same pixel {worst} apart at some frame \
         width — the ghost drifts against the picture it is explaining as the \
         crop is dragged"
    );
}

/// **The drift guard between the two mask-geometry derivations.**
///
/// `Resolved::mask_path` reads the committed tree and `scene::mask_geometry`
/// reads it through the overrides; they are written to be arm-for-arm mirrors,
/// and nothing but this holds them to it. Each case drags a *different* arm:
/// moving the mask exercises the transform, resizing it the geometry, and moving
/// the layer it masks exercises the case where the clip must **not** move with
/// what is being dragged.
///
/// The fixture masks with the group, so the recursive arm is the one under test
/// — the one that unions its contents and is easiest to get subtly wrong.
#[test]
fn a_mask_previews_the_clip_that_committing_would_draw() {
    let f = fixture();
    let mut doc = f.doc;
    doc.apply(&Transaction(vec![Operation::SetMask {
        id: f.group,
        mask: true,
    }]))
    .unwrap();
    // The group is child 0 of the artboard, so `rect` and `text` above it are the
    // run it governs — without them there is no clip to compare.
    assert_eq!(
        doc.get(f.artboard).unwrap().children(),
        &[f.group, f.rect, f.text],
        "fixture: the mask is at the bottom and two layers sit above it"
    );

    for (what, tx) in [
        (
            "the mask itself moves",
            Transaction(vec![Operation::SetTransform {
                id: f.group,
                transform: Affine::translate((17.0, -9.0)),
            }]),
        ),
        (
            "the mask's own geometry changes",
            Transaction(vec![Operation::SetGeometry {
                id: f.inner,
                geometry: GeometryPatch::Size(Size::new(90.0, 25.0)),
            }]),
        ),
        (
            "a layer under the mask moves",
            Transaction(vec![Operation::SetTransform {
                id: f.rect,
                transform: Affine::translate((200.0, 10.0)),
            }]),
        ),
    ] {
        assert_preview_matches_commit(&doc, &tx, what);
    }

    // **And again as an alpha mask**, where the mask is *drawn* rather than
    // turned into a clip — so the preview has to reproduce its ink, its position
    // and the `MaskPush` it sits inside, not just a path. Same three gestures,
    // because the arm that reads them is the same one and a regression would hit
    // both modes or neither.
    doc.apply(&Transaction(vec![Operation::SetMaskMode {
        id: f.group,
        mode: ondin_core::MaskMode::Alpha,
    }]))
    .unwrap();
    for (what, tx) in [
        (
            "the alpha mask itself moves",
            Transaction(vec![Operation::SetTransform {
                id: f.group,
                transform: Affine::translate((17.0, -9.0)),
            }]),
        ),
        (
            "the alpha mask's own geometry changes",
            Transaction(vec![Operation::SetGeometry {
                id: f.inner,
                geometry: GeometryPatch::Size(Size::new(90.0, 25.0)),
            }]),
        ),
        (
            "a layer under the alpha mask moves",
            Transaction(vec![Operation::SetTransform {
                id: f.rect,
                transform: Affine::translate((200.0, 10.0)),
            }]),
        ),
    ] {
        assert_preview_matches_commit(&doc, &tx, what);
    }
}

/// **Previewing an effect must ask for the same buffer committing it asks for.**
///
/// The box an effect layer is given comes from `Resolved::ink_bounds`, which is
/// built from the *committed* document — so a preview that grows the reach
/// (softening a blur, throwing a shadow further, widening a spread) was drawn
/// into a buffer sized for the stack it is replacing, and clipped. Reported from
/// the machine: *"the effect is clipped while I add it… it applies the proper
/// effect once I release the mouse"*, which is exactly the shape of a cache that
/// only the commit refreshes.
///
/// Three cases because three different numbers reach into `Effect::escape`, and a
/// fix that read only one of them would pass the others.
#[test]
fn previewing_a_bigger_effect_asks_for_the_bigger_buffer() {
    let (doc, id) = shadowed_layer();
    for (what, effects) in [
        (
            "a softer blur",
            vec![Effect::new(EffectKind::LayerBlur { radius: 60.0 })],
        ),
        (
            "a farther shadow",
            vec![Effect::new(EffectKind::DropShadow(Shadow {
                offset: Vec2::new(0.0, 80.0),
                blur: 4.0,
                spread: 0.0,
                color: Color::BLACK,
            }))],
        ),
        (
            "a wider spread",
            vec![Effect::new(EffectKind::DropShadow(Shadow {
                offset: Vec2::new(0.0, 0.0),
                blur: 0.0,
                spread: 40.0,
                color: Color::BLACK,
            }))],
        ),
    ] {
        assert_preview_matches_commit(
            &doc,
            &Transaction(vec![Operation::SetEffects {
                id,
                effects: keyed_by_position(effects),
            }]),
            what,
        );
    }
}

/// The same question asked of the **geometry** under an effect: resizing a
/// shadowed shape moves the box its shadow is cast from, and that box is cached
/// too.
#[test]
fn previewing_a_resize_under_an_effect_asks_for_the_right_buffer() {
    let (doc, id) = shadowed_layer();
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetGeometry {
            id,
            geometry: GeometryPatch::Size(Size::new(300.0, 300.0)),
        }]),
        "a resize under a shadow",
    );
}

/// A 100×60 rect at (100, 100) carrying a small drop shadow — small on purpose,
/// so that every preview above *grows* the reach and a stale box is too tight
/// rather than merely different.
fn shadowed_layer() -> (Document, NodeId) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let id = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 60.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((100.0, 100.0))),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }]),
        },
        Operation::SetEffects {
            id,
            effects: keyed_by_position([Effect::new(EffectKind::DropShadow(Shadow {
                offset: Vec2::new(0.0, 2.0),
                blur: 2.0,
                spread: 0.0,
                color: Color::BLACK,
            }))]),
        },
    ]))
    .unwrap();
    (doc, id)
}

/// A shadow big enough that a wrong buffer is a wrong *picture*, not a rounding
/// difference.
fn big_shadow() -> Effect {
    Effect::new(EffectKind::DropShadow(Shadow {
        offset: Vec2::new(0.0, 8.0),
        blur: 12.0,
        spread: 4.0,
        color: Color::BLACK,
    }))
}

/// **A ghost of a shadowed *container* previews with its shadow** — §15 D567.
///
/// `paint_ghost` built its effect box from
/// `geometry::local_path(&ghost.kind)?.bounding_box()`, and `local_path` answers
/// `None` for `Root | Group | Text | Boolean` on purpose. The `?` swallowed the
/// whole computation, so a ghost of any of those pushed **no effect layer at
/// all**: an Alt-drag copy of a shadowed group drew flat for the entire drag and
/// gained its shadow at the instant of release. Measured before the fix as 1
/// effect layer in the previewed stream against 2 in the committed one.
///
/// ⚠️ **`assert_preview_matches_commit` compares the whole call stream, so the
/// bounds are asserted as well as the count**, which is the half a
/// count-only test would miss — see the stroked case below, where the count was
/// right all along.
///
/// **Flip-checked** by making the container arm of `ghost_ink` answer `None`:
/// red here, green on the stroked case, which is the split the two tests exist
/// for.
#[test]
fn a_ghost_of_a_shadowed_group_previews_with_its_shadow() {
    let mut f = fixture();
    let mut ids = IdSource::new(0xE1E1);
    f.doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: f.group,
            effects: keyed_by_position([big_shadow()]),
        }]))
        .unwrap();

    let template = f.doc.capture_subtree(f.group).expect("capture the group");
    let (nodes, new_root) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let base = nodes
        .iter()
        .find(|n| n.id() == new_root)
        .map(|n| n.transform())
        .unwrap();
    let index = f.doc.get(f.artboard).unwrap().children().len();
    let tx = Transaction(vec![
        Operation::InsertSubtree {
            nodes,
            parent: f.artboard,
            index,
        },
        Operation::SetTransform {
            id: new_root,
            transform: Affine::translate((200.0, 0.0)) * base,
        },
    ]);

    // The fixture is in the state this test is about: committing really does
    // push two effect layers, so "the ghost pushed one" is a defect rather than
    // a document with one shadow in it.
    let mut committed = f.doc.clone();
    committed.apply(&tx).unwrap();
    let committed_res = Resolved::rebuild(&committed);
    assert_eq!(
        record(&committed, &committed_res, &RenderOverrides::default())
            .iter()
            .filter(|c| matches!(c, Call::Effect(..)))
            .count(),
        2,
        "fixture: the original and its copy each carry the shadow"
    );

    assert_preview_matches_commit(&f.doc, &tx, "dropping a copy of a shadowed group");
}

/// **And a ghost of a shadowed *stroked* shape asks for the buffer the stroke
/// needs** — the same entry, and the half a layer count cannot see.
///
/// The old box was the outline's, where `Resolved` measures a leaf with
/// `world_bounds_of_parts` — *including the stroke expansion*. A 40pt stroke put
/// the ghost's box 20 units inside the committed one on every side, so the
/// preview's shadow was cropped and snapped out at release. One effect layer
/// either way, which is why this needs the stream comparison rather than a count.
///
/// **Flip-checked** by putting the outline's box back in the leaf arm: red here
/// at `[-22, -14, 122, 80]` against `[-42, -34, 142, 100]` — 20 units all round,
/// exactly half the 40pt stroke — and green on the group case above.
#[test]
fn a_ghost_of_a_shadowed_stroked_shape_reserves_the_strokes_reach() {
    let mut f = fixture();
    let mut ids = IdSource::new(0xE2E2);
    f.doc
        .apply(&Transaction(vec![
            Operation::SetStrokes {
                id: f.rect,
                strokes: keyed_by_position([ondin_core::Stroke {
                    width: 40.0,
                    ..Default::default()
                }]),
            },
            Operation::SetEffects {
                id: f.rect,
                effects: keyed_by_position([big_shadow()]),
            },
        ]))
        .unwrap();

    let template = f.doc.capture_subtree(f.rect).expect("capture the rect");
    let (nodes, new_root) = ondin_core::remap_subtree(&template, &mut ids).expect("remap");
    let base = nodes
        .iter()
        .find(|n| n.id() == new_root)
        .map(|n| n.transform())
        .unwrap();
    let index = f.doc.get(f.artboard).unwrap().children().len();
    let tx = Transaction(vec![
        Operation::InsertSubtree {
            nodes,
            parent: f.artboard,
            index,
        },
        Operation::SetTransform {
            id: new_root,
            transform: Affine::translate((0.0, 200.0)) * base,
        },
    ]);
    assert_preview_matches_commit(&f.doc, &tx, "dropping a copy of a shadowed stroked rect");
}

/// **`absorb` treats thirteen operations as a no-op, not eleven** — §15 D659,
/// `[S19.2-L2-04]`. (Fifteen and thirteen since `SetComponent` and `SetLink`
/// joined the chrome arm, §15 D978; the paragraphs below keep the counts of the
/// finding they record.)
///
/// 🚨 **The design said the two lists were the same eleven, and a review pass
/// certified that they were.** `op::changes_ink` answers `false` for eleven
/// chrome-and-bookkeeping operations, pinned by
/// `op::changes_ink_tests::only_the_chrome_operations_change_no_ink`, which
/// asserts its own length. `RenderOverrides::absorb` holds those same eleven in
/// one arm **and a second no-op arm further down the same `match`**, behind a
/// long comment: `AddImage | RemoveImage`. `[A7-L8-04]` hand-counted the first
/// arm, stopped there, and on that basis certified `architecture.md` §5.3b's
/// *"those two lists are the same eleven operations"* as correct.
/// `[A6-L8-02]`'s shape — a check's own baseline carrying the thing it checks
/// for.
///
/// ⚠️ **This paragraph gave the distance as *"nineteen lines below it, behind a
/// sixteen-line comment"* until `arch-scribe` counted, and both figures were
/// wrong — in the file as it was *and* as it now is.** The eleven-arm ends at
/// `renderer.rs:909`; the image arm is at `:937`, and the comment between them
/// grew by the D659 note this very fix added. **A distance in lines is the one
/// measurement a comment inserted between the two sites invalidates**, and here
/// the comment that invalidated it was the fix's own. Dropped rather than
/// updated: a number that is wrong the moment somebody writes a paragraph is not
/// worth maintaining.
///
/// **Why it is worth a test rather than only a corrected sentence.**
/// `EditorSession::has_gesture_preview` is `!self.overrides.is_empty() && …`, so
/// *every* transaction `absorb` no-ops is invisible to `cancel_gesture`'s gate.
/// Anyone repairing `[A1-L2-02]` from the design's list repairs the wrong list.
///
/// **The relationship is the assertion, not the count.** `changes_ink`'s eleven
/// are a strict **subset** of `absorb`'s no-ops; the two extras are the image
/// operations, and they are there for a reason the comment on the arm gives
/// plainly — *"it is here because `RenderOverrides` has no image table to patch"*
/// — rather than because they are invisible. So this asserts both halves: all
/// thirteen absorb to an empty preview, and exactly two of the thirteen change
/// ink.
///
/// ⚠️ **What this cannot do is prove there is no fourteenth.** That needs a value
/// of every `Operation` variant, which is the `op.rs` fixture's job and is why
/// this is a *sibling* of that test rather than a replacement. What it does catch
/// is the two lists drifting apart again, which is how they got here.
#[test]
fn every_no_op_of_absorb_is_a_no_op_and_two_of_them_still_change_ink() {
    let f = fixture();
    let mut ids = IdSource::new(0xA850);
    let id = f.rect;
    let guide = ondin_core::guide::GuideId(ids.mint());
    let pic = ondin_core::ImageId("pic".into());

    // The thirteen `changes_ink` answers `false` for, in `op.rs`'s own order —
    // eleven until `SetComponent` and `SetLink` (§15 D978).
    let chrome = [
        Operation::SetName {
            id,
            name: "x".into(),
        },
        Operation::SetLocked { id, locked: true },
        Operation::SetProportionsLocked { id, locked: true },
        Operation::SetPivot { id, pivot: None },
        Operation::SetExports {
            id,
            exports: vec![],
        },
        Operation::SetLayoutGrids { id, grids: vec![] },
        Operation::SetComponent {
            id,
            component: false,
        },
        Operation::SetLink { id, link: None },
        Operation::AddGuide {
            guide: ondin_core::guide::Guide {
                id: guide,
                axis: ondin_core::guide::GuideAxis::Vertical,
                position: 0.0,
                color: None,
                owner: None,
            },
        },
        Operation::RemoveGuide { id: guide },
        Operation::SetGuidePosition {
            id: guide,
            position: 1.0,
        },
        Operation::SetGuideColor {
            id: guide,
            color: None,
        },
        Operation::SetGuideScope {
            id: guide,
            owner: None,
            position: 1.0,
        },
    ];
    // And the two that are no-ops here without being invisible.
    let images = [
        Operation::AddImage {
            id: pic.clone(),
            entry: ondin_core::ImageEntry {
                source: ondin_core::ImageSource::Embedded(vec![1, 2, 3].into()),
                format: ondin_core::ImageFormat::Png,
                width: 4,
                height: 4,
            },
        },
        Operation::RemoveImage { id: pic },
    ];

    // The fixture asserts its own lengths, for the reason `op.rs`'s does: it is
    // the only thing that makes the counts in the comment above checkable by
    // anything but a reader, and those counts have been wrong before.
    assert_eq!(chrome.len(), 13, "the chrome list is thirteen operations");
    assert_eq!(images.len(), 2, "and the image pair is two");

    let res = Resolved::rebuild(&f.doc);
    for op in chrome.iter().chain(images.iter()) {
        let ov = RenderOverrides::from_transaction(&f.doc, &res, &Transaction(vec![op.clone()]))
            .unwrap_or_else(|| panic!("{op:?} must be absorbed rather than refused"));
        assert!(
            ov.is_empty(),
            "{op:?} is a no-op in `absorb`, so it must leave an empty preview — \
             `has_gesture_preview` reads exactly this"
        );
    }

    // The half the design got wrong: `changes_ink` is a strict subset.
    for op in &chrome {
        assert!(!op.changes_ink(), "{op:?} is chrome and draws nothing");
    }
    for op in &images {
        assert!(
            op.changes_ink(),
            "{op:?} changes what is drawn and is a no-op here anyway — that \
             asymmetry is the whole finding, so it must not quietly go away"
        );
    }
}

/// A 300×200 frame holding four pinned layers — right-pinned, stretched, a
/// stretched text node and a stretched nested frame with a right-pinned child of
/// its own — and one that is not pinned at all. Returns the document, the frame
/// and the unpinned rect.
fn pinned_fixture() -> (Document, NodeId, NodeId) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    // **Every shape is painted**, or it draws nothing and the differential has
    // nothing to compare: the first cut of this fixture left them unfilled, and
    // the inset-edit test passed with `relayout` switched off entirely.
    let mut add = |doc: &mut Document, parent: NodeId, kind: NodeKind, at: (f64, f64)| {
        let id = ids.mint();
        let index = doc.get(parent).unwrap().children().len();
        let painted = !matches!(kind, NodeKind::Text { .. });
        let mut ops = vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        }];
        if painted {
            ops.push(Operation::SetFills {
                id,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(200, 60, 60, 255)),
                    visible: true,
                }]),
            });
        }
        doc.apply(&Transaction(ops)).unwrap();
        id
    };
    let rect = |w, h| NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
    };
    let frame = add(
        &mut doc,
        root,
        NodeKind::Artboard {
            size: Size::new(300.0, 200.0),
        },
        (0.0, 0.0),
    );
    let right = add(&mut doc, frame, rect(100.0, 40.0), (10.0, 10.0));
    let wide = add(&mut doc, frame, rect(100.0, 40.0), (10.0, 60.0));
    let words = add(
        &mut doc,
        frame,
        NodeKind::Text {
            content: "pinned text that wraps when its frame narrows".into(),
            style: Box::new(TextStyle::default()),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        (10.0, 110.0),
    );
    let nested = add(
        &mut doc,
        frame,
        NodeKind::Artboard {
            size: Size::new(80.0, 40.0),
        },
        (10.0, 150.0),
    );
    let grandchild = add(&mut doc, nested, rect(20.0, 20.0), (0.0, 0.0));
    let free = add(&mut doc, frame, rect(30.0, 30.0), (200.0, 150.0));
    let px = |v| Some(ondin_core::LengthPct::Px(v));
    let both = ondin_core::Insets {
        left: px(10.0),
        right: px(10.0),
        ..Default::default()
    };
    for (id, insets) in [
        (
            right,
            ondin_core::Insets {
                right: px(10.0),
                ..Default::default()
            },
        ),
        (wide, both),
        (words, both),
        (nested, both),
        (
            grandchild,
            ondin_core::Insets {
                right: px(0.0),
                ..Default::default()
            },
        ),
    ] {
        doc.apply(&Transaction(vec![Operation::SetInsets { id, insets }]))
            .unwrap();
    }
    (doc, frame, free)
}

/// **Dragging a frame's edge draws its pinned children where the release will**
/// (§15 D871) — `RenderOverrides::relayout` re-placing them against the preview's
/// frame size, the stretched text re-shaped at its new wrap width and the nested
/// frame's own pinned child following a frame that only moved because its parent
/// did. Narrower and wider, because the text's line count moves in only one of
/// them.
///
/// **Flip:** returning early from `relayout` fails the first case — the preview
/// leaves every pinned child where the committed frame put it, which is the snap
/// on release this exists to remove.
#[test]
fn a_frame_resize_previews_its_pinned_children_where_the_commit_puts_them() {
    let (doc, frame, _) = pinned_fixture();
    for w in [180.0, 420.0] {
        let tx = Transaction(vec![Operation::SetGeometry {
            id: frame,
            geometry: GeometryPatch::Size(Size::new(w, 200.0)),
        }]);
        assert_preview_matches_commit(&doc, &tx, &format!("frame resized to {w}"));
    }
}

/// Setting, changing and clearing a layer's insets previews as its commit —
/// the inspector's scrub over an inset field is this.
///
/// **Flip:** returning early from `relayout` fails *"pinned and stretched"*.
/// ⚠️ **It did not, on the fixture's first cut** — the shapes were unfilled and
/// drew nothing, so both streams were empty and equal; the resize test above only
/// failed because its text draws. Painting every shape is what gave this one
/// teeth.
#[test]
fn an_inset_edit_previews_as_its_commit() {
    let (doc, _, free) = pinned_fixture();
    let px = |v| Some(ondin_core::LengthPct::Px(v));
    let pin = |insets| Transaction(vec![Operation::SetInsets { id: free, insets }]);
    assert_preview_matches_commit(
        &doc,
        &pin(ondin_core::Insets {
            left: px(5.0),
            right: px(5.0),
            bottom: Some(ondin_core::LengthPct::Percent(10.0)),
            ..Default::default()
        }),
        "pinned and stretched",
    );
    // Rounding a *stretched* layer's corners: the patch lands on a kind at its
    // stored size, and the preview must still draw it stretched.
    let stretched = doc
        .get(free)
        .unwrap()
        .parent()
        .and_then(|f| doc.get(f))
        .unwrap()
        .children()[1];
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetGeometry {
            id: stretched,
            geometry: GeometryPatch::CornerRadius(6.0),
        }]),
        "a stretched layer's corners rounded",
    );
    // And un-pinning a pinned layer, which has to take it back to its transform.
    let pinned = doc
        .get(free)
        .unwrap()
        .parent()
        .and_then(|f| doc.get(f))
        .unwrap()
        .children()[0];
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetInsets {
            id: pinned,
            insets: ondin_core::Insets::default(),
        }]),
        "un-pinned",
    );
}

/// A frame laid out as a row holding three painted rects, a nested group laid
/// out as a column with two more, and a text node that wraps. Returns the
/// document, the frame, the first rect and the group.
fn flex_fixture() -> (Document, NodeId, NodeId, NodeId) {
    use ondin_core::container::{AlignItems, Display, Flex, FlexDirection};
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut add = |doc: &mut Document, parent: NodeId, kind: NodeKind| {
        let id = ids.mint();
        let index = doc.get(parent).unwrap().children().len();
        let painted = !matches!(kind, NodeKind::Text { .. } | NodeKind::Group);
        let mut ops = vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate((500.0, 500.0))),
            name: None,
        }];
        if painted {
            ops.push(Operation::SetFills {
                id,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(60, 120, 200, 255)),
                    visible: true,
                }]),
            });
        }
        doc.apply(&Transaction(ops)).unwrap();
        id
    };
    let rect = |w, h| NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
    };
    let frame = add(
        &mut doc,
        root,
        NodeKind::Artboard {
            size: Size::new(400.0, 200.0),
        },
    );
    let first = add(&mut doc, frame, rect(40.0, 30.0));
    add(&mut doc, frame, rect(60.0, 30.0));
    let group = add(&mut doc, frame, NodeKind::Group);
    add(&mut doc, group, rect(20.0, 20.0));
    add(&mut doc, group, rect(30.0, 20.0));
    add(
        &mut doc,
        frame,
        NodeKind::Text {
            content: "flex text that wraps".into(),
            style: Box::new(TextStyle::default()),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::AutoHeight(90.0),
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
    );
    doc.apply(&Transaction(vec![
        Operation::SetDisplay {
            id: frame,
            display: Some(Display::Flex(Flex {
                column_gap: 10.0,
                padding: [20.0; 4],
                align_items: AlignItems::Start,
                ..Default::default()
            })),
        },
        Operation::SetDisplay {
            id: group,
            display: Some(Display::Flex(Flex {
                direction: FlexDirection::Column,
                row_gap: 5.0,
                align_items: AlignItems::Start,
                ..Default::default()
            })),
        },
    ]))
    .unwrap();
    (doc, frame, first, group)
}

/// **A reorder previews as its commit** (§15 D877) — in a flex row, where it
/// moves every item after it, and in the nested column, where it swaps two; and
/// the reordered item's z-order is the commit's too, since the walk reads the
/// same order the layout does.
///
/// **Flip run**, `PreviewView::children` answering the committed order: fails on
/// *"the first item moved to the end of the row"* — the row's layout unchanged —
/// the predicted site. With the scene walk's `children_of` dropped instead it
/// fails there too, which was **not** the prediction: the prediction was that the
/// row would pass, no item overlapping another — but the differential compares
/// the recorded draw calls *in order*, not pixels, so a paint order the commit
/// does not share diverges whether or not it shows.
#[test]
fn a_reorder_previews_as_its_commit() {
    let (doc, _frame, first, group) = flex_fixture();
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::Reorder {
            id: first,
            index: 3,
        }]),
        "the first item moved to the end of the row",
    );
    let bottom = doc.get(group).unwrap().children()[0];
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::Reorder {
            id: bottom,
            index: 1,
        }]),
        "the column's two items swapped",
    );
}

/// **A dragged flex item's landing is its slot in the reordered row, and only
/// it has one** (§15 D877) — the fixture's first rect (40×30) carried off by a
/// translation while a reorder puts it second, in a row at (500, 500) with
/// padding 20 and a gap of 10 after the 60-wide second rect.
///
/// It lands at x 500 + 20 + 60 + 10 = 590, y 520, as its own 40×30 box — the
/// insertion indicator's outline — while the translation is where it is drawn.
///
/// **Flip run**, the landing push in `flex_relayout` deleted: fails on *"one
/// landing"* at 0, the predicted site.
#[test]
fn a_dragged_flex_item_lands_in_its_reordered_slot() {
    let (doc, _frame, first, _) = flex_fixture();
    let res = Resolved::rebuild(&doc);
    let tx = Transaction(vec![
        Operation::SetTransform {
            id: first,
            transform: Affine::translate((900.0, 900.0)),
        },
        Operation::Reorder {
            id: first,
            index: 1,
        },
    ]);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");
    let landings = ov.landings();
    assert_eq!(landings.len(), 1, "one landing");
    let (id, world, bx) = landings[0];
    assert_eq!(id, first);
    assert_eq!(world.translation(), Vec2::new(590.0, 520.0), "its slot");
    assert_eq!(bx.size(), Size::new(40.0, 30.0), "its own box");
}

/// **A drag that holds an item exactly on its new slot lands there too** (§15
/// D908) — `a_dragged_flex_item_lands_in_its_reordered_slot` with the drag's
/// transform the slot itself, (90, 20) in the frame: what a snap to the
/// neighbour's edge hands the preview.
///
/// `item_placed` answers `local: None` when the slot is the transform the pass
/// read — the drag's — and `landing_of` fell back to the document's, so the
/// outline was drawn at the rect's stored (500, 500) inside the frame: 1000, 1000
/// in the world. The same fallback §15 D905 found in `flex_relayout`.
///
/// **Flip run**, `landing_of` falling back to `node.transform()` again: fails on
/// *"its slot"* at (1000, 1000), the predicted site.
#[test]
fn a_flex_item_dragged_onto_its_slot_lands_there() {
    let (doc, _frame, first, _) = flex_fixture();
    let res = Resolved::rebuild(&doc);
    let tx = Transaction(vec![
        Operation::SetTransform {
            id: first,
            transform: Affine::translate((90.0, 20.0)),
        },
        Operation::Reorder {
            id: first,
            index: 1,
        },
    ]);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");
    let landings = ov.landings();
    assert_eq!(landings.len(), 1, "one landing");
    assert_eq!(
        landings[0].1.translation(),
        Vec2::new(590.0, 520.0),
        "its slot"
    );
}

/// **A landing whose reorder moves its container lands where the commit puts
/// it** (§15 D945, the release review's `[X4.2-L1-01]`): a 400 × 400 column
/// centring a hugging wrap row 100 wide, of rects 60, 30, 60 and 30 wide — two
/// lines. Dragging the 30 after the second 60 makes three lines, so the row grows
/// and the column re-centres it 10 higher; the outline was composed with the
/// row's *committed* world transform and sat 10 below the slot the siblings open.
///
/// **Flip run**, `landing_of` composing with `res.world_transform` again: fails
/// on *"where the commit puts it"*, 10 low, the predicted site.
#[test]
fn a_landing_whose_reorder_moves_its_container_lands_where_the_commit_puts_it() {
    use ondin_core::container::{
        AlignItems, Dimension, Display, Flex, FlexDirection, FlexWrap, JustifyContent,
    };
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let column = painted(
        &mut doc,
        &mut ids,
        root,
        sized_frame(400.0, 400.0),
        (0.0, 0.0),
    );
    let row = painted(
        &mut doc,
        &mut ids,
        column,
        sized_frame(100.0, 50.0),
        (0.0, 0.0),
    );
    let rects: Vec<NodeId> = [60.0, 30.0, 60.0, 30.0]
        .into_iter()
        .map(|w| painted(&mut doc, &mut ids, row, sized_rect(w, 20.0), (0.0, 0.0)))
        .collect();
    let mut item = *doc.get(row).unwrap().item();
    item.height = Dimension::FitContent;
    doc.apply(&Transaction(vec![
        Operation::SetDisplay {
            id: column,
            display: Some(Display::Flex(Flex {
                direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            })),
        },
        Operation::SetDisplay {
            id: row,
            display: Some(Display::Flex(Flex {
                wrap: FlexWrap::Wrap,
                align_items: AlignItems::Start,
                ..Default::default()
            })),
        },
        Operation::SetLayoutItem { id: row, item },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let dragged = rects[1];
    let reorder = Operation::Reorder {
        id: dragged,
        index: 3,
    };
    let tx = Transaction(vec![
        Operation::SetTransform {
            id: dragged,
            transform: Affine::translate((900.0, 900.0)),
        },
        reorder.clone(),
    ]);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");
    let mut committed = doc.clone();
    committed.apply(&Transaction(vec![reorder])).unwrap();
    let landed = Resolved::rebuild(&committed).world_bounds(dragged).unwrap();
    let was = res.world_bounds(row).unwrap();
    assert!(
        Resolved::rebuild(&committed).world_bounds(row).unwrap().y0 < was.y0,
        "the fixture: the row grows and re-centres higher"
    );
    let landings = ov.landings();
    assert_eq!(landings.len(), 1, "one landing");
    assert_eq!(
        landings[0].1.translation(),
        landed.origin().to_vec2(),
        "where the commit puts it"
    );
}

/// **A layout nested in a plain group previews as it commits** (§15 D911) — a
/// flex row holding a plain group, holding a laid column of two 20 × 20 rects;
/// the first resized to 40 × 40.
///
/// The commit re-lays the column: the second rect moves down to y 70 and
/// stretches to 40 wide. The preview laid out only the chain root — the row,
/// whose pass stops at the plain group — so it drew the second rect where it
/// was, 20 wide at y 50.
///
/// **Flip run**, `flex_relayout` laying out the chain root alone, as it did:
/// fails on the differential, the second rect's `Fill` at y 50 against 70 — the
/// predicted site, and the run that found it.
#[test]
fn a_layout_nested_in_a_plain_group_previews_as_it_commits() {
    use ondin_core::container::{Display, Flex, FlexDirection};
    let mut ids = IdSource::new(0xAB);
    let root = ids.mint();
    let (f, p, g, r1, r2) = (ids.mint(), ids.mint(), ids.mint(), ids.mint(), ids.mint());
    let mut doc = Document::new(root);
    let create = |id, parent, index, kind| Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: Some(Affine::translate((7.0, 7.0))),
        name: None,
    };
    let rect = |s| NodeKind::Rect {
        size: Size::new(s, s),
        corner_radii: RoundedRectRadii::default(),
    };
    let fill = |id| Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(60, 120, 200, 255)),
            visible: true,
        }]),
    };
    doc.apply(&Transaction(vec![
        create(
            f,
            root,
            0,
            NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
        ),
        create(p, f, 0, NodeKind::Group),
        create(g, p, 0, NodeKind::Group),
        create(r1, g, 0, rect(20.0)),
        create(r2, g, 1, rect(20.0)),
        fill(r1),
        fill(r2),
        Operation::SetDisplay {
            id: f,
            display: Some(Display::Flex(Flex {
                padding: [20.0; 4],
                ..Default::default()
            })),
        },
        Operation::SetDisplay {
            id: g,
            display: Some(Display::Flex(Flex {
                direction: FlexDirection::Column,
                row_gap: 5.0,
                ..Default::default()
            })),
        },
    ]))
    .unwrap();
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetGeometry {
            id: r1,
            geometry: ondin_core::GeometryPatch::Size(Size::new(40.0, 40.0)),
        }]),
        "a rect in a laid group in a plain group in a flex row, resized",
    );
}

/// **A left-handle resize of a flex item previews as its commit** (§15 D877's
/// amendment) — the fixture's first rect widened from 40 to 60 with the
/// `SetTransform` a held right edge writes beside it.
///
/// The layout places an in-flow item whatever its transform, so the commit draws
/// it in its slot at the new size; the preview used to treat the `SetTransform` as
/// a drag and draw it at the handle's box, off to the left of where it lands.
///
/// **Flip run**, `flex_relayout`'s `resized` exemption deleted: fails on the
/// differential, the predicted site.
#[test]
fn a_left_handle_resize_of_a_flex_item_previews_as_its_commit() {
    let (doc, _frame, first, _) = flex_fixture();
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![
            Operation::SetGeometry {
                id: first,
                geometry: GeometryPatch::Size(Size::new(60.0, 30.0)),
            },
            Operation::SetTransform {
                id: first,
                transform: Affine::translate((500.0, 520.0)),
            },
        ]),
        "a left-handle resize",
    );
}

/// **Rotating a flex item about a pivot other than its centre previews as its
/// commit** (§15 D896, the maintainer's ruling that an in-flow item turns about
/// its box centre in its slot). The fixture's first rect turned 30° about a point
/// far off to its lower right — the `SetTransform` a rotate tool writes for a
/// pivot that is not the centre, translation and all.
///
/// The layout keeps the item in its slot and turns it about its box centre with
/// the new linear part, ignoring the translation; the preview used to treat the
/// `SetTransform` as a drag and draw it about the tool's pivot, which jumped on
/// release (§15 D877's second amendment, measured nowhere until this).
///
/// **Flip run**, `flex_relayout`'s `turned` exemption deleted: fails on the
/// differential, the predicted site.
#[test]
fn rotating_a_flex_item_about_another_pivot_previews_as_its_commit() {
    let (doc, _frame, first, _) = flex_fixture();
    let pivot = ondin_core::kurbo::Vec2::new(300.0, 200.0);
    let turn = Affine::translate(pivot)
        * Affine::rotate(30f64.to_radians())
        * Affine::translate(-pivot)
        * doc.get(first).unwrap().transform();
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetTransform {
            id: first,
            transform: turn,
        }]),
        "a rotation about another pivot",
    );
}

/// **Resizing one flex item draws its siblings where the release will** — the
/// preview running the flex engine on its own state (`flex_relayout`), where a
/// field patch could only have moved the item and left the row overlapping.
///
/// **Flip:** returning early from `flex_relayout` fails the first case — the
/// siblings stay where the committed row put them.
#[test]
fn resizing_a_flex_item_previews_the_row_it_commits() {
    let (doc, frame, first, _) = flex_fixture();
    for (what, tx) in [
        (
            "an item resized",
            Transaction(vec![Operation::SetGeometry {
                id: first,
                geometry: GeometryPatch::Size(Size::new(110.0, 30.0)),
            }]),
        ),
        (
            "the frame resized",
            Transaction(vec![Operation::SetGeometry {
                id: frame,
                geometry: GeometryPatch::Size(Size::new(300.0, 200.0)),
            }]),
        ),
    ] {
        assert_preview_matches_commit(&doc, &tx, what);
    }
}

/// Layout and item edits preview as their commits: growing an item, switching a
/// row to a column, and taking the frame's layout away altogether.
///
/// **Flip:** the same early return fails here at *"an item grown"*. ⚠️ **Taking a
/// layout away failed on the first cut with `flex_relayout` running**: a container
/// with no layout roots no pass, so its former items kept the committed flex
/// placement until `flex_relayout` learned to put back what it no longer lays out.
#[test]
fn layout_and_item_edits_preview_as_their_commits() {
    use ondin_core::container::{Display, Flex, FlexDirection};
    let (doc, frame, first, group) = flex_fixture();
    let mut grow = *doc.get(first).unwrap().item();
    grow.grow = 1.0;
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetLayoutItem {
            id: first,
            item: grow,
        }]),
        "an item grown",
    );
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetDisplay {
            id: frame,
            display: Some(Display::Flex(Flex {
                direction: FlexDirection::Column,
                ..Default::default()
            })),
        }]),
        "the row made a column",
    );
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetDisplay {
            id: frame,
            display: None,
        }]),
        "the layout taken away",
    );
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetDisplay {
            id: group,
            display: None,
        }]),
        "the nested group's layout taken away",
    );
}

/// **A pinned child of a group with a layout previews as its commit** (§15 D887,
/// the preview half its entry left unchecked) — the fixture's nested column: its
/// second rect pinned to the group's bottom-right, which takes it out of the
/// column so the group re-lays and hugs smaller, and the pin is placed against
/// that new box. Then, with the pin committed, the column's other rect widened,
/// which resizes the box the pin is placed against.
///
/// `flex_relayout` must run before `relayout` for either to hold: the insets pass
/// reads the box the flex pass has just given the group.
///
/// **Flip run**, the two passes swapped in `from_transaction_shaped`: fails on
/// the **first** case, *"a laid group's child pinned"* — not on the second, as
/// predicted. Pinning alone is enough: the pin takes the rect out of the column,
/// the group hugs smaller, and an insets pass run first places the pin against
/// the group's committed, larger box.
#[test]
fn a_pinned_child_of_a_laid_group_previews_as_its_commit() {
    let (doc, _frame, _first, group) = flex_fixture();
    let kids = doc.get(group).unwrap().children().to_vec();
    let (top, pinned) = (kids[0], kids[1]);
    let pin = Operation::SetInsets {
        id: pinned,
        insets: ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(0.0)),
            bottom: Some(ondin_core::LengthPct::Px(0.0)),
            ..Default::default()
        },
    };
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![pin.clone()]),
        "a laid group's child pinned",
    );
    let mut pinned_doc = doc.clone();
    pinned_doc.apply(&Transaction(vec![pin])).unwrap();
    assert_preview_matches_commit(
        &pinned_doc,
        &Transaction(vec![Operation::SetGeometry {
            id: top,
            geometry: GeometryPatch::Size(Size::new(70.0, 20.0)),
        }]),
        "a pinned child of a laid group, then its sibling resized",
    );
}

/// **Moving a shape inside a plain group that is a flex item previews the row
/// reflowing** (§15 D899) — the fixture's nested group with its layout taken
/// away, so it is one atomic box in the row measured from its two rects, and one
/// of them moved far out: the group grows, and the row's text after it moves. The
/// preview picks what to re-lay through `container::chain_root`, which stopped at
/// the plain group until D899.
///
/// **Flip run**, `chain_root`'s climb through a plain group removed: fails on
/// the differential — the text drawn at its committed slot while the commit moves
/// it — the predicted site.
#[test]
fn moving_a_shape_inside_a_plain_group_item_previews_the_row_reflowing() {
    let (mut doc, _frame, _first, group) = flex_fixture();
    doc.apply(&Transaction(vec![Operation::SetDisplay {
        id: group,
        display: None,
    }]))
    .unwrap();
    let inner = doc.get(group).unwrap().children()[0];
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetTransform {
            id: inner,
            transform: Affine::translate((640.0, 500.0)),
        }]),
        "a shape moved inside a plain group item",
    );
}

/// A 400 × 200 frame at (500, 500) laid out as a grid — columns `100px 1fr 2fr`,
/// rows `50px 50px`, no gaps — holding four painted 20 × 20 rects under the
/// default `normal`, so each sits at the start of its cell (§15 D915). Returns
/// the document, the frame and the four rects.
fn grid_fixture() -> (Document, NodeId, [NodeId; 4]) {
    use ondin_core::container::{Display, Grid, Track, TrackBreadth, TrackSize};
    let mut ids = IdSource::new(0x6D);
    let root = ids.mint();
    let frame = ids.mint();
    let mut doc = Document::new(root);
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: frame,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 200.0),
        },
        transform: Some(Affine::translate((500.0, 500.0))),
        name: None,
    }]))
    .unwrap();
    let rects = [0; 4].map(|_| {
        let id = ids.mint();
        let index = doc.get(frame).unwrap().children().len();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: frame,
                index,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::translate((300.0, 150.0))),
                name: None,
            },
            Operation::SetFills {
                id,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(60, 120, 200, 255)),
                    visible: true,
                }]),
            },
        ]))
        .unwrap();
        id
    });
    let b = |b: TrackBreadth| Track::Size(TrackSize::Breadth(b));
    doc.apply(&Transaction(vec![Operation::SetDisplay {
        id: frame,
        display: Some(Display::Grid(Grid {
            columns: vec![
                b(TrackBreadth::Px(100.0)),
                b(TrackBreadth::Fr(1.0)),
                b(TrackBreadth::Fr(2.0)),
            ],
            rows: vec![b(TrackBreadth::Px(50.0)), b(TrackBreadth::Px(50.0))],
            ..Default::default()
        })),
    }]))
    .unwrap();
    (doc, frame, rects)
}

/// **A grid previews as it commits** (§15 D916) — a drop writing the first
/// rect's lines (column 3, row 2), which reflows the three auto-placed ones
/// around it; the frame resized, which reflows the `fr` columns; a stretched
/// item resized with the hold its commit adds; and the frame's layout switched
/// from grid to flex and back. The preview's pass is flex's, and lays a grid
/// through the same `lay_out` — read, until this, and not tested.
///
/// **Flip run**, `PreviewView::item` answering the document's item and not the
/// preview's: fails on *"a drop writing lines"* — the three siblings drawn where
/// they were — the predicted site.
#[test]
fn a_grid_previews_as_it_commits() {
    use ondin_core::container::{AlignItems, Display, Flex, GridLines, GridPlacement, LayoutItem};
    let (doc, frame, [first, second, ..]) = grid_fixture();
    let placed = LayoutItem {
        grid_column: GridLines {
            start: GridPlacement::Line(3),
            end: GridPlacement::Auto,
        },
        grid_row: GridLines {
            start: GridPlacement::Line(2),
            end: GridPlacement::Auto,
        },
        ..LayoutItem::default()
    };
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetLayoutItem {
            id: first,
            item: placed,
        }]),
        "a drop writing lines",
    );
    assert_preview_matches_commit(
        &doc,
        &Transaction(vec![Operation::SetGeometry {
            id: frame,
            geometry: GeometryPatch::Size(Size::new(700.0, 200.0)),
        }]),
        "the frame resized, the fr columns reflowing",
    );
    let stretched = LayoutItem {
        justify_self: Some(AlignItems::Stretch),
        ..LayoutItem::default()
    };
    let mut held = stretched;
    held.justify_self = Some(AlignItems::Start);
    let mut doc2 = doc.clone();
    doc2.apply(&Transaction(vec![Operation::SetLayoutItem {
        id: second,
        item: stretched,
    }]))
    .unwrap();
    assert_preview_matches_commit(
        &doc2,
        &Transaction(vec![
            Operation::SetGeometry {
                id: second,
                geometry: GeometryPatch::Size(Size::new(60.0, 20.0)),
            },
            Operation::SetLayoutItem {
                id: second,
                item: held,
            },
        ]),
        "a stretched item resized, with its hold",
    );
    let grid = doc.get(frame).unwrap().display().cloned();
    let mut flexed = doc.clone();
    flexed
        .apply(&Transaction(vec![Operation::SetDisplay {
            id: frame,
            display: Some(Display::Flex(Flex::default())),
        }]))
        .unwrap();
    assert_preview_matches_commit(
        &flexed,
        &Transaction(vec![Operation::SetDisplay {
            id: frame,
            display: grid,
        }]),
        "flex turned back into the grid",
    );
}

/// **A dragged grid item lands in the cell its drop writes** (§15 D916, D877's
/// indicator in a grid) — the first rect carried off by a translation while its
/// lines put it in column 3, row 2: in a 400-wide frame, `100px 1fr 2fr` is 100,
/// 100 and 200, so column 3 starts at x 200 and row 2 at y 50 — world (700, 550),
/// the rect's own 20 × 20 at the start of the cell.
#[test]
fn a_dragged_grid_item_lands_in_its_new_cell() {
    use ondin_core::container::{GridLines, GridPlacement, LayoutItem};
    let (doc, _frame, [first, ..]) = grid_fixture();
    let res = Resolved::rebuild(&doc);
    let tx = Transaction(vec![
        Operation::SetTransform {
            id: first,
            transform: Affine::translate((900.0, 900.0)),
        },
        Operation::SetLayoutItem {
            id: first,
            item: LayoutItem {
                grid_column: GridLines {
                    start: GridPlacement::Line(3),
                    end: GridPlacement::Auto,
                },
                grid_row: GridLines {
                    start: GridPlacement::Line(2),
                    end: GridPlacement::Auto,
                },
                ..LayoutItem::default()
            },
        },
    ]);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("representable");
    let landings = ov.landings();
    assert_eq!(landings.len(), 1, "one landing");
    let (id, world, bx) = landings[0];
    assert_eq!(id, first);
    assert_eq!(world.translation(), Vec2::new(700.0, 550.0), "its new cell");
    assert_eq!(bx.size(), Size::new(20.0, 20.0), "its own box");
}

/// A painted node under `parent`, stored at `at` — the nested-layout tests'
/// builder. Every node is filled, so the differential has ink to compare.
fn painted(
    doc: &mut Document,
    ids: &mut IdSource,
    parent: NodeId,
    kind: NodeKind,
    at: (f64, f64),
) -> NodeId {
    let id = ids.mint();
    let index = doc.get(parent).unwrap().children().len();
    let mut ops = vec![Operation::CreateNode {
        id,
        parent,
        index,
        kind: kind.clone(),
        transform: Some(Affine::translate(at)),
        name: None,
    }];
    if !matches!(kind, NodeKind::Group) {
        ops.push(Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(60, 120, 200, 255)),
                visible: true,
            }]),
        });
    }
    doc.apply(&Transaction(ops)).unwrap();
    id
}

fn sized_frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
    }
}

fn sized_rect(w: f64, h: f64) -> NodeKind {
    NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
    }
}

/// **A frame resize previews the flex frames inside it as its commit lays them**
/// (§15 D933, the release review's `[X4.2-L2-01]`). `RenderOverrides::relayout`
/// re-placed every child of the resized frame, and for one with no insets and no
/// preview of its own it fell back to the *stored* transform and kind — which,
/// for a layout root that hugs or an item laid in its flow, are places nobody has
/// been drawn at since the layout was set. So:
///
/// - (a) a hugging row inside a plain frame previewed at its stored 300 × 100,
///   its three rects piled at their stored (250, 250);
/// - (b) a row pinned `left: 20; right: 20` with `justify-content: end`, laid in
///   the width it is drawn at since D933's commit half, previewed piled too;
/// - (c) a flex frame resized while it holds a pinned flex frame of its own;
/// - (d) no frame resized at all: a sibling rect resized in a row shrinks a grown
///   plain frame, which holds a hugging row.
///
/// Each case as its own document, each through `assert_preview_matches_commit`.
///
/// **Flip runs**: `relayout`'s skip back to *"a layer whose kind this preview
/// changed"* only fails on (a) at the hugging frame's fill, 300 × 100 against
/// 160 × 50, the predicted site; its pinned-root pass switched off fails on (b),
/// the row's items piled where (a) is now right.
#[test]
fn a_resize_previews_nested_layouts_as_its_commit_lays_them() {
    use ondin_core::container::{AlignItems, Dimension, Display, Flex, JustifyContent};
    let row = |justify| {
        Some(Display::Flex(Flex {
            column_gap: 10.0,
            padding: [10.0; 4],
            justify_content: justify,
            align_items: AlignItems::Start,
            ..Default::default()
        }))
    };
    let three = |doc: &mut Document, ids: &mut IdSource, f: NodeId| {
        for _ in 0..3 {
            painted(doc, ids, f, sized_rect(40.0, 30.0), (250.0, 250.0));
        }
    };
    let hug = |doc: &mut Document, f: NodeId| {
        let mut item = *doc.get(f).unwrap().item();
        item.width = Dimension::FitContent;
        item.height = Dimension::FitContent;
        doc.apply(&Transaction(vec![Operation::SetLayoutItem { id: f, item }]))
            .unwrap();
    };
    let setup = |doc: &mut Document, ops: Vec<Operation>| doc.apply(&Transaction(ops)).unwrap();
    let resize = |id, w, h| {
        Transaction(vec![Operation::SetGeometry {
            id,
            geometry: GeometryPatch::Size(Size::new(w, h)),
        }])
    };

    // (a)
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let outer = painted(
        &mut doc,
        &mut ids,
        root,
        sized_frame(600.0, 400.0),
        (0.0, 0.0),
    );
    let inner = painted(
        &mut doc,
        &mut ids,
        outer,
        sized_frame(300.0, 100.0),
        (20.0, 20.0),
    );
    three(&mut doc, &mut ids, inner);
    hug(&mut doc, inner);
    setup(
        &mut doc,
        vec![Operation::SetDisplay {
            id: inner,
            display: row(JustifyContent::Start),
        }],
    );
    assert_preview_matches_commit(&doc, &resize(outer, 700.0, 400.0), "(a) a hugging row");

    // (b)
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let outer = painted(
        &mut doc,
        &mut ids,
        root,
        sized_frame(600.0, 400.0),
        (0.0, 0.0),
    );
    let inner = painted(
        &mut doc,
        &mut ids,
        outer,
        sized_frame(300.0, 100.0),
        (20.0, 20.0),
    );
    three(&mut doc, &mut ids, inner);
    setup(
        &mut doc,
        vec![
            Operation::SetDisplay {
                id: inner,
                display: row(JustifyContent::End),
            },
            Operation::SetInsets {
                id: inner,
                insets: ondin_core::Insets {
                    left: Some(ondin_core::LengthPct::Px(20.0)),
                    right: Some(ondin_core::LengthPct::Px(20.0)),
                    ..Default::default()
                },
            },
        ],
    );
    assert_preview_matches_commit(&doc, &resize(outer, 700.0, 400.0), "(b) a pinned row");

    // (c)
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let outer = painted(
        &mut doc,
        &mut ids,
        root,
        sized_frame(600.0, 400.0),
        (0.0, 0.0),
    );
    painted(
        &mut doc,
        &mut ids,
        outer,
        sized_rect(40.0, 30.0),
        (0.0, 0.0),
    );
    let inner = painted(
        &mut doc,
        &mut ids,
        outer,
        sized_frame(100.0, 60.0),
        (0.0, 0.0),
    );
    three(&mut doc, &mut ids, inner);
    hug(&mut doc, inner);
    setup(
        &mut doc,
        vec![
            Operation::SetDisplay {
                id: outer,
                display: row(JustifyContent::Start),
            },
            Operation::SetDisplay {
                id: inner,
                display: row(JustifyContent::Start),
            },
            Operation::SetInsets {
                id: inner,
                insets: ondin_core::Insets {
                    top: Some(ondin_core::LengthPct::Px(10.0)),
                    right: Some(ondin_core::LengthPct::Px(10.0)),
                    ..Default::default()
                },
            },
        ],
    );
    assert_preview_matches_commit(
        &doc,
        &resize(outer, 700.0, 400.0),
        "(c) a pinned row in a resized row",
    );

    // (d)
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let outer = painted(
        &mut doc,
        &mut ids,
        root,
        sized_frame(600.0, 400.0),
        (0.0, 0.0),
    );
    let sibling = painted(
        &mut doc,
        &mut ids,
        outer,
        sized_rect(40.0, 30.0),
        (0.0, 0.0),
    );
    let grown = painted(
        &mut doc,
        &mut ids,
        outer,
        sized_frame(100.0, 100.0),
        (0.0, 0.0),
    );
    let inner = painted(
        &mut doc,
        &mut ids,
        grown,
        sized_frame(100.0, 60.0),
        (5.0, 5.0),
    );
    three(&mut doc, &mut ids, inner);
    hug(&mut doc, inner);
    let mut item = *doc.get(grown).unwrap().item();
    item.grow = 1.0;
    setup(
        &mut doc,
        vec![
            Operation::SetDisplay {
                id: outer,
                display: row(JustifyContent::Start),
            },
            Operation::SetDisplay {
                id: inner,
                display: row(JustifyContent::Start),
            },
            Operation::SetLayoutItem { id: grown, item },
        ],
    );
    assert_preview_matches_commit(
        &doc,
        &resize(sibling, 120.0, 30.0),
        "(d) a sibling shrinks a grown frame",
    );
}

/// **A stretched, growing flex item resized from its left, its top and its
/// top-left corner previews as its commit, through the doors** (the release
/// review's `[X4.2-L6-01]`) — a 400 × 200 row, padding 20 and gap 10, of three
/// 40 × 30 rects under the default `align-items: stretch`, the middle one growing
/// to 260 × 160. Each resize writes the shifted slot beside its size, which the
/// commit door drops; the hold stops the growth (§15 D875) and, for the top
/// handle, aligns the item to the end so its bottom stays (§15 D905).
///
/// `a_left_handle_resize_of_a_flex_item_previews_as_its_commit` checks the same
/// shape through the plain harness, against a commit that keeps the
/// `SetTransform` the app's commit drops, on an item with no stretch or growth
/// to hold.
///
/// **Flip run**, `build::keep_flex_sizes` leaving out the holds it appends: fails
/// on *"from its left: preview diverged from commit"* — the commit grows the item
/// back to 260 while the preview holds it at 230 — the predicted site. The grid
/// test beside this one fails the same way. Every test that uses the plain
/// harness stays green, which is the finding.
#[test]
fn a_stretched_flex_item_resized_from_its_start_edges_previews_as_it_commits() {
    use ondin_core::container::{Display, Flex};
    let mut ids = IdSource::new(0xAF);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = painted(
        &mut doc,
        &mut ids,
        root,
        sized_frame(400.0, 200.0),
        (0.0, 0.0),
    );
    let rects: Vec<NodeId> = (0..3)
        .map(|_| {
            painted(
                &mut doc,
                &mut ids,
                frame,
                sized_rect(40.0, 30.0),
                (250.0, 250.0),
            )
        })
        .collect();
    let middle = rects[1];
    let mut item = *doc.get(middle).unwrap().item();
    item.grow = 1.0;
    doc.apply(&Transaction(vec![
        Operation::SetDisplay {
            id: frame,
            display: Some(Display::Flex(Flex {
                column_gap: 10.0,
                padding: [20.0; 4],
                ..Default::default()
            })),
        },
        Operation::SetLayoutItem { id: middle, item },
    ]))
    .unwrap();
    assert_eq!(
        Resolved::rebuild(&doc).world_bounds(middle).unwrap(),
        Rect::new(70.0, 20.0, 330.0, 180.0),
        "the fixture: the middle rect grown to 260 and stretched to 160"
    );
    for (what, left, top, held) in [
        (
            "from its left",
            30.0,
            0.0,
            Rect::new(70.0, 20.0, 300.0, 180.0),
        ),
        (
            "from its top",
            0.0,
            40.0,
            Rect::new(70.0, 60.0, 330.0, 180.0),
        ),
        (
            "from its top-left",
            30.0,
            40.0,
            Rect::new(70.0, 60.0, 300.0, 180.0),
        ),
    ] {
        let committed = assert_preview_matches_commit_through_the_doors(
            &doc,
            &start_edge_resize(&doc, middle, left, top),
            what,
        );
        assert_eq!(
            Resolved::rebuild(&committed).world_bounds(middle).unwrap(),
            held,
            "{what}: the hold held"
        );
    }
}

/// **A stretched grid item resized from its left, its top and its top-left
/// corner previews as its commit, through the doors** (the release review's
/// `[X4.2-L6-01]`) — `grid_fixture`'s first rect, given `justify-self` and
/// `align-self: stretch` so it fills its 100 × 50 cell. The hold is grid's
/// (§15 D913's second ruling), with D905's end edge on both axes: a start-edge
/// resize writes `end`, so the far edges stay at 600 and 550. Until this, those
/// holds were pinned at commit in `ondin-core`'s `tests/grid.rs` and in no
/// preview; `a_grid_previews_as_it_commits` writes a right-edge hold by hand.
///
/// **Flip run**, `build::keep_flex_sizes` leaving out the holds it appends: fails
/// on *"from its left: preview diverged from commit"*, the predicted site.
///
/// **Flip run**, `build::flex_holds`' grid arm answering the item unchanged (the
/// finding's own flip): fails on *"from its left: the hold held"*, the item
/// stretched back to its cell at 500–600. ⚠️ **The differential does not
/// bite on it, and cannot**: `keep_flex_sizes` takes its holds from
/// `flex_holds`, so the preview and the commit both lose the hold and still
/// agree. That is why each case also asserts the committed box. The finding
/// expected the parity check alone to catch this flip.
#[test]
fn a_stretched_grid_item_resized_from_its_start_edges_previews_as_it_commits() {
    use ondin_core::container::{AlignItems, LayoutItem};
    let (mut doc, _frame, [first, ..]) = grid_fixture();
    doc.apply(&Transaction(vec![Operation::SetLayoutItem {
        id: first,
        item: LayoutItem {
            justify_self: Some(AlignItems::Stretch),
            align_self: Some(AlignItems::Stretch),
            ..LayoutItem::default()
        },
    }]))
    .unwrap();
    assert_eq!(
        Resolved::rebuild(&doc).world_bounds(first).unwrap(),
        Rect::new(500.0, 500.0, 600.0, 550.0),
        "the fixture: the first rect stretched over its 100 × 50 cell"
    );
    for (what, left, top, held) in [
        (
            "from its left",
            30.0,
            0.0,
            Rect::new(530.0, 500.0, 600.0, 550.0),
        ),
        (
            "from its top",
            0.0,
            20.0,
            Rect::new(500.0, 520.0, 600.0, 550.0),
        ),
        (
            "from its top-left",
            30.0,
            20.0,
            Rect::new(530.0, 520.0, 600.0, 550.0),
        ),
    ] {
        let committed = assert_preview_matches_commit_through_the_doors(
            &doc,
            &start_edge_resize(&doc, first, left, top),
            what,
        );
        assert_eq!(
            Resolved::rebuild(&committed).world_bounds(first).unwrap(),
            held,
            "{what}: the hold held"
        );
    }
}

/// **A pinned layer resized from its left, and one moved, previews as its
/// commit, through the doors** (the release review's `[X4.2-L6-01]`) —
/// `pinned_fixture`'s layer stretched between `left: 10` and `right: 10`, its left
/// edge dragged 30 in; then its right-pinned sibling moved. The preview draws
/// each where the tool's own ops put it, since `relayout` skips a layer the
/// transaction places. The commit turns them into insets through
/// `build::keep_insets` (§15 D874). Each case also asserts the committed box.
///
/// **Flip run**, `build::keep_insets` returning its transaction untouched: fails
/// on *"a stretched pinned layer resized from its left: preview diverged from
/// commit"*, the predicted site. The old insets stretch the layer back to 280 at
/// x 10.
///
/// **Flip run**, `RenderOverrides::relayout` no longer skipping a layer the
/// transaction places: fails at the same site the other way round, with the
/// *preview* re-pinned at 280 by the old insets and the commit at 250 from x 40.
/// No other test in this file fails on that flip, so this is that skip's only
/// pin here.
#[test]
fn a_pinned_layer_resized_or_moved_previews_as_it_commits() {
    let (doc, frame, _) = pinned_fixture();
    let kids = doc.get(frame).unwrap().children().to_vec();
    let (right, wide) = (kids[0], kids[1]);
    assert_eq!(
        Resolved::rebuild(&doc).world_bounds(wide).unwrap(),
        Rect::new(10.0, 60.0, 290.0, 100.0),
        "the fixture: the layer stretched between its two insets"
    );
    let committed = assert_preview_matches_commit_through_the_doors(
        &doc,
        &start_edge_resize(&doc, wide, 30.0, 0.0),
        "a stretched pinned layer resized from its left",
    );
    assert_eq!(
        Resolved::rebuild(&committed).world_bounds(wide).unwrap(),
        Rect::new(40.0, 60.0, 290.0, 100.0),
        "re-pinned where it was dropped"
    );
    let committed = assert_preview_matches_commit_through_the_doors(
        &doc,
        &Transaction(vec![Operation::SetTransform {
            id: right,
            transform: Affine::translate((150.0, 10.0)),
        }]),
        "a right-pinned layer moved",
    );
    assert_eq!(
        Resolved::rebuild(&committed).world_bounds(right).unwrap(),
        Rect::new(150.0, 10.0, 250.0, 50.0),
        "re-pinned where it was moved"
    );
}
