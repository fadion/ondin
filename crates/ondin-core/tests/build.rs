//! Composite builders and the world-space projection facade (`build.rs`).
//!
//! The property every one of these guards is the same: a structural edit must
//! not move anything on screen. Grouping, ungrouping and reparenting all change
//! which parent a node hangs off, and since transforms are local (invariant 6)
//! that silently teleports the node unless the builder re-projects it.

use ondin_core::kurbo::{Affine, Point, RoundedRectRadii, Size, Vec2};
use ondin_core::{
    Document, History, IdSource, NodeId, NodeKind, OpError, Operation, Resolved, Transaction, build,
};

fn rect(w: f64, h: f64) -> NodeKind {
    NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
    }
}

fn artboard() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(800.0, 600.0),
    }
}

/// A curve to hang type on — the shape anyone actually sets text around.
fn ellipse(w: f64, h: f64) -> NodeKind {
    NodeKind::Ellipse {
        size: Size::new(w, h),
    }
}

struct Fixture {
    doc: Document,
    hist: History,
    ids: IdSource,
    root: NodeId,
    artboard: NodeId,
}

impl Fixture {
    /// Root → artboard translated by (50, 30), so parent space is never the
    /// same as world space and a missing re-projection shows up immediately.
    fn new() -> Self {
        let mut ids = IdSource::new(0xB0A7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut hist = History::new();
        let artboard_id = ids.mint();
        hist.commit(
            &mut doc,
            Transaction(vec![Operation::CreateNode {
                id: artboard_id,
                parent: root,
                index: 0,
                kind: artboard(),
                transform: Some(Affine::translate((50.0, 30.0))),
                name: None,
            }]),
        )
        .unwrap();
        Self {
            doc,
            hist,
            ids,
            root,
            artboard: artboard_id,
        }
    }

    fn add(&mut self, parent: NodeId, kind: NodeKind, at: (f64, f64)) -> NodeId {
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
                    transform: Some(Affine::translate(at)),
                    name: None,
                }]),
            )
            .unwrap();
        id
    }

    fn resolved(&self) -> Resolved {
        Resolved::rebuild(&self.doc)
    }

    fn commit(&mut self, tx: Transaction) {
        self.hist.commit(&mut self.doc, tx).expect("commit");
    }

    /// Where the node's local origin sits in world space.
    fn world_origin(&self, id: NodeId) -> Point {
        self.resolved().world_transform(id).unwrap() * Point::ZERO
    }

    fn children_of(&self, id: NodeId) -> Vec<NodeId> {
        self.doc.get(id).unwrap().children().to_vec()
    }
}

fn approx(a: Point, b: Point, what: &str) {
    assert!(
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9,
        "{what}: {a:?} vs {b:?}"
    );
}

#[test]
fn group_preserves_world_positions_and_relative_order() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (100.0, 100.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (200.0, 150.0));
    let before = [f.world_origin(a), f.world_origin(b)];

    let (tx, g) = build::group(&f.doc, &mut f.ids, &[b, a]).unwrap();
    f.commit(tx);

    // Members moved inside the group, in their original z-order (a then b),
    // regardless of the order they were passed in.
    assert_eq!(f.children_of(g), vec![a, b]);
    assert_eq!(f.doc.get(a).unwrap().parent(), Some(g));
    // Nothing moved on screen.
    approx(f.world_origin(a), before[0], "a stayed put");
    approx(f.world_origin(b), before[1], "b stayed put");
}

#[test]
fn group_takes_the_z_position_of_its_topmost_member() {
    let mut f = Fixture::new();
    let back = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let mid = f.add(f.artboard, rect(10.0, 10.0), (10.0, 0.0));
    let front = f.add(f.artboard, rect(10.0, 10.0), (20.0, 0.0));

    // Group the middle and front shapes; `back` must stay behind the group.
    let (tx, g) = build::group(&f.doc, &mut f.ids, &[mid, front]).unwrap();
    f.commit(tx);
    assert_eq!(f.children_of(f.artboard), vec![back, g]);
}

/// **`[S3.1-L2-01]`'s loss: the placement rule was the *bottom-most* member's
/// slot, on all five container verbs.**
///
/// ⚠️ **`group_takes_the_z_position_of_its_topmost_member` above cannot see it,
/// and neither can `a_frame_takes_the_z_position_of_its_topmost_member`.** The
/// first groups `{mid, front}` — contiguous, and including the top of the stack,
/// so the two rules give the same answer. The second frames a *single* member,
/// where they are identical by construction. Both are named for the rule and
/// **both pass against the wrong one**, which is this project's own second
/// vacuity shape: an assertion the wrong version also produces.
///
/// The two rules differ only when the selection has a **gap** in it, and then a
/// layer the user did not select jumps in front of one they did: select `back`
/// and `front`, leave `mid` out, press Ctrl+G, and the frontmost layer ends up
/// inside a container painted *behind* `mid`.
///
/// **All five verbs, because there were four literal copies of the four lines**
/// and `mask` reaches them through `group` (§15 D286). A single-site fix would
/// have left the other three, which is why the rule is one function now.
///
/// Flip-check, run: `position` for `rposition` in `container_slot` reports **all
/// five** verbs in one failure, each `got [front, mid], want [mid, front]`.
/// ⚠️ **And the other 85 tests in this file stay green under that flip**, which
/// is the measurement that says the coverage was vacuous rather than merely
/// thin — the wrong placement rule was invisible to every z-order test the
/// project has, including `group_then_ungroup_round_trips_world_positions` and
/// `ungroup_splices_children_back_in_place`.
#[test]
fn a_container_takes_the_topmost_slot_even_when_the_selection_has_a_gap() {
    // Index 0 is the bottom of the stack.
    let three = |f: &mut Fixture| {
        let back = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
        let mid = f.add(f.artboard, rect(10.0, 10.0), (40.0, 0.0));
        let front = f.add(f.artboard, rect(10.0, 10.0), (80.0, 0.0));
        (back, mid, front)
    };

    // ⚠️ **Collected and asserted once, rather than five `assert_eq!`s.** Five
    // separate assertions abort on the first, so a red run would say nothing
    // about the other four — and "the other four" is the whole reason this test
    // names five verbs. This way one flip reports every verb that regressed.
    let mut wrong: Vec<String> = Vec::new();
    let mut check = |name: &str, f: &Fixture, want: Vec<NodeId>| {
        let got = f.children_of(f.artboard);
        if got != want {
            wrong.push(format!("{name}: got {got:?}, want {want:?}"));
        }
    };

    let mut f = Fixture::new();
    let (back, mid, front) = three(&mut f);
    let (tx, g) = build::group(&f.doc, &mut f.ids, &[back, front]).unwrap();
    f.commit(tx);
    check("group", &f, vec![mid, g]);

    let mut f = Fixture::new();
    let (back, mid, front) = three(&mut f);
    let (tx, fr) = build::frame(&f.doc, &f.resolved(), &mut f.ids, &[back, front]).unwrap();
    f.commit(tx);
    check("frame", &f, vec![mid, fr]);

    let mut f = Fixture::new();
    let (back, mid, front) = three(&mut f);
    let (tx, b) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[back, front],
        ondin_core::BoolOp::Union,
        None,
    )
    .unwrap();
    f.commit(tx);
    check("boolean", &f, vec![mid, b]);

    let mut f = Fixture::new();
    let (back, mid, front) = three(&mut f);
    let (tx, fl) = build::flatten(&f.doc, &f.resolved(), &mut f.ids, &[back, front]).unwrap();
    f.commit(tx);
    check("flatten", &f, vec![mid, fl]);

    // `mask` has no copy of its own — it calls `group` — and is here because the
    // rule reaching it is exactly what D286 records.
    let mut f = Fixture::new();
    let (back, mid, front) = three(&mut f);
    let (tx, m) = build::mask(&f.doc, &mut f.ids, &[back, front], None).unwrap();
    f.commit(tx);
    check("mask", &f, vec![mid, m]);

    assert!(
        wrong.is_empty(),
        "the container must land where the front member was:\n  {}",
        wrong.join("\n  ")
    );
}

#[test]
fn group_rejects_members_from_different_parents() {
    let mut f = Fixture::new();
    let inner = f.add(f.artboard, NodeKind::Group, (0.0, 0.0));
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(inner, rect(10.0, 10.0), (0.0, 0.0));
    assert!(matches!(
        build::group(&f.doc, &mut f.ids, &[a, b]),
        Err(OpError::InvalidParent)
    ));
}

#[test]
fn group_rejects_artboards_and_the_root() {
    let mut f = Fixture::new();
    assert!(matches!(
        build::group(&f.doc, &mut f.ids, &[f.artboard]),
        Err(OpError::WrongKindForOp)
    ));
    assert!(matches!(
        build::group(&f.doc, &mut f.ids, &[f.root]),
        Err(OpError::WrongKindForOp)
    ));
    assert!(build::group(&f.doc, &mut f.ids, &[]).is_err());
}

/// **A frame is sized to the union and nothing moves** (§15 D249). Two claims that
/// pull against each other: the frame gets a position, so every member has to be
/// moved back by exactly that position, where `group` holds the same invariant by
/// touching nothing at all.
///
/// The fixture's artboard is translated by (50, 30), so parent space is never world
/// space — a correction applied in the wrong space lands 50 units out rather than
/// looking right.
///
/// ⚠️ Flipped by dropping the `SetTransform` half (the spelling that reads as
/// `group`'s and is the whole difference): both members jump by the union's corner,
/// `a stayed put` failing at (250, 230) for (150, 130).
#[test]
fn a_frame_is_the_union_of_its_members_and_moves_none_of_them() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 20.0), (100.0, 100.0));
    let b = f.add(f.artboard, rect(30.0, 10.0), (200.0, 150.0));
    let before = [f.world_origin(a), f.world_origin(b)];

    let (tx, fr) = build::frame(&f.doc, &f.resolved(), &mut f.ids, &[b, a]).unwrap();
    f.commit(tx);

    // Original z-order, whatever order the members were named in.
    assert_eq!(f.children_of(fr), vec![a, b]);
    approx(f.world_origin(a), before[0], "a stayed put");
    approx(f.world_origin(b), before[1], "b stayed put");

    // The box is the union: (100,100)..(230,160) in the artboard's space.
    let NodeKind::Artboard { size } = f.doc.get(fr).unwrap().kind() else {
        panic!("framing made something that is not a frame");
    };
    assert_eq!(*size, Size::new(130.0, 60.0));
    // **Two accessors now, where this was one destructure** (§15 D400): the size is
    // the kind's and the fill is the node's paint, because a frame's ground stopped
    // being a field of `NodeKind::Artboard`. The rule it pins is unchanged and is
    // the whole reason `build::frame` emits no `SetFills`.
    assert!(
        f.doc.get(fr).unwrap().paint().fills.is_empty(),
        "framing must not paint anything over what was behind the selection"
    );
    // And it sits at the union's corner, in world space through the artboard's own
    // translation — the check that the corner was measured in the right space.
    approx(
        f.world_origin(fr),
        Point::new(150.0, 130.0),
        "the frame's corner",
    );
}

/// §5.3: an `Artboard` hangs off the root or another `Artboard`. So a selection
/// **inside a group** cannot be framed where it stands, and the refusal is the
/// answer rather than quietly lifting the artwork out of its group to make room.
///
/// The mirror of it is the difference from `build::group` worth having a test
/// for: **a frame may be a member**, where a group refuses one outright.
#[test]
fn framing_answers_the_placement_rule_in_both_directions() {
    let mut f = Fixture::new();
    let (tx, g) = {
        let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
        build::group(&f.doc, &mut f.ids, &[a]).unwrap()
    };
    f.commit(tx);
    let inside = f.children_of(g);
    assert_eq!(inside.len(), 1, "the fixture put a layer inside a group");
    assert!(matches!(
        build::frame(&f.doc, &f.resolved(), &mut f.ids, &inside),
        Err(OpError::ArtboardPlacement)
    ));

    // A frame inside the artboard frames fine — frames nest, so this is the case
    // `group` has no honest answer for and refuses.
    let nested = f.add(f.artboard, artboard(), (10.0, 10.0));
    assert!(matches!(
        build::group(&f.doc, &mut f.ids, &[nested]),
        Err(OpError::WrongKindForOp)
    ));
    let (tx, fr) = build::frame(&f.doc, &f.resolved(), &mut f.ids, &[nested]).unwrap();
    f.commit(tx);
    assert_eq!(f.doc.get(nested).unwrap().parent(), Some(fr));
}

/// The frame lands in the **topmost member's** slot, so framing does not restack
/// the artwork relative to what it was in front of — `group`'s rule, and it has to
/// be the same one or the two verbs would reorder the canvas differently.
#[test]
fn a_frame_takes_the_z_position_of_its_topmost_member() {
    let mut f = Fixture::new();
    let behind = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let a = f.add(f.artboard, rect(10.0, 10.0), (20.0, 0.0));
    let front = f.add(f.artboard, rect(10.0, 10.0), (40.0, 0.0));

    let (tx, fr) = build::frame(&f.doc, &f.resolved(), &mut f.ids, &[a]).unwrap();
    f.commit(tx);
    assert_eq!(
        f.children_of(f.artboard),
        vec![behind, fr, front],
        "the frame took the slot its member had"
    );
}

#[test]
fn ungroup_splices_children_back_in_place() {
    let mut f = Fixture::new();
    let behind = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let a = f.add(f.artboard, rect(10.0, 10.0), (100.0, 100.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (200.0, 150.0));
    let before = [f.world_origin(a), f.world_origin(b)];

    let (tx, g) = build::group(&f.doc, &mut f.ids, &[a, b]).unwrap();
    f.commit(tx);
    // Offset the group itself, then ungroup: the offset must be folded into
    // the children rather than lost.
    f.commit(Transaction(vec![Operation::SetTransform {
        id: g,
        transform: Affine::translate((7.0, -3.0)),
    }]));
    let after_offset = [f.world_origin(a), f.world_origin(b)];
    assert!((after_offset[0].x - before[0].x - 7.0).abs() < 1e-9);

    let tx = build::ungroup(&f.doc, g).unwrap();
    f.commit(tx);

    assert!(!f.doc.contains(g), "group node is gone");
    assert_eq!(f.children_of(f.artboard), vec![behind, a, b]);
    approx(f.world_origin(a), after_offset[0], "a kept world position");
    approx(f.world_origin(b), after_offset[1], "b kept world position");
}

#[test]
fn group_then_ungroup_round_trips_world_positions() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (12.0, 34.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (56.0, 78.0));
    let before = [f.world_origin(a), f.world_origin(b)];

    let (tx, g) = build::group(&f.doc, &mut f.ids, &[a, b]).unwrap();
    f.commit(tx);
    let tx = build::ungroup(&f.doc, g).unwrap();
    f.commit(tx);

    approx(f.world_origin(a), before[0], "a round-tripped");
    approx(f.world_origin(b), before[1], "b round-tripped");
    assert_eq!(f.children_of(f.artboard), vec![a, b]);
}

#[test]
fn ungroup_rejects_non_groups() {
    let mut f = Fixture::new();
    let r = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    assert!(matches!(
        build::ungroup(&f.doc, r),
        Err(OpError::WrongKindForOp)
    ));
}

/// **Two containers dissolved in one transaction land their children in the
/// right order, and in the same order whichever way round they are named**
/// (§15 D521).
///
/// This is the assertion that says why `ungroup_all` simulates rather than
/// concatenating. `ungroup` reads a container's position among its parent's
/// children, so the second container's index is measured against a child list
/// the first one's ops have already spliced — two extra children where there
/// used to be one.
///
/// ⚠️ Flipped by replacing the simulation with a plain concatenation against
/// the original document. Red **here**, at the first `children_of`, on the
/// first of the two orderings — and the wrong answer is
/// `[behind, a, c, d, b]`, not the interleave that was predicted: `g2`'s
/// children land at the stale indices 2 and 3, which are *inside* the run `g1`
/// has already spliced, and `b` is pushed off the end. Green on every
/// world-position assertion below it, because the wrong indices still name the
/// right parent — **a test asserting positions alone would have passed.**
#[test]
fn two_containers_ungroup_in_one_transaction_without_interleaving() {
    for reversed in [false, true] {
        let mut f = Fixture::new();
        let behind = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
        let a = f.add(f.artboard, rect(10.0, 10.0), (100.0, 100.0));
        let b = f.add(f.artboard, rect(10.0, 10.0), (140.0, 100.0));
        let c = f.add(f.artboard, rect(10.0, 10.0), (200.0, 150.0));
        let d = f.add(f.artboard, rect(10.0, 10.0), (240.0, 150.0));
        let before = [
            f.world_origin(a),
            f.world_origin(b),
            f.world_origin(c),
            f.world_origin(d),
        ];

        let (tx, g1) = build::group(&f.doc, &mut f.ids, &[a, b]).unwrap();
        f.commit(tx);
        let (tx, g2) = build::group(&f.doc, &mut f.ids, &[c, d]).unwrap();
        f.commit(tx);
        assert_eq!(f.children_of(f.artboard), vec![behind, g1, g2]);

        let named = if reversed { [g2, g1] } else { [g1, g2] };
        let tx = build::ungroup_all(&f.doc, &named).expect("two live groups");
        f.commit(tx);

        assert_eq!(
            f.children_of(f.artboard),
            vec![behind, a, b, c, d],
            "named {named:?}: each group's children take its own slot, in order"
        );
        assert!(!f.doc.contains(g1) && !f.doc.contains(g2), "both are gone");
        for (i, (id, was)) in [a, b, c, d].into_iter().zip(before).enumerate() {
            approx(f.world_origin(id), was, &format!("child {i} did not move"));
        }
    }
}

/// **A container that cannot be dissolved takes the whole command down with
/// it, rather than half of it** (§15 D521).
///
/// The loop this replaced committed as it went, so a second container that
/// refused left the first already dissolved *and* reported *"Cannot
/// ungroup: …"* — a command that half-applied and said it failed. A builder
/// that returns `Err` has built nothing, so there is nothing to half-apply.
///
/// ⚠️ Flipped by having `ungroup_all` skip the failures instead of returning:
/// red here, and it is the only assertion that can see it — the order test
/// above never reaches a bad id.
#[test]
fn one_bad_container_builds_no_ops_at_all() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (40.0, 0.0));
    let r = f.add(f.artboard, rect(10.0, 10.0), (80.0, 0.0));
    let (tx, g) = build::group(&f.doc, &mut f.ids, &[a, b]).unwrap();
    f.commit(tx);

    assert!(matches!(
        build::ungroup_all(&f.doc, &[g, r]),
        Err(OpError::WrongKindForOp)
    ));
    assert!(
        f.doc.contains(g),
        "and the builder is pure, so the good one is untouched"
    );
}

#[test]
fn reparent_preserving_world_does_not_move_the_node() {
    let mut f = Fixture::new();
    // A group with its own offset, so plain Reparent would visibly teleport.
    let g = f.add(f.artboard, NodeKind::Group, (300.0, 200.0));
    let r = f.add(f.artboard, rect(10.0, 10.0), (100.0, 100.0));
    let before = f.world_origin(r);

    let res = f.resolved();
    let tx = build::reparent_preserving_world(&f.doc, &res, r, g, 0).unwrap();
    f.commit(tx);

    assert_eq!(f.doc.get(r).unwrap().parent(), Some(g));
    approx(f.world_origin(r), before, "reparent kept world position");
}

#[test]
fn plain_reparent_would_have_moved_it() {
    // Guards the reason `reparent_preserving_world` exists: the primitive op
    // keeps the local transform, so the node jumps by the parent delta.
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group, (300.0, 200.0));
    let r = f.add(f.artboard, rect(10.0, 10.0), (100.0, 100.0));
    let before = f.world_origin(r);

    f.commit(Transaction(vec![Operation::Reparent {
        id: r,
        new_parent: g,
        index: 0,
    }]));
    let after = f.world_origin(r);
    assert!(
        (after.x - before.x - 300.0).abs() < 1e-9,
        "expected the naive op to shift by the group offset"
    );
}

#[test]
fn place_at_world_sets_the_exact_world_transform() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group, (300.0, 200.0));
    let r = f.add(g, rect(10.0, 10.0), (5.0, 5.0));

    let target = Affine::translate((-40.0, 90.0));
    let res = f.resolved();
    let tx = build::place_at_world(&f.doc, &res, r, target).unwrap();
    f.commit(tx);

    let got = f.resolved().world_transform(r).unwrap();
    for (a, b) in got.as_coeffs().iter().zip(target.as_coeffs().iter()) {
        assert!((a - b).abs() < 1e-9, "{got:?} vs {target:?}");
    }
}

