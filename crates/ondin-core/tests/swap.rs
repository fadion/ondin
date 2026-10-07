//! Instance swap (§15 D983, `ondin_core::swap`): a nested copy inside an outer
//! instance showing another main's contents in its slot — the rewrite the commit
//! makes, what follows the slot and what follows the swap, the swap as an
//! override, and every door that moves a swapped copy's link.

use ondin_core::component::LinkRule;
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::reset::{self, Kind};
use ondin_core::variant::{self, PropKind, PropValue, VariantProp, VariantSet};
use ondin_core::{
    Brush, Document, Fill, GeometryPatch, IdSource, Keyed, NodeId, NodeKind, OpError, Operation,
    Placement, Transaction, io, keyed_by_position, swap,
};

fn fills(r: u8) -> Vec<Keyed<Fill>> {
    keyed_by_position([Fill {
        brush: Brush::Solid(Color::from_rgba8(r, 0, 0, 255)),
        visible: true,
    }])
}

fn frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
    }
}

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: RoundedRectRadii::default(),
    }
}

fn create(id: NodeId, parent: NodeId, index: usize, kind: NodeKind, name: &str) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: None,
        name: Some(name.into()),
    }
}

/// Three icon mains — `Icons / Star` (24², a red `Shape`), `Icons / Heart` (32², a
/// `Shape` of 200 and a `Shine`) and `Icons / Dot`, a **group** — and a `Button`
/// main holding a nested instance `n` of Star beside a `Label`. `b1` is an
/// instance of Button under the root, and `r` its copy of `n`: the nested copy a
/// swap is made on.
struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    star: NodeId,
    sshape: NodeId,
    heart: NodeId,
    hshape: NodeId,
    hshine: NodeId,
    dot: NodeId,
    button: NodeId,
    n: NodeId,
    b1: NodeId,
    r: NodeId,
}

fn instance(
    doc: &Document,
    ids: &mut IdSource,
    main: NodeId,
    parent: NodeId,
) -> (Transaction, NodeId) {
    let (tx, made) = ondin_core::insert_subtrees(
        doc,
        ids,
        &[Placement {
            nodes: doc.capture_subtree(main).unwrap(),
            parent,
            index: None,
        }],
        Default::default(),
    );
    (tx, made[0])
}

fn fixture() -> F {
    let mut ids = IdSource::new(0x5A);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [
        star,
        sshape,
        heart,
        hshape,
        hshine,
        dot,
        dshape,
        button,
        label,
    ] = [(); 9].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(star, root, 0, frame(24.0, 24.0), "Icons / Star"),
        create(sshape, star, 0, rect(), "Shape"),
        create(heart, root, 1, frame(32.0, 32.0), "Icons / Heart"),
        create(hshape, heart, 0, rect(), "Shape"),
        create(hshine, heart, 1, rect(), "Shine"),
        create(dot, root, 2, NodeKind::Group, "Icons / Dot"),
        create(dshape, dot, 0, rect(), "Shape"),
        create(button, root, 3, frame(120.0, 40.0), "Button"),
        create(label, button, 0, rect(), "Label"),
        Operation::SetFills {
            id: sshape,
            fills: fills(10),
        },
        Operation::SetFills {
            id: hshape,
            fills: fills(200),
        },
        Operation::SetFills {
            id: hshine,
            fills: fills(90),
        },
        Operation::SetComponent {
            id: star,
            component: true,
        },
        Operation::SetComponent {
            id: heart,
            component: true,
        },
        Operation::SetComponent {
            id: dot,
            component: true,
        },
    ]))
    .expect("three icons and a button frame");
    let (tx, n) = instance(&doc, &mut ids, star, button);
    doc.apply(&tx).expect("a Star inside the button");
    doc.apply(&Transaction(vec![Operation::SetComponent {
        id: button,
        component: true,
    }]))
    .expect("the button a main");
    let (tx, b1) = instance(&doc, &mut ids, button, root);
    doc.apply(&tx).expect("an instance of the button");
    let r = child_linked(&doc, b1, n).expect("b1's copy of the nested Star");
    F {
        doc,
        ids,
        root,
        star,
        sshape,
        heart,
        hshape,
        hshine,
        dot,
        button,
        n,
        b1,
        r,
    }
}

fn child_linked(doc: &Document, parent: NodeId, src: NodeId) -> Option<NodeId> {
    doc.get(parent)?
        .children()
        .iter()
        .copied()
        .find(|c| doc.get(*c).and_then(|n| n.link()) == Some(src))
}

impl F {
    /// `ops` with everything `EditorSession::commit_inner` appends
    /// (`propagate::owed`) — the swap's rewrite first.
    fn commit(&mut self, ops: Vec<Operation>) {
        let mut tx = Transaction(ops);
        ondin_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        self.doc
            .apply(&tx)
            .expect("the edit and everything it owes");
    }

    /// `F::commit` through a `History`, then held to what a reset owes the
    /// document: the result reloads, and one undo restores exactly the document
    /// before it. Leaves the committed document in place.
    fn commit_undoable(&mut self, ops: Vec<Operation>) {
        let before = self.doc.clone();
        let mut tx = Transaction(ops);
        ondin_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        let mut history = ondin_core::History::new();
        history
            .commit(&mut self.doc, tx)
            .expect("the edit and everything it owes");
        io::load(&io::save(&self.doc).unwrap()).expect("the committed document reloads");
        let after = self.doc.clone();
        history.undo(&mut self.doc).unwrap();
        assert!(self.doc == before, "one undo restores the document");
        self.doc = after;
    }

    fn swap_to(&mut self, root: NodeId, main: NodeId) {
        let tx = swap::swap(&self.doc, root, main).expect("a swap this fixture allows");
        self.commit(tx.0);
    }

    fn node(&self, id: NodeId) -> &ondin_core::Node {
        self.doc.get(id).unwrap()
    }

    fn kids(&self, id: NodeId) -> Vec<NodeId> {
        self.node(id).children().to_vec()
    }

    fn fill(&self, id: NodeId) -> Brush {
        self.node(id).paint().fills[0].brush.clone()
    }

    fn size(&self, id: NodeId) -> Size {
        match self.node(id).kind() {
            NodeKind::Artboard { size } => *size,
            _ => panic!("not a frame"),
        }
    }
}

fn red(r: u8) -> Brush {
    Brush::Solid(Color::from_rgba8(r, 0, 0, 255))
}

/// **The swap rewrites in place**: `r` keeps its id, its link (the slot) and its
/// size, gains the swap, and its `Shape` — the same node — is relinked to Heart's
/// and takes Heart's fill; Heart's `Shine` arrives as a new linked copy.
///
/// Flip: `swap::settle` returning nothing fails **every** test in this file that
/// swaps, and none of them at its own assertions — at the commit's `apply`:
/// `component::check` refuses a swapped root whose members still link into its
/// slot (`LinkRule::Membership`, read against the swap). So an unexpanded swap
/// cannot be committed at all; the rewrite and the check are two defences, and
/// this test's assertions are the half that says *what* the rewrite does.
#[test]
fn a_swap_rewrites_the_copys_contents_in_place() {
    let mut f = fixture();
    let shape = f.kids(f.r)[0];
    f.swap_to(f.r, f.heart);
    assert_eq!(f.node(f.r).swap(), Some(f.heart));
    assert_eq!(f.node(f.r).link(), Some(f.n), "the link keeps the slot");
    let kids = f.kids(f.r);
    assert_eq!(kids.len(), 2, "Heart's Shape and Shine");
    assert_eq!(kids[0], shape, "the matched layer keeps its id");
    assert_eq!(f.node(shape).link(), Some(f.hshape));
    assert_eq!(f.fill(shape), red(200));
    assert_eq!(f.node(kids[1]).link(), Some(f.hshine));
    assert_eq!(
        f.size(f.r),
        Size::new(24.0, 24.0),
        "the slot's size, not Heart's 32"
    );
    assert_eq!(ondin_core::component::main_of(&f.doc, f.r), Some(f.heart));
}

