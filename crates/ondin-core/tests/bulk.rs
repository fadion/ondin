//! Bulk property edits over a selection (`build.rs`, "bulk property edits").
//!
//! These back the multi-selection inspector and the Group Colors panel. The
//! property they all turn on is scope: *which* nodes an edit reaches, and how
//! many times it reaches each one. Getting that wrong is invisible in a
//! screenshot and obvious in a document — an opacity applied twice, a colour
//! change that missed the strokes, a group recoloured when only one shape was
//! selected.

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, Cap, Join, RoundedRectRadii, Size};
use ondin_core::peniko::{Color, Gradient};
use ondin_core::{
    Document, Fill, History, IdSource, NodeId, NodeKind, Operation, Stroke, StrokeAlign,
    Transaction, build,
};

const RED: Color = Color::from_rgb8(0xEB, 0x6E, 0x5A);
const BLUE: Color = Color::from_rgb8(0x3C, 0x78, 0xDC);
const GREEN: Color = Color::from_rgb8(0x2E, 0xA0, 0x43);

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: RoundedRectRadii::default(),
    }
}

fn solid_fill(c: Color) -> Fill {
    Fill {
        brush: Brush::Solid(c),
        visible: true,
    }
}

fn solid_stroke(c: Color) -> Stroke {
    Stroke {
        brush: Brush::Solid(c),
        width: 1.0,
        join: Join::Miter,
        cap: Cap::Butt,
        dashes: Vec::new(),
        dash_offset: 0.0,
        sides: Default::default(),
        miter_limit: 4.0,
        dash_fit: false,
        align: StrokeAlign::Center,
        visible: true,
    }
}

struct Fixture {
    doc: Document,
    hist: History,
    ids: IdSource,
    artboard: NodeId,
}

impl Fixture {
    fn new() -> Self {
        let mut ids = IdSource::new(0xB01C);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut hist = History::new();
        let artboard = ids.mint();
        hist.commit(
            &mut doc,
            Transaction(vec![Operation::CreateNode {
                id: artboard,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            }]),
        )
        .unwrap();
        Self {
            doc,
            hist,
            ids,
            artboard,
        }
    }

    fn add(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.hist
            .commit(
                &mut self.doc,
                Transaction(vec![Operation::CreateNode {
                    id,
                    parent,
                    index,
                    kind,
                    transform: Some(Affine::IDENTITY),
                    name: None,
                }]),
            )
            .unwrap();
        id
    }

    fn paint(&mut self, id: NodeId, fills: Vec<Fill>, strokes: Vec<Stroke>) {
        self.commit(Transaction(vec![
            Operation::SetFills { id, fills },
            Operation::SetStrokes { id, strokes },
        ]));
    }

    fn commit(&mut self, tx: Transaction) {
        self.hist.commit(&mut self.doc, tx).expect("commit");
    }

    fn fill0(&self, id: NodeId) -> Brush {
        self.doc.get(id).unwrap().paint().fills[0].brush.clone()
    }
}

/// **The apply-once rule for properties (§5.7a).** A selection holding both a
/// group and something inside it must visit each node exactly once: a colour
/// written twice is harmless, but the same walk feeds edits where it is not.
///
/// `build::outermost` alone is the wrong filter here, and this is the test that
/// says why — it hands back the group and loses the shapes inside it, which are
/// the things that actually carry paint.
#[test]
fn a_selection_holding_a_group_and_its_child_visits_each_node_once() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let a = f.add(g, rect());
    let b = f.add(g, rect());

    // The group *and* one of its children — what a careless shift-click makes.
    assert_eq!(
        build::subtree_nodes(&f.doc, &[g, a]),
        vec![g, a, b],
        "the group, then its children, each exactly once"
    );
    // And the answer does not depend on the order they were selected in: an op
    // list has to replay identically on another replica (§12).
    assert_eq!(build::subtree_nodes(&f.doc, &[a, g]), vec![g, a, b]);
    // A node reached only as a descendant is still reached.
    assert!(build::subtree_nodes(&f.doc, &[g]).contains(&b));
}

/// The census counts every place a colour is used — every fill and every stroke,
/// a frame's fills among them — and leads with the commonest, so the list opens on
/// the colours the artwork is actually made of.
#[test]
fn the_color_census_counts_uses_and_leads_with_the_commonest() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    let b = f.add(f.artboard, rect());
    let c = f.add(f.artboard, rect());
    f.paint(a, vec![solid_fill(RED)], vec![solid_stroke(RED)]);
    f.paint(b, vec![solid_fill(RED)], vec![]);
    f.paint(c, vec![solid_fill(BLUE)], vec![]);

    let found: Vec<(Color, usize)> = build::colors_in(&f.doc, &[f.artboard])
        .iter()
        .map(|u| (u.color, u.uses))
        .collect();
    assert_eq!(
        found,
        vec![(RED, 3), (BLUE, 1)],
        "three reds (one of them a stroke), one blue: {found:?}"
    );

    // Scope is real: asking about one shape sees only that shape's colours.
    let just_c = build::colors_in(&f.doc, &[c]);
    assert_eq!(just_c.len(), 1);
    assert_eq!(just_c[0].color, BLUE);
}

/// A gradient is not one colour, so it is left out rather than flattened to a
/// stop. A row claiming a gradient was `#EB6E5A` would recolour something the
/// user never pointed at.
#[test]
fn the_color_census_leaves_gradients_out() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    f.paint(
        a,
        vec![
            Fill {
                brush: Brush::Gradient(Gradient::new_linear((0.0, 0.0), (10.0, 0.0)).into()),
                visible: true,
            },
            solid_fill(GREEN),
        ],
        vec![],
    );
    let census = build::colors_in(&f.doc, &[a]);
    assert_eq!(census.len(), 1, "only the solid: {census:?}");
    assert_eq!(census[0].color, GREEN);
}