#[test]
fn move_by_world_does_not_move_a_node_twice() {
    // The multi-select trap: a group and one of its children are both selected.
    // Moving both would translate the child by 2x delta.
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group, (10.0, 10.0));
    let child = f.add(g, rect(10.0, 10.0), (5.0, 5.0));
    let before = f.world_origin(child);

    let delta = Vec2::new(25.0, -15.0);
    let res = f.resolved();
    let tx = build::move_by_world(&f.doc, &res, &[g, child], delta).unwrap();
    // Only the outermost node is touched.
    assert_eq!(tx.0.len(), 1, "expected one op, got {:?}", tx.0);
    f.commit(tx);

    approx(
        f.world_origin(child),
        before + delta,
        "child moved exactly once",
    );
}

#[test]
fn move_by_world_translates_independent_nodes_under_rotated_parents() {
    let mut f = Fixture::new();
    let spun = f.ids.mint();
    let index = f.doc.get(f.artboard).unwrap().children().len();
    f.commit(Transaction(vec![Operation::CreateNode {
        id: spun,
        parent: f.artboard,
        index,
        kind: NodeKind::Group,
        transform: Some(Affine::rotate(0.7)),
        name: None,
    }]));
    let a = f.add(spun, rect(10.0, 10.0), (10.0, 20.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (60.0, 70.0));
    let before = [f.world_origin(a), f.world_origin(b)];

    let delta = Vec2::new(13.0, 4.0);
    let res = f.resolved();
    let tx = build::move_by_world(&f.doc, &res, &[a, b], delta).unwrap();
    f.commit(tx);

    approx(f.world_origin(a), before[0] + delta, "rotated parent");
    approx(f.world_origin(b), before[1] + delta, "plain parent");
}

#[test]
fn outermost_deduplicates_and_drops_covered_descendants() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group, (0.0, 0.0));
    let inner = f.add(g, NodeKind::Group, (0.0, 0.0));
    let leaf = f.add(inner, rect(1.0, 1.0), (0.0, 0.0));
    let loose = f.add(f.artboard, rect(1.0, 1.0), (0.0, 0.0));

    // A deeply nested descendant is dropped even when the ancestor is not its
    // direct parent, and duplicates collapse.
    assert_eq!(
        build::outermost(&f.doc, &[g, leaf, loose, g, inner]),
        vec![g, loose]
    );
}

/// **An export's subject comes out bottom-of-the-stack first however it was
/// picked**, which is the property `svg_of` and `png_of` both require of a caller
/// and neither can check.
///
/// The bug this pins is one click deep: `Selection::ids` is **pick** order —
/// shift-clicking the front shape and then the one behind it puts the front one
/// first — and `outermost` keeps its input order, so the writer was handed a
/// reversed stack and the file came back with the artwork restacked. SVG has no
/// z-index, so the last element emitted is the one on top; there is nothing
/// downstream that could have corrected it.
///
/// **The fixture is asserted first.** `outermost` on the reversed pick has to
/// actually *be* reversed, or this test is comparing document order against
/// document order and would pass against no implementation at all.
///
/// ⚠️ Flipped against `outermost` alone (the shipped behaviour, which fails the
/// second assertion) and against a version that descends into a matched member
/// (which passes here — nothing is nested — and is why the third case exists).
#[test]
fn an_export_subject_comes_out_in_document_order_however_it_was_picked() {
    let mut f = Fixture::new();
    let bottom = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let middle = f.add(f.artboard, rect(10.0, 10.0), (5.0, 5.0));
    let top = f.add(f.artboard, rect(10.0, 10.0), (10.0, 10.0));

    // The fixture: picking back-to-front really does hand over a reversed list,
    // so the two functions below are being asked different questions.
    assert_eq!(
        build::outermost(&f.doc, &[top, middle, bottom]),
        vec![top, middle, bottom],
        "outermost keeps input order — if this ever changes, this test is moot"
    );
    assert_eq!(
        build::in_document_order(&f.doc, &[top, middle, bottom]),
        vec![bottom, middle, top],
        "an export subject is ordered by the document, not by the clicking"
    );
    // Already in order, and picked with a duplicate: unchanged and deduplicated.
    assert_eq!(
        build::in_document_order(&f.doc, &[bottom, top, bottom]),
        vec![bottom, top]
    );

    // A member inside a group is reached, and a descendant carried alongside its
    // own ancestor is dropped rather than emitted twice — `outermost`'s job, kept.
    let g = f.add(f.artboard, NodeKind::Group, (0.0, 0.0));
    let inside = f.add(g, rect(1.0, 1.0), (0.0, 0.0));
    assert_eq!(
        build::in_document_order(&f.doc, &[g, inside, bottom]),
        vec![bottom, g],
        "the group is one member and comes after the rect it sits above"
    );
    assert_eq!(
        build::in_document_order(&f.doc, &[inside, bottom]),
        vec![bottom, inside],
        "a member deeper than the top level is still reached"
    );
}

#[test]
fn reserve_existing_ids_prevents_collisions_after_a_load() {
    // The open-a-file bug: a session whose counter is behind the document's
    // ids re-mints one that already exists, and every create fails.
    let mut f = Fixture::new();
    for i in 0..5 {
        f.add(f.artboard, rect(1.0, 1.0), (i as f64, 0.0));
    }
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let loaded = ondin_core::io::load(&bytes).unwrap();

    // A fresh session that happens to reuse the actor starts at seq 1.
    let mut stale = IdSource::new(0xB0A7);
    let collides = stale.mint();
    assert!(
        loaded.contains(collides),
        "fixture must actually collide for this test to mean anything"
    );

    let mut fixed = IdSource::new(0xB0A7);
    ondin_core::reserve_existing_ids(&loaded, &mut fixed);
    for _ in 0..10 {
        assert!(
            !loaded.contains(fixed.mint()),
            "reserved source must never mint an existing id"
        );
    }
}

#[test]
fn reserve_existing_ids_never_rewinds_the_counter() {
    let f = Fixture::new();
    let mut ids = IdSource::new(0xB0A7);
    for _ in 0..50 {
        ids.mint();
    }
    let before = ids.next_seq();
    ondin_core::reserve_existing_ids(&f.doc, &mut ids);
    assert_eq!(ids.next_seq(), before, "must only ever move forward");
}

// --- insert_subtrees: paste and duplicate ---------------------------------

#[test]
fn insert_subtrees_places_several_copies_in_one_transaction() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (50.0, 0.0));
    let templates: Vec<Vec<ondin_core::Node>> = [a, b]
        .iter()
        .map(|id| f.doc.capture_subtree(*id).unwrap())
        .collect();

    let placements: Vec<build::Placement> = templates
        .into_iter()
        .map(|nodes| build::Placement {
            nodes,
            parent: f.artboard,
            index: None,
        })
        .collect();
    let offset = Vec2::new(10.0, 10.0);
    let (tx, created) = build::insert_subtrees(&f.doc, &mut f.ids, &placements, offset);

    assert_eq!(created.len(), 2, "both copies were placed");
    let before = f.hist.undo_depth();
    f.commit(tx);
    assert_eq!(
        f.hist.undo_depth(),
        before + 1,
        "pasting several layers is ONE undo step"
    );

    // Both copies exist, appended in order after the originals.
    assert_eq!(
        f.children_of(f.artboard),
        vec![a, b, created[0], created[1]]
    );
    // And each landed at its original position plus the offset.
    approx(
        f.world_origin(created[0]),
        f.world_origin(a) + offset,
        "first copy",
    );
    approx(
        f.world_origin(created[1]),
        f.world_origin(b) + offset,
        "second copy",
    );
}

#[test]
fn insert_subtrees_mints_fresh_ids_for_every_copy() {
    let mut f = Fixture::new();
    let g = f.add(f.artboard, NodeKind::Group, (0.0, 0.0));
    f.add(g, rect(5.0, 5.0), (0.0, 0.0));
    let template = f.doc.capture_subtree(g).unwrap();

    // The same template placed twice must not collide with itself.
    let placements = vec![
        build::Placement {
            nodes: template.clone(),
            parent: f.artboard,
            index: None,
        },
        build::Placement {
            nodes: template,
            parent: f.artboard,
            index: None,
        },
    ];
    let (tx, created) = build::insert_subtrees(&f.doc, &mut f.ids, &placements, Vec2::ZERO);
    f.commit(tx);

    assert_eq!(created.len(), 2);
    assert_ne!(created[0], created[1]);
    // Each copy brought its own child, also freshly minted.
    let kids: Vec<NodeId> = created
        .iter()
        .flat_map(|id| f.doc.get(*id).unwrap().children().to_vec())
        .collect();
    assert_eq!(kids.len(), 2);
    assert_ne!(kids[0], kids[1]);
}

#[test]
fn insert_subtrees_honours_explicit_indices() {
    // Duplicate semantics: each copy sits immediately above its original.
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let template_a = f.doc.capture_subtree(a).unwrap();

    let (tx, created) = build::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[build::Placement {
            nodes: template_a,
            parent: f.artboard,
            index: Some(1),
        }],
        Vec2::ZERO,
    );
    f.commit(tx);
    assert_eq!(f.children_of(f.artboard), vec![a, created[0], b]);
}

#[test]
fn insert_subtrees_of_nothing_is_a_no_op() {
    let mut f = Fixture::new();
    let (tx, created) = build::insert_subtrees(&f.doc, &mut f.ids, &[], Vec2::ZERO);
    assert!(created.is_empty());
    assert!(tx.0.is_empty(), "must not create an empty undo step");
}

// --- the world-space inspector projection ---------------------------------

#[test]
fn setting_a_world_position_lands_exactly_there_under_a_rotated_parent() {
    // What the inspector's X/Y fields do. The parent is rotated, so anything
    // that treats the typed number as a local translation lands elsewhere.
    let mut f = Fixture::new();
    let spun = f.ids.mint();
    let index = f.doc.get(f.artboard).unwrap().children().len();
    f.commit(Transaction(vec![Operation::CreateNode {
        id: spun,
        parent: f.artboard,
        index,
        kind: NodeKind::Group,
        transform: Some(Affine::rotate(0.9)),
        name: None,
    }]));
    let r = f.add(spun, rect(10.0, 10.0), (11.0, 22.0));

    // Keep the existing rotation, move to a chosen world point — the inspector
    // strips translation from the world basis and re-applies it.
    let world = f.resolved().world_transform(r).unwrap().as_coeffs();
    let basis = Affine::new([world[0], world[1], world[2], world[3], 0.0, 0.0]);
    let target = Point::new(-40.0, 90.0);
    let tx = build::place_at_world(
        &f.doc,
        &f.resolved(),
        r,
        Affine::translate(target.to_vec2()) * basis,
    )
    .unwrap();
    f.commit(tx);

    approx(f.world_origin(r), target, "landed at the typed coordinates");
    // And the rotation survived.
    let after = f.resolved().world_transform(r).unwrap().as_coeffs();
    for i in 0..4 {
        assert!(
            (after[i] - world[i]).abs() < 1e-9,
            "rotation changed: {after:?} vs {world:?}"
        );
    }
}

// --- arrangement ----------------------------------------------------------

/// Bring-forward on a middle layer trades places with exactly one neighbour.
///
/// 🚨 **The `Backward` half was here and could not fail** (§15 D645,
/// `[S3.2-L6-06]`). It ran on `a` sitting at index 1, and index 1 is the one
/// place `Backward`'s loop direction is unobservable: the arm is
/// `for i in 1..out.len() { if sel.contains(out[i]) && !sel.contains(out[i-1])
/// { swap } }`, and reversing it to `.rev()` makes a swapped member match again
/// on the next iteration and walk all the way to 0 — *Send backward* becomes
/// *Send to back*. From index 1 the two spellings agree.
///
/// The other `Backward` site in the tree, in
/// `z_order_at_the_end_of_the_stack_is_not_an_edit`, runs at index 0 and asserts
/// an **empty** transaction, so it never enters the loop at all. Those two were
/// the complete set: the arm was reachable in one direction only.
///
/// ⚠️ **`Forward`'s mirror *is* covered**, which is what makes the pair
/// asymmetric and is worth saying rather than leaving as a symmetry argument:
/// the `[a,b,c]` / `{a}` / `Forward` case above fails with `[b,c,a]` under the
/// `.rev()` its arm carries. **The spelling that carries a comment is the one
/// that was checked.**
///
/// Flips, both run against `[a,b,c]` with `{c}` selected:
/// - `Backward`'s loop reversed to `(1..out.len()).rev()`: red at the new
///   assertion with `[c, a, b]` — the member walked to the front of the stack.
/// - `Forward`'s `.rev()` dropped: red at the *first* assertion, as before.
#[test]
fn z_order_one_step_swaps_with_the_nearest_unselected_neighbour() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let c = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    assert_eq!(f.children_of(f.artboard), vec![a, b, c]);

    f.commit(build::z_order(&f.doc, &[a], build::ZMove::Forward));
    assert_eq!(f.children_of(f.artboard), vec![b, a, c]);

    f.commit(build::z_order(&f.doc, &[a], build::ZMove::Backward));
    assert_eq!(f.children_of(f.artboard), vec![a, b, c]);

    // `c` is at index 2, which is the only place a one-step move backward can
    // be told from a move to the back.
    f.commit(build::z_order(&f.doc, &[c], build::ZMove::Backward));
    assert_eq!(
        f.children_of(f.artboard),
        vec![a, c, b],
        "one step, not all the way to the back"
    );
}

/// **A non-contiguous multi-selection moves as two runs, not as one block**, and
/// nothing reached this arm of `restacked` before (§15 D645, `[S3.2-L6-06]`).
///
/// `z_order_preserves_relative_order_within_the_selection` covers an *adjacent*
/// pair, where the whole selection is one run and the question does not arise.
/// With `{a, c}` in `[a, b, c, d]` each selected member trades with its own
/// nearest unselected neighbour, independently — so `a` passes `b` and `c`
/// passes `d`, and the two selected layers end up further apart than they
/// started rather than being gathered.
///
/// ⚠️ **That is the behaviour, not merely what the code does**, and it is
/// recorded here because nothing else states it: `design-oracle` reports that
/// neither `architecture.md` nor §15 says which neighbour a one-step move trades
/// with, or whether a run of adjacent members moves as a unit. The comment in
/// `restacked` was the only record, and it was checked on one of two arms.
///
/// 🚨 **Two flips, and neither bites here — which is the finding.** The
/// `Forward` arm's `.rev()` dropped, and the `!sel.contains(&out[i + 1])` clause
/// dropped: both leave this test **green**, and the prediction written before
/// running them said the first would fail with `[b, d, a, c]`. It does not.
/// With `{a, c}` in `[a, b, c, d]` neither selected member is ever adjacent to
/// the other in the direction of travel, so the loop order and the neighbour
/// clause are both unobservable — every spelling gives `[b, a, d, c]`.
///
/// ⚠️ **So this test covers the *shape* the finding named and pins no mutation
/// of its own**, and saying so is worth more than the assertion. What the
/// clause-dropping flip did find is
/// `z_order_at_the_end_of_the_stack_is_not_an_edit`'s new last assertion — a
/// whole selection already at the top — which was the one case in the workspace
/// that could tell the two spellings apart, and had no test.
#[test]
fn z_order_moves_each_run_of_a_split_selection_on_its_own() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let c = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let d = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    assert_eq!(f.children_of(f.artboard), vec![a, b, c, d]);

    f.commit(build::z_order(&f.doc, &[a, c], build::ZMove::Forward));
    assert_eq!(f.children_of(f.artboard), vec![b, a, d, c]);
}

/// Already at the end of the stack, a step is a no-op — and specifically not a
/// transaction, so it does not spend an undo step doing nothing.
#[test]
fn z_order_at_the_end_of_the_stack_is_not_an_edit() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));

    assert!(
        build::z_order(&f.doc, &[b], build::ZMove::Forward)
            .0
            .is_empty()
    );
    assert!(
        build::z_order(&f.doc, &[a], build::ZMove::Backward)
            .0
            .is_empty()
    );
    assert!(
        build::z_order(&f.doc, &[b], build::ZMove::Front)
            .0
            .is_empty()
    );
    // 🚨 **The whole selection already at the top, which is what the
    // `!sel.contains(&out[i + 1])` clause is for** (§15 D645, `[S3.2-L6-06]`).
    // Every other case in this file leaves that clause **redundant**: the loop
    // runs `.rev()`, so by the time index `i` is reached a selected `i + 1` has
    // already moved up and the neighbour under test is unselected either way.
    // The one place the two spellings disagree is here, where there is nothing
    // above to move into — without the clause `a` swaps with `b` anyway, which
    // both reverses the selection's internal order and spends an undo step
    // doing it.
    //
    // ⚠️ **This was found by a flip that did *not* bite.** Dropping the clause
    // left all six `z_order` tests green, including the two written this session
    // for the neighbouring gap; the arm it guards was reachable by no assertion
    // in the workspace.
    assert!(
        build::z_order(&f.doc, &[a, b], build::ZMove::Forward)
            .0
            .is_empty(),
        "a selection already at the top has nowhere to go"
    );
    assert_eq!(f.children_of(f.artboard), vec![a, b]);
}

/// A multi-selection keeps its own relative order however far it moves — the
/// property a naive per-node loop breaks, because each move invalidates the
/// next one's index.
#[test]
fn z_order_preserves_relative_order_within_the_selection() {
    let mut f = Fixture::new();
    let ids: Vec<NodeId> = (0..5)
        .map(|_| f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0)))
        .collect();
    let (a, b, c, d, e) = (ids[0], ids[1], ids[2], ids[3], ids[4]);

    f.commit(build::z_order(&f.doc, &[a, c], build::ZMove::Front));
    assert_eq!(f.children_of(f.artboard), vec![b, d, e, a, c]);

    f.commit(build::z_order(&f.doc, &[a, c], build::ZMove::Back));
    assert_eq!(f.children_of(f.artboard), vec![a, c, b, d, e]);

    // `a` and `c` are now adjacent, so they are one run: a step moves the pair
    // past `b` together rather than shuffling them against each other.
    f.commit(build::z_order(&f.doc, &[a, c], build::ZMove::Forward));
    assert_eq!(f.children_of(f.artboard), vec![b, a, c, d, e]);
}

/// "To the front of wherever you are" — a selection spanning two parents
/// restacks in each, and nothing changes parent.
#[test]
fn z_order_restacks_each_parent_independently() {
    let mut f = Fixture::new();
    let second = f.add(f.root, artboard(), (900.0, 0.0));
    let a1 = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let a2 = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b1 = f.add(second, rect(10.0, 10.0), (0.0, 0.0));
    let b2 = f.add(second, rect(10.0, 10.0), (0.0, 0.0));

    f.commit(build::z_order(&f.doc, &[a1, b1], build::ZMove::Front));
    assert_eq!(f.children_of(f.artboard), vec![a2, a1]);
    assert_eq!(f.children_of(second), vec![b2, b1]);
    assert_eq!(f.doc.get(a1).unwrap().parent(), Some(f.artboard));
}

/// A selected group swallows its selected child: reordering the child inside
/// the group is not what the user asked for by selecting both.
#[test]
fn z_order_ignores_members_inside_other_members() {
    let mut f = Fixture::new();
    let outer = f.add(f.artboard, NodeKind::Group, (0.0, 0.0));
    let inner = f.add(outer, rect(10.0, 10.0), (0.0, 0.0));
    let sibling = f.add(outer, rect(10.0, 10.0), (0.0, 0.0));
    let other = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));

    f.commit(build::z_order(&f.doc, &[outer, inner], build::ZMove::Front));
    assert_eq!(f.children_of(f.artboard), vec![other, outer]);
    assert_eq!(
        f.children_of(outer),
        vec![inner, sibling],
        "the group's own children are left alone"
    );
}