/// **Overrides carry** (§15 D983 (4)): a fill the copy had overridden stays
/// through the swap, where an untouched one takes the new main's.
#[test]
fn a_swap_carries_the_copys_overrides() {
    let mut f = fixture();
    let shape = f.kids(f.r)[0];
    f.commit(vec![Operation::SetFills {
        id: shape,
        fills: fills(77),
    }]);
    f.swap_to(f.r, f.heart);
    assert_eq!(
        f.fill(shape),
        red(77),
        "the copy's own fill survives the swap"
    );
    // And swapped back, it is still the copy's own.
    f.swap_to(f.r, f.star);
    assert_eq!(
        f.node(f.r).swap(),
        None,
        "Star is the slot's own: the swap clears"
    );
    assert_eq!(f.node(shape).link(), f.kids(f.n).first().copied());
    assert_eq!(f.fill(shape), red(77));
}

/// **An override made on the slot, inside the outer main, carries through a swap
/// too** (§15 D991). The maintainer's case: a Toolbar main whose nested Button
/// has its label set to "Save", and a Toolbar instance switching that Button to
/// another variant — the label stayed "Save" on screen until the switch and then
/// read "Button". The copy holds the slot's value, so measured against the slot
/// it is untouched; measured against the main the slot shows it is an override,
/// which is what it is. Here the slot's `Shape` is given fill 77 in the Button
/// main, reaches `r` by propagation, and survives the swap to Heart (whose own is
/// 200); swapped back, the slot's 77 is what `r` shows and no longer an override.
///
/// Flip, run: `carry` handed `s_old` again rather than `base_of(s_old)` fails at
/// *"the slot's override survives"* with Heart's 200.
#[test]
fn a_swap_carries_an_override_made_on_the_slot() {
    let mut f = fixture();
    let slot_shape = f.kids(f.n)[0];
    let shape = f.kids(f.r)[0];
    f.commit(vec![Operation::SetFills {
        id: slot_shape,
        fills: fills(77),
    }]);
    assert_eq!(f.fill(shape), red(77), "the fixture: r follows its slot");
    f.swap_to(f.r, f.heart);
    assert_eq!(f.fill(shape), red(77), "the slot's override survives");
    f.swap_to(f.r, f.star);
    assert_eq!(f.node(f.r).swap(), None);
    assert_eq!(f.fill(shape), red(77));
    assert!(
        reset::overrides(&f.doc, shape).is_empty(),
        "back on the slot, 77 is the slot's own value rather than r's override"
    );
}

/// **A variant's own override on a nested instance is not the instance's** — the
/// boundary §15 D991's measure stops at. Two variants each hold a Star whose
/// `Shape` they have recoloured, 30 and 60; an instance of the first switched to
/// the second takes 60. Measured all the way down to Star's 10, the instance's 30
/// would read as an override of its own and stay.
///
/// Flip, run: `base_of` walking the content sources to their end rather than
/// stopping inside the main `from` shows fails at *"the second variant's own"*
/// with 30.
#[test]
fn a_variant_switch_drops_the_old_variants_own_nested_override() {
    let mut f = fixture();
    let [set, v1, v2] = [(); 3].map(|_| f.ids.mint());
    f.commit(vec![
        create(set, f.root, 0, frame(300.0, 300.0), "Set"),
        create(v1, set, 0, frame(40.0, 40.0), "A"),
        create(v2, set, 1, frame(40.0, 40.0), "B"),
    ]);
    for v in [v1, v2] {
        let (tx, _) = instance(&f.doc, &mut f.ids, f.star, v);
        f.commit(tx.0);
    }
    let icon = |f: &F, v: NodeId| f.kids(f.kids(v)[0])[0];
    let (s1, s2) = (icon(&f, v1), icon(&f, v2));
    f.commit(vec![
        Operation::SetFills {
            id: s1,
            fills: fills(30),
        },
        Operation::SetFills {
            id: s2,
            fills: fills(60),
        },
        Operation::SetComponent {
            id: v1,
            component: true,
        },
        Operation::SetComponent {
            id: v2,
            component: true,
        },
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![VariantProp {
                    name: "V".into(),
                    values: vec!["A".into(), "B".into()],
                }],
            }),
        },
        Operation::SetVariant {
            id: v1,
            values: vec!["A".into()],
        },
        Operation::SetVariant {
            id: v2,
            values: vec!["B".into()],
        },
    ]);
    let (tx, i) = instance(&f.doc, &mut f.ids, v1, f.root);
    f.commit(tx.0);
    assert_eq!(f.fill(icon(&f, i)), red(30), "the fixture: A's own");
    let tx = variant::switch(&f.doc, i, v2, &mut f.ids).expect("a switch to B");
    f.commit(tx.0);
    assert_eq!(f.fill(icon(&f, i)), red(60), "the second variant's own");
}

/// **The slot keeps placement, size and visibility; the swap brings the rest**
/// (§15 D983 (3)). Moving the icon in Button and resizing it reach the swapped
/// copy; Star's root fill does not, and Heart's does; Heart's size does not.
///
/// Flip: `propagate`'s `Takes::Slot` arm admitting every op lets Star's fill in
/// (fails at the fill assertion). Flip: `Takes::Content` admitting the slot's
/// fields **did not bite** until the visibility case was added — Heart's size
/// change never reaches a copy holding a size of its own, so that assertion is
/// true under both; hiding the Heart main is the one slot field the copy still
/// holds Heart's old value of, and it fails there.
#[test]
fn the_slot_keeps_placement_size_and_visibility() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    f.commit(vec![
        Operation::SetTransform {
            id: f.n,
            transform: Affine::translate((8.0, 6.0)),
        },
        Operation::SetGeometry {
            id: f.n,
            geometry: GeometryPatch::Size(Size::new(20.0, 20.0)),
        },
    ]);
    assert_eq!(f.node(f.r).transform(), Affine::translate((8.0, 6.0)));
    assert_eq!(f.size(f.r), Size::new(20.0, 20.0));
    f.commit(vec![
        Operation::SetFills {
            id: f.star,
            fills: fills(1),
        },
        Operation::SetGeometry {
            id: f.heart,
            geometry: GeometryPatch::Size(Size::new(40.0, 40.0)),
        },
    ]);
    assert!(
        f.node(f.r).paint().fills.is_empty(),
        "Star's root fill is not the swapped copy's"
    );
    assert_eq!(
        f.size(f.r),
        Size::new(20.0, 20.0),
        "Heart's size is not either"
    );
    // The one slot field the copy still holds Heart's old value of: hiding the
    // Heart main on its components page must not hide the swapped icon.
    f.commit(vec![Operation::SetVisible {
        id: f.heart,
        visible: false,
    }]);
    assert!(
        f.node(f.r).visible(),
        "Heart's visibility is not the copy's"
    );
    f.commit(vec![Operation::SetFills {
        id: f.heart,
        fills: fills(2),
    }]);
    assert_eq!(f.fill(f.r), red(2), "Heart's root fill is");
}