/// A hidden fill still carries a colour. Leaving it out of the census would mean
/// a recolour skipped it and the old colour reappeared the moment the fill was
/// switched back on.
#[test]
fn the_color_census_counts_paints_that_are_switched_off() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    f.paint(
        a,
        vec![Fill {
            brush: Brush::Solid(RED),
            visible: false,
        }],
        vec![],
    );
    assert_eq!(build::colors_in(&f.doc, &[a]).len(), 1);

    f.commit(build::recolor(&f.doc, &[a], RED, GREEN));
    let fill = &f.doc.get(a).unwrap().paint().fills[0];
    assert_eq!(fill.brush, Brush::Solid(GREEN));
    assert!(!fill.visible, "recolouring must not switch it back on");
}

/// **Recolour by colour, not by layer.** One Group Colors row changes every use
/// of that colour wherever it hides — a fill here, a stroke there — in one
/// transaction, so a palette change is one undo step rather than a hunt. Colours
/// it was not asked about, and layers outside the scope, must not move.
#[test]
fn recoloring_changes_every_use_and_nothing_else() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let a = f.add(g, rect());
    let b = f.add(g, rect());
    let outside = f.add(f.artboard, rect());
    f.paint(a, vec![solid_fill(RED)], vec![solid_stroke(RED)]);
    f.paint(b, vec![solid_fill(BLUE)], vec![]);
    f.paint(outside, vec![solid_fill(RED)], vec![]);

    // Scoped to the group, so the red outside it is out of reach.
    f.commit(build::recolor(&f.doc, &[g], RED, GREEN));

    assert_eq!(f.fill0(a), Brush::Solid(GREEN), "the fill changed");
    assert_eq!(
        f.doc.get(a).unwrap().paint().strokes[0].brush,
        Brush::Solid(GREEN),
        "the stroke of the same colour changed with it"
    );
    assert_eq!(f.fill0(b), Brush::Solid(BLUE), "a colour not asked about");
    assert_eq!(
        f.fill0(outside),
        Brush::Solid(RED),
        "outside the scope, untouched"
    );

    // One undo puts all of it back — the whole point of one transaction.
    f.hist.undo(&mut f.doc).unwrap();
    assert_eq!(f.fill0(a), Brush::Solid(RED));
    assert_eq!(
        f.doc.get(a).unwrap().paint().strokes[0].brush,
        Brush::Solid(RED)
    );
}

/// Layers that do not carry the colour contribute no operation, so the undo step
/// is the size of the change rather than the size of the selection.
#[test]
fn recoloring_leaves_untouched_layers_out_of_the_transaction() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    let b = f.add(f.artboard, rect());
    f.paint(a, vec![solid_fill(RED)], vec![]);
    f.paint(b, vec![solid_fill(BLUE)], vec![]);

    let tx = build::recolor(&f.doc, &[f.artboard], RED, GREEN);
    assert_eq!(tx.0.len(), 1, "only the red layer: {:?}", tx.0);
}

/// A frame's fills are colours in the room too, and the panel lists them, so
/// recolouring has to reach them.
///
/// ⚠️ **Two fills, and the second is the point** (§15 D400). This used to set one
/// `SetArtboardBackground` and read one field back, because that was all a frame
/// could hold — and `build::recolor` had a third branch, beside its fill loop and
/// its stroke loop, that could only ever see the one. A frame goes through the
/// fill loop now, so the assertion that says so is a frame carrying **more paint
/// than the old branch could have found**.
///
/// Flipped by reverting `recolor` to skip `Artboard` in `paint_targets`: fails on
/// the census, before the recolour is even attempted.
#[test]
fn recoloring_reaches_every_one_of_a_frames_fills() {
    let mut f = Fixture::new();
    f.paint(f.artboard, vec![solid_fill(RED), solid_fill(RED)], vec![]);
    let census = build::colors_in(&f.doc, &[f.artboard]);
    assert_eq!(census[0].color, RED);
    assert_eq!(census[0].uses, 2, "two fills, two uses: {census:?}");

    f.commit(build::recolor(&f.doc, &[f.artboard], RED, BLUE));
    let fills = &f.doc.get(f.artboard).unwrap().paint().fills;
    assert_eq!(
        fills.iter().map(|f| f.brush.clone()).collect::<Vec<_>>(),
        vec![Brush::Solid(BLUE), Brush::Solid(BLUE)],
        "both, not just the bottom one"
    );
}

/// **Opacity reaches the outermost members only.** It composes down the tree, so
/// setting it on a group *and* on each child would land at the product — ask for
/// 50% over a group and get 25% on everything inside it.
#[test]
fn setting_opacity_over_a_group_does_not_also_set_its_children() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let a = f.add(g, rect());

    let tx = build::set_opacity_all(&f.doc, &[g], 0.5);
    assert_eq!(tx.0.len(), 1, "the group alone: {:?}", tx.0);
    f.commit(tx);
    assert!((f.doc.get(g).unwrap().opacity() - 0.5).abs() < 1e-6);
    assert!(
        (f.doc.get(a).unwrap().opacity() - 1.0).abs() < 1e-6,
        "the child keeps its own opacity, or the two would multiply"
    );
}