/// Align works on world bounds and writes local transforms — under a
/// translated artboard those differ, which is the whole point of routing
/// through `local_for_world`.
///
/// **All six `(axis, edge)` cells, not one** (`[S3.2-L6-02]`, §15 D499). This
/// test asserted `(X, Min)` alone, and a census of every test and production
/// caller of an arrangement builder in the workspace found **every one of them
/// `Axis::X`, and every `align` call `Edge::Min`** — so five of the six arms of
/// `align`'s coordinate table were executed by nothing, while `input.rs` binds
/// `(Y, Max)` to a key and the inspector's align grid paints all four Y cells.
/// A six-arm table written by symmetry is exactly the shape where a copy-paste
/// slip is invisible in review and obvious on screen: copying `(X, Max)`'s
/// expression into `(Y, Max)` makes *Align bottom* align **right**, with the
/// whole suite green.
///
/// The Y arms were read line by line and are correct; this is coverage, not a
/// defect.
///
/// ⚠️ **The fixture has to differ on both axes**, or a Y align is a no-op on a
/// horizontal row and passes vacuously. `b` sits 190 to the right of `a` and 60
/// below it, and each case asserts the **cross** axis is untouched as well as
/// the aligned one — which is what catches an arm that writes the wrong
/// component of the `Vec2`.
///
/// **Flip run**, `(Axis::Y, Edge::Max)` given `(Axis::X, Edge::Max)`'s
/// expression: `Y/Max: layer 0 sits at 50 and the target edge is 150`. **The
/// predicted site was the cross-axis assertion and it was wrong** — the edge
/// assertion is checked first per layer, and with the delta on the wrong
/// component `max_y` never reaches the target at all. Both assertions would have
/// fired; the edge one gets there first. Recorded because a prediction that names
/// the *later* of two assertions is not evidence about which one is
/// load-bearing, and only running it says so.
#[test]
fn align_moves_world_bounds_to_the_target_edge() {
    use build::{Axis, Edge};

    // Read from a box the way the arm under test reads it, so the expectation is
    // stated once per edge rather than once per cell.
    let at = |r: &kurbo::Rect, axis: Axis, edge: Edge| match (axis, edge) {
        (Axis::X, Edge::Min) => r.min_x(),
        (Axis::X, Edge::Mid) => r.center().x,
        (Axis::X, Edge::Max) => r.max_x(),
        (Axis::Y, Edge::Min) => r.min_y(),
        (Axis::Y, Edge::Mid) => r.center().y,
        (Axis::Y, Edge::Max) => r.max_y(),
    };
    let across = |r: &kurbo::Rect, axis: Axis| match axis {
        Axis::X => r.min_y(),
        Axis::Y => r.min_x(),
    };

    for axis in [Axis::X, Axis::Y] {
        for edge in [Edge::Min, Edge::Mid, Edge::Max] {
            let mut f = Fixture::new();
            // Different in **both** axes and different in size, so no cell is a
            // no-op and `Mid` is not `Min` by coincidence.
            let a = f.add(f.artboard, rect(100.0, 20.0), (10.0, 0.0));
            let b = f.add(f.artboard, rect(40.0, 60.0), (200.0, 60.0));
            let res = f.resolved();
            let target = res
                .world_bounds(a)
                .unwrap()
                .union(res.world_bounds(b).unwrap());
            let before = [
                across(&res.world_bounds(a).unwrap(), axis),
                across(&res.world_bounds(b).unwrap(), axis),
            ];

            f.commit(build::align(&f.doc, &res, &[a, b], target, axis, edge));
            let res = f.resolved();
            let want = at(&target, axis, edge);
            for (i, id) in [a, b].into_iter().enumerate() {
                let got = res.world_bounds(id).unwrap();
                assert!(
                    (at(&got, axis, edge) - want).abs() < 1e-9,
                    "{axis:?}/{edge:?}: layer {i} sits at {} and the target edge is {want}",
                    at(&got, axis, edge)
                );
                assert!(
                    (across(&got, axis) - before[i]).abs() < 1e-9,
                    "{axis:?}/{edge:?}: layer {i} moved across the axis, {} to {}",
                    before[i],
                    across(&got, axis)
                );
            }
        }
    }
}

/// Already-aligned layers produce no operations at all.
#[test]
fn align_that_changes_nothing_is_an_empty_transaction() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (0.0, 40.0));
    let res = f.resolved();
    let target = res
        .world_bounds(a)
        .unwrap()
        .union(res.world_bounds(b).unwrap());
    let tx = build::align(
        &f.doc,
        &res,
        &[a, b],
        target,
        build::Axis::X,
        build::Edge::Min,
    );
    assert!(tx.0.is_empty());
}

/// Equal gaps, not equal centres: the outer two stay put and differently-sized
/// layers still end up with identical space between them.
///
/// **Both axes** (`[S3.2-L6-02]`, §15 D499). Every arrangement call in the whole
/// workspace — production and test — passed `Axis::X`, so `lead(_, Axis::Y)`,
/// `extent(_, Axis::Y)` and `along(Axis::Y, _)` were executed by nothing, while
/// `input.rs` binds vertical distribute to a key. Rewriting `along`'s
/// `Axis::Y => Vec2::new(0.0, d)` as `Vec2::new(d, 0.0)` — *Distribute vertical*
/// moving layers **sideways** — left the whole suite green.
///
/// ⚠️ **The Y fixture is the X one turned, not a second one written by hand.**
/// The three layers differ in the arranged extent and are identical across it, so
/// the numbers below are the same numbers on either axis and a Y answer cannot be
/// right by inheriting an X one.
///
/// **Flip run**, `along(Axis::Y, d) = Vec2::new(d, 0.0)`: fails on the Y pass's
/// *"the middle layer did not move across the axis, 50 to 100"* — the predicted
/// site, and right this time.
///
/// ⚠️ **The gap assertion behind it was then disabled and the flip re-run**,
/// because a predicted site that fires first says nothing about whether the rest
/// of the test has teeth. It does: with the across-assertion out, the flip fails
/// at *"Y: 20 vs 120"*. The middle layer never moves along Y, so the gaps stay as
/// the fixture left them, and the fixture's own gaps are **unequal** — which is
/// what makes them an assertion rather than a coincidence.
#[test]
fn distribute_equalises_the_gaps_and_leaves_the_ends_alone() {
    for axis in [build::Axis::X, build::Axis::Y] {
        let horizontal = axis == build::Axis::X;
        // Extents 10 / 40 / 10 across a 200 span: 140 of gap over two gaps.
        let place = |lead: f64| {
            if horizontal { (lead, 0.0) } else { (0.0, lead) }
        };
        let size = |along: f64| {
            if horizontal {
                rect(along, 10.0)
            } else {
                rect(10.0, along)
            }
        };
        let lead = |r: &kurbo::Rect| if horizontal { r.min_x() } else { r.min_y() };
        let trail = |r: &kurbo::Rect| if horizontal { r.max_x() } else { r.max_y() };
        let across = |r: &kurbo::Rect| if horizontal { r.min_y() } else { r.min_x() };

        let mut f = Fixture::new();
        let a = f.add(f.artboard, size(10.0), place(0.0));
        let b = f.add(f.artboard, size(40.0), place(30.0));
        let c = f.add(f.artboard, size(10.0), place(190.0));
        let res = f.resolved();
        let (a0, b0, c0) = (
            res.world_bounds(a).unwrap(),
            res.world_bounds(b).unwrap(),
            res.world_bounds(c).unwrap(),
        );

        f.commit(build::distribute(&f.doc, &res, &[a, b, c], axis));
        let res = f.resolved();
        let (ra, rb, rc) = (
            res.world_bounds(a).unwrap(),
            res.world_bounds(b).unwrap(),
            res.world_bounds(c).unwrap(),
        );
        assert!(
            (lead(&ra) - lead(&a0)).abs() < 1e-9,
            "{axis:?}: first stays put"
        );
        assert!(
            (lead(&rc) - lead(&c0)).abs() < 1e-9,
            "{axis:?}: last stays put"
        );
        assert!(
            (across(&rb) - across(&b0)).abs() < 1e-9,
            "{axis:?}: the middle layer did not move across the axis, {} to {}",
            across(&b0),
            across(&rb)
        );
        let gap1 = lead(&rb) - trail(&ra);
        let gap2 = lead(&rc) - trail(&rb);
        assert!((gap1 - gap2).abs() < 1e-9, "{axis:?}: {gap1} vs {gap2}");
        assert!((gap1 - 70.0).abs() < 1e-9, "{axis:?}: gap was {gap1}");
    }
}

/// **Distributing overlapping layers does not reorder them** (§15 D487,
/// `[S3.2-L1-01]`).
///
/// A background rect 800 wide at x=0 with two 40-wide icons at 10 and 100 — the
/// most ordinary overlapping selection there is. Equal gaps put the gap at
/// `(140 − 880) / 2`, hugely negative, and the layout loop then advanced the
/// cursor by each box's own extent: the middle layer travelled **420 units** and
/// landed 330 to the **right** of the layer that had been in front of it. Leading
/// edges `0 / 10 / 100` → `0 / 430 / 100`. One `SetTransform`, one undo step, no
/// message.
///
/// **The assertion is the post-condition, not the coordinates**, and that is the
/// point: the gaps *did* come out equal, so a gap-equality assertion cannot see
/// this at all, and the existing test above is exactly such an assertion on a set
/// that happens not to overlap. What has to hold is `sorted_along`'s own stated
/// rule — *"arranging a set never also reorders it: whatever is leftmost stays
/// leftmost"*.
///
/// ⚠️ **The equal-extent case is asserted beside it as a fixed point.** Three
/// 100-wide rects at 0/10/20 overlap heavily and are unmoved under *both*
/// spellings — which is why the old comment's claim that overlapping layers "tuck
/// back under each other evenly" read as true. It is true for equal extents and
/// for nothing else.
///
/// ⚠️ **Three flips, and the first two did not bite** — which is the finding this
/// test is worth more than its fix. Restoring the old `span` alone leaves it
/// green; restoring the gap-only loop alone leaves it green; **only both together
/// reproduce the defect**, at `[50, 60, 150]` → `[50, 480, 150]`, the middle layer
/// landing past the one that had been in front of it.
///
/// So the two halves are not one fix and a refinement — **either one alone closes
/// this fixture**, and the review's write-up, which proposed them as alternatives,
/// was right to. They are kept together for different reasons: the span is a plain
/// arithmetic error (the trailing edge of the box with the greatest *lead* is not
/// the greatest trailing edge), and the fallback is what makes the promise hold
/// for a set the corrected span still cannot fit.
///
/// Predicted site: the order assertion, and it is. The two ends stay where they
/// were and the gaps still come out equal under every flip, so nothing else in
/// this file notices any of them.
#[test]
fn distributing_overlapping_layers_does_not_reorder_them() {
    let leads = |res: &ondin_core::Resolved, ids: &[NodeId]| -> Vec<f64> {
        ids.iter()
            .map(|id| res.world_bounds(*id).unwrap().min_x())
            .collect()
    };

    let mut f = Fixture::new();
    let bg = f.add(f.artboard, rect(800.0, 200.0), (0.0, 0.0));
    let i1 = f.add(f.artboard, rect(40.0, 40.0), (10.0, 0.0));
    let i2 = f.add(f.artboard, rect(40.0, 40.0), (100.0, 0.0));
    let ids = [bg, i1, i2];
    let res = f.resolved();
    let before = leads(&res, &ids);
    assert!(
        before[0] < before[1] && before[1] < before[2],
        "the fixture has to start strictly ordered: {before:?}"
    );

    f.commit(build::distribute(&f.doc, &res, &ids, build::Axis::X));
    let res = f.resolved();
    let after = leads(&res, &ids);
    // ⚠️ **No strict *inversion*, asserted pairwise, rather than an equality of two
    // sorted id lists.** A sorted-list assertion was written first and a flip
    // proved it vacuous: with the span corrected but the gap loop still in place
    // this fixture comes out with two layers at the *same* lead, and a stable sort
    // of a tie reports the original order. Ties are a compromise; a swap is the
    // bug, and only the pairwise test can tell them apart.
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        assert!(
            before[i] >= before[j] || after[i] <= after[j],
            "layers {i} and {j} traded places: {before:?} became {after:?} — \
             distributing a set must not also reorder it"
        );
    }

    // Equal extents, heavy overlap: a fixed point, and it was one before too.
    let mut g = Fixture::new();
    let (p, q, r) = (
        g.add(g.artboard, rect(100.0, 10.0), (0.0, 0.0)),
        g.add(g.artboard, rect(100.0, 10.0), (10.0, 0.0)),
        g.add(g.artboard, rect(100.0, 10.0), (20.0, 0.0)),
    );
    let res = g.resolved();
    let leads: Vec<f64> = [p, q, r]
        .iter()
        .map(|id| res.world_bounds(*id).unwrap().min_x())
        .collect();
    g.commit(build::distribute(&g.doc, &res, &[p, q, r], build::Axis::X));
    let res = g.resolved();
    for (id, was) in [p, q, r].iter().zip(&leads) {
        let now = res.world_bounds(*id).unwrap().min_x();
        assert!((now - was).abs() < 1e-9, "{now} moved from {was}");
    }
}

/// Two layers have no interior to space out, so distribute declines rather
/// than inventing a movement.
#[test]
fn distribute_needs_three_layers() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (100.0, 0.0));
    let res = f.resolved();
    assert!(
        build::distribute(&f.doc, &res, &[a, b], build::Axis::X)
            .0
            .is_empty()
    );
}

/// Three layers at 0/30/190, widths 10/40/10 — the same set the even
/// distribution above uses, so the two verbs can be read against each other.
fn three_across() -> (Fixture, NodeId, NodeId, NodeId) {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(40.0, 10.0), (30.0, 0.0));
    let c = f.add(f.artboard, rect(10.0, 10.0), (190.0, 0.0));
    (f, a, b, c)
}

/// **A named gap is what the key layer is for**, which is the whole argument for
/// the key having been worth building past align: `distribute` is bounded by the
/// two extremes and has nothing for a designation to change, and this one leaves
/// the run free to slide so something has to hold it.
///
/// Asserted on the layer *before* the key as well as on the key itself, and that
/// is the discriminating half: anchoring on the first member — the fallback, and
/// the version somebody would write without thinking about a key at all —
/// produces exactly the same two gaps and puts every layer 4 units further on.
#[test]
fn a_named_gap_packs_outwards_from_the_key_and_leaves_the_key_alone() {
    let (mut f, a, b, c) = three_across();
    let res = f.resolved();
    let b0 = res.world_bounds(b).unwrap();
    f.commit(build::distribute_spacing(
        &f.doc,
        &res,
        &[a, b, c],
        build::Axis::X,
        Some(b),
        24.0,
    ));
    let res = f.resolved();
    let (ra, rb, rc) = (
        res.world_bounds(a).unwrap(),
        res.world_bounds(b).unwrap(),
        res.world_bounds(c).unwrap(),
    );
    assert!(
        (rb.min_x() - b0.min_x()).abs() < 1e-9,
        "the key moved, from {} to {}",
        b0.min_x(),
        rb.min_x()
    );
    assert!(
        (rb.min_x() - ra.max_x() - 24.0).abs() < 1e-9
            && (rc.min_x() - rb.max_x() - 24.0).abs() < 1e-9,
        "gaps came out {} and {}",
        rb.min_x() - ra.max_x(),
        rc.min_x() - rb.max_x()
    );
    assert!(
        (ra.min_x() - (b0.min_x() - 24.0 - 10.0)).abs() < 1e-9,
        "the layer before the key landed at {} rather than backwards from it",
        ra.min_x()
    );
}

/// With no key the first member on the axis holds, which is the one fallback
/// that cannot surprise: "exactly 24 apart" leaves the leftmost where it was.
#[test]
fn a_named_gap_with_no_key_holds_the_first_layer_on_the_axis() {
    let (mut f, a, b, c) = three_across();
    let res = f.resolved();
    let a0 = res.world_bounds(a).unwrap();
    f.commit(build::distribute_spacing(
        &f.doc,
        &res,
        &[a, b, c],
        build::Axis::X,
        None,
        24.0,
    ));
    let res = f.resolved();
    let (ra, rb) = (res.world_bounds(a).unwrap(), res.world_bounds(b).unwrap());
    assert!(
        (ra.min_x() - a0.min_x()).abs() < 1e-9,
        "the first one moved"
    );
    assert!((rb.min_x() - ra.max_x() - 24.0).abs() < 1e-9);
}

/// **Two layers is enough here and not enough for `distribute`**, and the pair
/// is asserted together because the difference is the point: two layers have no
/// interior to spread evenly, and they have exactly one gap, which is a number
/// worth being able to type.
#[test]
fn two_layers_have_a_gap_to_name_even_though_they_have_none_to_spread() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (100.0, 0.0));
    let res = f.resolved();
    assert!(
        build::distribute(&f.doc, &res, &[a, b], build::Axis::X)
            .0
            .is_empty(),
        "the even distribution should still decline at two"
    );
    let a0 = res.world_bounds(a).unwrap();
    f.commit(build::distribute_spacing(
        &f.doc,
        &res,
        &[a, b],
        build::Axis::X,
        None,
        5.0,
    ));
    let res = f.resolved();
    let (ra, rb) = (res.world_bounds(a).unwrap(), res.world_bounds(b).unwrap());
    assert!((ra.min_x() - a0.min_x()).abs() < 1e-9);
    assert!(
        (rb.min_x() - ra.max_x() - 5.0).abs() < 1e-9,
        "the gap came out {}",
        rb.min_x() - ra.max_x()
    );
}

/// The readout the panel's field shows. **In axis order, not the order it is
/// handed the boxes** — so the number beside two layers does not change when
/// they are picked the other way round — and **negative where they overlap**,
/// which is a state the field can write back and so must be able to report.
///
/// Over boxes rather than over ids because the panel feeds it *preview* bounds:
/// a readout taken from the committed tree cannot move while its own drag is
/// previewing (§15 D245).
#[test]
fn gaps_read_in_axis_order_and_go_negative_on_an_overlap() {
    let (f, a, b, c) = three_across();
    let res = f.resolved();
    // Deliberately scrambled: the sort is what has to put them right.
    let boxes: Vec<_> = [c, a, b]
        .iter()
        .map(|id| res.world_bounds(*id).unwrap())
        .collect();
    let gaps = build::gaps_along(&boxes, build::Axis::X);
    assert_eq!(gaps.len(), 2, "three layers have two gaps, got {gaps:?}");
    assert!(
        (gaps[0] - 20.0).abs() < 1e-9 && (gaps[1] - 120.0).abs() < 1e-9,
        "gaps read {gaps:?} rather than 20 and 120"
    );

    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(40.0, 10.0), (5.0, 0.0));
    let res = f.resolved();
    let boxes: Vec<_> = [a, b]
        .iter()
        .map(|id| res.world_bounds(*id).unwrap())
        .collect();
    let gaps = build::gaps_along(&boxes, build::Axis::X);
    assert!(
        (gaps[0] + 5.0).abs() < 1e-9,
        "an overlap of 5 read as {gaps:?}"
    );
}

// --- flip ------------------------------------------------------------------

/// One layer flips in place: the box it occupies is unchanged, because the
/// mirror line runs through its own centre.
#[test]
fn flipping_one_layer_leaves_its_box_where_it_was() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(40.0, 10.0), (100.0, 20.0));
    let before = f.resolved().world_bounds(a).unwrap();

    let res = f.resolved();
    f.commit(build::flip(&f.doc, &res, &[a], build::Axis::X));

    let after = f.resolved().world_bounds(a).unwrap();
    approx(before.origin(), after.origin(), "flipped box moved");
    assert!((before.width() - after.width()).abs() < 1e-9);
    assert!((before.height() - after.height()).abs() < 1e-9);
}

/// Several layers flip as one arrangement — they swap sides across the union's
/// centreline rather than each spinning about its own.
#[test]
fn flipping_a_selection_swaps_the_layers_across_the_union() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (90.0, 0.0));
    let res = f.resolved();
    let (a0, b0) = (res.world_bounds(a).unwrap(), res.world_bounds(b).unwrap());

    f.commit(build::flip(&f.doc, &res, &[a, b], build::Axis::X));
    let res = f.resolved();
    let (a1, b1) = (res.world_bounds(a).unwrap(), res.world_bounds(b).unwrap());
    assert!((a1.min_x() - b0.min_x()).abs() < 1e-9, "a took b's place");
    assert!((b1.min_x() - a0.min_x()).abs() < 1e-9, "b took a's place");
    // Vertically nothing moved: a horizontal flip is a mirror in x alone.
    assert!((a1.min_y() - a0.min_y()).abs() < 1e-9);
}

/// Flipping twice is the identity — the property that catches a mirror written
/// as "negate the x translation", which is not one.
#[test]
fn flipping_twice_restores_the_original_transform() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(40.0, 10.0), (100.0, 20.0));
    f.commit(Transaction(vec![Operation::SetTransform {
        id: a,
        transform: Affine::translate((100.0, 20.0)) * Affine::rotate(0.4),
    }]));
    let before = f.doc.get(a).unwrap().transform().as_coeffs();

    for _ in 0..2 {
        let res = f.resolved();
        f.commit(build::flip(&f.doc, &res, &[a], build::Axis::Y));
    }

    let after = f.doc.get(a).unwrap().transform().as_coeffs();
    for (x, y) in before.iter().zip(after.iter()) {
        assert!((x - y).abs() < 1e-9, "{before:?} vs {after:?}");
    }
}

/// The inspector reads rotation back out of the matrix and writes a rebuilt one
/// in. Without the mirror the round trip silently un-flips a flipped node, and
/// without the skew it silently un-leans a sheared one, so what has to survive
/// `of` → `to_affine` is the *basis*, not the three fields: `R(θ)·FLIP_Y` and
/// `R(θ+π)·FLIP_X` are the same reflection, and `of` is free to name it either
/// way.
#[test]
fn orientation_round_trips_every_mirror_and_lean() {
    for mirror in [build::Mirror::None, build::Mirror::X, build::Mirror::Y] {
        for deg in [0.0f64, 30.0, 179.0, -95.0, 150.0] {
            for skew_deg in [0.0f64, 12.0, -40.0, 75.0] {
                let want = build::Orientation {
                    angle: deg.to_radians(),
                    skew: skew_deg.to_radians(),
                    mirror,
                };
                let there = want.to_affine();
                let back = build::Orientation::of(there).to_affine();
                for (x, y) in there.as_coeffs().iter().zip(back.as_coeffs().iter()) {
                    assert!(
                        (x - y).abs() < 1e-9,
                        "{deg}° skew {skew_deg}° {mirror:?}: {:?} vs {:?}",
                        there.as_coeffs(),
                        back.as_coeffs()
                    );
                }
            }
        }
    }
}