/// **Heart's edits reach the swapped copy and Star's do not** — a field, and a
/// child gained.
#[test]
fn the_swapped_mains_edits_reach_the_copy() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    let shape = f.kids(f.r)[0];
    f.commit(vec![Operation::SetFills {
        id: f.hshape,
        fills: fills(150),
    }]);
    assert_eq!(f.fill(shape), red(150));
    let (grew, sgrew) = (f.ids.mint(), f.ids.mint());
    f.commit(vec![
        create(grew, f.heart, 2, rect(), "Glint"),
        create(sgrew, f.star, 1, rect(), "Point"),
    ]);
    let kids = f.kids(f.r);
    assert_eq!(kids.len(), 3, "Heart's new child, and not Star's");
    assert_eq!(f.node(kids[2]).link(), Some(grew));
    assert_eq!(
        f.fill(f.kids(f.n)[0]),
        red(10),
        "the main's own nested Star is untouched"
    );
}

/// **The swap is one override** (§15 D983 (2)), and *Reset all* puts the slot's
/// own contents back — in place, the matched layer keeping its id, Heart's
/// `Shine` gone.
#[test]
fn reset_all_takes_the_swap_back() {
    let mut f = fixture();
    let shape = f.kids(f.r)[0];
    f.swap_to(f.r, f.heart);
    let drift = reset::drift(&f.doc, f.b1);
    assert_eq!(drift.fields, 1, "the swap, and nothing else: {drift:?}");
    assert!(reset::overrides(&f.doc, f.r).iter().any(|o| o.reset
        == Operation::SetSwap {
            id: f.r,
            swap: None
        }));
    let ops = reset::reset(&f.doc, Kind::All, &[f.b1], &mut f.ids);
    f.commit(ops);
    assert_eq!(f.node(f.r).swap(), None);
    assert_eq!(f.kids(f.r), vec![shape], "Shine gone, Shape kept");
    assert_eq!(f.node(shape).link(), Some(f.kids(f.n)[0]));
    assert_eq!(f.fill(shape), red(10));
    assert!(!reset::drift(&f.doc, f.b1).any());
}

/// A swapped copy's own field overrides are counted against **its swap's** main
/// and reset with the swap. Flip: `reset::scope_nodes` not taking in a swapped
/// copy's members counts 1 for 2.
#[test]
fn a_swapped_copys_field_overrides_count_against_its_swap() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    let shape = f.kids(f.r)[0];
    f.commit(vec![Operation::SetFills {
        id: shape,
        fills: fills(33),
    }]);
    assert_eq!(
        reset::drift(&f.doc, f.b1).fields,
        2,
        "the swap and the fill"
    );
    let ops = reset::reset(&f.doc, Kind::Fields, &[f.b1], &mut f.ids);
    f.commit(ops);
    assert_eq!(f.node(f.r).swap(), None);
    assert_eq!(f.fill(shape), red(10), "back to Star's, through Heart's");
}

/// **A local instance of the main a slot is swapped to stays a local addition**
/// (`[X4-L1-02]`; §15 D981 (1) as D994 amends it, *local layers survive every
/// reset*). `l`, a Heart placed in `b1` after `r` with its own opacity and fill,
/// is not in `b1`'s scope because `r` shows Heart too: `scope_nodes` had grown
/// one set with the whole of every swap it met, keyed on link targets, so `l`
/// (linked to Heart) and its `Shape` (linked to Heart's) passed — the card read
/// *0 local layers* and *Reset all* put `l` back to Heart's look. Only when `l`
/// came after `r` in preorder, which is the order here. The scope now reads a
/// swap's members only under the swapped copy, and a link to a main only at the
/// scope's own root.
///
/// Flip, run: `scope_nodes` back to the one shared set (every swap's subtree,
/// its root included, admitting any node) fails the first assertion, `drift`
/// reading `fields: 3, local: 0` for `fields: 1, local: 1`. Dropping only the
/// rule that a link to a main admits the scope's root alone leaves `l` out (it
/// is under no swapped copy) and fails on `inner`, a Heart placed inside `r`,
/// under the reset scoped to `r` — whose source is Heart itself.
#[test]
fn a_local_instance_of_a_swaps_main_is_not_reset_with_the_slot() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    let (tx, l) = instance(&f.doc, &mut f.ids, f.heart, f.b1);
    f.commit(tx.0);
    assert_eq!(f.kids(f.b1).last(), Some(&l), "placed after `r`");
    let lshape = f.kids(l)[0];
    f.commit(vec![
        Operation::SetOpacity {
            id: l,
            opacity: 0.5,
        },
        Operation::SetFills {
            id: lshape,
            fills: fills(77),
        },
    ]);
    let drift = reset::drift(&f.doc, f.b1);
    assert_eq!(
        (drift.fields, drift.local),
        (1, 1),
        "the swap; `l`: {drift:?}"
    );
    let scope = reset::scope_nodes(&f.doc, f.b1);
    assert!(!scope.contains(&l) && !scope.contains(&lshape));
    // And one placed inside `r` itself, read from a reset scoped to `r`, whose
    // source — Heart — is the main `inner` is linked to.
    let (tx, inner) = instance(&f.doc, &mut f.ids, f.heart, f.r);
    f.commit(tx.0);
    for scope in [f.b1, f.r] {
        let nodes = reset::scope_nodes(&f.doc, scope);
        assert!(!nodes.contains(&inner), "`inner` is local to {scope:?}");
        assert!(!nodes.contains(&f.kids(inner)[0]));
    }
    let ops = reset::reset(&f.doc, Kind::All, &[f.b1], &mut f.ids);
    f.commit_undoable(ops);
    assert_eq!(f.node(f.r).swap(), None, "the slot's swap is reset");
    assert_eq!(f.node(l).opacity(), 0.5, "`l` keeps its own opacity");
    assert_eq!(f.fill(lshape), red(77), "and its own fill");
}

/// **Two slots swapped to one main do not mask each other's removed children**
/// (`[X4-L1-03]`). `b1` holds `r` and `r2`, both swapped to Heart; `r`'s `Shine`
/// is deleted. `reset::missing` looked for a counterpart under the *outermost*
/// root — it climbed from `r` to `b1`, as it must for a member dragged out of a
/// nested copy — and found `r2`'s `Shine`, linked to the same Heart node, so
/// *Restore removed children* read 0 and restored nothing. A swapped copy's
/// members link into its swap, which nothing outside the copy may claim for the
/// outer instance (`settle_links` cuts a member dragged out of it), so the climb
/// stops at a swapped root.
///
/// Flip, run: `outermost_root` climbing through a swapped root again fails the
/// `removed` assertion, 0 for 1. (Asserted first: with `r2` unswapped the same
/// deletion counts 1, so the fixture is the masking and not a lost deletion.)
#[test]
fn two_slots_swapped_to_one_main_do_not_mask_a_removed_child() {
    let mut f = fixture();
    let (tx, n2) = instance(&f.doc, &mut f.ids, f.star, f.button);
    f.commit(tx.0);
    let r2 = child_linked(&f.doc, f.b1, n2).expect("b1's copy of the second Star");
    f.swap_to(f.r, f.heart);
    let shine = child_linked(&f.doc, f.r, f.hshine).expect("r's Shine");
    f.commit(vec![Operation::DeleteNode { id: shine }]);
    assert_eq!(reset::drift(&f.doc, f.b1).removed, 1, "one slot swapped");
    f.swap_to(r2, f.heart);
    assert!(
        child_linked(&f.doc, r2, f.hshine).is_some(),
        "r2 holds a Shine"
    );
    assert_eq!(reset::drift(&f.doc, f.b1).removed, 1, "both swapped");
    let ops = reset::reset(&f.doc, Kind::Children, &[f.b1], &mut f.ids);
    assert!(
        matches!(ops.as_slice(), [Operation::InsertSubtree { parent, .. }] if *parent == f.r),
        "{ops:?}"
    );
    f.commit_undoable(ops);
    assert!(
        child_linked(&f.doc, f.r, f.hshine).is_some(),
        "r's Shine back"
    );
    assert_eq!(reset::drift(&f.doc, f.b1).removed, 0);
}