/// The corner-radius control appears only when there is a rect to act on, and
/// reports a shared value only when the rects agree *and* each is evenly
/// rounded — an unevenly rounded rect has no single number to show.
#[test]
fn corner_radius_is_shared_only_when_every_rect_agrees_evenly() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    assert!(!build::any_rect_in(&f.doc, &[g]), "no rect yet");

    let a = f.add(g, rect());
    let b = f.add(g, rect());
    assert!(build::any_rect_in(&f.doc, &[g]));
    assert_eq!(build::shared_corner_radius(&f.doc, &[g]), Some(0.0));

    f.commit(build::set_corner_radius_all(&f.doc, &[g], 6.0));
    assert_eq!(build::shared_corner_radius(&f.doc, &[g]), Some(6.0));

    f.commit(Transaction(vec![Operation::SetGeometry {
        id: b,
        geometry: ondin_core::GeometryPatch::CornerRadii(RoundedRectRadii::new(6.0, 0.0, 6.0, 6.0)),
    }]));
    assert_eq!(
        build::shared_corner_radius(&f.doc, &[g]),
        None,
        "one corner differs, so there is no single radius to show"
    );
    let _ = a;
}

/// Radius reaches rects **inside** a selected group, unlike opacity. It does not
/// compose, so there is no double-application to avoid, and "round these corners"
/// plainly means the ones that are there.
#[test]
fn corner_radius_reaches_into_a_selected_group() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let inner = f.add(g, rect());

    f.commit(build::set_corner_radius_all(&f.doc, &[g], 4.0));
    let NodeKind::Rect { corner_radii, .. } = f.doc.get(inner).unwrap().kind() else {
        panic!("not a rect");
    };
    assert_eq!(corner_radii.top_left, 4.0);
}

/// **A control that loses focus without being edited must produce nothing.**
///
/// Every inspector field commits on `lost_focus`, edited or not. A builder that
/// emits an operation per node regardless would turn each stray click into an
/// undo step made entirely of writes that change nothing — and the user's Ctrl+Z
/// would then do nothing visible, several times in a row, before reaching the
/// edit they wanted back.
#[test]
fn setting_a_property_to_what_it_already_is_produces_no_operations() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let a = f.add(g, rect());
    let b = f.add(g, rect());
    f.commit(build::set_corner_radius_all(&f.doc, &[g], 6.0));
    f.commit(build::set_opacity_all(&f.doc, &[g], 0.5));

    assert!(
        build::set_corner_radius_all(&f.doc, &[g], 6.0).0.is_empty(),
        "the rects are already at 6"
    );
    assert!(
        build::set_opacity_all(&f.doc, &[g], 0.5).0.is_empty(),
        "the group is already at 50%"
    );
    // And a real change still lands, so the filter has not eaten the edit.
    assert_eq!(build::set_corner_radius_all(&f.doc, &[g], 7.0).0.len(), 2);
    let _ = (a, b);
}

/// Recolouring a colour to itself is the same non-edit, reached the other way:
/// opening a Group Colors picker and closing it without moving anything.
#[test]
fn recoloring_to_the_same_color_produces_no_operations() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    f.paint(a, vec![solid_fill(RED)], vec![]);
    assert!(
        build::recolor(&f.doc, &[f.artboard], RED, RED).0.is_empty(),
        "recolouring red to red is not an edit"
    );
    // A real recolour still lands, so the guard has not eaten the edit.
    f.commit(build::recolor(&f.doc, &[f.artboard], RED, GREEN));
    assert_eq!(f.fill0(a), Brush::Solid(GREEN));
}

// --- the shared paint list, and how frames take part ----------------------

/// **What the Fill panel shows over a selection is the shared *list*, not the
/// first brush.** The panel offers add and remove like any other, so agreement
/// has to mean the whole list agrees — two layers that both start red but differ
/// in their second fill do not share a fill list, and a panel claiming they did
/// would delete one of them on the next edit.
///
/// **And an unpainted layer among painted ones is mixed, not empty.** That was the
/// other way round when the mixed row was first built, on the reading that a
/// stand-in row needs every target to have something to stand for. It is the
/// commonest disagreement there is — one layer with a stroke, one without — and
/// reporting it as "No stroke" is false of half the selection, in the direction that
/// matters: the stroke it denies is the one on screen. Only a selection where
/// *nothing* is painted is empty.
#[test]
fn a_shared_fill_list_means_the_whole_list_agrees() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let a = f.add(g, rect());
    let b = f.add(g, rect());
    f.paint(a, vec![solid_fill(RED)], vec![]);
    f.paint(b, vec![solid_fill(RED)], vec![]);
    assert_eq!(
        build::shared_fills(&f.doc, &[g]),
        build::PaintShown::List(vec![solid_fill(RED)]),
        "identical lists"
    );

    f.paint(b, vec![solid_fill(RED), solid_fill(BLUE)], vec![]);
    assert_eq!(
        build::shared_fills(&f.doc, &[g]),
        build::PaintShown::Mixed,
        "same first fill, different lists — and both layers are painted"
    );

    // One unpainted layer among painted ones: the mixed row, because there is a
    // fill in the selection and "No fill" would deny it.
    f.paint(b, vec![solid_fill(RED)], vec![]);
    let _bare = f.add(g, rect());
    assert_eq!(
        build::shared_fills(&f.doc, &[g]),
        build::PaintShown::Mixed,
        "a layer with no fill beside two with one is mixed, not empty"
    );

    // Only when nothing at all is painted is the panel empty — the one reading of
    // "No fill" that is not a lie about somebody. The layer added above is still
    // there and still unpainted; what changed is that now they agree.
    for id in [a, b] {
        f.paint(id, vec![], vec![]);
    }
    assert_eq!(
        build::shared_fills(&f.doc, &[g]),
        build::PaintShown::List(Vec::new()),
        "nothing painted anywhere is the empty state"
    );
}