/// A collapsed basis has no angle and no lean to measure, and the split must
/// answer with *numbers* rather than propagate a division by zero.
///
/// Exactness is not on offer here — a matrix that has lost a dimension cannot be
/// rebuilt from anything — but a NaN would not stay local: it goes into a
/// transform, multiplies through every descendant's world transform, and takes
/// the bounds, the culling and the scene walk with it.
#[test]
fn a_collapsed_basis_decomposes_to_numbers_not_nan() {
    let flat = [
        Affine::scale_non_uniform(0.0, 0.0),
        Affine::scale_non_uniform(0.0, 5.0),
        Affine::scale_non_uniform(5.0, 0.0),
        // Singular but not zero: both columns on one line.
        Affine::new([1.0, 1.0, 2.0, 2.0, 0.0, 0.0]),
    ];
    for m in flat {
        let split = build::Basis::of(m);
        assert!(
            split.orientation.angle.is_finite()
                && split.orientation.skew.is_finite()
                && split.scale.x.is_finite()
                && split.scale.y.is_finite(),
            "{:?} decomposed to {:?} / {:?}",
            m.as_coeffs(),
            split.orientation,
            split.scale
        );
        for v in split.orientation.to_affine().as_coeffs() {
            assert!(v.is_finite(), "{:?} rebuilt with a NaN", m.as_coeffs());
        }
    }
}

/// **The split is exact for every *invertible* matrix**, which is the property
/// the whole group- and selection-resize path rests on (§5.6, §15 D50): whatever
/// a world-space edit multiplies out to, `orientation · scale` reproduces it, so
/// the part that cannot go in the transform can be handed to the geometry
/// without anything being lost on the way.
///
/// ⚠️ **This heading read "for every matrix" until 2026-09-09** (§15 D714), and
/// every case below is invertible, so nothing here could ever have disagreed
/// with it. §5.6's own wording is the narrower and correct one; the copies in
/// this file and in `tools::split_geometry_scale`'s doc had both dropped the
/// word. `an_unrepresentable_basis_is_refused_rather_than_approximated` below
/// drives the cases this one cannot, and `a_collapsed_basis_decomposes_to_
/// numbers_not_nan` above was already saying so in as many words.
///
/// Includes bases carrying a scale — which a node's own transform never does,
/// but a *world* basis under a sheared ancestor does — and reflections, where
/// the sign lives in the mirror and the scale stays positive.
#[test]
fn a_basis_splits_into_a_storable_orientation_and_a_scale() {
    let cases = [
        Affine::IDENTITY,
        Affine::rotate(0.7),
        Affine::scale_non_uniform(3.0, 0.5),
        Affine::rotate(0.4) * Affine::scale_non_uniform(2.0, 5.0),
        // The case the old code could not hold: a non-uniform scale applied
        // across a rotation, which is a lean plus a scale and nothing less.
        Affine::scale_non_uniform(3.0, 1.0) * Affine::rotate(45f64.to_radians()),
        Affine::FLIP_X * Affine::rotate(1.2) * Affine::scale_non_uniform(1.0, 4.0),
        Affine::rotate(-2.5) * Affine::FLIP_Y * Affine::scale_non_uniform(0.25, 0.25),
        // Translation must be ignored rather than smeared into the basis.
        Affine::translate((90.0, -12.0)) * Affine::rotate(0.3),
    ];
    for m in cases {
        let split = build::Basis::of(m);
        assert!(
            split.scale.x >= 0.0 && split.scale.y >= 0.0,
            "{:?}: negative scale {:?} — the sign belongs to the mirror",
            m.as_coeffs(),
            split.scale
        );
        let rebuilt =
            split.orientation.to_affine() * Affine::scale_non_uniform(split.scale.x, split.scale.y);
        // Compare the basis only; `of` is documented to drop the translation.
        let [a, b, c, d, _, _] = m.as_coeffs();
        let [ra, rb, rc, rd, _, _] = rebuilt.as_coeffs();
        for (x, y) in [a, b, c, d].iter().zip([ra, rb, rc, rd].iter()) {
            assert!(
                (x - y).abs() < 1e-9,
                "{:?} split to {:?}×{:?} and rebuilt as {:?}",
                m.as_coeffs(),
                split.orientation,
                split.scale,
                rebuilt.as_coeffs()
            );
        }
    }
}

/// **`Basis::exact` is the half of the split that can say no**, and the two
/// tests above it are the reason it exists (§15 D714).
///
/// `a_collapsed_basis_decomposes_to_numbers_not_nan` says outright that
/// *"exactness is not on offer"* for a matrix that has lost a dimension, and it
/// drives `[1, 1, 2, 2]` — a rank-1 basis — to prove the numbers stay finite.
/// `a_basis_splits_into_a_storable_orientation_and_a_scale` used to open **"The
/// split is exact for every matrix"** and drove only invertible ones. Both were
/// green, both were about `Basis::of`, and between them they left the caller with
/// no way to tell which of the two regimes it was in.
///
/// 🚨 **That is the whole defect `[S13.1-L1-07]` names, and it is not an
/// arithmetic bug.** `Orientation::to_affine` is `R · skewX · mirror`, whose
/// determinant is always ±1, so `orientation · diag(sx, sy)` can only ever
/// produce a second column of **zero** when `sy` is zero. A rank-1 basis has a
/// second column that is emphatically *not* zero — `[1, 2, 2, 4]`'s is `(2, 4)` —
/// so no return value of `Basis::of` could have been exact. The function had no
/// way to say *"I cannot represent this"*, so it returned something plausible and
/// the caller acted on it.
///
/// **The threshold is not re-derived here.** `exact` reads the very numbers
/// `of`'s two degenerate arms tested — `scale.x` and `scale.y` against
/// `DEGENERATE` — rather than recomputing `sx` and `sy` beside them, so the two
/// cannot drift apart. That is deliberate: this project keeps finding rules with
/// one statement and more than one implementation.
#[test]
fn an_unrepresentable_basis_is_refused_rather_than_approximated() {
    // Rank-1: the second column is (2,4) and no orientation-times-scale can
    // produce it, because the only second column available is zero.
    let rank_one = Affine::new([1.0, 2.0, 2.0, 4.0, 5.0, 6.0]);
    assert_eq!(rank_one.determinant(), 0.0, "the fixture is singular");
    assert!(
        build::Basis::exact(rank_one).is_none(),
        "a rank-1 basis has no exact split and must say so"
    );

    // The other degenerate arm: invertible in principle, below the threshold
    // `of` measures an angle against. `sx` comes back 0 with an *identity*
    // orientation, so the consumer reads "no scale at all" for a real one.
    assert!(
        build::Basis::exact(Affine::scale(1e-13)).is_none(),
        "a basis under DEGENERATE has no angle to measure and must say so"
    );

    // Non-finite never reaches the arithmetic.
    assert!(build::Basis::exact(Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0])).is_none());

    // And everything the exactness test drives still splits, with `exact`
    // agreeing coefficient for coefficient with `of` — the control that stops
    // this being a guard that simply refuses everything.
    for m in [
        Affine::IDENTITY,
        Affine::rotate(0.7),
        Affine::scale_non_uniform(3.0, 0.5),
        Affine::scale_non_uniform(3.0, 1.0) * Affine::rotate(45f64.to_radians()),
        Affine::FLIP_X * Affine::rotate(1.2) * Affine::scale_non_uniform(1.0, 4.0),
    ] {
        let exact = build::Basis::exact(m).unwrap_or_else(|| {
            panic!("{:?} is invertible and must split", m.as_coeffs());
        });
        let of = build::Basis::of(m);
        assert_eq!(exact.scale, of.scale, "{:?}", m.as_coeffs());
        assert_eq!(
            exact.orientation.to_affine().as_coeffs(),
            of.orientation.to_affine().as_coeffs(),
            "{:?}",
            m.as_coeffs()
        );
    }
}

/// Flipping an upright layer must not make the inspector claim it is rotated.
/// Both mirrors are the same reflection 180° apart, so a decomposition that only
/// knows `FLIP_X` reports −180° for a plain vertical flip — technically true and
/// visibly wrong.
#[test]
fn a_plain_flip_reads_as_no_rotation() {
    for (basis, want) in [
        (Affine::FLIP_X, build::Mirror::X),
        (Affine::FLIP_Y, build::Mirror::Y),
    ] {
        let o = build::Orientation::of(basis);
        assert_eq!(o.mirror, want, "{:?}", basis.as_coeffs());
        assert!(
            o.angle.abs() < 1e-9,
            "{:?} read as {}°",
            basis.as_coeffs(),
            o.angle.to_degrees()
        );
    }
}

/// A layer whose pivot has been **moved** mirrors about the pivot, not about its
/// own centre — a 40-wide rect with its pivot on the left edge flips to the other
/// side of that edge rather than staying put.
#[test]
fn flipping_a_layer_with_a_moved_pivot_mirrors_about_the_pivot() {
    use ondin_core::Pivot;
    use ondin_core::kurbo::Vec2;

    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(40.0, 10.0), (100.0, 20.0));
    let before = f.resolved().world_bounds(a).unwrap();

    f.commit(Transaction(vec![Operation::SetPivot {
        id: a,
        pivot: Some(Pivot::Normalized(Vec2::new(0.0, 0.5))),
    }]));
    let res = f.resolved();
    f.commit(build::flip(&f.doc, &res, &[a], build::Axis::X));
    let after = f.resolved().world_bounds(a).unwrap();

    // The mirror line is the left edge, so the box lands entirely to its left.
    assert!(
        (after.max_x() - before.min_x()).abs() < 1e-9,
        "expected the box to end where it began: {before:?} → {after:?}"
    );
    assert!((after.width() - before.width()).abs() < 1e-9);
    // Vertically nothing moved: a horizontal flip is a mirror in x alone.
    assert!((after.min_y() - before.min_y()).abs() < 1e-9);
}

/// **A pivot nobody has moved changes nothing.** The default resolves to the box's
/// own centre, which is what `flip` used before the field existed — but the two are
/// only the same point for a shape, and a *rotated group's* differ (the union of
/// its children's axis-aligned bounds is not the bounding box of their rotated
/// union). So `flip` asks whether a pivot has been *placed*, not where one would
/// resolve to, and this is the case that catches the difference.
#[test]
fn an_unset_pivot_flips_a_rotated_group_exactly_as_before() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(40.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 40.0), (60.0, 30.0));
    let g = match build::group(&f.doc, &mut f.ids, &[a, b]) {
        Ok((tx, id)) => {
            f.commit(tx);
            id
        }
        Err(e) => panic!("group: {e:?}"),
    };
    // Turned, so the group's local box and its world AABB genuinely disagree.
    f.commit(Transaction(vec![Operation::SetTransform {
        id: g,
        transform: f.doc.get(g).unwrap().transform() * Affine::rotate(0.5),
    }]));

    let res = f.resolved();
    let expected = res.world_bounds(g).unwrap().center().to_vec2();
    f.commit(build::flip(&f.doc, &res, &[g], build::Axis::X));

    // The union centre is what it mirrored about, so it is the fixed point.
    let after = f.resolved().world_bounds(g).unwrap();
    assert!(
        (after.center().x - expected.x).abs() < 1e-9,
        "an untouched pivot must leave the union centreline as the mirror line"
    );
}

// --- booleans -------------------------------------------------------------

fn text_node() -> NodeKind {
    NodeKind::Text {
        content: "hi".into(),
        style: Box::new(ondin_core::TextStyle {
            font_family: "Inter".into(),
            font_size: 16.0,
            ..Default::default()
        }),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: ondin_core::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

/// The same structural property `group` has — operands go inside in z-order, the
/// container takes the topmost member's slot, nothing moves — plus the one thing a
/// boolean adds: **the result inherits the bottom operand's paint.**
///
/// That last part is not cosmetic. A fresh container has no fills, so a boolean
/// without it evaluates correctly and then draws nothing, which reads as the
/// operation having deleted the artwork.
#[test]
fn a_boolean_wraps_its_operands_and_takes_the_bottom_ones_paint() {
    let mut f = Fixture::new();
    let back = f.add(f.artboard, rect(80.0, 40.0), (10.0, 10.0));
    let front = f.add(f.artboard, rect(80.0, 40.0), (40.0, 20.0));
    let blue = ondin_core::Fill {
        brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
            0, 0, 255, 255,
        )),
        visible: true,
    };
    f.commit(Transaction(vec![Operation::SetFills {
        id: back,
        fills: vec![blue.clone()],
    }]));
    let before = (f.world_origin(back), f.world_origin(front));

    let (tx, b) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[front, back],
        ondin_core::BoolOp::Subtract,
        None,
    )
    .expect("two rects are booleanable");
    f.commit(tx);

    assert_eq!(
        f.children_of(b),
        vec![back, front],
        "operands in z-order, bottom first — which is the operand order Subtract uses"
    );
    approx(f.world_origin(back), before.0, "the bottom operand");
    approx(f.world_origin(front), before.1, "the top operand");
    assert_eq!(
        f.doc.get(b).unwrap().paint().fills,
        vec![blue],
        "the result took the bottom operand's fill"
    );
}

/// **Refused, not silently accepted.** Each of these would produce a container
/// that cannot do what its name says, and the dropdown is disabled on exactly the
/// same answers — `inspector::booleanable_selection` asks this builder rather than
/// re-deriving the rule.
#[test]
fn a_boolean_refuses_what_it_cannot_combine() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let t = f.add(f.artboard, text_node(), (0.0, 0.0));
    let elsewhere = f.add(f.root, rect(10.0, 10.0), (0.0, 0.0));
    let op = ondin_core::BoolOp::Union;

    let cases: [(&str, Vec<NodeId>); 4] = [
        ("one layer is not a boolean", vec![a]),
        ("nothing selected", vec![]),
        ("text has no outline to combine", vec![a, t]),
        ("different parents", vec![a, elsewhere]),
    ];
    for (why, ids) in cases {
        assert!(
            build::boolean(&f.doc, &mut f.ids, &ids, op, None).is_err(),
            "should refuse: {why}"
        );
    }
}

/// Switching the operation is a *geometry* edit: the children and the paint stay,
/// and undo puts the previous operation back rather than dissolving the container.
#[test]
fn switching_the_operation_keeps_the_children_and_undoes_to_the_previous_one() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(80.0, 40.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(80.0, 40.0), (40.0, 0.0));
    let (tx, node) =
        build::boolean(&f.doc, &mut f.ids, &[a, b], ondin_core::BoolOp::Union, None).unwrap();
    f.commit(tx);

    let tx = build::set_boolean_op(&f.doc, node, ondin_core::BoolOp::Intersect).unwrap();
    f.commit(tx);
    assert_eq!(
        f.doc.get(node).unwrap().kind(),
        &NodeKind::Boolean {
            op: ondin_core::BoolOp::Intersect
        }
    );
    assert_eq!(f.children_of(node), vec![a, b], "children untouched");

    f.hist.undo(&mut f.doc).expect("undo the switch");
    assert_eq!(
        f.doc.get(node).unwrap().kind(),
        &NodeKind::Boolean {
            op: ondin_core::BoolOp::Union
        },
        "back to the operation it replaced, not to no container at all"
    );

    // Setting the operation it already has is no edit at all — an empty
    // transaction, so a dropdown re-picking the current value adds no undo step.
    let same = build::set_boolean_op(&f.doc, node, ondin_core::BoolOp::Union).unwrap();
    assert!(same.0.is_empty());
    assert!(
        build::set_boolean_op(&f.doc, a, ondin_core::BoolOp::Union).is_err(),
        "and it is not an operation on a rect"
    );
}

/// **The layer name follows the operation, unless the user has renamed it.**
///
/// A boolean is created named after its operation, so leaving the name alone left a
/// row reading "Union" over an intersection. But the name is the user's the moment
/// they touch it: silently rewriting "Badge cutout" would be worse than a stale
/// label, so the test is exact — one of the four labels, or theirs.
#[test]
fn switching_the_operation_renames_only_a_default_name() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(80.0, 40.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(80.0, 40.0), (40.0, 0.0));
    let (tx, node) =
        build::boolean(&f.doc, &mut f.ids, &[a, b], ondin_core::BoolOp::Union, None).unwrap();
    f.commit(tx);
    assert_eq!(f.doc.get(node).unwrap().name(), "Union 1");

    let tx = build::set_boolean_op(&f.doc, node, ondin_core::BoolOp::Intersect).unwrap();
    f.commit(tx);
    assert_eq!(
        f.doc.get(node).unwrap().name(),
        "Intersect 1",
        "a name we gave it follows the operation, number and all"
    );

    // Now the user's own name, which must survive.
    f.commit(Transaction(vec![Operation::SetName {
        id: node,
        name: "Badge cutout".into(),
    }]));
    let tx = build::set_boolean_op(&f.doc, node, ondin_core::BoolOp::Exclude).unwrap();
    f.commit(tx);
    assert_eq!(
        f.doc.get(node).unwrap().name(),
        "Badge cutout",
        "a name the user typed is never overwritten"
    );
    assert_eq!(
        f.doc.get(node).unwrap().kind(),
        &NodeKind::Boolean {
            op: ondin_core::BoolOp::Exclude
        },
        "…and the operation still changed"
    );
}

/// Net area of a path, holes subtracting — enough to tell one boolean answer from
/// another, which is what the flatten tests are about.
fn net_area(path: &ondin_core::kurbo::BezPath) -> f64 {
    ondin_core::kurbo::Shape::area(path).abs()
}

/// **Releasing a boolean gives the operands back, where they were.**
///
/// `build::ungroup` is the whole of it: the operands were never destroyed — that is
/// what non-destructive means — so taking the container apart is the identical splice
/// a group needs, transform folded into each child so nothing moves on screen.
#[test]
fn releasing_a_boolean_gives_its_operands_back_in_place() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(80.0, 40.0), (10.0, 10.0));
    let b = f.add(f.artboard, rect(80.0, 40.0), (40.0, 20.0));
    let other = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let before = f.world_origin(a);

    let (tx, node) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[a, b],
        ondin_core::BoolOp::Subtract,
        None,
    )
    .unwrap();
    f.commit(tx);
    // A boolean gets a transform of its own, so the splice has real work to do.
    f.commit(Transaction(vec![Operation::SetTransform {
        id: node,
        transform: Affine::translate((25.0, 15.0)),
    }]));
    let moved = (f.world_origin(a), f.world_origin(b));
    assert!(
        moved.0 != before,
        "moving the container moved the operands with it"
    );

    let tx = build::ungroup(&f.doc, node).expect("a boolean releases like a group");
    f.commit(tx);

    assert!(f.doc.get(node).is_none(), "the container is gone");
    assert_eq!(
        f.children_of(f.artboard),
        vec![a, b, other],
        "the operands are back among their siblings, in the container's z-slot"
    );
    approx(f.world_origin(a), moved.0, "a did not move");
    approx(f.world_origin(b), moved.1, "b did not move");
}

/// **Flattening a boolean keeps the picture and drops the layers that made it.**
///
/// The shape is the assertion: the `Path` left behind has the same outline the
/// boolean drew, in the same place, with the same paint — everything the eye was
/// looking at — and the operands are gone, which is the part `ungroup` would not do.
#[test]
fn flattening_a_boolean_keeps_its_outline_and_discards_the_operands() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(100.0, 100.0), (50.0, 0.0));
    let front = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let red = ondin_core::Fill {
        brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
            255, 0, 0, 255,
        )),
        visible: true,
    };
    f.commit(Transaction(vec![Operation::SetFills {
        id: a,
        fills: vec![red.clone()],
    }]));

    let (tx, node) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[a, b],
        ondin_core::BoolOp::Subtract,
        None,
    )
    .unwrap();
    f.commit(tx);
    f.commit(Transaction(vec![Operation::SetTransform {
        id: node,
        transform: Affine::translate((7.0, 3.0)),
    }]));
    let res = f.resolved();
    let outline = res.boolean_path(node).expect("a result").clone();
    let world_before = f.world_origin(node);

    let (tx, flat) = build::flatten(&f.doc, &res, &mut f.ids, &[node]).expect("a boolean flattens");
    f.commit(tx);

    assert!(f.doc.get(node).is_none(), "the container is gone");
    assert!(
        f.doc.get(a).is_none() && f.doc.get(b).is_none(),
        "and so are its operands"
    );
    assert_eq!(
        f.children_of(f.artboard),
        vec![flat, front],
        "the path took the boolean's own z-slot, under the sibling in front of it"
    );
    let NodeKind::Path { path, .. } = f.doc.get(flat).unwrap().kind() else {
        panic!("a Path");
    };
    assert!(
        (net_area(path) - net_area(&outline)).abs() < 1.0,
        "the same outline: {} vs {}",
        net_area(path),
        net_area(&outline)
    );
    approx(f.world_origin(flat), world_before, "and in the same place");
    assert_eq!(
        f.doc.get(flat).unwrap().paint().fills,
        vec![red],
        "with the paint that was governing the result"
    );
    assert_eq!(
        f.doc.get(flat).unwrap().name(),
        "Subtract 1",
        "and the name, which is the only record left of what it was"
    );

    // **One undo puts all of it back.** Worth pinning because flatten is the one
    // shape command that destroys layers, and it does so with a single `DeleteNode`
    // over a node that still *has* children — the case where "the inverse captures
    // the subtree" is load-bearing rather than incidental.
    f.hist.undo(&mut f.doc).expect("undo the flatten");
    assert!(f.doc.get(flat).is_none(), "the path is gone again");
    assert!(
        f.doc.get(node).is_some() && f.doc.get(a).is_some() && f.doc.get(b).is_some(),
        "and the boolean is back, with both operands inside it"
    );
    assert_eq!(f.children_of(node), vec![a, b]);
    assert_eq!(f.children_of(f.artboard), vec![node, front]);
    approx(f.world_origin(node), world_before, "in the same place");
}