/// The rules: a swap only on a nested copy inside an outer instance, only to a
/// main of its own kind.
#[test]
fn the_check_holds_a_swap_to_a_nested_copy_and_a_main_of_its_kind() {
    let mut f = fixture();
    let refused = |doc: &mut Document, id: NodeId, to: NodeId| match doc.apply(&Transaction(vec![
        Operation::SetSwap { id, swap: Some(to) },
    ])) {
        Err(OpError::BadLink(_, rule)) => rule,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(
        refused(&mut f.doc, f.b1, f.heart),
        LinkRule::Swap,
        "linked straight to a main"
    );
    assert_eq!(
        refused(&mut f.doc, f.n, f.heart),
        LinkRule::Swap,
        "nested inside a main"
    );
    assert_eq!(
        refused(&mut f.doc, f.r, f.dot),
        LinkRule::Swap,
        "a group for a frame"
    );
    assert_eq!(
        refused(&mut f.doc, f.r, f.hshape),
        LinkRule::Swap,
        "not a main"
    );
    assert!(!swap::can_swap_to(&f.doc, f.r, f.dot));
    assert!(swap::can_swap_to(&f.doc, f.r, f.heart));
}

/// A swap that would put a main inside itself is not offered.
#[test]
fn a_swap_into_its_own_container_is_not_offered() {
    let mut f = fixture();
    let panel = f.ids.mint();
    f.commit(vec![create(panel, f.root, 4, frame(200.0, 200.0), "Panel")]);
    let (tx, b2) = instance(&f.doc, &mut f.ids, f.button, panel);
    f.commit(tx.0);
    f.commit(vec![Operation::SetComponent {
        id: panel,
        component: true,
    }]);
    let r2 = child_linked(&f.doc, b2, f.n).unwrap();
    assert!(!swap::can_swap_to(&f.doc, r2, panel), "Panel holds r2");
    assert!(!swap::options(&f.doc, r2, "", "").contains(&panel));
    assert!(swap::options(&f.doc, r2, "", "").contains(&f.heart));
}

/// The picker's list: a prefix filter, a search anywhere, both without case;
/// only mains of the slot's kind.
#[test]
fn the_options_filter_by_prefix_and_search() {
    let mut f = fixture();
    let other = f.ids.mint();
    f.commit(vec![
        create(other, f.root, 4, frame(10.0, 10.0), "Logos / Star"),
        Operation::SetComponent {
            id: other,
            component: true,
        },
    ]);
    let all = swap::options(&f.doc, f.r, "", "");
    assert!(all.contains(&f.star) && all.contains(&f.heart) && all.contains(&other));
    assert!(!all.contains(&f.dot), "a group main for a frame slot");
    assert_eq!(
        swap::options(&f.doc, f.r, "icons /", ""),
        vec![f.heart, f.star]
    );
    assert_eq!(swap::options(&f.doc, f.r, "", "STAR"), vec![f.star, other]);
    assert_eq!(swap::options(&f.doc, f.r, "Icons", "heart"), vec![f.heart]);
}

#[test]
fn a_swapped_copy_round_trips_through_a_file() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    let bytes = io::save(&f.doc).unwrap();
    let loaded = io::load(&bytes).unwrap();
    assert_eq!(loaded.get(f.r).unwrap().swap(), Some(f.heart));
    assert_eq!(io::save(&loaded).unwrap(), bytes);
}

/// **Deleting the swapped-to main clears the swap** and keeps the look: the
/// copy's layers are cut loose as its own, as a main's deletion detaches its
/// instances (§15 D979 (c)).
#[test]
fn deleting_the_swapped_to_main_clears_the_swap() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    let shape = f.kids(f.r)[0];
    let mut ops = ondin_core::component::relink_for_delete(&f.doc, &[f.heart]);
    ops.push(Operation::DeleteNode { id: f.heart });
    f.commit(ops);
    assert_eq!(f.node(f.r).swap(), None);
    assert_eq!(f.node(f.r).link(), Some(f.n), "still the slot's copy");
    assert_eq!(f.node(shape).link(), None, "Heart's layers kept as its own");
    assert_eq!(f.fill(shape), red(200));
}

/// **Detaching the outer instance lands the swapped copy as an instance of what
/// it shows** (`swap::landed`): linked straight to Heart, no swap. Flip:
/// `landed` ignoring the swap leaves it linked to Star *and* swapped, which
/// `component::check` refuses at the commit.
#[test]
fn detaching_the_outer_instance_keeps_the_swap_as_an_instance() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    let shape = f.kids(f.r)[0];
    let tx = ondin_core::component::detach(&f.doc, f.b1).unwrap();
    f.commit(tx.0);
    assert_eq!(f.node(f.b1).link(), None);
    assert_eq!(f.node(f.r).link(), Some(f.heart));
    assert_eq!(f.node(f.r).swap(), None);
    assert_eq!(f.node(shape).link(), Some(f.hshape));
}

/// Detaching the swapped copy itself leaves plain layers — no link, no swap.
#[test]
fn detaching_the_swapped_copy_leaves_plain_layers() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    let shape = f.kids(f.r)[0];
    let tx = ondin_core::component::detach(&f.doc, f.r).unwrap();
    f.commit(tx.0);
    assert_eq!((f.node(f.r).link(), f.node(f.r).swap()), (None, None));
    assert_eq!(f.node(shape).link(), None);
    assert_eq!(f.fill(shape), red(200));
}

/// **A copy of a swapped copy follows it** — through the swap and back — with no
/// swap of its own: a Button instance swapped inside a `Card` main, and an
/// instance of Card.
#[test]
fn a_copy_of_a_swapped_copy_follows_it() {
    let mut f = fixture();
    let card = f.ids.mint();
    f.commit(vec![create(card, f.root, 4, frame(200.0, 200.0), "Card")]);
    let (tx, b2) = instance(&f.doc, &mut f.ids, f.button, card);
    f.commit(tx.0);
    f.commit(vec![Operation::SetComponent {
        id: card,
        component: true,
    }]);
    let r2 = child_linked(&f.doc, b2, f.n).unwrap();
    let (tx, c1) = instance(&f.doc, &mut f.ids, card, f.root);
    f.commit(tx.0);
    let b3 = child_linked(&f.doc, c1, b2).unwrap();
    let r3 = child_linked(&f.doc, b3, r2).unwrap();
    let shape3 = f.kids(r3)[0];
    f.swap_to(r2, f.heart);
    assert_eq!(
        f.node(r3).swap(),
        None,
        "it follows, it is not swapped itself"
    );
    assert_eq!(f.kids(r3).len(), 2, "Shine reached it");
    assert_eq!(f.kids(r3)[0], shape3, "in place, one link up");
    assert_eq!(f.fill(shape3), red(200));
    assert_eq!(ondin_core::component::main_of(&f.doc, r3), Some(f.heart));
    // And it can override the swap one link up, which is then its own.
    assert!(swap::can_swap(&f.doc, r3));
    f.swap_to(r3, f.star);
    assert_eq!(
        f.node(r3).swap(),
        Some(f.star),
        "Star is not what its slot shows now"
    );
    assert_eq!(f.fill(shape3), red(10));
    // Back on r2: r3's swap now names what its slot shows, and is cleared
    // (§15 D1003 (4)) — it shows Star either way.
    f.swap_to(r2, f.star);
    assert_eq!(f.node(r2).swap(), None);
    assert_eq!(f.node(r3).swap(), None);
    assert_eq!(f.fill(shape3), red(10));
}