/// The mixed row's values are asked **property by property**, over every paint of
/// every target — which is the whole reason it can be more than the word "Mixed".
///
/// Two layers that disagree only about how many fills they have still agree about
/// the colour, and a row that said otherwise would be reporting a disagreement
/// that is not there. The write is the same shape: it changes the one property and
/// leaves each list its length and its order, so nothing is destroyed by an edit
/// nobody asked to be destructive.
#[test]
fn a_mixed_row_states_what_the_paints_agree_on_and_edits_one_property() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let one = f.add(g, rect());
    let two = f.add(g, rect());
    f.paint(one, vec![solid_fill(RED)], vec![]);
    f.paint(two, vec![solid_fill(RED), solid_fill(RED)], vec![]);
    assert_eq!(build::shared_fills(&f.doc, &[g]), build::PaintShown::Mixed);
    assert_eq!(
        build::shared_over_fills(&f.doc, &[g], |x| x.brush.clone()),
        Some(Brush::Solid(RED)),
        "three fills, one colour: the row says red rather than mixed"
    );

    // Hide them all — the property edit every target takes, and the one that must
    // not cost the two-fill layer its second fill.
    f.commit(build::edit_fills_all(&f.doc, &[g], |x| x.visible = false));
    assert_eq!(f.doc.get(two).unwrap().paint().fills.len(), 2, "still two");
    assert_eq!(
        build::shared_over_fills(&f.doc, &[g], |x| x.visible),
        Some(false)
    );
    assert_eq!(
        build::shared_over_fills(&f.doc, &[g], |x| x.brush.clone()),
        Some(Brush::Solid(RED)),
        "and the colour was not touched on the way past"
    );
    assert_eq!(
        build::shared_fills(&f.doc, &[g]),
        build::PaintShown::Mixed,
        "the lists still differ, so the panel is still showing the one row"
    );

    // A disagreement in one property is invisible to the others.
    f.commit(build::set_fills_all(&f.doc, &[one], &[solid_fill(BLUE)]));
    assert_eq!(
        build::shared_over_fills(&f.doc, &[g], |x| x.brush.clone()),
        None,
        "now the colour is mixed"
    );
    assert_eq!(
        build::shared_over_fills(&f.doc, &[g], |x| x.visible),
        None,
        "and set_fills_all wrote a visible fill over a hidden one"
    );
}

/// The stroke half of the same rule, and the one place it has more than a colour
/// to say: a set that agrees about the weight and not the position shows the
/// weight as itself.
#[test]
fn a_mixed_stroke_row_agrees_per_property_too() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let a = f.add(g, rect());
    let b = f.add(g, rect());
    let wide = |align| Stroke {
        width: 4.0,
        align,
        ..solid_stroke(RED)
    };
    f.paint(a, vec![], vec![wide(StrokeAlign::Inside)]);
    f.paint(b, vec![], vec![wide(StrokeAlign::Outside)]);
    assert_eq!(
        build::shared_strokes(&f.doc, &[g]),
        build::PaintShown::Mixed
    );
    assert_eq!(
        build::shared_over_strokes(&f.doc, &[g], |s| s.width),
        Some(4.0),
        "the weight is not in dispute"
    );
    assert_eq!(
        build::shared_over_strokes(&f.doc, &[g], |s| s.align),
        None,
        "the position is"
    );

    f.commit(build::edit_strokes_all(&f.doc, &[g], |s| {
        s.align = StrokeAlign::Center
    }));
    assert_eq!(
        build::shared_strokes(&f.doc, &[g]),
        build::PaintShown::List(vec![wide(StrokeAlign::Center)]),
        "settling the one property they disagreed about ends the disagreement"
    );
}

/// Writing the list gives every target *exactly* it, which is what lets the panel
/// be believed: what it shows is then true of all of them.
#[test]
fn setting_the_fill_list_replaces_it_everywhere() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let two = f.add(g, rect());
    let none = f.add(g, rect());
    f.paint(two, vec![solid_fill(RED), solid_fill(BLUE)], vec![]);

    f.commit(build::set_fills_all(&f.doc, &[g], &[solid_fill(GREEN)]));
    for id in [two, none] {
        let fills = &f.doc.get(id).unwrap().paint().fills;
        assert_eq!(fills.len(), 1, "one fill each now");
        assert_eq!(fills[0].brush, Brush::Solid(GREEN));
    }
    assert_eq!(
        build::shared_fills(&f.doc, &[g]),
        build::PaintShown::List(vec![solid_fill(GREEN)]),
        "and they now agree, so the panel shows the list it just wrote"
    );
}

/// **A group is organisation; a frame is a thing.** A paint edit passes straight
/// through a group to the shapes inside — selecting a group is the same as
/// selecting its contents — but stops *at* a frame, whose background is the
/// target and whose contents are its own.
#[test]
fn paint_reaches_through_a_group_and_stops_at_a_frame() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group);
    let inside_group = f.add(g, rect());
    let inside_frame = f.add(f.artboard, rect());

    assert_eq!(
        build::paint_targets(&f.doc, &[g]),
        vec![inside_group],
        "a group hands over its contents"
    );
    assert_eq!(
        build::paint_targets(&f.doc, &[f.artboard]),
        vec![f.artboard],
        "a frame is the target itself, not its contents"
    );
    let _ = inside_frame;
}