/// **Flattening an `Exclude` has to carry the fill rule, or every hole fills in.**
///
/// The one case where "the same outline" is not the same picture. `Node::fill_rule`
/// **derives** even-odd from the operation for an `Exclude` (§15 D239) rather than
/// storing it — which is what keeps files written before the field existed correct —
/// and `flatten` throws the operation away. So the `Path` left behind holds exactly
/// the subpaths the container held, under a *different* rule, unless the rule is
/// carried across: an exclusion's outline is its operands concatenated, and
/// concatenated operands read non-zero are the **union**.
///
/// Asserted as the picture rather than as the field, each path sampled under the
/// rule its own node reports — because the geometry survives either way and the
/// field alone would not say what the eye gets. Two 100-wide rects at 0 and +50
/// leave odd bands [0,50) and [100,150) with the shared band cut out, so it is the
/// **middle** sample that flips when the rule is lost.
///
/// **Two flips, and this is the one that discriminates them.** Removing the carry
/// altogether gives `[true, true, true]` — the union, which is what the app did until
/// 2026-08-31 — and carrying `node.fill_rule` (the stored *field*) instead of
/// `node.fill_rule()` (the effective rule) gives the identical failure, because an
/// `Exclude`'s stored field is the untouched default. The outline test below cannot
/// tell those two implementations apart; this one can.
#[test]
fn flattening_an_exclude_carries_the_even_odd_rule_its_holes_depend_on() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(100.0, 100.0), (50.0, 0.0));

    let (tx, node) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[a, b],
        ondin_core::BoolOp::Exclude,
        None,
    )
    .unwrap();
    f.commit(tx);

    let bands = |f: &Fixture, id: NodeId, path: &ondin_core::kurbo::BezPath| -> Vec<bool> {
        let rule = f.doc.get(id).expect("a live node").fill_rule();
        [25.0, 75.0, 125.0]
            .iter()
            .map(|x| rule.contains(path, Point::new(*x, 50.0)))
            .collect()
    };

    let res = f.resolved();
    let outline = res.boolean_path(node).expect("a result").clone();
    assert_eq!(
        bands(&f, node, &outline),
        vec![true, false, true],
        "the exclusion: both rects, less the band they share"
    );

    let (tx, flat) = build::flatten(&f.doc, &res, &mut f.ids, &[node]).expect("a boolean flattens");
    f.commit(tx);

    let NodeKind::Path { path, .. } = f.doc.get(flat).unwrap().kind() else {
        panic!("a Path");
    };
    let path = path.clone();
    // The picture first and the field second, deliberately: without the fix *both*
    // fail, and it is the band that says what the user would have seen.
    assert_eq!(
        bands(&f, flat, &path),
        vec![true, false, true],
        "the same picture: the overlap is still a hole rather than filled in"
    );
    assert_eq!(
        f.doc.get(flat).unwrap().fill_rule(),
        ondin_core::FillRule::EvenOdd,
        "the rule the container derived is the path's own now, stored"
    );
}

/// **Flattening several layers is their union, in one step.**
///
/// 100x100 at 0 and 100x100 at +50 overlap over a 50-wide band, so the union is
/// 15000 where appending the two outlines into one path would be read even-odd and
/// punch the overlap out (10000). That difference is why this goes through
/// `boolean::evaluate` rather than concatenating.
#[test]
fn flattening_a_selection_is_the_union_of_its_outlines() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(100.0, 100.0), (50.0, 0.0));
    let res = f.resolved();

    let (tx, flat) = build::flatten(&f.doc, &res, &mut f.ids, &[b, a]).expect("two rects flatten");
    f.commit(tx);

    let NodeKind::Path { path, .. } = f.doc.get(flat).unwrap().kind() else {
        panic!("a Path");
    };
    assert!(
        (net_area(path) - 15_000.0).abs() < 1.0,
        "the union, not the two outlines concatenated: {}",
        net_area(path)
    );
    // The outlines came out in the *parent's* space, which is where an identity
    // transform puts them — so the shape covers the ground the rects did.
    approx(
        f.world_origin(flat),
        f.world_origin(f.artboard),
        "the new path sits on its parent's origin",
    );
    assert_eq!(f.children_of(f.artboard), vec![flat]);
}

/// **An abandoned boolean and an empty one are two different refusals, and the
/// message the user gets has to say which** (§15 D736, `[S10.1-L2-06]`).
///
/// `boolean::evaluate` answers `None` for both — a correct empty result and a
/// `catch_unwind` out of flo_curves — so the counter is the only thing that
/// separates them, which is `boolean::failures`' whole doc. Until D736 this door
/// mapped both onto `OpError::EmptyGeometry`, whose own doc argues from what the
/// user can do next: *"the geometry was empty, which is a thing the user can fix by
/// moving a shape rather than by selecting differently."* **Moving a shape does not
/// help an unwind**, so the message was true about the outcome and false about the
/// cause — the one thing §6.2's *"a missing edit is recoverable and a lying one is
/// not"* is about, arriving in the error text rather than in the model.
///
/// **The two rects genuinely overlap**, which is what makes this a test about the
/// *reason* rather than about the geometry: the same fixture flattens to a 15000
/// union in the test above, so nothing here is empty and `EmptyGeometry` would be
/// wrong on its own terms.
///
/// ⚠️ **`poison_next(1)` and not more.** `operand_path` on a plain rect goes through
/// `local_path` and never reaches `evaluate`, so the union is the *first* call and
/// one poison is exactly enough. A fixture with a group or a nested boolean in it
/// would spend the token somewhere else and this test would go green for the wrong
/// reason — which is the failure mode `poison_next`'s own doc warns about, the token
/// being consumed by the innermost `evaluate`.
///
/// ⚠️ **Flip-check, run.** Putting the old `.ok_or(OpError::EmptyGeometry)` back
/// fails here on the `BooleanAbandoned` assertion and leaves the un-poisoned control
/// green, which is the predicted site. **The control is the load-bearing half**: a
/// version of this test without it passes against a `flatten_union` that returns
/// `BooleanAbandoned` unconditionally.
#[test]
fn a_flatten_says_the_boolean_was_abandoned_rather_than_that_the_geometry_was_empty() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(100.0, 100.0), (50.0, 0.0));
    let res = f.resolved();

    ondin_core::boolean::poison_next(1);
    let err = build::flatten(&f.doc, &res, &mut f.ids, &[b, a])
        .expect_err("the union was abandoned, so there is nothing to flatten");
    assert!(
        matches!(err, OpError::BooleanAbandoned),
        "an abandoned boolean must not be reported as empty geometry: {err:?}"
    );
    // And the message does not send the user to move a shape.
    let said = err.to_string();
    assert!(
        said.contains("could not be computed"),
        "the message has to name the cause: {said}"
    );

    // **The control, in the same test**, because the assertion above is about which
    // of two refusals fires and is worth nothing without the case that must not.
    assert!(
        build::flatten(&f.doc, &res, &mut f.ids, &[b, a]).is_ok(),
        "unpoisoned, the identical selection flattens"
    );
}

/// **The one-boolean door asks the same question of the cache** (§15 D736,
/// `[S10.1-L2-06]`).
///
/// `flatten_boolean` never calls `evaluate` — it reads `Resolved::boolean_path`,
/// filled on some earlier pass — so this thread's counter has nothing to say about
/// it and reading the counter here would be the *wrong* question. What it reads
/// instead is §15 D298's own mark: `Resolved` brackets every `evaluate_node` and
/// records the id in `failed`, so a `Boolean` with no cached outline is an abandoned
/// one exactly when the layers row is already wearing its warning colour. **The two
/// doors need different mechanisms for the same distinction**, which is why the
/// finding's *"just bracket the four sites"* would have been wrong at one of them.
///
/// ⚠️ **`poison_next(1)` before `f.resolved()`, not before `flatten`.** The failure
/// has to happen while the cache is being built; arming it at the flatten would leave
/// the token unspent, because there is no `evaluate` on that path at all — and the
/// test would then be green against code that never consulted the mark.
///
/// ⚠️ **Flip-check, run.** Replacing the `res.boolean_failed(id)` arm with the old
/// `.ok_or(OpError::EmptyGeometry)` fails here and leaves
/// `flattening_a_boolean_keeps_its_outline_and_discards_the_operands` green, which is
/// the predicted site: that test never poisons, so its cache is always full and it
/// cannot reach either refusal.
#[test]
fn flattening_an_abandoned_boolean_says_so_rather_than_blaming_the_geometry() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(100.0, 100.0), (50.0, 0.0));
    let (tx, node) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[a, b],
        ondin_core::BoolOp::Subtract,
        None,
    )
    .expect("two rects make a boolean");
    f.commit(tx);

    ondin_core::boolean::poison_next(1);
    let res = f.resolved();
    // The fixture is in the state the test names, asserted rather than hoped for:
    // the cache is empty *and* the mark is set. Without this the case below could be
    // any other empty cache.
    assert!(res.boolean_failed(node), "the boolean is marked as failed");
    assert!(
        res.boolean_path(node).is_none(),
        "and has no cached outline"
    );

    let err = build::flatten(&f.doc, &res, &mut f.ids, &[node])
        .expect_err("an abandoned boolean has no outline to flatten");
    assert!(
        matches!(err, OpError::BooleanAbandoned),
        "the mark is what separates this from an empty result: {err:?}"
    );
}

/// **Refused, not quietly turned into something else.** A lone rect is not a
/// boolean, and *Flatten* is not the word for converting one — that is
/// `build::outline`, which exists now and is a separate verb precisely so that this
/// answer can stay no (§15 D230). Text has no outline until its glyphs are
/// converted, as `boolean` also says.
#[test]
fn flatten_refuses_a_lone_shape_and_anything_with_no_outline() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let t = f.add(f.artboard, text_node(), (0.0, 0.0));
    let elsewhere = f.add(f.root, rect(10.0, 10.0), (0.0, 0.0));
    let res = f.resolved();

    let cases: [(&str, Vec<NodeId>); 4] = [
        ("one shape is not a boolean", vec![a]),
        ("nothing selected", vec![]),
        ("text has no outline", vec![a, t]),
        ("different parents", vec![a, elsewhere]),
    ];
    for (why, ids) in cases {
        assert!(
            build::flatten(&f.doc, &res, &mut f.ids, &ids).is_err(),
            "should refuse: {why}"
        );
    }
}

/// **Outlining a shape keeps the picture and everything about the layer that is not
/// its geometry** (§15 D230).
///
/// The rect that goes in has a corner radius, a fill, a name, a reduced opacity, a
/// moved pivot and a sibling in front of it; the path that comes out has to have all
/// of those and be the same shape in the same place. Each is a thing
/// `replace_with_path` could quietly drop, and dropping any of them looks like the row
/// having done something violent.
#[test]
fn outlining_a_rect_keeps_its_shape_its_paint_and_its_place() {
    let mut f = Fixture::new();
    let r = f.add(
        f.artboard,
        NodeKind::Rect {
            size: ondin_core::kurbo::Size::new(100.0, 60.0),
            corner_radii: ondin_core::kurbo::RoundedRectRadii::from_single_radius(10.0),
        },
        (7.0, 3.0),
    );
    let front = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let red = ondin_core::Fill {
        brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
            255, 0, 0, 255,
        )),
        visible: true,
    };
    f.commit(Transaction(vec![
        Operation::SetFills {
            id: r,
            fills: vec![red.clone()],
        },
        Operation::SetOpacity {
            id: r,
            opacity: 0.5,
        },
        Operation::SetPivot {
            id: r,
            pivot: Some(ondin_core::Pivot::Local(Point::new(0.0, 0.0))),
        },
        Operation::SetName {
            id: r,
            name: "Badge".into(),
        },
        Operation::SetMask { id: r, mask: true },
        Operation::SetEffects {
            id: r,
            effects: vec![ondin_core::Effect::new(ondin_core::EffectKind::DropShadow(
                ondin_core::Shadow {
                    blur: 14.0,
                    ..ondin_core::Shadow::default()
                },
            ))],
        },
    ]));
    // The fixture: a *rounded* rect, so `local_path` has something to bake and the
    // area assertion below is not the one a square-cornered rect would also pass.
    let before = ondin_core::geometry::local_path(f.doc.get(r).unwrap().kind()).unwrap();
    let square = 100.0 * 60.0;
    assert!(
        net_area(&before) < square - 30.0,
        "the corners have to be visibly rounded, or this tests a plain rect: {} vs {square}",
        net_area(&before)
    );
    let world_before = f.world_origin(r);

    let (tx, made) = build::outline(&f.doc, &mut f.ids, r).expect("a rect outlines");
    f.commit(tx);

    assert!(f.doc.get(r).is_none(), "the rect is gone");
    assert_eq!(
        f.children_of(f.artboard),
        vec![made, front],
        "and the path took its z-slot, under the sibling in front of it"
    );
    let node = f.doc.get(made).unwrap();
    let NodeKind::Path { path, corner_radii } = node.kind() else {
        panic!("a Path");
    };
    assert!(
        (net_area(path) - net_area(&before)).abs() < 1.0,
        "the same outline, rounding baked in: {} vs {}",
        net_area(path),
        net_area(&before)
    );
    // **The radii are baked, not carried.** `Path::corner_radii` is indexed by
    // anchor, and the arcs that replaced the corners have no anchor to be indexed by
    // — carrying `10.0` would round something a second time.
    assert!(
        corner_radii.iter().all(|r| *r <= 0.0),
        "the radii are in the geometry now: {corner_radii:?}"
    );
    approx(f.world_origin(made), world_before, "in the same place");
    assert_eq!(node.paint().fills, vec![red], "with its paint");
    assert_eq!(node.name(), "Badge", "and its name");
    assert_eq!(node.opacity(), 0.5, "and its opacity");
    assert_eq!(
        node.pivot(),
        Some(ondin_core::Pivot::Local(Point::new(0.0, 0.0))),
        "and the pivot the user placed — dropped until 2026-08-19, which reset the \
         rotation centre of a layer that had one on purpose"
    );
    assert!(
        node.mask(),
        "and the mask: the same shape in the same slot clips the same run, so \
         outlining one must not release it and shove the artwork above back to \
         full size with nothing on screen saying why"
    );
    // **The effect stack, which is the pivot's loss one field later** (§5.3a).
    // `replace_with_path` carries name, opacity, visibility, pivot and mask and
    // names lock and proportion-lock as deliberate exclusions; effects were in
    // neither list when they landed, so *Outline shape*, *Convert to path* and a
    // one-boolean *Flatten* all silently flattened a shadow away. Found by
    // reading this function against its own list rather than by anything failing.
    assert_eq!(
        node.effects().len(),
        1,
        "the shadow comes across: outlining a shape is not a way to remove one"
    );
    assert!(
        matches!(
            &node.effects()[0].kind,
            ondin_core::EffectKind::DropShadow(s) if s.blur == 14.0
        ),
        "and with the numbers the user tuned, not a fresh default: {:?}",
        node.effects()[0]
    );

    // One undo, because this destroys a layer.
    f.hist.undo(&mut f.doc).expect("undo the outline");
    assert!(f.doc.get(made).is_none() && f.doc.get(r).is_some());
    assert_eq!(f.children_of(f.artboard), vec![r, front]);
}

/// **The same carry, from the other direction: an outlined `Path` keeps the rule its
/// author set.**
///
/// The `Exclude` case above is `replace_with_path` losing a rule the container
/// *derived*; this is the same line losing one that is **stored**, and it is the case
/// the app can actually author — `Item::EvenOdd` is offered on a single `Path` and
/// nothing else, so a compound path with a hole in it is the only even-odd shape a
/// user can make. *Outline shape* is then reachable on it the moment one corner is
/// rounded, and it would bake the radii and fill the hole in.
///
/// Two same-wound subpaths, which is the fixture the rule is *about*: a 100 square
/// with a 40 square inside it fills solid under non-zero and is a ring under
/// even-odd, so the centre sample is the whole assertion and the corner radii are
/// only what makes `can_outline` say yes.
///
/// The flip that bites here is removing the carry — the centre sample fills in, and
/// it is that assertion rather than the field one that fails first. Swapping the
/// effective rule for the stored field leaves this green, since for a `Path` the two
/// are the same value; the `Exclude` test above is what pins that half.
#[test]
fn outlining_a_path_keeps_the_fill_rule_the_user_set_on_it() {
    let mut f = Fixture::new();
    let mut compound = ondin_core::kurbo::BezPath::new();
    let square = |x0, y0, x1, y1| {
        ondin_core::kurbo::Shape::to_path(&ondin_core::kurbo::Rect::new(x0, y0, x1, y1), 0.01)
    };
    compound.extend(square(0.0, 0.0, 100.0, 100.0));
    compound.extend(square(30.0, 30.0, 70.0, 70.0));
    let p = f.add(
        f.artboard,
        NodeKind::Path {
            path: compound,
            corner_radii: vec![10.0; 8],
        },
        (0.0, 0.0),
    );
    f.commit(Transaction(vec![Operation::SetFillRule {
        id: p,
        rule: ondin_core::FillRule::EvenOdd,
    }]));

    let holed = |f: &Fixture, id: NodeId| -> bool {
        let node = f.doc.get(id).expect("a live node");
        let NodeKind::Path { path, .. } = node.kind() else {
            panic!("a Path");
        };
        let rule = node.fill_rule();
        rule.contains(path, Point::new(10.0, 50.0)) && !rule.contains(path, Point::new(50.0, 50.0))
    };
    assert!(
        holed(&f, p),
        "the fixture is a ring before anything is outlined, or this tests nothing"
    );
    assert!(
        build::can_outline(f.doc.get(p).unwrap().kind()),
        "and the row is offered on it, which is what makes the loss reachable"
    );

    let (tx, made) = build::outline(&f.doc, &mut f.ids, p).expect("a rounded path outlines");
    f.commit(tx);

    assert!(
        holed(&f, made),
        "still a ring: baking the radii must not fill the hole in"
    );
    assert_eq!(
        f.doc.get(made).unwrap().fill_rule(),
        ondin_core::FillRule::EvenOdd,
        "and the rule is the same one, stored"
    );
}

/// **What `can_outline` says yes and no to, and the one kind where it is a question
/// about *state*** (§15 D230).
///
/// Five kinds always answer yes and five always answer no, each for its own reason —
/// and a `Path` answers yes only while its radii are still in the model, because
/// `round_corners` returns the path untouched otherwise and the operation would be a
/// no-op wearing a live menu row.
///
/// ⚠️ **Flipped against `Path => true`**, the obvious spelling, which makes *Outline
/// shape* a live row on every path in the document that then does nothing at all —
/// the "present and dim is better than present and dead" rule of
/// `context-menus.md` §3, failing in the direction that has no symptom until someone
/// clicks.
#[test]
fn can_outline_takes_the_five_shapes_and_asks_a_path_about_its_radii() {
    use ondin_core::kurbo::{BezPath, Size};
    let size = Size::new(10.0, 10.0);

    for yes in [
        rect(10.0, 10.0),
        NodeKind::Ellipse { size },
        NodeKind::Polygon { size, sides: 5 },
        NodeKind::Star {
            size,
            points: 5,
            inner_ratio: 0.5,
        },
        NodeKind::Line {
            end: Point::new(10.0, 0.0),
        },
    ] {
        assert!(build::can_outline(&yes), "{yes:?} has an outline to trace");
    }

    // A boolean is `flatten`'s; a group has no outline of its own; a frame is a page;
    // text needs its glyphs converted first.
    for no in [
        NodeKind::Boolean {
            op: ondin_core::BoolOp::Union,
        },
        NodeKind::Group,
        NodeKind::Artboard { size },
        text_node(),
        NodeKind::Root,
    ] {
        assert!(!build::can_outline(&no), "{no:?} must be refused");
    }

    // The one kind that is a question about state.
    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((10.0, 0.0));
    path.line_to((10.0, 10.0));
    path.close_path();
    assert!(
        !build::can_outline(&NodeKind::Path {
            path: path.clone(),
            corner_radii: vec![0.0, 0.0, 0.0],
        }),
        "a path with nothing to bake is already an outline"
    );
    assert!(
        build::can_outline(&NodeKind::Path {
            path,
            corner_radii: vec![0.0, 3.0, 0.0],
        }),
        "and one radius in the model is enough to make baking mean something"
    );
}