/// **A nested copy's variant switch is a swap within its set** (§15 D983 (6)).
#[test]
fn a_nested_variant_switch_is_a_swap() {
    let mut f = fixture();
    let set = f.ids.mint();
    // Star and Heart into a set, as two values of `Icon`.
    f.commit(vec![
        create(set, f.root, 0, frame(300.0, 300.0), "Icon"),
        Operation::Reparent {
            id: f.star,
            new_parent: set,
            index: 0,
        },
        Operation::Reparent {
            id: f.heart,
            new_parent: set,
            index: 1,
        },
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![VariantProp {
                    name: "Icon".into(),
                    values: vec!["Star".into(), "Heart".into()],
                }],
            }),
        },
        Operation::SetVariant {
            id: f.star,
            values: vec!["Star".into()],
        },
        Operation::SetVariant {
            id: f.heart,
            values: vec!["Heart".into()],
        },
    ]);
    assert!(variant::can_switch(&f.doc, f.r));
    let to = variant::switch_target(&f.doc, f.star, 0, "Heart").unwrap();
    let tx = variant::switch(&f.doc, f.r, to, &mut f.ids).unwrap();
    assert_eq!(
        tx.0,
        vec![Operation::SetSwap {
            id: f.r,
            swap: Some(f.heart)
        }]
    );
    f.commit(tx.0);
    assert_eq!(f.kids(f.r).len(), 2);
    assert_eq!(f.fill(f.kids(f.r)[0]), red(200));
}

/// **The swap property** (§15 D983 (5)): bound to the nested instance in the
/// main, its filter suggested from the main the slot shows, set on an instance by
/// swapping the copy, read as the main it shows, overridden while swapped, reset
/// by clearing.
#[test]
fn a_swap_property_swaps_and_resets_the_copy() {
    let mut f = fixture();
    let (tx, item) = variant::define(
        &f.doc,
        &mut f.ids,
        f.button,
        "Icon",
        PropKind::Swap,
        vec![f.n],
    )
    .expect("a swap property on the nested Star");
    f.commit(tx.0);
    let p = variant::instance_properties(&f.doc, f.b1)
        .into_iter()
        .find(|p| p.id == item)
        .unwrap()
        .value;
    assert_eq!(
        p.filter, "Icons",
        "the suggestion: Star's name less its last segment"
    );
    assert_eq!(variant::counterparts(&f.doc, f.b1, &p), vec![f.r]);
    assert_eq!(
        variant::property_state(&f.doc, f.b1, &p),
        Some((PropValue::Swap(f.star), false))
    );
    let ops = variant::set_property(&f.doc, &[f.b1], &p, &PropValue::Swap(f.heart));
    f.commit(ops);
    assert_eq!(
        variant::property_state(&f.doc, f.b1, &p),
        Some((PropValue::Swap(f.heart), true))
    );
    assert_eq!(f.kids(f.r).len(), 2);
    let ops = variant::reset_property(&f.doc, &[f.b1], &p);
    f.commit(ops);
    assert_eq!(f.node(f.r).swap(), None);
    assert_eq!(f.kids(f.r).len(), 1);
}

/// A swap property binds only a nested instance: a plain layer inside the main
/// is refused.
#[test]
fn a_swap_property_binds_only_a_nested_instance() {
    let mut f = fixture();
    let label = f.kids(f.button)[0];
    let (tx, _) = variant::define(
        &f.doc,
        &mut f.ids,
        f.button,
        "Icon",
        PropKind::Swap,
        vec![label],
    )
    .unwrap();
    match f.doc.apply(&tx) {
        Err(OpError::BadLink(_, LinkRule::Variant(variant::VariantRule::Binding))) => {}
        other => panic!("expected the binding refused, got {other:?}"),
    }
}

/// `sshape` and `hshine` are read by the other tests through the copies; this
/// keeps the fixture's names honest about what each is.
#[test]
fn the_fixture_is_what_it_says() {
    let f = fixture();
    assert_eq!(f.node(f.sshape).name(), "Shape");
    assert_eq!(f.node(f.hshine).name(), "Shine");
    assert_eq!(f.node(f.r).link(), Some(f.n));
    assert_eq!(f.node(f.n).link(), Some(f.star));
    assert!(swap::can_swap(&f.doc, f.r));
    assert!(!swap::can_swap(&f.doc, f.n));
    assert!(!swap::can_swap(&f.doc, f.b1));
    let _ = f.hshape;
}

// ── The release review (`v0.4.1..7d0c666`) ─────────────────────────────────────

/// The card fixture of `a_copy_of_a_swapped_copy_follows_it`: a `Card` main
/// holding `b2`, an instance of Button, and `c1`, an instance of Card — so `r2`
/// is the nested copy inside the Card main and `r3` its copy in `c1`.
fn card(f: &mut F) -> (NodeId, NodeId, NodeId, NodeId) {
    let card = f.ids.mint();
    f.commit(vec![create(card, f.root, 4, frame(200.0, 200.0), "Card")]);
    let (tx, b2) = instance(&f.doc, &mut f.ids, f.button, card);
    f.commit(tx.0);
    f.commit(vec![Operation::SetComponent {
        id: card,
        component: true,
    }]);
    let r2 = child_linked(&f.doc, b2, f.n).unwrap();
    let (tx, c1) = instance(&f.doc, &mut f.ids, card, f.root);
    f.commit(tx.0);
    let b3 = child_linked(&f.doc, c1, b2).unwrap();
    let r3 = child_linked(&f.doc, b3, r2).unwrap();
    (card, c1, r2, r3)
}

/// **A swap matches layers by the shown main's names, not the slot's**
/// (`[X5-L1-03]`): a name set on the slot inside the Button main is an override,
/// and matching by it left `r`'s `Body` unmatched — its fill dropped, a stray
/// layer beside Heart's `Shape`, and a second `Body` on the swap back.
///
/// Flip: `paths_by` naming each layer by its own name (the slot's) fails "one
/// layer for Heart's Shape", three children against two.
#[test]
fn a_swap_matches_by_the_shown_mains_names() {
    let mut f = fixture();
    let ns = f.kids(f.n)[0];
    f.commit(vec![Operation::SetName {
        id: ns,
        name: "Body".into(),
    }]);
    let body = f.kids(f.r)[0];
    assert_eq!(f.node(body).name(), "Body", "the fixture: it followed");
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: body,
            fills: fills(77),
        }]))
        .unwrap();
    f.swap_to(f.r, f.heart);
    assert_eq!(
        f.kids(f.r).len(),
        2,
        "one layer for Heart's Shape, and Shine"
    );
    assert_eq!(f.kids(f.r)[0], body, "the same layer");
    assert_eq!(f.node(body).link(), Some(f.hshape));
    assert_eq!(f.fill(body), red(77), "its override carried");
    f.swap_to(f.r, f.star);
    assert_eq!(f.kids(f.r), vec![body], "back, and no second Body");
    assert_eq!(f.node(body).link(), Some(ns));
    assert_eq!(f.fill(body), red(77));
}