/// A frame's fill *is* a fill, so one gesture can colour a frame and a shape
/// together — which is what "a fill applies to all selected frames and shapes" has
/// to mean.
///
/// ⚠️ **The list handed over is two long, and that is the assertion this test is
/// now for** (§15 D400). It used to pass one fill, because a frame took
/// `fills.first()` and dropped the rest — the "known limitation" `architecture.md`
/// §5.7a recorded, and the state where the panel's own list stopped being the
/// truth for part of what it was showing. Both targets get both fills now.
///
/// Flipped by putting `fills.first()` back in `set_fills_all`'s frame arm: fails
/// on the length, reporting one fill where the panel is showing two.
#[test]
fn a_fill_over_a_frame_and_a_shape_colours_both() {
    let mut f = Fixture::new();
    let shape = f.add(f.artboard, rect());
    let root = f.doc.root();
    // The frame and a loose shape, selected together.
    let loose = f.add(root, rect());

    let both = [solid_fill(RED), solid_fill(BLUE)];
    f.commit(build::set_fills_all(&f.doc, &[f.artboard, loose], &both));

    assert_eq!(
        f.doc.get(f.artboard).unwrap().paint().fills,
        both,
        "the frame took the whole list, not its first entry"
    );
    assert_eq!(
        f.doc.get(loose).unwrap().paint().fills,
        both,
        "so did the shape"
    );

    // And the frame's own contents were left alone — the walk stopped at it.
    assert!(
        f.doc.get(shape).unwrap().paint().fills.is_empty(),
        "a shape inside the frame is not part of the selection"
    );

    // The two now agree, so the panel reads back one shared list — the whole list,
    // which is what made this the assertion the old one-fill frame could not make.
    assert_eq!(
        build::shared_fills(&f.doc, &[f.artboard, loose]),
        build::PaintShown::List(both.to_vec())
    );
}

/// **The rest of the cast, on a frame: reorder, hide, remove** (§15 D400).
///
/// The three verbs the Fill panel's rows offer that a frame's old single
/// background had nowhere to put — the grip, the eye and the `x`. None of them is a
/// verb of its own in the model: the panel rewrites the list and commits one
/// `SetFills`, so what this pins is that the list survives each of the three
/// *shapes* of rewrite on a frame exactly as it does on a shape.
///
/// The visibility one is the one worth the test rather than the reorder. A frame's
/// ground had no `visible` flag at all, so `edit_fills_all` pulled only the *brush*
/// back out of the fill it handed the closure — which meant "hide every fill in the
/// selection" silently skipped the frames in it, and the panel's eye had nothing to
/// draw. Flipped by dropping the `visible` write: the middle assertion fails.
#[test]
fn a_frames_fill_list_reorders_hides_and_removes_like_any_other() {
    let mut f = Fixture::new();
    let ab = f.artboard;
    f.paint(ab, vec![solid_fill(RED), solid_fill(BLUE)], vec![]);
    let fills = |f: &Fixture| f.doc.get(ab).unwrap().paint().fills.clone();

    // Reorder: the panel's grip hands the whole list back swapped.
    let swapped = vec![fills(&f)[1].clone(), fills(&f)[0].clone()];
    f.commit(build::set_fills_all(&f.doc, &[ab], &swapped));
    assert_eq!(
        fills(&f)
            .iter()
            .map(|x| x.brush.clone())
            .collect::<Vec<_>>(),
        vec![Brush::Solid(BLUE), Brush::Solid(RED)],
        "blue is the ground now and red is over it"
    );

    // Hide: through `edit_fills_all`, which is the bulk verb the mixed row uses and
    // the one that used to drop this property on the floor for a frame.
    f.commit(build::edit_fills_all(&f.doc, &[ab], |fill| {
        fill.visible = false
    }));
    assert!(
        fills(&f).iter().all(|x| !x.visible),
        "both hidden, and a frame is not exempt: {:?}",
        fills(&f)
    );

    // Remove: the row's `x` is a shorter list, down to the empty one, which is what
    // "delete the colour" has always meant on a frame and still does.
    f.commit(build::set_fills_all(&f.doc, &[ab], &fills(&f)[..1]));
    assert_eq!(fills(&f).len(), 1);
    f.commit(build::set_fills_all(&f.doc, &[ab], &[]));
    assert!(
        fills(&f).is_empty(),
        "a frame with no ground is transparent"
    );
}

/// **A frame is a full participant in a shared stroke** — counted, disagreed with,
/// and written to.
///
/// This used to assert the opposite, when the scene walk painted a frame's
/// background and returned. Frames stroke now (§15 D144), so "give everything
/// selected a 2pt border" reaches a frame and a rectangle in one gesture, as the
/// colour version always did.
///
/// ⚠️ **It also used to be a test about a *disagreement*** — its name was
/// `a_frame_takes_a_stroke_where_its_fill_is_its_background`, and the point was
/// that `takes_stroke` and `takes_paint` answered differently on exactly this kind.
/// They do not any more (§15 D400): `any_strokeable_in` has gone, the scope the
/// Stroke panel writes to is `paint_targets` like the Fill panel's, and the
/// question this asks is now the same one asked of a rectangle. Kept anyway,
/// because "a frame is in the stroke scope" is still load-bearing and nothing else
/// asserts it.
#[test]
fn a_frame_is_counted_in_a_shared_stroke() {
    let mut f = Fixture::new();
    let root = f.doc.root();
    let loose = f.add(root, rect());
    f.paint(loose, vec![], vec![solid_stroke(BLUE)]);

    assert!(
        build::any_paint_in(&f.doc, &[f.artboard]),
        "a frame alone has an outline to stroke, so the panel has somewhere to write"
    );
    // Counted, so a frame with no stroke beside a shape with one is the ordinary
    // disagreement rather than an invisible abstention.
    assert_eq!(
        build::shared_strokes(&f.doc, &[f.artboard, loose]),
        build::PaintShown::Mixed
    );

    f.commit(build::set_strokes_all(
        &f.doc,
        &[f.artboard, loose],
        &[solid_stroke(GREEN)],
    ));
    assert_eq!(
        f.doc.get(f.artboard).unwrap().paint().strokes[0].brush,
        Brush::Solid(GREEN),
        "the frame took the stroke"
    );
    assert_eq!(
        f.doc.get(loose).unwrap().paint().strokes[0].brush,
        Brush::Solid(GREEN)
    );
    // And they now agree, which is the other half of being counted.
    assert_eq!(
        build::shared_strokes(&f.doc, &[f.artboard, loose]),
        build::PaintShown::List(vec![solid_stroke(GREEN)])
    );
}