/// **A text layer becomes a `Path` whose ink lands where the text's did**, keeping
/// the layer's slot, name, transform and paint (`context-menus.md` §5.6).
///
/// The structural half is `build::outline`'s and is shared code; what is this
/// verb's own is the **space**. `text::outline` places each glyph at the
/// coordinates the layout gave it, which are the node's own local coordinates —
/// the same ones the render walk hands a backend — so the conversion has nothing
/// to transform on the way out. Getting that wrong is the whole risk here, and it
/// is silent: a path is still a path wherever it lands.
///
/// So the assertion is that the result's **world bounds** sit inside the box the
/// text occupied. World rather than local, because the fixture's artboard is
/// translated and its child is translated again, so a conversion that emitted in
/// parent space or forgot `TextLayout::origin` comes out somewhere else entirely.
/// The tolerance is a tenth of the em, which admits a round letter's optical
/// overshoot below the baseline and admits nothing like a whole-box offset.
///
/// ⚠️ Flipped by translating the path by the node's own transform before handing it
/// to `replace_with_path` — the plausible mistake, since the glyphs *look* like
/// they should need placing — where the ink lands a full `(70, 50)` away and every
/// containment assertion fails.
#[test]
fn converting_text_to_a_path_keeps_its_slot_and_puts_the_ink_where_it_was() {
    let mut f = Fixture::new();
    let before = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let t = f.add(f.artboard, text_node(), (20.0, 20.0));
    let after = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    f.commit(Transaction(vec![
        Operation::SetName {
            id: t,
            name: "Label".into(),
        },
        Operation::SetOpacity {
            id: t,
            opacity: 0.5,
        },
    ]));

    let res = f.resolved();
    let was = res
        .world_bounds(t)
        .expect("Inter is bundled, so this shapes");
    let (tx, made) =
        build::outline_text(&f.doc, &res, &mut f.ids, t).expect("two letters leave ink");
    f.commit(tx);
    let res = f.resolved();

    // The layer is gone and a path stands in its slot, between the two rects.
    assert!(
        f.doc.get(t).is_none(),
        "the text layer is replaced, not kept"
    );
    assert!(
        matches!(f.doc.get(made).unwrap().kind(), NodeKind::Path { .. }),
        "the result is a path"
    );
    assert_eq!(
        f.doc.get(f.artboard).unwrap().children(),
        &[before, made, after],
        "in the slot the text held, so nothing above or below it moved"
    );
    // Everything about the layer that is not its geometry survives.
    let node = f.doc.get(made).unwrap();
    assert_eq!(node.name(), "Label");
    assert_eq!(node.opacity(), 0.5);
    assert_eq!(node.transform(), Affine::translate((20.0, 20.0)));

    // The space, which is the assertion this verb is actually about.
    let now = res.world_bounds(made).expect("a path has bounds");
    let slack = 16.0 * 0.1; // a tenth of the em — overshoot, not an offset
    assert!(
        now.x0 >= was.x0 - slack
            && now.y0 >= was.y0 - slack
            && now.x1 <= was.x1 + slack
            && now.y1 <= was.y1 + slack,
        "the glyph ink must land inside the box the text occupied: {now:?} against \
         {was:?}"
    );
    // And it must be *ink*, not an empty box sitting politely inside the old one.
    assert!(
        now.width() > 1.0 && now.height() > 1.0,
        "the fixture: two letters at 16pt are bigger than this ({now:?})"
    );
}

/// **The two refusals, and the second is the reason `can_outline_text` exists.**
///
/// A non-text kind is the obvious one. The interesting one is text with nothing in
/// it: it is still `NodeKind::Text`, it still shapes to a layout, and it has no
/// contours — so a row gated on the *kind* would be live and then answer
/// `EmptyGeometry` after the click, which `context-menus.md` §3 names as the one
/// thing a menu must not do. Replacing it would be worse than the error: a `Path`
/// holding nothing draws nothing and can never be picked on the canvas again.
///
/// ⚠️ Flipped against gating on `matches!(kind, NodeKind::Text { .. })`, where the
/// empty node reports outlineable and the conversion then fails; and against
/// gating on the glyph *count*, which is the cheap proxy — a run of spaces has
/// glyphs and no contours, so the last case below is what rejects it.
#[test]
fn converting_refuses_a_non_text_kind_and_text_with_no_contours() {
    let mut f = Fixture::new();
    let r = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let with_content = |s: &str| match text_node() {
        NodeKind::Text {
            style,
            spans,
            para_spans,
            paragraph,
            block,
            sizing,
            ..
        } => NodeKind::Text {
            content: s.into(),
            style,
            spans,
            para_spans,
            paragraph,
            block,
            sizing,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        other => other,
    };
    let empty = f.add(f.artboard, with_content(""), (0.0, 0.0));
    let spaces = f.add(f.artboard, with_content("   "), (0.0, 0.0));
    let res = f.resolved();

    assert!(matches!(
        build::outline_text(&f.doc, &res, &mut f.ids, r),
        Err(OpError::WrongKindForOp)
    ));
    assert!(!build::can_outline_text(&res, r));

    for (id, what) in [(empty, "nothing typed"), (spaces, "only spaces")] {
        assert!(
            !build::can_outline_text(&res, id),
            "{what}: the row must be dim rather than fail after the click"
        );
        assert!(
            matches!(
                build::outline_text(&f.doc, &res, &mut f.ids, id),
                Err(OpError::EmptyGeometry)
            ),
            "{what}"
        );
    }
}

/// **A group inside a boolean is one operand, and that is what it is for.**
///
/// Noticed as a possible slip — entering a boolean, selecting two operands and
/// pressing Ctrl+G really does group them — and it is a feature: a group contributes
/// the *union* of what is in it (`boolean::operand_of`), so grouping
/// re-parenthesises the operation. Figma allows it for the same reason.
///
/// Measured, because the claim is only interesting if the answer changes. Three
/// 100-wide bands over the same 100-tall strip, at x = 0, 50 and 80 — so the sets are
/// [0,100], [50,150] and [80,180]:
///
/// - ungrouped, `Exclude` folds left to right — `((a ⊻ b) ⊻ c)` — and leaves
///   [0,50] ∪ [80,100] ∪ [150,180], i.e. **100** wide;
/// - grouped, it is `a ⊻ (b ∪ c)` = [0,50] ∪ [100,180], i.e. **130** wide.
///
/// Subtract would have hidden this: `a − b − c` and `a − (b ∪ c)` are the same set,
/// which is exactly why the test uses Exclude. (The first numbers written here were
/// 120 and 150, from reading the third band as [80,200]; the areas came back 10000
/// and 13000 and the probe that printed the subpaths is what said which of the two
/// was wrong. flo_curves was right.)
#[test]
fn a_group_inside_a_boolean_is_a_single_operand() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(100.0, 100.0), (50.0, 0.0));
    let c = f.add(f.artboard, rect(100.0, 100.0), (80.0, 0.0));

    let (tx, node) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[a, b, c],
        ondin_core::BoolOp::Exclude,
        None,
    )
    .unwrap();
    f.commit(tx);
    // ⚠️ **Bands, not area, since `Exclude` became even-odd** (§15 D239). `net_area`
    // is `Shape::area`, a *winding* integral, and an exclusion's outline is now its
    // operands concatenated — so it reports 30,000 here, the three rects added up,
    // whatever the shape actually is. Testing the bands is exact where a sampled
    // area would only be close, and it pins the shape rather than its size: three
    // rects at [0,100], [50,150] and [80,180] give coverage 1,2,3,2,1 across x, so
    // the odd bands are [0,50), [80,100) and [150,180).
    let bands = |path: &ondin_core::kurbo::BezPath| -> Vec<bool> {
        [25.0, 65.0, 90.0, 125.0, 165.0]
            .iter()
            .map(|x| {
                ondin_core::FillRule::EvenOdd
                    .contains(path, ondin_core::kurbo::Point::new(*x, 50.0))
            })
            .collect()
    };
    let ungrouped = bands(f.resolved().boolean_path(node).expect("a result"));
    assert_eq!(
        ungrouped,
        vec![true, false, true, false, true],
        "three operands, folded left to right"
    );

    // Now group two of them *inside* the boolean, which is the act in question.
    let (tx, g) = build::group(&f.doc, &mut f.ids, &[b, c]).expect("group inside a boolean");
    f.commit(tx);
    assert_eq!(f.doc.get(g).unwrap().parent(), Some(node));
    assert_eq!(f.children_of(node), vec![a, g], "two operands now");

    // `a ⊻ (b ∪ c)`: the group unions to [50,180], so the odd bands are [0,50) and
    // [100,180) — the middle sample at 90 flips, which is the whole point.
    let grouped = bands(f.resolved().boolean_path(node).expect("a result"));
    assert_eq!(
        grouped,
        vec![true, false, false, true, true],
        "the group is one operand — its contents unioned first"
    );
    assert_ne!(
        grouped, ungrouped,
        "and that is a different shape, which is the whole use of it"
    );
}

/// **Several copies out of one parent land in the right slots whichever order they are
/// placed in.**
///
/// Each copy goes immediately above its own original, so two layers copied at once give
/// placements whose indices *descend* when the selection is walked top-down. A flat
/// per-parent count of "how many have I added" is right only for ascending indices — it
/// turned the second index into the first one's and stacked a copy above the wrong
/// sibling. Both orders are asserted because the ascending one passes with the bug in
/// place, which is exactly how it survived.
#[test]
fn several_copies_land_above_their_own_originals_in_either_order() {
    for top_down in [false, true] {
        let mut f = Fixture::new();
        let x = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
        let y = f.add(f.artboard, rect(10.0, 10.0), (20.0, 0.0));

        // Each placement is "immediately above this layer", as paste and Alt-drag now
        // compute it; the only difference between the two runs is the order they are
        // handed over in.
        let mut sources = [x, y];
        if top_down {
            sources.reverse();
        }
        let placements: Vec<build::Placement> = sources
            .iter()
            .map(|id| {
                let index = f
                    .doc
                    .get(f.artboard)
                    .unwrap()
                    .children()
                    .iter()
                    .position(|c| c == id)
                    .map(|i| i + 1);
                build::Placement {
                    nodes: f.doc.capture_subtree(*id).expect("capture"),
                    parent: f.artboard,
                    index,
                }
            })
            .collect();

        let (tx, created) = build::insert_subtrees(&f.doc, &mut f.ids, &placements, Vec2::ZERO);
        f.commit(tx);
        // `created` is in placement order, so name the copies by the source they came
        // from rather than by position.
        let copy_of: Vec<(NodeId, NodeId)> = sources.iter().copied().zip(created).collect();
        let of = |src: NodeId| copy_of.iter().find(|(s, _)| *s == src).unwrap().1;

        assert_eq!(
            f.children_of(f.artboard),
            vec![x, of(x), y, of(y)],
            "top_down={top_down}: each copy sits immediately above its own original"
        );
    }
}

// ---------------------------------------------------------------------------
// The key layer (§9.4)
// ---------------------------------------------------------------------------

/// The key becomes the base operand, and the rest keep their z-order under it.
///
/// **Two assertions in one, because either alone would pass on an accident.** The
/// key reaching index 0 is the feature; the *others* holding their relative order
/// is what says a three-operand subtract still cuts in the order the layer list
/// showed, rather than being permuted by whatever did the reordering.
#[test]
fn the_key_becomes_the_bottom_operand_of_a_subtract() {
    let mut f = Fixture::new();
    let bottom = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let middle = f.add(f.artboard, rect(100.0, 100.0), (20.0, 0.0));
    let top = f.add(f.artboard, rect(100.0, 100.0), (40.0, 0.0));

    let (tx, node) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[bottom, middle, top],
        ondin_core::BoolOp::Subtract,
        Some(top),
    )
    .expect("three rects are booleanable");
    f.commit(tx);

    assert_eq!(
        f.children_of(node),
        vec![top, bottom, middle],
        "the key goes to the bottom; the others keep their relative z-order"
    );
}

/// Without a key the operand order is the layer order, exactly as before — the
/// A-half of the test above, so "the key moved it" cannot pass against a builder
/// that reorders unconditionally.
#[test]
fn without_a_key_a_boolean_keeps_the_layer_order() {
    let mut f = Fixture::new();
    let bottom = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let top = f.add(f.artboard, rect(100.0, 100.0), (40.0, 0.0));

    let (tx, node) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[bottom, top],
        ondin_core::BoolOp::Subtract,
        None,
    )
    .unwrap();
    f.commit(tx);
    assert_eq!(f.children_of(node), vec![bottom, top]);
}

/// **The key is inert where the operation cannot see it.** Union, Intersect and
/// Exclude all fold commutatively, so honouring a key there would reorder the
/// layer list and hand the container a different member's paint while leaving the
/// shape identical — a designation that appeared to do something real.
#[test]
fn a_key_is_ignored_by_the_order_free_operations() {
    for op in [
        ondin_core::BoolOp::Union,
        ondin_core::BoolOp::Intersect,
        ondin_core::BoolOp::Exclude,
    ] {
        let mut f = Fixture::new();
        let bottom = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
        let top = f.add(f.artboard, rect(100.0, 100.0), (40.0, 0.0));

        let (tx, node) = build::boolean(&f.doc, &mut f.ids, &[bottom, top], op, Some(top)).unwrap();
        f.commit(tx);
        assert_eq!(
            f.children_of(node),
            vec![bottom, top],
            "{op:?} does not read operand order, so the key must not move anything"
        );
    }
}

/// The container inherits the *base* operand's paint, so designating a key
/// changes which paint that is. This is the other visible half of the reorder:
/// the shape changes because Subtract cuts the other way, and the colour follows
/// it, which is what makes the result look like the key rather than like a shape
/// that lost its fill.
#[test]
fn the_key_also_decides_the_paint_the_boolean_inherits() {
    use ondin_core::Brush;
    use ondin_core::Fill;
    use ondin_core::peniko::Color;
    let mut f = Fixture::new();
    let bottom = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let top = f.add(f.artboard, rect(100.0, 100.0), (40.0, 0.0));
    let red = Fill {
        brush: Brush::Solid(Color::from_rgba8(255, 0, 0, 255)),
        visible: true,
    };
    f.commit(Transaction(vec![Operation::SetFills {
        id: top,
        fills: vec![red.clone()],
    }]));

    let (tx, node) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[bottom, top],
        ondin_core::BoolOp::Subtract,
        Some(top),
    )
    .unwrap();
    f.commit(tx);
    assert_eq!(
        f.doc.get(node).unwrap().paint().fills,
        vec![red],
        "the key is the base, so its paint is the one carried up"
    );
}

/// A key that is not one of the members is not an error and not a silent
/// reordering — it simply does not apply. The app filters one out before calling,
/// but the builder is the last place that can be wrong about it.
#[test]
fn a_key_outside_the_members_changes_nothing() {
    let mut f = Fixture::new();
    let bottom = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let top = f.add(f.artboard, rect(100.0, 100.0), (40.0, 0.0));
    let bystander = f.add(f.artboard, rect(10.0, 10.0), (200.0, 200.0));

    let (tx, node) = build::boolean(
        &f.doc,
        &mut f.ids,
        &[bottom, top],
        ondin_core::BoolOp::Subtract,
        Some(bystander),
    )
    .unwrap();
    f.commit(tx);
    assert_eq!(f.children_of(node), vec![bottom, top]);
}

// --- layer names (§15 D127) ------------------------------------------------

fn names_in(doc: &Document, parent: NodeId) -> Vec<String> {
    doc.get(parent)
        .unwrap()
        .children()
        .iter()
        .map(|c| doc.get(*c).unwrap().name().to_string())
        .collect()
}

/// **`name: None` numbers within the parent**, so ten rectangles are not all
/// literally "Rectangle" — the reported state, and the one that leaves the MCP
/// snapshot and Command Mode's name fallback ambiguous.
///
/// **The first of a kind carries a number too.** It did not at first, and that was
/// reported: a bare "Rectangle" beside a "Rectangle 2" reads as though the first
/// one were somehow special, and it costs the layers panel its symmetry for
/// nothing.
#[test]
fn a_fresh_layer_is_numbered_among_its_new_siblings() {
    let mut f = Fixture::new();
    for _ in 0..3 {
        f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    }
    f.add(
        f.artboard,
        NodeKind::Ellipse {
            size: Size::new(5.0, 5.0),
        },
        (0.0, 0.0),
    );
    assert_eq!(
        names_in(&f.doc, f.artboard),
        ["Rectangle 1", "Rectangle 2", "Rectangle 3", "Ellipse 1"],
        "each kind is numbered on its own, from 1, with nothing left bare"
    );
}

/// **A freed number comes back**, which is what needs no stored counter — and a
/// persisted per-kind counter is the alternative that would silently reset on
/// reload, since nothing in the save format carries one.
#[test]
fn deleting_a_numbered_layer_frees_its_number_for_the_next_one() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    assert_eq!(f.doc.get(b).unwrap().name(), "Rectangle 2");

    f.commit(Transaction(vec![Operation::DeleteNode { id: b }]));
    let next = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    assert_eq!(
        f.doc.get(next).unwrap().name(),
        "Rectangle 2",
        "the gap left by a delete is the number the next layer takes"
    );
    // And the number is per parent, not per document: 1 is free again inside a
    // sibling frame.
    let other = f.add(f.root, artboard(), (0.0, 0.0));
    let inside = f.add(other, rect(10.0, 10.0), (0.0, 0.0));
    assert_eq!(f.doc.get(inside).unwrap().name(), "Rectangle 1");
    assert_eq!(f.doc.get(a).unwrap().name(), "Rectangle 1");
}

/// **Undo and redo have to land on the same name.** The name is resolved from the
/// parent's children when the op runs, so a redo re-resolves it — and it has to
/// re-resolve to the same string, or a layer would rename itself for having been
/// undone.
#[test]
fn redoing_a_create_resolves_the_same_name_it_did_the_first_time() {
    let mut f = Fixture::new();
    f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let second = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    assert_eq!(f.doc.get(second).unwrap().name(), "Rectangle 2");

    f.hist.undo(&mut f.doc).expect("undo");
    assert_eq!(names_in(&f.doc, f.artboard), ["Rectangle 1"]);
    f.hist.redo(&mut f.doc).expect("redo");
    assert_eq!(
        names_in(&f.doc, f.artboard),
        ["Rectangle 1", "Rectangle 2"],
        "the redone create must resolve to the name it resolved to before"
    );
}

/// **A copy keeps a typed name and increments a numbered one**, and the two halves
/// are one rule: the app owns the names it generated, the user owns the ones they
/// typed. Ten layers called "Dot" in a chart is correct; renaming them would be
/// hostile.
#[test]
fn a_duplicate_keeps_a_typed_name_and_increments_a_numbered_one() {
    let mut f = Fixture::new();
    let generated = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let typed = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    f.commit(Transaction(vec![Operation::SetName {
        id: typed,
        name: "Dot".into(),
    }]));

    for source in [generated, typed] {
        let template = f.doc.capture_subtree(source).expect("subtree");
        let (tx, _) = build::insert_subtrees(
            &f.doc,
            &mut f.ids,
            &[build::Placement {
                nodes: template,
                parent: f.artboard,
                index: None,
            }],
            Vec2::ZERO,
        );
        f.commit(tx);
    }
    assert_eq!(
        names_in(&f.doc, f.artboard),
        ["Rectangle 1", "Dot", "Rectangle 2", "Dot"],
        "the generated name numbered and the typed one was left alone"
    );
}

/// **Several copies in one transaction have to see each other.** Each name is
/// decided before its op is built and nothing in the transaction has been applied
/// yet, so without `build::CopyNames` all three would ask the same unchanged
/// document and all three would come back "Rectangle 2" — verified by neutering the
/// accumulator, which prints exactly that.
#[test]
fn three_copies_in_one_transaction_get_three_different_numbers() {
    let mut f = Fixture::new();
    let source = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let template = f.doc.capture_subtree(source).expect("subtree");
    let placements: Vec<_> = (0..3)
        .map(|_| build::Placement {
            nodes: template.clone(),
            parent: f.artboard,
            index: None,
        })
        .collect();
    let (tx, made) = build::insert_subtrees(&f.doc, &mut f.ids, &placements, Vec2::ZERO);
    f.commit(tx);
    assert_eq!(made.len(), 3);
    assert_eq!(
        names_in(&f.doc, f.artboard),
        ["Rectangle 1", "Rectangle 2", "Rectangle 3", "Rectangle 4"]
    );
}

