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
    // Back on r2: r3 keeps its own.
    f.swap_to(r2, f.star);
    assert_eq!(f.node(r2).swap(), None);
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