/// **The census counts uses but carries layers, and the two differ.** A shape whose
/// fill and stroke are the same colour is *two* uses of it and *one* layer with it —
/// so the number shown beside the Group Colors row and the selection its button
/// makes have to come from the node set, or the row would read "3 matches" and
/// select two things.
#[test]
fn the_color_census_carries_the_layers_as_well_as_the_use_count() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    let b = f.add(f.artboard, rect());
    let c = f.add(f.artboard, rect());
    // `a` uses red twice — once as a fill, once as a stroke.
    f.paint(a, vec![solid_fill(RED)], vec![solid_stroke(RED)]);
    f.paint(b, vec![solid_fill(RED)], vec![]);
    f.paint(c, vec![solid_fill(BLUE)], vec![]);

    let census = build::colors_in(&f.doc, &[f.artboard]);
    let red = census.iter().find(|u| u.color == RED).expect("red");
    assert_eq!(red.uses, 3, "three paint slots carry red");
    assert_eq!(
        red.nodes,
        vec![a, b],
        "but only two layers do, and `a` must not be listed twice"
    );
    let blue = census.iter().find(|u| u.color == BLUE).expect("blue");
    assert_eq!(blue.nodes, vec![c]);
}

/// The layer list is in **document order**, so the selection the button makes is
/// deterministic — it has to replay identically to be worth asserting on, and an
/// order that depended on which paint was met first would shuffle as paints were
/// added.
///
/// 🚨 **The scope half is new and it is the half that was false** (§15 D646,
/// `[S3.2-L2-05]`). This test passed a **single root** (`&[f.artboard]`), which
/// is the one case where the claim holds for free: `subtree_nodes` is pre-order
/// *within* a subtree, and with one subtree that is the whole answer. It paints
/// out of order and calls that the discriminating half — but paint order was
/// never what could break it, because the walk visits nodes and asks each one
/// what it carries, so the paint sequence is not consulted at any point.
///
/// The shape that was broken is the one the Group Colors panel actually passes:
/// `group_color_scope` returns `node.children()` only for a *single* container
/// and `selection.ids()` — **pick order**, "`Selection::ids` pushes as you
/// shift-click" — for every multi-selection. `subtree_nodes` seeded its stack
/// from `outermost`, whose own doc says *"in input order"*, so the row's
/// reticle re-selected two shift-clicked shapes in the order they were clicked.
///
/// ⚠️ **Three places asserted the rule and none of them tested this**:
/// `architecture.md` (*"`nodes` the distinct layers, **in document order**"*),
/// `ColorUse::nodes`' own doc, and the census's dedupe comment.
///
/// Flip: `subtree_nodes`' root ordering removed (`let roots = outermost(…)`
/// alone). Red at the **second** assertion with `[c, a, b]`; the first stays
/// green, which is exactly the gap this test had.
#[test]
fn the_censuss_layers_come_back_in_document_order() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    let b = f.add(f.artboard, rect());
    let c = f.add(f.artboard, rect());
    // Painted out of order: c, a, b.
    f.paint(c, vec![solid_fill(RED)], vec![]);
    f.paint(a, vec![solid_fill(RED)], vec![]);
    f.paint(b, vec![solid_fill(RED)], vec![]);

    let census = build::colors_in(&f.doc, &[f.artboard]);
    assert_eq!(
        census[0].nodes,
        vec![a, b, c],
        "tree order, not paint order"
    );

    // The scope handed over back-to-front, which is what a shift-click from the
    // front of the stack gives the panel.
    let picked = build::colors_in(&f.doc, &[c, a, b]);
    assert_eq!(
        picked[0].nodes,
        vec![a, b, c],
        "tree order, not the order the scope was passed in"
    );
}

/// A frame's ground counts, and the frame is the layer that carries it.
///
/// The census used to reach **three** kinds of paint slot — fills, strokes, and a
/// frame's `background` out of a branch of its own — and this pinned that all three
/// named their node. There are two now (§15 D400), and the branch this was written
/// against is gone; kept because "the frame, not something inside it" is still the
/// answer that would be wrong if the census ever walked a frame's contents into its
/// own row.
///
/// ⚠️ **This test executed zero assertions between §15 D400 and D480, and its own
/// doc comment is why** (`[S3.2-L6-04]`). It looped the census skipping the child's
/// colour and asserted on whatever was left, on the stated premise that *"the
/// fixture's artboard has a background"*. That was true while a frame's ground was
/// a field on `NodeKind::Artboard`; D400 moved it into the fill list and
/// `op_create` gives every node an **empty** one, so the loop ran on nothing. The
/// rule the test is named for went unpinned, and the census could have walked a
/// frame's contents into the frame's own row with this green.
///
/// The fixture paints the artboard now, and the assertion is direct rather than a
/// filtered loop — **a loop over rows that might not exist is the shape that made
/// this vacuous**, and naming the row asserts it is there.
///
/// ⚠️ **Flip-check, run: `build::colors_in`'s `bump` made to record the painted
/// node's *parent*.** Fails on `nodes`, which is the predicted site — GREEN then
/// names the root rather than the artboard. The pre-fix version stays green under
/// that same flip, which is the measurement that says it was asserting nothing.
#[test]
fn a_frames_background_names_the_frame_as_its_layer() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    f.paint(a, vec![solid_fill(BLUE)], vec![]);
    // The frame's own ground, which since D400 is a fill on the frame like any
    // other and has to be put there rather than assumed.
    f.paint(f.artboard, vec![solid_fill(GREEN)], vec![]);

    let census = build::colors_in(&f.doc, &[f.artboard]);
    let ground = census
        .iter()
        .find(|u| u.color == GREEN)
        .unwrap_or_else(|| panic!("the frame's own ground is counted at all: {census:?}"));
    assert_eq!(
        ground.nodes,
        vec![f.artboard],
        "the frame's own background must name the frame, and nothing inside it"
    );
    assert_eq!(ground.uses, 1, "one fill, one use");
}