/// **A layer the slot adds of its own carries across a swap** (§15 D1003 (3)):
/// one the Button main put into its nested Star stays in `r`, where it sat,
/// still linked to the slot's — through the swap and back, with no duplicate.
/// It was deleted as an untouched counterpart.
///
/// Flip: `rewrite`'s slot-own `continue` removed fails "kept, still linked to
/// the slot's"; `component::check`'s `slot_own` dropped fails the swap's commit,
/// `Membership`.
#[test]
fn a_layer_the_slot_adds_carries_across_a_swap() {
    let mut f = fixture();
    let badge = f.ids.mint();
    let n = f.n;
    f.commit(vec![create(badge, n, 1, rect(), "Badge")]);
    let mine = *f.kids(f.r).last().unwrap();
    assert_eq!(
        f.node(mine).link(),
        Some(badge),
        "the fixture: r has its copy"
    );
    f.swap_to(f.r, f.heart);
    let kids = f.kids(f.r);
    assert!(kids.contains(&mine), "kept");
    assert_eq!(
        f.node(mine).link(),
        Some(badge),
        "still linked to the slot's"
    );
    assert_eq!(kids.len(), 3, "Heart's two and the slot's own");
    // Above the Shape it sat above; Heart's Shine arrives at its own anchor.
    assert_eq!(
        kids.iter().position(|k| *k == mine),
        Some(2),
        "where it sat"
    );
    assert_eq!(f.node(kids[0]).link(), Some(f.hshape));
    f.swap_to(f.r, f.star);
    let copies = f
        .kids(f.r)
        .into_iter()
        .filter(|k| f.node(*k).link() == Some(badge))
        .count();
    assert_eq!(copies, 1, "back, and no duplicate");
}

/// **A nested instance the slot adds carries across a swap too** (§15 D1003 (3),
/// D983's amendment building it): the Button main puts an instance of Dot into
/// its nested Star, bare and inside a group, and `r` — then `r'` in a second
/// instance, with a swap already in the document — is swapped to Heart. Each
/// copy of Dot stays where it sat, linked to the slot's and an instance of Dot.
/// `slot_own` asked that the slot's layer link to nothing, so the first swap
/// was refused `Membership` by `check` (no `settle_links` without a swap before
/// it), and the second, where `settle_links` runs, cut both copies to plain
/// layers.
///
/// Flip, run: `component::slot_adds` back to `slot_own`'s unlinked-source test
/// (`nodes[&src].link.is_none()`) in both `check` and `settle_links` fails the
/// first swap's commit, "the swap commits: BadLink(_, Membership)"; in
/// `settle_links` alone it fails the second, "an instance of Dot, kept (r')",
/// cut to `None`.
#[test]
fn a_nested_instance_the_slot_adds_carries_across_a_swap() {
    let mut f = fixture();
    let (n, dot, root) = (f.n, f.dot, f.root);
    let (tx, x) = instance(&f.doc, &mut f.ids, dot, n);
    f.commit(tx.0);
    let g = f.ids.mint();
    f.commit(vec![create(g, n, 2, NodeKind::Group, "Holder")]);
    let (tx, y) = instance(&f.doc, &mut f.ids, dot, g);
    f.commit(tx.0);
    let (tx, b2) = instance(&f.doc, &mut f.ids, f.button, root);
    f.commit(tx.0);
    let r2 = child_linked(&f.doc, b2, n).unwrap();
    // `Square` matches Star layer for layer, so its swap writes no structural op
    // and nothing but `check` reads the membership; Heart's brings its Shine in.
    let [square, qshape] = [(); 2].map(|_| f.ids.mint());
    f.commit(vec![
        create(square, root, 6, frame(24.0, 24.0), "Icons / Square"),
        create(qshape, square, 0, rect(), "Shape"),
        Operation::SetComponent {
            id: square,
            component: true,
        },
    ]);
    for (r, to, which) in [(f.r, square, "r"), (r2, f.heart, "r'")] {
        let xr = child_linked(&f.doc, r, x).expect("the fixture: r's copy of x");
        let gr = child_linked(&f.doc, r, g).expect("the fixture: r's copy of g");
        let yr = child_linked(&f.doc, gr, y).expect("the fixture: and of y");
        let before = f.doc.clone();
        let mut tx = swap::swap(&f.doc, r, to).unwrap();
        ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
        let undo = f.doc.apply(&tx).expect("the swap commits");
        assert_eq!(f.node(r).swap(), Some(to), "swapped ({which})");
        for (c, s) in [(xr, x), (yr, y)] {
            assert_eq!(
                f.node(c).link(),
                Some(s),
                "an instance of Dot, kept ({which})"
            );
            assert_eq!(ondin_core::component::main_of(&f.doc, c), Some(dot));
        }
        assert_eq!(f.node(xr).parent(), Some(r), "where it sat ({which})");
        assert_eq!(f.node(gr).link(), Some(g), "the group too ({which})");
        let bytes = io::save(&f.doc).unwrap();
        let loaded = io::load(&bytes).expect("it reloads");
        assert_eq!(io::save(&loaded).unwrap(), bytes);
        let after = f.doc.clone();
        f.doc.apply(&undo.inverse).expect("one undo");
        assert_eq!(
            io::save(&f.doc).unwrap(),
            io::save(&before).unwrap(),
            "restores it ({which})"
        );
        f.doc = after;
    }
}

/// **A copy and its copy swapped in one transaction** — *Select all instances*,
/// then the swap property (`[X5-L1-02]`). `r3` was rewritten against `r2`'s new
/// contents while measured from its old, and came out with a cut Shine, a
/// "removed" child and a redundant swap. Its swap is now found redundant — its
/// slot shows Heart — and cleared, and it follows `r2`.
///
/// Flip: dropping `settle`'s redundant-swap clear fails "it follows its slot",
/// `Some(Heart)`. ⚠️ **`settle`'s new order is not what this test pins**: put back
/// to depth then id it stays green, because the clear reads the slot's `swap`
/// field off the scratch whatever order the rewrites run in. The order is the
/// second defence, for a copy whose rewrite reads its slot's children.
#[test]
fn a_copy_and_its_copy_swapped_together_follow_one_another() {
    let mut f = fixture();
    let (_, c1, r2, r3) = card(&mut f);
    let ops: Vec<Operation> = [f.r, r2, r3]
        .into_iter()
        .map(|id| Operation::SetSwap {
            id,
            swap: Some(f.heart),
        })
        .collect();
    f.commit(ops);
    assert_eq!(f.node(r2).swap(), Some(f.heart));
    assert_eq!(f.node(r3).swap(), None, "it follows its slot");
    let shine2 = f.kids(r2)[1];
    assert_eq!(
        f.node(f.kids(r3)[1]).link(),
        Some(shine2),
        "Shine linked one link up"
    );
    let d = reset::drift(&f.doc, c1);
    assert!(!d.any(), "nothing the user made: {d:?}");
}

/// **A swap left naming what its slot shows is cleared** (§15 D1003 (4),
/// `[X5-L2-01]`): `r3` swapped to Star while its slot shows Heart is an
/// override; once `r2` goes back to Star it names what its slot shows, counts
/// nothing, and follows the slot's next swap.
///
/// Flip: `settle`'s redundant-swap clear dropped fails `r3`'s swap, `Some(Star)`.
#[test]
fn a_swap_equal_to_its_slots_is_cleared() {
    let mut f = fixture();
    let (_, c1, r2, r3) = card(&mut f);
    f.swap_to(r2, f.heart);
    f.swap_to(r3, f.star);
    assert!(reset::drift(&f.doc, c1).any(), "the fixture: an override");
    f.swap_to(r2, f.star);
    assert_eq!(f.node(r3).swap(), None);
    let d = reset::drift(&f.doc, c1);
    assert!(!d.any(), "no override that changes nothing: {d:?}");
    f.swap_to(r2, f.heart);
    assert_eq!(
        ondin_core::component::main_of(&f.doc, r3),
        Some(f.heart),
        "it follows"
    );
}