/// **A copied group's children keep their names**, because their parent is the new
/// group and they are unique within it. Renaming down the subtree would make a
/// duplicated component read as a renumbered stranger.
#[test]
fn duplicating_a_group_renames_the_group_and_nothing_inside_it() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (20.0, 0.0));
    let (tx, group) = build::group(&f.doc, &mut f.ids, &[a, b]).unwrap();
    f.commit(tx);
    assert_eq!(names_in(&f.doc, group), ["Rectangle 1", "Rectangle 2"]);

    let template = f.doc.capture_subtree(group).expect("subtree");
    let (tx, made) = build::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[build::Placement {
            nodes: template,
            parent: f.artboard,
            index: None,
        }],
        Vec2::ZERO,
    );
    f.commit(tx);
    let copy = made[0];
    assert_eq!(f.doc.get(copy).unwrap().name(), "Group 2");
    assert_eq!(
        names_in(&f.doc, copy),
        ["Rectangle 1", "Rectangle 2"],
        "the children are numbered within their own new parent, so nothing moved"
    );
}

/// **Numbering never runs again after creation, and reparenting is allowed to
/// collide.** Recomputing names on move would rename layers behind the user's
/// back, which is worse than a repeated name in a panel that shows the tree.
///
/// This is the assertion that keeps uniqueness from becoming load-bearing: two
/// siblings with one name is a legal document, and everything downstream addresses
/// by `NodeId`.
#[test]
fn reparenting_into_a_collision_leaves_both_names_alone() {
    let mut f = Fixture::new();
    let other = f.add(f.root, artboard(), (400.0, 0.0));
    let here = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let there = f.add(other, rect(10.0, 10.0), (0.0, 0.0));
    assert_eq!(f.doc.get(here).unwrap().name(), "Rectangle 1");
    assert_eq!(f.doc.get(there).unwrap().name(), "Rectangle 1");

    let res = f.resolved();
    let at = f.children_of(f.artboard).len();
    f.commit(build::reparent_preserving_world(&f.doc, &res, there, f.artboard, at).unwrap());
    assert_eq!(
        names_in(&f.doc, f.artboard),
        ["Rectangle 1", "Rectangle 1"],
        "a move must not rename anything, even into a collision"
    );
}

/// **A builder that creates and deletes in one transaction must number against
/// the survivors.** `flatten_union` emits its `CreateNode` before the deletes, so
/// numbering at apply time counted the very layers being replaced: flattening a
/// path called "Path" produced "Path 2" and then took the "Path" away, leaving a
/// number 2 with nothing above it.
#[test]
fn flattening_numbers_the_result_against_the_layers_that_survive() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(100.0, 100.0), (40.0, 0.0));
    // Both operands are called "Path", so a name resolved against them would land
    // on 3 — the loudest form of the bug.
    for id in [a, b] {
        f.commit(Transaction(vec![Operation::SetName {
            id,
            name: "Path".into(),
        }]));
    }
    let res = f.resolved();
    let (tx, _) = build::flatten(&f.doc, &res, &mut f.ids, &[a, b]).unwrap();
    f.commit(tx);
    assert_eq!(
        names_in(&f.doc, f.artboard),
        ["Path 1"],
        "the operands are gone, so the result takes the first number in the series"
    );
}

/// **Switching a boolean's operation still renames a name we wrote, now that ours
/// always carries a number.** The old test compared against the four labels exactly,
/// so every boolean — "Union 1" from the very first one — read as user-named and kept
/// a row saying "Union 1" over an intersection.
///
/// The number comes across rather than being re-derived, so flipping the operation
/// back and forth does not walk it.
#[test]
fn a_numbered_boolean_is_still_renamed_when_its_operation_changes() {
    let mut f = Fixture::new();
    let make_boolean = |f: &mut Fixture| {
        let a = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
        let b = f.add(f.artboard, rect(100.0, 100.0), (40.0, 0.0));
        let (tx, node) =
            build::boolean(&f.doc, &mut f.ids, &[a, b], ondin_core::BoolOp::Union, None).unwrap();
        f.commit(tx);
        node
    };
    let first = make_boolean(&mut f);
    let second = make_boolean(&mut f);
    assert_eq!(f.doc.get(first).unwrap().name(), "Union 1");
    assert_eq!(f.doc.get(second).unwrap().name(), "Union 2");

    f.commit(build::set_boolean_op(&f.doc, second, ondin_core::BoolOp::Intersect).unwrap());
    assert_eq!(
        f.doc.get(second).unwrap().name(),
        "Intersect 2",
        "the number rides along; only the label changes"
    );
    // Back again, and it lands where it started rather than a number further on.
    f.commit(build::set_boolean_op(&f.doc, second, ondin_core::BoolOp::Union).unwrap());
    assert_eq!(f.doc.get(second).unwrap().name(), "Union 2");

    // A name the user typed is never overwritten, numbered or not.
    f.commit(Transaction(vec![Operation::SetName {
        id: second,
        name: "Badge 2".into(),
    }]));
    f.commit(build::set_boolean_op(&f.doc, second, ondin_core::BoolOp::Exclude).unwrap());
    assert_eq!(f.doc.get(second).unwrap().name(), "Badge 2");
}

// --- a copied picture carries its bytes -----------------------------------

/// A node holds an image *key*; the bytes live in a table on the document. So a
/// captured subtree is half a picture, and pasting it into a document whose table
/// has no such key resolves to nothing — which the scene walk draws as the
/// missing-picture placeholder.
///
/// **The bug this pins was silent and cross-document**: copy an image layer, open
/// another document, paste. The clipboard deliberately survives an open (a copy
/// between documents is a feature), so the nodes arrived and the photograph did
/// not. `build::image_ids_in` and `build::missing_image_ops` are the two halves of
/// carrying it.
///
/// ⚠️ The last assertion is the flip, kept in the test rather than run by hand:
/// inserting the same subtrees *without* the ops reproduces the bug, so a change
/// that made `missing_image_ops` return nothing would fail here rather than pass
/// quietly.
#[test]
fn a_copied_picture_arrives_in_a_document_that_never_had_it() {
    use ondin_core::{Fill, ImageEntry, ImageFormat, ImageId, ImageSource, image_brush};

    let key = ImageId("hash-of-the-photo".to_string());
    let entry = ImageEntry {
        source: ImageSource::Embedded(vec![1, 2, 3, 4].into()),
        format: ImageFormat::Png,
        width: 20,
        height: 10,
    };

    // The document the copy is taken from: one rect wearing the picture.
    let mut from = Fixture::new();
    let shape = from.add(from.artboard, rect(20.0, 10.0), (0.0, 0.0));
    from.commit(Transaction(vec![
        Operation::AddImage {
            id: key.clone(),
            entry: entry.clone(),
        },
        Operation::SetFills {
            id: shape,
            fills: vec![Fill {
                brush: image_brush(key.clone()),
                visible: true,
            }],
        },
    ]));
    assert!(from.doc.has_image(&key), "the fixture is in the state");

    // What a copy takes: the nodes, and the entries they key into.
    let template = from.doc.capture_subtree(shape).unwrap();
    let held: Vec<(ImageId, ImageEntry)> = build::image_ids_in(&template)
        .into_iter()
        .filter_map(|id| Some((id.clone(), from.doc.image(&id)?.clone())))
        .collect();
    assert_eq!(
        held.len(),
        1,
        "one picture referenced, and the fill is where it was found"
    );

    // A different document, which has never seen this photograph.
    let mut into = Fixture::new();
    assert!(!into.doc.has_image(&key));
    let placements = vec![build::Placement {
        nodes: template.clone(),
        parent: into.artboard,
        index: None,
    }];
    let (tx, created) = build::insert_subtrees(&into.doc, &mut into.ids, &placements, Vec2::ZERO);
    let mut ops = build::missing_image_ops(&into.doc, &held);
    assert_eq!(ops.len(), 1, "the target is missing it, so it is carried");
    ops.extend(tx.0);
    into.commit(Transaction(ops));

    assert!(
        into.doc.has_image(&key),
        "the picture did not travel with the layer that uses it"
    );
    let pasted = created[0];
    assert_eq!(
        into.doc.get(pasted).unwrap().paint().fills.len(),
        1,
        "and the pasted layer still wears it"
    );

    // Idempotent: pasting again into the *same* document adds nothing, which is
    // what makes this safe to run on every paste rather than only the interesting
    // one. Ids are content hashes, so the second copy is recognised, not stored.
    assert!(
        build::missing_image_ops(&into.doc, &held).is_empty(),
        "a document that already holds the picture must not be given it twice"
    );

    // ⚠️ The flip: the same paste with the ops left out is the reported bug.
    let mut bare = Fixture::new();
    let (tx, _) = build::insert_subtrees(
        &bare.doc,
        &mut bare.ids,
        &[build::Placement {
            nodes: template,
            parent: bare.artboard,
            index: None,
        }],
        Vec2::ZERO,
    );
    bare.commit(tx);
    assert!(
        !bare.doc.has_image(&key),
        "without the carried entries this cannot fail, and the test would prove nothing"
    );
}

/// **Masking two or more layers wraps them**, and the mask lands at the bottom of
/// the wrapper.
///
/// The wrapping is the whole reason this is a builder rather than a `SetMask`: a
/// mask and its subjects stay independent layers, so without a container the user
/// re-selects the set by hand every time they want to move the picture and its
/// mask together. Both halves are asserted — that a group appeared where the
/// members were, and that the mask is child 0 of it, which is the only position
/// from which it clips them.
#[test]
fn masking_a_pair_wraps_them_in_a_group_with_the_mask_at_the_bottom() {
    let mut f = Fixture::new();
    let circle = f.add(f.artboard, rect(40.0, 40.0), (0.0, 0.0));
    let photo = f.add(f.artboard, rect(300.0, 200.0), (0.0, 0.0));
    let bystander = f.add(f.artboard, rect(10.0, 10.0), (500.0, 0.0));
    assert_eq!(
        f.children_of(f.artboard),
        vec![circle, photo, bystander],
        "fixture: bottom-first, and a third layer that must not be swept in"
    );

    let (tx, made) =
        build::mask(&f.doc, &mut f.ids, &[circle, photo], None).expect("two layers mask");
    f.commit(tx);

    assert_eq!(
        f.children_of(f.artboard),
        vec![made, bystander],
        "the pair became one layer, in the slot the topmost of them held"
    );
    assert!(matches!(f.doc.get(made).unwrap().kind(), NodeKind::Group));
    assert_eq!(
        f.children_of(made),
        vec![circle, photo],
        "and inside it the mask is at the bottom, where it clips what is above"
    );
    assert!(
        f.doc.get(circle).unwrap().mask(),
        "the lower one is the mask"
    );
    assert!(!f.doc.get(photo).unwrap().mask());
    assert!(!f.doc.get(bystander).unwrap().mask());

    f.hist.undo(&mut f.doc).expect("one undo");
    assert_eq!(
        f.children_of(f.artboard),
        vec![circle, photo, bystander],
        "and it is one undo step, group and flag together"
    );
    assert!(!f.doc.get(circle).unwrap().mask());
}

/// **The key layer becomes the mask and moves to the bottom**, and everything else
/// keeps its relative z-order.
///
/// The same `Alt+Shift` designation a `Subtract`'s base operand uses, doing the
/// same thing to the order — so the second assertion is the one that matters:
/// moving the key to index 0 must be the *smallest* rearrangement, not a sort.
/// Keyed on the **top** layer deliberately, since that is the case the default
/// (bottom-most) would answer differently.
#[test]
fn the_key_layer_becomes_the_mask_and_the_rest_keep_their_order() {
    let mut f = Fixture::new();
    let low = f.add(f.artboard, rect(30.0, 30.0), (0.0, 0.0));
    let mid = f.add(f.artboard, rect(30.0, 30.0), (10.0, 0.0));
    let top = f.add(f.artboard, rect(30.0, 30.0), (20.0, 0.0));

    let (tx, made) =
        build::mask(&f.doc, &mut f.ids, &[low, mid, top], Some(top)).expect("three layers mask");
    f.commit(tx);

    assert_eq!(
        f.children_of(made),
        vec![top, low, mid],
        "the key went to the bottom and low/mid kept their order relative to each other"
    );
    assert!(f.doc.get(top).unwrap().mask(), "the key is the mask");
    assert!(
        !f.doc.get(low).unwrap().mask(),
        "and the default one is not"
    );
}

/// **One layer takes no group**, because there is nothing to wrap it with — and
/// grouping it alone would change *which* layers it masks, a group being a new
/// parent.
#[test]
fn masking_a_lone_layer_flags_it_where_it_stands() {
    let mut f = Fixture::new();
    let low = f.add(f.artboard, rect(30.0, 30.0), (0.0, 0.0));
    let above = f.add(f.artboard, rect(30.0, 30.0), (0.0, 0.0));

    let (tx, made) = build::mask(&f.doc, &mut f.ids, &[low], None).expect("one layer masks");
    f.commit(tx);

    assert_eq!(
        made, low,
        "the layer itself is what to select, not a wrapper"
    );
    assert_eq!(
        f.children_of(f.artboard),
        vec![low, above],
        "nothing was wrapped and nothing moved"
    );
    assert!(f.doc.get(low).unwrap().mask());
    assert_eq!(
        f.doc.governing_mask(above),
        Some(low),
        "and it masks the sibling it already had, which is the point of not grouping"
    );
}

/// A frame cannot be a mask, and neither can anything inside a boolean — the two
/// refusals `build::mask` owns rather than inherits from `group`.
#[test]
fn mask_refuses_a_frame_and_a_booleans_operands() {
    let mut f = Fixture::new();
    let inner_frame = f.add(f.artboard, artboard(), (0.0, 0.0));
    assert!(
        matches!(
            build::mask(&f.doc, &mut f.ids, &[inner_frame], None),
            Err(OpError::WrongKindForOp)
        ),
        "a frame is a page and already clips its own contents"
    );

    let a = f.add(f.artboard, rect(30.0, 30.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(30.0, 30.0), (10.0, 0.0));
    let (tx, boolean) =
        build::boolean(&f.doc, &mut f.ids, &[a, b], ondin_core::BoolOp::Union, None).unwrap();
    f.commit(tx);
    let operands = f.children_of(boolean);
    assert_eq!(operands.len(), 2, "fixture: the boolean took both");
    assert!(
        matches!(
            build::mask(&f.doc, &mut f.ids, &operands[..1], None),
            Err(OpError::WrongKindForOp)
        ),
        "operands are combined rather than drawn, so a mask there would do nothing"
    );
}

/// Put a picture on `id`, so `Paint::image_fill` says yes about it.
fn make_it_a_picture(f: &mut Fixture, id: NodeId) {
    use ondin_core::{Fill, ImageEntry, ImageFormat, ImageId, ImageSource, image_brush};
    let key = ImageId(format!("photo-{id:?}"));
    f.commit(Transaction(vec![
        Operation::AddImage {
            id: key.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(vec![1, 2, 3, 4].into()),
                format: ImageFormat::Png,
                width: 20,
                height: 10,
            },
        },
        Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: image_brush(key),
                visible: true,
            }],
        },
    ]));
}

/// **A shape beats a photograph, whichever is lower.**
///
/// The default was plain bottom-most, and it got this exact case wrong the
/// ordinary way round: drop a photo, draw a circle over it, mask — the photo is
/// underneath, so it became the mask and cropped everything to its rectangle. The
/// picture is in the **bottom** slot here deliberately, since that is the case the
/// old rule answers differently; with the shape at the bottom both rules agree and
/// the test would prove nothing.
#[test]
fn a_shape_is_preferred_over_a_picture_however_they_are_stacked() {
    let mut f = Fixture::new();
    let photo = f.add(f.artboard, rect(300.0, 200.0), (0.0, 0.0));
    let circle = f.add(f.artboard, rect(80.0, 80.0), (0.0, 0.0));
    make_it_a_picture(&mut f, photo);
    assert_eq!(
        f.children_of(f.artboard),
        vec![photo, circle],
        "fixture: the picture is the lower of the two"
    );

    assert_eq!(
        build::mask_target(&f.doc, &[photo, circle], None),
        Some(circle),
        "the shape masks the photograph, not the other way round"
    );

    let (tx, made) = build::mask(&f.doc, &mut f.ids, &[photo, circle], None).expect("masks");
    f.commit(tx);
    assert_eq!(
        f.children_of(made),
        vec![circle, photo],
        "and the shape moved under the photograph, which is where a mask sits"
    );
    assert!(f.doc.get(circle).unwrap().mask());
}

/// **The key still wins, because it is the user saying they meant it.**
///
/// Masking with a photograph is a real thing to want — you get its box — and
/// `Alt+Shift` is how that is said. Refusing an image outright would be defensible
/// and is worse: the gesture would silently do nothing on the one selection where
/// the user had been most explicit.
#[test]
fn keying_the_picture_makes_it_the_mask_anyway() {
    let mut f = Fixture::new();
    let photo = f.add(f.artboard, rect(300.0, 200.0), (0.0, 0.0));
    let circle = f.add(f.artboard, rect(80.0, 80.0), (0.0, 0.0));
    make_it_a_picture(&mut f, photo);

    assert_eq!(
        build::mask_target(&f.doc, &[photo, circle], Some(photo)),
        Some(photo),
        "the designation overrides the preference, as it overrides z-order"
    );
    let (tx, made) = build::mask(&f.doc, &mut f.ids, &[photo, circle], Some(photo)).expect("masks");
    f.commit(tx);
    assert_eq!(f.children_of(made), vec![photo, circle]);
    assert!(f.doc.get(photo).unwrap().mask());
}

/// With **nothing but pictures** there is no better answer, so it falls back to
/// bottom-most rather than refusing.
#[test]
fn two_pictures_fall_back_to_the_bottom_most() {
    let mut f = Fixture::new();
    let lower = f.add(f.artboard, rect(300.0, 200.0), (0.0, 0.0));
    let upper = f.add(f.artboard, rect(300.0, 200.0), (0.0, 0.0));
    make_it_a_picture(&mut f, lower);
    make_it_a_picture(&mut f, upper);

    assert_eq!(
        build::mask_target(&f.doc, &[lower, upper], None),
        Some(lower),
        "one photograph cropping another is odd but it is what was asked for"
    );
}

/// **The Text tool's click on an outline makes a railed text node in the shape's
/// own slot, and consumes the shape** (§15 D408).
///
/// Four claims, and the last two are the ones a careless implementation gets
/// wrong:
///
/// - the new node carries the shape's outline as its rail, so typing follows the
///   curve that was clicked rather than a straight line beside it;
/// - the shape is gone, which is what makes this the same destructive bargain
///   *Text on path* already makes;
/// - **the rail needs no rebasing**, because the node is created in the space the
///   curve is already in — asserted by giving the shape a transform the fixture's
///   parent does not share, so a version that dropped it would land the type
///   somewhere else;
/// - **and the new layer takes the consumed one's z-slot.** A rail is usually
///   drawn as part of an arrangement, and sending the result to the front would
///   reorder the drawing as a side effect of typing on it.
#[test]
fn clicking_an_outline_makes_a_railed_node_in_the_shapes_own_slot() {
    let mut f = Fixture::new();
    // Three siblings, so "the middle one's slot" is a real place rather than
    // either end — the index a careless append would visibly miss.
    let under = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let shape = f.add(f.artboard, ellipse(80.0, 40.0), (120.0, 70.0));
    let over = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));

    let at = f.doc.get(shape).unwrap().transform();
    let outline = ondin_core::geometry::local_path(f.doc.get(shape).unwrap().kind()).unwrap();
    let parts = ondin_core::TextParts::default();

    // The click, in **world** space: the ellipse spans (0,0)–(80,40) in its own
    // space, sits at (120, 70) inside an artboard at (50, 30), so its leftmost
    // point is at (170, 120) in the world. Clicking there is what the offset
    // assertion below is about, and the nesting is what makes the world/local
    // distinction observable — a version reading `node.transform()` alone is out
    // by the artboard.
    let res = Resolved::rebuild(&f.doc);
    let (tx, made) = ondin_core::build::text_on_new_path(
        &f.doc,
        &res,
        &mut f.ids,
        shape,
        &parts,
        Point::new(170.0, 120.0),
    )
    .expect("a rail");
    f.doc.apply(&tx).expect("the transaction applies");

    let node = f.doc.get(made).expect("the text node exists");
    let NodeKind::Text {
        on_path: Some(rail),
        on_path_offset,
        content,
        ..
    } = node.kind()
    else {
        panic!("the new node is not railed text");
    };
    // **The type starts where the click landed** (§15 D409). kurbo's ellipse begins
    // at its rightmost point and runs clockwise, so the leftmost point — where the
    // click was — is half way round: 0.5 of the rail's length.
    //
    // ⚠️ **This is the assertion the world/local confusion fails**, and it fails
    // *quietly*: with the artboard's (50, 30) left in, the click lands somewhere
    // else on the ellipse entirely and the offset comes back a plausible-looking
    // number rather than an error.
    assert!(
        (on_path_offset - 0.5).abs() < 0.01,
        "the text starts where the pointer was, half way round: {on_path_offset}"
    );
    assert!(content.is_empty(), "it is empty and waiting to be typed in");
    assert_eq!(
        rail.to_svg(),
        outline.to_svg(),
        "the rail is the shape's own outline, unchanged"
    );
    assert_eq!(
        node.transform(),
        at,
        "and it is placed in the shape's own space, so nothing had to be rebased"
    );
    assert!(f.doc.get(shape).is_none(), "the shape was consumed");
    assert_eq!(
        f.doc.get(f.artboard).unwrap().children(),
        &[under, made, over],
        "the text takes the shape's slot rather than going to the front"
    );
}