/// **A gradient is counted and findable even though it cannot be recoloured.**
///
/// The census used to be solids only, on the reasoning that a gradient is not one
/// colour — true of the *edit*, and the wrong conclusion about the row: a selection's
/// gradients were neither counted nor selectable, so there was no way to answer "is
/// that ramp in here, and where".
///
/// Keyed by the whole brush, because a gradient's identity is its stops **and** its
/// geometry. Two ramps of the same colours at different angles are two gradients, and
/// merging them would print a count no selection could match.
#[test]
fn the_census_counts_gradients_separately_from_colours() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    let b = f.add(f.artboard, rect());
    let c = f.add(f.artboard, rect());
    let ramp = |from: (f64, f64), to: (f64, f64)| Fill {
        brush: Brush::Gradient(Gradient::new_linear(from, to).into()),
        visible: true,
    };
    // Two layers share one ramp; the third has the same one turned on its side.
    f.paint(a, vec![ramp((0.0, 0.0), (10.0, 0.0))], vec![]);
    f.paint(
        b,
        vec![ramp((0.0, 0.0), (10.0, 0.0)), solid_fill(RED)],
        vec![],
    );
    f.paint(c, vec![ramp((0.0, 0.0), (0.0, 10.0))], vec![]);

    let found = build::gradients_in(&f.doc, &[f.artboard]);
    assert_eq!(found.len(), 2, "two distinct ramps: {found:?}");
    assert_eq!(found[0].uses, 2, "the shared one leads, most-used first");
    assert_eq!(
        found[0].nodes,
        vec![a, b],
        "and it names the layers, in document order, for the select button"
    );
    assert_eq!(
        found[1].uses, 1,
        "the same colours at another angle is a second gradient, not the same one"
    );

    // The colour census is unchanged by any of it: still solids only, so the rows
    // that can be recoloured are exactly the rows that offer to be.
    let colors = build::colors_in(&f.doc, &[f.artboard]);
    assert_eq!(colors.len(), 1, "one solid among the ramps: {colors:?}");
    assert_eq!(colors[0].color, RED);
}

/// **A gradient row is keyed by a place, and that key outlives its own edit.**
///
/// A gradient's identity is its stops *and* its geometry, which does not fit in the
/// `Copy + Eq` slot the Group Colors panel hands the picker — four bytes of RGBA do.
/// So the census reports the first place it found each ramp, the picker edits through
/// that, and `repaint` changes every use of whatever is there.
///
/// The property that makes it work, and the one a value key does not have: after the
/// edit the place still holds the paint, so the row and the open picker are still
/// pointing at it. `recolor`'s key names a colour the document has just stopped
/// having, which is why it needs `retarget_group_color` and this needs nothing.
#[test]
fn a_gradient_is_keyed_by_where_it_was_found_and_the_key_survives_the_edit() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    let b = f.add(f.artboard, rect());
    let ramp = |to: (f64, f64)| Brush::Gradient(Gradient::new_linear((0.0, 0.0), to).into());
    let across = ramp((10.0, 0.0));
    // `a` carries it as its second fill; `b` as a stroke. Neither is fill 0, so a
    // location that assumed the obvious place would miss both.
    f.paint(
        a,
        vec![
            solid_fill(RED),
            Fill {
                brush: across.clone(),
                visible: true,
            },
        ],
        vec![],
    );
    f.paint(
        b,
        vec![],
        vec![Stroke {
            brush: across.clone(),
            ..solid_stroke(BLUE)
        }],
    );

    let found = build::gradients_in(&f.doc, &[f.artboard]);
    assert_eq!(found.len(), 1, "one ramp in two places: {found:?}");
    let at = found[0].at;
    assert_eq!(
        at,
        build::PaintAt {
            node: a,
            target: build::PaintTarget::Fill,
            index: 1
        },
        "the first place in document order, index and all"
    );
    assert_eq!(build::paint_at(&f.doc, at).as_ref(), Some(&across));

    // The edit the row makes: read the brush back from the place, change every use.
    let down = ramp((0.0, 10.0));
    f.commit(build::repaint(&f.doc, &[f.artboard], &across, &down));
    assert_eq!(
        f.doc.get(a).unwrap().paint().fills[1].brush,
        down,
        "the fill followed"
    );
    assert_eq!(
        f.doc.get(b).unwrap().paint().strokes[0].brush,
        down,
        "and so did the stroke, in the same transaction"
    );
    assert_eq!(
        f.doc.get(a).unwrap().paint().fills[0].brush,
        Brush::Solid(RED),
        "and nothing else was touched"
    );

    // **The key still names the paint.** This is the whole reason it is a place.
    assert_eq!(
        build::paint_at(&f.doc, at).as_ref(),
        Some(&down),
        "the place holds the new gradient, so the picker keeps editing it"
    );
    assert_eq!(build::gradients_in(&f.doc, &[f.artboard])[0].at, at);
}