/// **A climb past a swapped node carries the swap** (`[X5-L1-01]`): `r3`
/// follows `r2`'s swap to Heart one link up, and detaching `c1` — or deleting
/// the Card main, which detaches it — climbs `r3` past `r2` to Star's slot. It
/// landed showing Star with Heart's layers cut loose; it lands showing Heart.
///
/// Flip: `land_ops_through` ignoring `met` fails "still Heart" for both doors.
#[test]
fn detaching_an_outer_instance_keeps_a_swap_made_one_link_up() {
    for delete in [false, true] {
        let mut f = fixture();
        let (card, c1, r2, r3) = card(&mut f);
        f.swap_to(r2, f.heart);
        let shape3 = f.kids(r3)[0];
        if delete {
            let mut ops = ondin_core::component::relink_for_delete(&f.doc, &[card]);
            ops.push(Operation::DeleteNode { id: card });
            f.commit(ops);
        } else {
            let tx = ondin_core::component::detach(&f.doc, c1).unwrap();
            f.commit(tx.0);
        }
        assert_eq!(
            ondin_core::component::main_of(&f.doc, r3),
            Some(f.heart),
            "still Heart ({delete})"
        );
        assert_eq!(
            f.node(shape3).link(),
            Some(f.hshape),
            "Heart's, still linked"
        );
        // `b3` is an instance of Button now, and showing Heart at Star's slot is
        // one override against Button — the swap, and nothing cut loose.
        let b3 = f.node(r3).parent().unwrap();
        let d = reset::drift(&f.doc, b3);
        assert_eq!(
            (d.fields, d.removed, d.local),
            (1, 0, 0),
            "the swap alone: {d:?}"
        );
    }
}

/// **A pasted instance holding a swapped copy commits in a document with no
/// link** (`[X5-L1-04]`): `settle_copy` dropped the copy's link to the slot and
/// left its swap for `settle_links`, which a link-free document never reaches,
/// and `check` refused the paste. Into a fresh document the copy is plain
/// layers; into one still holding Heart it is an instance of Heart.
///
/// Flip: `settle_copy` leaving the swap where it drops the link fails both
/// pastes, `BadLink(_, Swap)`.
#[test]
fn a_pasted_swapped_copy_commits_where_its_slot_is_not() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    let clip = f.doc.capture_subtree(f.b1).unwrap();
    // (a) a fresh document.
    let mut ids = IdSource::new(0x5B);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (mut tx, made) = ondin_core::insert_subtrees(
        &doc,
        &mut ids,
        &[Placement {
            nodes: clip.clone(),
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    ondin_core::propagate::owed(&doc, &mut tx, &mut ids);
    doc.apply(&tx).expect("a plain paste");
    let copy = ondin_core::build::subtree_nodes(&doc, &[made[0]]);
    assert!(copy.iter().all(|id| doc.get(*id).unwrap().swap().is_none()));
    // (b) the same document, the Button and its instance gone.
    let (b1, button) = (f.b1, f.button);
    let mut ops = ondin_core::component::relink_for_delete(&f.doc, &[b1, button]);
    ops.extend([
        Operation::DeleteNode { id: b1 },
        Operation::DeleteNode { id: button },
    ]);
    f.commit(ops);
    let root = f.root;
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: clip,
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    f.commit(tx.0);
    let r = *f.kids(made[0]).last().unwrap();
    assert_eq!(
        f.node(r).link(),
        Some(f.heart),
        "an instance of what it shows"
    );
    assert_eq!(f.node(r).swap(), None);
}

/// **Detaching a nested instance bound to a swap property commits** — the
/// binding goes (`[X6.1-L2-03]`). `variant::settle` ran only after an edit that
/// moves a layer, and a detach writes links alone, so the binding a detached
/// slot no longer fits was left for `check` to refuse.
///
/// Flip: `settle`'s gate without `SetLink` fails the detach's commit,
/// `Variant(Binding)`.
#[test]
fn detaching_a_bound_slot_drops_its_binding() {
    let mut f = fixture();
    let (tx, _) = variant::define(
        &f.doc,
        &mut f.ids,
        f.button,
        "Icon",
        PropKind::Swap,
        vec![f.n],
    )
    .expect("a swap property");
    f.commit(tx.0);
    let tx = ondin_core::component::detach(&f.doc, f.n).unwrap();
    f.commit(tx.0);
    assert_eq!(f.node(f.n).link(), None, "detached");
    let bound: Vec<NodeId> = f
        .node(f.button)
        .props()
        .iter()
        .flat_map(|p| p.bound.clone())
        .collect();
    assert!(bound.is_empty(), "unbound: {bound:?}");
}

/// **A nested instance inside a copied member climbs** (`[X2-L1-03]`): the
/// Button main's Star wrapped in a group, and `b1`'s copy of that group
/// duplicated — the copy's Star is an instance of Star, as the nested copy
/// duplicated alone is. It came out a cut frame with members still linked.
///
/// Flip: the climb gated on the template's root again fails "an instance of
/// Star".
#[test]
fn a_nested_instance_in_a_copied_member_climbs() {
    let mut f = fixture();
    let (g, button, n) = (f.ids.mint(), f.button, f.n);
    f.commit(vec![
        create(g, button, 1, NodeKind::Group, "Icon"),
        Operation::Reparent {
            id: n,
            new_parent: g,
            index: 0,
        },
    ]);
    let gc = child_linked(&f.doc, f.b1, g).expect("b1's copy of the group");
    // Grouping a nested instance in a main moves its copy into the group's copy —
    // the moves loop asked the copy, a nested instance root, for the instance
    // holding it, and left it where it was.
    assert_eq!(
        f.node(f.r).parent(),
        Some(gc),
        "r moved into the group's copy"
    );
    let b1 = f.b1;
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(gc).unwrap(),
            parent: b1,
            index: None,
        }],
        Default::default(),
    );
    f.commit(tx.0);
    let star = f.kids(made[0])[0];
    assert_eq!(f.node(star).link(), Some(f.star), "an instance of Star");
    assert_eq!(f.node(f.kids(star)[0]).link(), Some(f.sshape));
}

/// **Ungrouping a group main leaves the nested instances in its instances
/// instances of their own main**, as deleting it does (`[X2-L2-03]`): the
/// nested source survives an ungroup, so nothing climbed, and the membership
/// cut the copy to plain layers.
///
/// Flip: dropping `settle_links`' detached-owner climb fails "an instance of
/// Star".
#[test]
fn ungrouping_a_group_main_keeps_its_instances_nested_instances() {
    let mut f = fixture();
    let (g, root) = (f.ids.mint(), f.root);
    f.commit(vec![create(g, root, 5, NodeKind::Group, "Group main")]);
    let (tx, n2) = instance(&f.doc, &mut f.ids, f.star, g);
    f.commit(tx.0);
    f.commit(vec![Operation::SetComponent {
        id: g,
        component: true,
    }]);
    let (tx, io) = instance(&f.doc, &mut f.ids, g, root);
    f.commit(tx.0);
    let cn = child_linked(&f.doc, io, n2).unwrap();
    let res = ondin_core::Resolved::rebuild(&f.doc);
    let tx = ondin_core::build::ungroup(&f.doc, &res, g).unwrap();
    f.commit(tx.0);
    assert_eq!(f.node(io).link(), None, "its instance detached");
    assert_eq!(f.node(cn).link(), Some(f.star), "an instance of Star");
    assert_eq!(f.node(f.kids(cn)[0]).link(), Some(f.sshape));
}