/// **The way back gives the curve back, unchanged, in the space the text is in**
/// (`[S3.1-L6-04]`, §15 D498).
///
/// ⚠️ **`build::detach_text_path` had no test caller anywhere in the workspace.**
/// Its outbound twin above has four stated claims and a ⚠️ on the one that fails
/// quietly; the return half of the same two-way door had none, and its one
/// production caller is a `&mut self` inspector method no test can reach (§15
/// D269) — so the builder was the only testable seam and nothing was on it.
///
/// The round trip is what makes the assertion strong: the rail that comes back is
/// compared against the outline that went in, not against a rebuilt one, so
/// *"unchanged"* is the claim rather than *"plausible"*.
///
/// Four decisions ride on it and each is asserted:
///
/// - **the rail comes back identical** — `to_svg()` against the outline the
///   ellipse gave `text_on_new_path`;
/// - **the new `Path` carries the text node's transform**, because the rail was
///   stored in that node's space. `Affine::IDENTITY` here would put the curve
///   somewhere the text has never been, and is `text_on_new_path`'s own quiet
///   failure arriving from the opposite direction;
/// - **it lands directly above the text**, at `at + 1` — `at` or `at + 2` is the
///   off-by-one the outbound test was written to catch on the other side;
/// - **`on_path_offset` survives while `on_path` goes to `None`.** Inert today,
///   since nothing else can set `on_path`, and worth pinning before something can:
///   re-railing a node that remembers where it used to start is a different
///   picture from one that starts at the seam.
///
/// Both refusals are exercised too, and they are worth separating because they
/// answer the **same** `OpError` for different reasons: a node that is not text at
/// all, and a text node that is simply not on a rail.
///
/// **Two flips run.** `transform: Some(node.transform())` replaced with
/// `Some(Affine::IDENTITY)` fails on the transform assertion — and *only* that
/// one, the rail's own `to_svg` being untouched by where the layer sits, which is
/// why the two are asserted separately. `index: at + 1` replaced with `at` fails
/// on the sibling order at `[under, freed, made, over]`.
#[test]
fn detaching_a_railed_node_gives_the_curve_back_above_the_text() {
    let mut f = Fixture::new();
    let under = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let shape = f.add(f.artboard, ellipse(80.0, 40.0), (120.0, 70.0));
    let over = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));

    let at = f.doc.get(shape).unwrap().transform();
    let outline = ondin_core::geometry::local_path(f.doc.get(shape).unwrap().kind()).unwrap();
    let res = Resolved::rebuild(&f.doc);
    let (tx, made) = ondin_core::build::text_on_new_path(
        &f.doc,
        &res,
        &mut f.ids,
        shape,
        &ondin_core::TextParts::default(),
        Point::new(170.0, 120.0),
    )
    .expect("a rail");
    f.commit(tx);

    let (tx, freed) =
        ondin_core::build::detach_text_path(&f.doc, &mut f.ids, made).expect("it detaches");
    f.commit(tx);

    let path = f.doc.get(freed).expect("the freed layer exists");
    let NodeKind::Path { path: curve, .. } = path.kind() else {
        panic!("the rail came back as something other than a path");
    };
    assert_eq!(
        curve.to_svg(),
        outline.to_svg(),
        "the curve that comes back is the one that went in"
    );
    assert_eq!(
        path.transform(),
        at,
        "and it is in the text node's space, which is the space the rail was stored in"
    );
    assert_eq!(
        f.children_of(f.artboard),
        vec![under, made, freed, over],
        "directly above the text, where a path drawn by hand would have been"
    );

    let NodeKind::Text {
        on_path,
        on_path_offset,
        ..
    } = f.doc.get(made).expect("the text is still there").kind()
    else {
        panic!("the text stopped being text");
    };
    assert!(on_path.is_none(), "and the text is off its rail");
    assert!(
        (on_path_offset - 0.5).abs() < 0.01,
        "the offset is deliberately left set: {on_path_offset}"
    );

    // The two refusals, which share an `OpError` and not a reason.
    let plain = f.add(f.artboard, text_node(), (0.0, 0.0));
    assert!(matches!(
        ondin_core::build::detach_text_path(&f.doc, &mut f.ids, plain),
        Err(OpError::WrongKindForOp)
    ));
    assert!(matches!(
        ondin_core::build::detach_text_path(&f.doc, &mut f.ids, under),
        Err(OpError::WrongKindForOp)
    ));
}

/// **A boolean's operand is not a rail, and is not even offered as one.**
///
/// `[S3.1-L1-02]`. The fixture is the ordinary geometry rather than a contrived
/// one: two rects overlapping, combined with `Subtract`, so the subtrahend's edge
/// lies **exactly on the result's silhouette** — which is what a boolean *is*.
/// A Text-tool click on that edge used to pick the operand, and
/// `text_on_new_path` consumes what it is handed.
///
/// Three things happened at once and none was announced: the operand was
/// **deleted**, so a two-operand subtract silently became its base; a `Text` node
/// was installed **inside** the `Boolean`, where `scene.rs` returns before the
/// children loop and so never paints it; and the status line said *"the shape
/// became the rail"*. The caret was live over a blank page and `Ctrl+Z` was the
/// only way back, with nothing saying so.
///
/// ⚠️ **Two guards, and the test asserts them separately because they fail
/// separately.** `build::text_on_new_path` refuses — the door every caller comes
/// through, and necessary because `can_parent(Boolean, Text)` is `true`, so
/// `Document::apply` has no reason of its own to say no. `query::outline_at`
/// skips — so the affordance never lights up and the user is not offered a
/// gesture that would be refused. Neither covers the other: without the builder
/// guard an MCP call or a future caller walks straight in; without the pick guard
/// the user gets a live rail cursor over an edge that then does nothing.
///
/// ⚠️ **Flipped** per guard. Removing the `text_on_new_path` refusal fails on the
/// `Err(WrongKindForOp)` assertion. Removing the `outline_at` skip fails on the
/// pick assertion with the operand's id — and **leaves the builder assertion
/// green**, which is what says the two are independent rather than one written
/// twice.
#[test]
fn a_boolean_operand_is_neither_offered_as_a_rail_nor_accepted_as_one() {
    let mut f = Fixture::new();
    // Two 100×100 rects, the second overlapping the first from x=60. Subtract
    // leaves the strip 0..60, whose right edge is the subtrahend's left edge.
    let back = f.add(f.artboard, rect(100.0, 100.0), (0.0, 0.0));
    let front = f.add(f.artboard, rect(100.0, 100.0), (60.0, 0.0));

    let (tx, b) = ondin_core::build::boolean(
        &f.doc,
        &mut f.ids,
        &[front, back],
        ondin_core::BoolOp::Subtract,
        None,
    )
    .expect("two rects are booleanable");
    f.commit(tx);
    let operands = f.children_of(b);
    assert_eq!(operands.len(), 2, "fixture: the boolean has two operands");

    let res = Resolved::rebuild(&f.doc);

    // The builder refuses the operand outright.
    let parts = ondin_core::TextParts::default();
    let err = ondin_core::build::text_on_new_path(
        &f.doc,
        &res,
        &mut f.ids,
        operands[1],
        &parts,
        Point::new(110.0, 80.0),
    )
    .expect_err("an operand may not become a rail");
    assert!(
        matches!(err, ondin_core::OpError::WrongKindForOp),
        "refused as the wrong kind, the way `mask` refuses inside a boolean: {err:?}"
    );

    // And the pick never offers it. The click is on the shared edge, in world
    // space — the artboard's own offset included, since `outline_at` works there.
    let edge = f.world_origin(front) + kurbo::Vec2::new(0.0, 30.0);
    let picked = ondin_core::query::outline_at(&res, &f.doc, edge, 4.0);
    assert!(
        picked != Some(operands[0]) && picked != Some(operands[1]),
        "the pick must not answer an operand: {picked:?}"
    );

    // The control: an ordinary sibling outline is still offered and still
    // accepted, so this is a test about booleans and not about `outline_at`
    // having stopped working.
    let loose = f.add(f.artboard, ellipse(80.0, 40.0), (400.0, 400.0));
    let res = Resolved::rebuild(&f.doc);
    // Its leftmost point, not its origin: the ellipse spans (0,0)–(80,40) in its
    // own space, so the outline passes through origin + (0, 20).
    let left = f.world_origin(loose) + kurbo::Vec2::new(0.0, 20.0);
    assert_eq!(
        ondin_core::query::outline_at(&res, &f.doc, left, 4.0),
        Some(loose),
        "a plain shape is still a rail candidate"
    );
    assert!(
        ondin_core::build::text_on_new_path(&f.doc, &res, &mut f.ids, loose, &parts, left).is_ok(),
        "and still becomes one"
    );
}

/// **A frame is not a rail either, and the ancestry guard does not reach it.**
///
/// `[S3.1-L2-03]`, the other half of §15 D453 — filed as closed by the same
/// builder refusal and **not**: `arch-scribe` found it reading D453's brief
/// against the code. The guard that shipped tests the *parent's* kind, and a
/// frame's parent is a frame or the root, so a frame walks straight through it.
///
/// ⚠️ **The damage is a file that never opens again, and that is what this
/// asserts first.** `geometry::local_path` answers `Some` for an `Artboard`
/// deliberately (*"a frame has an outline, and it is its own frame"*, §15 D144),
/// and `text_on_new_path` read "has a local path" as "may be deleted" — true of
/// every other kind and false of this one. The emitted `DeleteNode` carries none
/// of the `RemoveGuide` ops `build::guides_of` exists to supply, so the frame
/// goes and its guide stays, and `io::load` refuses the saved bytes for ever:
/// *"guide … is scoped to …, which is not a frame in this document"*.
///
/// ⚠️ **Unreachable is not refused, which is why the guard is in core.** A click
/// cannot reach this today, because `query::outline_at` skips `Artboard` by kind
/// (§15 D408: a frame is a page, its edge is a boundary rather than a drawn
/// curve). That rule lives one layer above the builder that needs it, and the
/// MCP write surface, the command line and any second entrance added later all
/// arrive here directly — `build::mask`'s own argument for its refusal.
///
/// ⚠️ **Flipped** by deleting the `NodeKind::Artboard { .. } | NodeKind::Root`
/// arm: fails on the **reload** assertion, not on the error code — printing the
/// integrity error, which is the loss in one line. The boolean test above stays
/// green under that flip, and this one stays green under the boolean flip, which
/// is what says the ancestry guard and the kind guard are two guards.
#[test]
fn a_frame_is_not_a_rail_and_taking_one_would_strand_its_guides() {
    let mut f = Fixture::new();
    let inner = f.add(f.artboard, artboard(), (10.0, 10.0));
    let gid = ondin_core::GuideId(f.ids.mint());
    f.commit(Transaction(vec![Operation::AddGuide {
        guide: ondin_core::Guide {
            id: gid,
            axis: ondin_core::GuideAxis::Vertical,
            position: 20.0,
            color: None,
            owner: Some(inner),
        },
    }]));
    assert_eq!(f.doc.guides().len(), 1, "fixture: the guide is scoped");

    let res = f.resolved();
    let parts = ondin_core::TextParts::default();
    // On the frame's left edge: the artboard's outline is its own box, so its
    // local origin is a corner of it.
    let on_edge = f.world_origin(inner) + kurbo::Vec2::new(0.0, 20.0);
    let asked =
        ondin_core::build::text_on_new_path(&f.doc, &res, &mut f.ids, inner, &parts, on_edge);

    // **The loss before the mechanism.** If the builder ever accepts again, this
    // is what the user is left holding, so drive it rather than asserting only on
    // the error code.
    if let Ok((tx, _)) = &asked {
        f.commit(tx.clone());
        let bytes = ondin_core::io::save(&f.doc).expect("a document with a stranded guide saves");
        let reloaded = ondin_core::io::load(&bytes);
        assert!(
            reloaded.is_ok(),
            "consuming a frame as a rail leaves its guide owned by a node that is \
             gone, and the saved file never opens again: {:?}",
            reloaded.err()
        );
    }

    let err = asked.expect_err("a frame may not become a rail");
    assert!(
        matches!(err, ondin_core::OpError::WrongKindForOp),
        "refused as the wrong kind, beside the operand refusal: {err:?}"
    );
}

/// **`can_frame` agrees with `frame`, which is the whole reason it exists** —
/// §15 D661, `[S3.1-L6-05]`.
///
/// 🚨 **`can_frame` had zero test callers workspace-wide.** Its doc states its
/// contract in one sentence — *"the builder's own question, so a row cannot offer
/// what the verb would refuse"* — and nothing checked it. **One** `menu.rs` test
/// sets `cx.can_frame` by hand as a literal `bool`
/// (`frame_selection_is_offered_where_group_is_and_on_the_frames_group_refuses`);
/// every other menu test takes the fixture's hard-coded `can_frame: true`. Either
/// way the flag's *provenance* is never exercised — what those tests prove is
/// that the row reacts to the boolean, not that the boolean is right — and the
/// path from a real document to a dimmed *Frame selection* row did not exist.
/// (⚠️ This said *"`menu.rs`'s two tests"* until `arch-scribe` looked; the
/// conclusion survives and the argument was one test wide, not two.)
///
/// **Asserted as the contract rather than as plain equality**, because the two
/// deliberately disagree in one place and a test that demanded equality would
/// report the *documented* case as the bug. The last row below is that case, and
/// it is the assertion that stops the exception being "fixed" into a divergence
/// later.
///
/// 🚨 **Flip run, and the first one does not bite — which is a finding about the
/// code rather than a failed experiment.** Deleting `can_frame`'s
/// `matches!(node.kind(), NodeKind::Root)` arm, which is the mutation the review
/// proposed, leaves every row here green. The reason is the guard two lines below
/// it: **`Root` is the only node in a `Document` that has no parent** —
/// `Document::new` is the one place a `Node` is built with `parent: None` — so
/// `let Some(p) = node.parent() else { return false }` already refuses it, and the
/// kind test is expressive rather than load-bearing. It is worth keeping for
/// exactly that reason, and worth knowing that nothing would notice if it went.
///
/// ⚠️ **A flip that does bite**, for the row that has no other guard behind it:
/// neutering the shared-parent comparison (`let _ = *parent.get_or_insert(p) != p;`)
/// fails on **two different parents**, `true` against `false`. ⚠️ *Spelling it
/// `if false && …` instead fails the wrong row* — the short-circuit skips
/// `get_or_insert`, so `parent` stays `None` and every case comes back `false`.
/// That is a mutation with a different meaning and the same one-line description,
/// which is CLAUDE.md's own warning arriving in miniature.
#[test]
fn can_frame_answers_what_frame_would_do() {
    let mut f = Fixture::new();
    let a = f.add(f.artboard, rect(10.0, 10.0), (0.0, 0.0));
    let b = f.add(f.artboard, rect(10.0, 10.0), (20.0, 0.0));
    let nested_frame = f.add(f.artboard, artboard(), (40.0, 0.0));
    let (tx, group) = build::group(&f.doc, &mut f.ids, &[b]).unwrap();
    f.commit(tx);
    let in_group = f.children_of(group);
    let root = f.root;

    // Every case the builder can refuse for a reason a user can act on, plus the
    // one it accepts, plus the documented exception. Each row: what is selected,
    // what `can_frame` must say, and what `frame` must do.
    let res = f.resolved();
    for (what, members, allowed) in [
        ("nothing selected", vec![], false),
        ("the root itself", vec![root], false),
        ("two different parents", vec![a, in_group[0]], false),
        ("a selection inside a group", in_group.clone(), false),
        ("an ordinary pair", vec![a, group], true),
        ("a frame among the members", vec![a, nested_frame], true),
    ] {
        assert_eq!(
            build::can_frame(&f.doc, &members),
            allowed,
            "can_frame disagrees with the row for {what}"
        );
        assert_eq!(
            build::frame(&f.doc, &res, &mut f.ids, &members).is_ok(),
            allowed,
            "and `frame` itself disagrees with `can_frame` for {what} — which is \
             the contract `can_frame`'s doc states and nothing checked"
        );
    }

    // ⚠️ **The one deliberate disagreement**, and it is in the permissive
    // direction: `can_frame` does not ask whether the members' boxes resolve,
    // because that needs a `Resolved` and *"it is not a thing a user can act
    // on"*. An empty group has no measurable extent, so the row stays live and
    // the verb reports the failure.
    let (tx, empty) = build::group(&f.doc, &mut f.ids, &[a]).unwrap();
    f.commit(tx);
    let inner = f.children_of(empty);
    f.commit(Transaction(vec![Operation::DeleteNode { id: inner[0] }]));
    let res = f.resolved();
    assert!(
        f.children_of(empty).is_empty(),
        "fixture: the group has to be empty, or this row is about nothing"
    );
    assert!(
        build::can_frame(&f.doc, &[empty]),
        "the row stays live — `can_frame` deliberately does not measure"
    );
    assert!(
        matches!(
            build::frame(&f.doc, &res, &mut f.ids, &[empty]),
            Err(OpError::MalformedSubtree)
        ),
        "and the verb is the one that reports it"
    );
}

/// One question, one predicate — and the four spellings it replaced disagreed
/// with each other (§15 D762).
///
/// `affine_is_invertible` asks *will the inverse be representable*, which is the
/// question every caller actually has, and is why it carries no threshold. Until
/// D762 the tree asked it four ways: `> 1e-12` and `== 0.0` in `svg_in`, and
/// `< f64::EPSILON` and `!is_finite() || == 0.0` in the SVG writer. Two of those
/// are the same test with the operands reversed; the other two disagree with each
/// other by twelve orders of magnitude.
///
/// The interesting row is the last. `f64::EPSILON` is ~2.22e-16 and is a
/// **relative** quantity — the gap between 1.0 and the next double — where a
/// determinant is in **squared world units**. A node at 1e5 units with a
/// perfectly ordinary scale has a determinant far above it; one at millimetre
/// scale can have a legitimate determinant far below it and be refused. So that
/// guard stopped guarding at large coordinates and over-refused at small ones,
/// which is the whole argument for a predicate that does not pick a number.
///
/// Plain backticks throughout: this is `crates/*/tests/`, where no gate reads a
/// doc link (§15 D319, D622).
///
/// Flip, run: making the predicate `determinant().abs() >= f64::EPSILON` fails at
/// the **`tiny`** row, not the `small` one.
///
/// ⚠️ The predicted site was wrong and the correction is the useful part: `small`
/// is a 1e-3 scale, determinant **1e-6**, which is four hundred million times
/// `f64::EPSILON` and was never in danger. The threshold only bites once the
/// determinant is under ~2.2e-16, i.e. a scale under ~1.5e-8 — so the old guard's
/// real fault is not that it refuses ordinary small shapes but that it is
/// **calibrated to nothing**: the number where it starts refusing has no relation
/// to the units it is measuring. `small` stays in the test as the row that says
/// so.
#[test]
fn affine_is_invertible_asks_one_question_and_the_old_spellings_disagreed() {
    let singular = Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0]);
    assert_eq!(singular.determinant(), 0.0, "the fixture is singular");
    assert!(!build::affine_is_invertible(singular));
    assert!(!build::affine_is_invertible(Affine::new([
        f64::NAN,
        0.0,
        0.0,
        1.0,
        0.0,
        0.0
    ])));
    assert!(build::affine_is_invertible(Affine::IDENTITY));

    // A millimetre-scale node: determinant 1e-6. Comfortably above `f64::EPSILON`
    // and never at risk from it — which is the point. The old threshold was not
    // wrong about *this* shape, it was calibrated against nothing at all.
    let small = Affine::scale(1e-3);
    assert_eq!(small.determinant(), 1e-6);
    assert!(
        build::affine_is_invertible(small),
        "a millimetre-scale transform is invertible and must not be refused"
    );

    // 🚨 The row that says why a relative epsilon is the wrong tool: a transform
    // whose determinant is *below* `f64::EPSILON` and whose inverse is still
    // perfectly representable. `< f64::EPSILON` refuses this; the question the
    // callers ask says yes.
    let tiny = Affine::scale(1e-9);
    assert!(
        tiny.determinant() < f64::EPSILON,
        "fixture: the determinant is under the old threshold, or this row is \
         about nothing"
    );
    assert!(
        build::affine_is_invertible(tiny),
        "its inverse is 1e9 — representable, finite, and what the caller wanted"
    );
}