// --- copy / paste properties ------------------------------------------------

/// **A group has no appearance to give, and answering with an empty one would be
/// worse than refusing** (`build::properties_of`).
///
/// The failure this guards is not a panic: `Properties { fills: [], strokes: [],
/// opacity }` off a group is a perfectly well-formed payload, and pasting it
/// **clears** whatever it lands on. So the refusal has to be at the source. It is
/// the same line `build::paint_targets` draws — a group is organisation, a frame
/// is a thing — and this asserts both sides of it.
#[test]
fn a_group_has_no_properties_to_copy_and_a_frame_does() {
    let mut f = Fixture::new();
    let group = f.add(f.artboard, NodeKind::Group);
    let shape = f.add(group, rect());
    f.paint(shape, vec![solid_fill(RED)], vec![solid_stroke(BLUE)]);

    assert!(
        build::properties_of(f.doc.get(group).unwrap()).is_none(),
        "a group's empty paint list is not an appearance — pasting it would clear"
    );
    assert!(build::properties_of(f.doc.get(shape).unwrap()).is_some());

    // A frame is the container that *is* a thing, and it carries an ordinary fill
    // list, so its colour can be handed to a rectangle. This used to go through a
    // `fills_seen_as` that made a one-entry stand-in out of its single background
    // (§15 D400), which is also why `props.fills` could never be longer than one.
    f.commit(Transaction(vec![Operation::SetFills {
        id: f.artboard,
        fills: vec![Fill {
            brush: Brush::Solid(GREEN),
            visible: true,
        }],
    }]));
    let props = build::properties_of(f.doc.get(f.artboard).unwrap()).expect("a frame has paint");
    assert_eq!(props.fills.len(), 1);
    assert_eq!(props.fills[0].brush, Brush::Solid(GREEN));
}

/// **Pasting properties uses the scope each property already has, and the two
/// scopes differ** — paint reaches *through* a group to the shapes inside, opacity
/// stops at the outermost member.
///
/// That difference is not an accident of composition, it is the reason
/// `build::paste_properties` is three existing verbs rather than one new walk:
/// opacity composes down the tree, so writing it to a group *and* its children
/// lands at the square (`set_opacity_all`'s own note), while a fill written to a
/// group would go nowhere at all because a group has no paint.
///
/// ⚠️ Flipped against the version somebody would write first — one walk over
/// `paint_targets` setting all three — where the group keeps opacity 1.0 and each
/// child gets 0.5, so the *rendered* result is 0.5 either way and only the
/// document says which happened. Asserted on the document for exactly that reason.
#[test]
fn pasting_properties_paints_through_a_group_and_fades_only_the_group() {
    let mut f = Fixture::new();
    let source = f.add(f.artboard, rect());
    f.paint(source, vec![solid_fill(RED)], vec![solid_stroke(GREEN)]);
    f.commit(Transaction(vec![Operation::SetOpacity {
        id: source,
        opacity: 0.5,
    }]));

    let group = f.add(f.artboard, NodeKind::Group);
    let a = f.add(group, rect());
    let b = f.add(group, rect());
    f.paint(a, vec![solid_fill(BLUE)], vec![]);
    f.paint(b, vec![solid_fill(BLUE)], vec![]);

    let props = build::properties_of(f.doc.get(source).unwrap()).unwrap();
    f.commit(build::paste_properties(&f.doc, &[group], &props));

    for child in [a, b] {
        assert_eq!(f.fill0(child), Brush::Solid(RED), "the fill reached inside");
        assert_eq!(
            f.doc.get(child).unwrap().paint().strokes.len(),
            1,
            "and so did the stroke"
        );
        assert_eq!(
            f.doc.get(child).unwrap().opacity(),
            1.0,
            "but the opacity did not — it would have compounded with the group's"
        );
    }
    assert_eq!(f.doc.get(group).unwrap().opacity(), 0.5);
    assert!(
        f.doc.get(group).unwrap().paint().fills.is_empty(),
        "and the group itself has no paint to have been given"
    );
}

/// **Pasting what a layer already looks like produces no operations**, so the
/// chord cannot fill the history with undo steps that change nothing.
///
/// Free by construction — all three composed verbs drop targets that already agree
/// — which is exactly why it is worth pinning: it is a property of the composition
/// rather than of anything written here, so a fourth property added by hand later
/// would silently lose it.
#[test]
fn pasting_the_properties_a_layer_already_has_is_not_an_edit() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect());
    let b = f.add(f.artboard, rect());
    for id in [a, b] {
        f.paint(id, vec![solid_fill(RED)], vec![solid_stroke(BLUE)]);
    }

    let props = build::properties_of(f.doc.get(a).unwrap()).unwrap();
    assert!(
        build::paste_properties(&f.doc, &[b], &props).0.is_empty(),
        "b already looks like a"
    );

    // And one property out of step is enough to make it an edit — the fixture
    // check that stops the assertion above passing over a builder that returns
    // nothing at all.
    f.commit(Transaction(vec![Operation::SetOpacity {
        id: b,
        opacity: 0.25,
    }]));
    let tx = build::paste_properties(&f.doc, &[b], &props);
    assert_eq!(tx.0.len(), 1, "only the opacity differs: {tx:?}");
    f.commit(tx);
    assert_eq!(f.doc.get(b).unwrap().opacity(), 1.0);
}