/// **The ungroup's climb carries a swap it passes** (`[X2-L2-03]`, D983's
/// amendment building D1003 (3)–(5)): a group main holding `b2`, an instance of
/// Button whose nested copy `r2` is swapped to Heart, and `io`, an instance of the
/// group — so `r3`, `io`'s copy of `r2`, follows the swap one link up. Ungrouping
/// the main climbs `b3` to Button and `r3` past `r2` to Star's slot; it must take
/// `r2`'s swap with it, as `detach` and `relink_past` do, or it lands showing Star
/// while its layers, climbing on past `r2`'s, link to Heart's. The climb landed
/// with `land_ops` and `check` refused the ungroup, `Membership` — before
/// `c84a940` every copy was cut and the ungroup committed.
///
/// Flip, run: the climb's `met` dropped — `land_ops_through(&scratch, kn, to,
/// None)`, which is `c84a940`'s `land_ops` — fails the ungroup's commit, "the
/// ungroup commits: BadLink(_, Membership)".
#[test]
fn ungrouping_a_group_main_keeps_a_swap_one_link_up() {
    let mut f = fixture();
    let (g, root) = (f.ids.mint(), f.root);
    f.commit(vec![create(g, root, 5, NodeKind::Group, "Group main")]);
    let (tx, b2) = instance(&f.doc, &mut f.ids, f.button, g);
    f.commit(tx.0);
    f.commit(vec![Operation::SetComponent {
        id: g,
        component: true,
    }]);
    let r2 = child_linked(&f.doc, b2, f.n).unwrap();
    f.swap_to(r2, f.heart);
    let (tx, io) = instance(&f.doc, &mut f.ids, g, root);
    f.commit(tx.0);
    let b3 = child_linked(&f.doc, io, b2).unwrap();
    let r3 = child_linked(&f.doc, b3, r2).unwrap();
    assert_eq!(
        ondin_core::component::main_of(&f.doc, r3),
        Some(f.heart),
        "the fixture: r3 follows r2's swap"
    );
    let shape3 = f.kids(r3)[0];
    let res = ondin_core::Resolved::rebuild(&f.doc);
    let mut tx = ondin_core::build::ungroup(&f.doc, &res, g).unwrap();
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    let before = f.doc.clone();
    let undo = f.doc.apply(&tx).expect("the ungroup commits");
    assert_eq!(f.node(io).link(), None, "its instance detached");
    assert_eq!(f.node(b3).link(), Some(f.button), "an instance of Button");
    assert_eq!(
        ondin_core::component::main_of(&f.doc, r3),
        Some(f.heart),
        "still Heart"
    );
    assert_eq!(f.node(r3).link(), Some(f.n), "at Star's slot");
    assert_eq!(
        f.node(shape3).link(),
        Some(f.hshape),
        "Heart's, still linked"
    );
    let bytes = io::save(&f.doc).unwrap();
    let loaded = io::load(&bytes).expect("it reloads");
    assert_eq!(loaded.get(r3).unwrap().swap(), Some(f.heart));
    f.doc.apply(&undo.inverse).expect("one undo");
    assert_eq!(
        io::save(&f.doc).unwrap(),
        io::save(&before).unwrap(),
        "restores it"
    );
}

/// **`swap::tidy`, two of its arms** (`[X5-L6-01]` — every arm was untested,
/// and the whole suite passed with it answering nothing): a swapped copy whose
/// link a bare `SetLink` cuts is no instance and loses its swap; and a copy
/// swapped to a group main that is ungrouped — no `relink_for_delete` on that
/// door — loses its swap and keeps the look.
///
/// Flip: `tidy` answering nothing fails the first commit, `Swap` (the second is
/// then never reached; the review's probe saw it refused `Dangling`).
#[test]
fn tidy_clears_a_swap_that_no_longer_stands() {
    let mut f = fixture();
    f.swap_to(f.r, f.heart);
    let r = f.r;
    f.commit(vec![Operation::SetLink { id: r, link: None }]);
    assert_eq!(f.node(r).swap(), None, "no instance, no swap");

    let mut f = fixture();
    let [pin, flag, ps, fs, holder] = [(); 5].map(|_| f.ids.mint());
    let root = f.root;
    f.commit(vec![
        create(pin, root, 5, NodeKind::Group, "Icons / Pin"),
        create(ps, pin, 0, rect(), "Shape"),
        create(flag, root, 6, NodeKind::Group, "Icons / Flag"),
        create(fs, flag, 0, rect(), "Shape"),
        create(holder, root, 7, frame(50.0, 50.0), "Holder"),
        Operation::SetComponent {
            id: pin,
            component: true,
        },
        Operation::SetComponent {
            id: flag,
            component: true,
        },
    ]);
    let (tx, np) = instance(&f.doc, &mut f.ids, pin, holder);
    f.commit(tx.0);
    f.commit(vec![Operation::SetComponent {
        id: holder,
        component: true,
    }]);
    let (tx, h1) = instance(&f.doc, &mut f.ids, holder, root);
    f.commit(tx.0);
    let rp = child_linked(&f.doc, h1, np).unwrap();
    f.swap_to(rp, flag);
    let shape = f.kids(rp)[0];
    let res = ondin_core::Resolved::rebuild(&f.doc);
    let tx = ondin_core::build::ungroup(&f.doc, &res, flag).unwrap();
    f.commit(tx.0);
    assert_eq!(f.node(rp).swap(), None, "the swap's main is gone");
    assert!(f.doc.get(shape).is_some(), "the look kept");
}

/// **A swap target that contains the outer main is not offered** (`[X5-L6-02]`):
/// `Wrap` holds an instance of `Panel`, which holds `r2` — showing Wrap at `r2`
/// would put Panel inside itself. The only cycle test swapped to Panel itself,
/// which the search's first step answers.
///
/// Flip: `would_cycle` not searching past the target fails "Wrap holds Panel".
#[test]
fn a_swap_to_a_main_holding_the_outer_main_is_not_offered() {
    let mut f = fixture();
    let [panel, wrap] = [(); 2].map(|_| f.ids.mint());
    let root = f.root;
    f.commit(vec![create(panel, root, 4, frame(200.0, 200.0), "Panel")]);
    let (tx, b2) = instance(&f.doc, &mut f.ids, f.button, panel);
    f.commit(tx.0);
    f.commit(vec![Operation::SetComponent {
        id: panel,
        component: true,
    }]);
    let r2 = child_linked(&f.doc, b2, f.n).unwrap();
    f.commit(vec![create(wrap, root, 5, frame(24.0, 24.0), "Wrap")]);
    let (tx, _) = instance(&f.doc, &mut f.ids, panel, wrap);
    f.commit(tx.0);
    f.commit(vec![Operation::SetComponent {
        id: wrap,
        component: true,
    }]);
    assert!(!swap::can_swap_to(&f.doc, r2, wrap), "Wrap holds Panel");
    assert!(!swap::options(&f.doc, r2, "", "").contains(&wrap));
    let forced = Transaction(vec![Operation::SetSwap {
        id: r2,
        swap: Some(wrap),
    }]);
    let mut tx = forced;
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    assert!(matches!(
        f.doc.clone().apply(&tx),
        Err(OpError::BadLink(_, LinkRule::ComponentCycle))
    ));
}
