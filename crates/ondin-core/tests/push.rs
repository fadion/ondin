//! Push to main (§5.3d, `ondin_core::push`): an instance's differences written
//! into the main it is compared with, carried to the other instances by the
//! commit's own passes — one with a value of its own keeps it.

use ondin_core::Brush;
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::push::{self, Blocked, Kind, LayerBlocked, Outcome};
use ondin_core::reset;
use ondin_core::variant::{PropKind, Property};
use ondin_core::{
    Document, Fill, IdSource, Keyed, NodeId, NodeKind, Operation, Placement, TextSizing, TextStyle,
    Transaction, io, keyed_by_position, swap,
};

fn solid(r: u8) -> Brush {
    Brush::Solid(Color::from_rgba8(r, 0, 0, 255))
}

fn fills_of(r: u8) -> Vec<Keyed<Fill>> {
    keyed_by_position([Fill {
        brush: solid(r).into(),
        visible: true,
    }])
}

fn frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn text_kind(content: &str) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(TextStyle {
            font_family: "Inter".into(),
            font_size: 12.0,
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

/// An instance of `main` under `parent`, through the copy path.
fn instance(doc: &mut Document, ids: &mut IdSource, main: NodeId, parent: NodeId) -> NodeId {
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
    doc.apply(&tx).expect("an instance");
    made[0]
}

/// A `Card` main `m` holding a rect `a` and a text `t`, and three instances of
/// it: `i` (with copies `ia`, `it`), the one that pushes, and `j` and `k`.
struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    m: NodeId,
    a: NodeId,
    t: NodeId,
    i: NodeId,
    ia: NodeId,
    it: NodeId,
    j: NodeId,
    k: NodeId,
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xB5);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [m, a, t] = [(); 3].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(m, root, 0, frame(100.0, 100.0), "Card"),
        create(a, m, 0, rect(), "Cover"),
        create(t, m, 1, text_kind("Title"), "Title"),
        Operation::SetFills {
            id: a,
            fills: fills_of(10),
        },
        Operation::SetComponent {
            id: m,
            component: true,
        },
    ]))
    .expect("a main");
    let i = instance(&mut doc, &mut ids, m, root);
    let j = instance(&mut doc, &mut ids, m, root);
    let k = instance(&mut doc, &mut ids, m, root);
    let kids = doc.get(i).unwrap().children().to_vec();
    F {
        doc,
        ids,
        root,
        m,
        a,
        t,
        i,
        ia: kids[0],
        it: kids[1],
        j,
        k,
    }
}

impl F {
    /// `ops` through the passes a commit runs (`propagate::owed`), held to a
    /// reload; the document before it returned.
    fn commit(&mut self, ops: Vec<Operation>) -> Document {
        let before = self.doc.clone();
        let mut tx = Transaction(ops);
        ondin_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        self.doc
            .apply(&tx)
            .expect("the push and everything it owes");
        io::load(&io::save(&self.doc).unwrap()).expect("the pushed document reloads");
        before
    }

    /// An instance's own edit, applied bare.
    fn edit(&mut self, ops: Vec<Operation>) {
        self.doc.apply(&Transaction(ops)).expect("an instance edit");
    }

    fn push(&mut self, kind: Kind) -> (Vec<Operation>, Outcome) {
        let ops = push::push(&self.doc, kind, self.i, &mut self.ids);
        assert!(!ops.is_empty(), "{kind:?} pushes something");
        let before = self.commit(ops.clone());
        let out = push::outcome(&before, &self.doc, self.i, &ops);
        (ops, out)
    }

    fn kids(&self, id: NodeId) -> Vec<NodeId> {
        self.doc.get(id).unwrap().children().to_vec()
    }

    fn fills(&self, id: NodeId) -> Vec<Keyed<Fill>> {
        self.doc.get(id).unwrap().paint().fills.clone()
    }

    fn content(&self, id: NodeId) -> String {
        match self.doc.get(id).unwrap().kind() {
            NodeKind::Text { content, .. } => content.clone(),
            _ => unreachable!(),
        }
    }

    /// `instance`'s child linked to `src`.
    fn copy_of(&self, instance: NodeId, src: NodeId) -> Option<NodeId> {
        self.kids(instance)
            .into_iter()
            .find(|c| self.doc.get(*c).and_then(|n| n.link()) == Some(src))
    }
}

/// *Push fields*: `i`'s fill and text go into the main; `j`, still the main's,
/// takes both, and `k`, which recoloured its cover, keeps its own colour and
/// takes the text. Afterwards `i` has nothing left to push or reset, and the
/// outcome counts `k` as the one that kept its own. Flip: `field_push` aiming the
/// copy's value at the copy (`retargeted(c)`) leaves the main's fill at 10.
#[test]
fn pushed_fields_reach_the_main_and_every_instance_that_followed_it() {
    let mut f = fixture();
    f.edit(vec![
        Operation::SetFills {
            id: f.ia,
            fills: keyed_by_position([Fill {
                brush: solid(77).into(),
                visible: true,
            }]),
        },
        Operation::SetText {
            id: f.it,
            content: "Quarterly report".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        },
    ]);
    let ka = f.copy_of(f.k, f.a).unwrap();
    f.edit(vec![Operation::SetFills {
        id: ka,
        fills: fills_of(150),
    }]);
    let p = push::pushable(&f.doc, f.i);
    assert_eq!((p.fields, p.properties, p.layers, p.order), (2, 0, 0, 0));

    let (_, out) = f.push(Kind::Fields);
    assert_eq!(
        f.fills(f.a)[0].brush,
        solid(77).into(),
        "the main took the fill"
    );
    assert_eq!(f.content(f.t), "Quarterly report");
    let ja = f.copy_of(f.j, f.a).unwrap();
    let jt = f.copy_of(f.j, f.t).unwrap();
    assert_eq!(f.fills(ja)[0].brush, solid(77).into(), "j followed");
    assert_eq!(f.content(jt), "Quarterly report");
    assert_eq!(f.fills(ka)[0].brush, solid(150).into(), "k kept its own");
    assert_eq!(reset::drift(&f.doc, f.i), reset::Drift::default());
    assert_eq!(push::pushable(&f.doc, f.i), push::Pushable::default());
    assert_eq!(out, Outcome { follow: 1, kept: 1 });
}

/// *Push local layers*: a rect `i` placed between its two copies lands in the
/// main between their sources, `i`'s rect linked to it, so the commit gives a
/// copy to `j` and `k` and **not** a second one to `i`. Flip: dropping the
/// `SetLink`s from `insert_layer` leaves `i` with four children.
#[test]
fn a_pushed_local_layer_lands_in_the_main_once_per_instance() {
    let mut f = fixture();
    let x = f.ids.mint();
    f.edit(vec![create(x, f.i, 1, rect(), "Badge")]);
    assert_eq!(reset::drift(&f.doc, f.i).local, 1);
    assert_eq!(push::pushable(&f.doc, f.i).layers, 1);

    let (_, out) = f.push(Kind::Layers);
    let mk = f.kids(f.m);
    assert_eq!(mk.len(), 3, "the main gained the badge");
    assert_eq!(
        (mk[0], mk[2]),
        (f.a, f.t),
        "between the cover and the title"
    );
    let new = mk[1];
    assert_eq!(f.doc.get(new).unwrap().name(), "Badge");
    assert_eq!(
        f.kids(f.i),
        vec![f.ia, x, f.it],
        "i holds its own badge once"
    );
    assert_eq!(f.doc.get(x).unwrap().link(), Some(new));
    for other in [f.j, f.k] {
        assert_eq!(f.kids(other).len(), 3);
        assert!(f.copy_of(other, new).is_some(), "{other:?} has the badge");
    }
    assert_eq!(reset::drift(&f.doc, f.i), reset::Drift::default());
    assert_eq!(out, Outcome { follow: 2, kept: 0 });
}

/// *Push order*: `i` put its title under its cover; the main takes that order
/// and `j` follows it. Flip: `order_ops` keeping the main's own order writes no
/// reorder, and the push asserts something to push.
#[test]
fn a_pushed_order_reorders_the_main() {
    let mut f = fixture();
    f.edit(vec![Operation::Reorder { id: f.it, index: 0 }]);
    assert_eq!(push::pushable(&f.doc, f.i).order, 1);
    f.push(Kind::Order);
    assert_eq!(f.kids(f.m), vec![f.t, f.a]);
    let jk = f.kids(f.j);
    assert_eq!(f.doc.get(jk[0]).unwrap().link(), Some(f.t), "j followed");
    assert_eq!(reset::drift(&f.doc, f.i).order, 0);
}

/// *Push removals*: `i` deleted its cover, so the main loses it; `j`, untouched,
/// loses it too, and `k`, which recoloured its cover, keeps it unlinked. *Push
/// all* never deletes. Flip: `removal_ops` emitting nothing fails the push.
#[test]
fn a_pushed_removal_deletes_the_main_layer_and_spares_edited_copies() {
    let mut f = fixture();
    f.edit(vec![Operation::DeleteNode { id: f.ia }]);
    let ka = f.copy_of(f.k, f.a).unwrap();
    f.edit(vec![Operation::SetFills {
        id: ka,
        fills: fills_of(150),
    }]);
    assert_eq!(push::pushable(&f.doc, f.i).removals, 1);
    assert!(
        !push::push(&f.doc, Kind::All, f.i, &mut f.ids)
            .iter()
            .any(|op| matches!(op, Operation::DeleteNode { .. })),
        "Push all never deletes"
    );
    let (_, out) = f.push(Kind::Removals);
    assert!(f.doc.get(f.a).is_none(), "the main lost the cover");
    assert_eq!(f.kids(f.j).len(), 1, "j lost it");
    assert!(f.doc.get(ka).is_some(), "k kept its edited cover");
    assert_eq!(f.doc.get(ka).unwrap().link(), None, "unlinked");
    assert_eq!(out, Outcome { follow: 1, kept: 1 });
}

/// A locked main takes no push, nor one inside a locked parent:
/// [`push::blocked`] says why and every entry point is empty.
/// Two defences keep each entry point empty — its own `blocked` test and
/// `push::inside` leaving a blocked main out — so flipping one alone stays
/// green; flip both in `push::fields` → fails at its `is_empty`.
#[test]
fn a_locked_main_refuses_every_push() {
    for lock_parent in [false, true] {
        let mut f = fixture();
        let holder = f.ids.mint();
        let ka = f.ids.mint();
        let (m, root, i) = (f.m, f.root, f.i);
        f.edit(vec![
            create(holder, root, 0, frame(400.0, 400.0), "Holder"),
            Operation::Reparent {
                id: m,
                new_parent: holder,
                index: 0,
            },
        ]);
        let item = f.fills(f.ia)[0].id;
        f.edit(vec![
            Operation::SetOpacity {
                id: f.ia,
                opacity: 0.5,
            },
            Operation::SetFills {
                id: f.ia,
                fills: Vec::new(),
            },
            create(ka, i, 2, rect(), "Local"),
            Operation::SetLocked {
                id: if lock_parent { holder } else { m },
                locked: true,
            },
        ]);
        assert_eq!(push::blocked(&f.doc, i), Some(Blocked::Locked));
        assert!(push::push(&f.doc, Kind::All, i, &mut f.ids).is_empty());
        let reset = Operation::SetOpacity {
            id: f.ia,
            opacity: 1.0,
        };
        assert!(push::fields(&f.doc, &[reset], &mut f.ids).is_empty());
        let list = Operation::SetFills {
            id: f.ia,
            fills: Vec::new(),
        };
        assert!(push::item_removal(&f.doc, f.ia, &list, item).is_none());
        assert!(push::push_layer(&f.doc, ka, &mut f.ids).is_empty());
    }
}

/// Pushed from a nested copy, the instance holding it is the one that pushed,
/// not one that followed: `j` and `k` follow, `i` is not counted, and the
/// prediction says the same.
/// flip: `push::outcome` excluding only the nested copy's subtree → 3.
#[test]
fn a_push_from_a_nested_copy_counts_the_other_instances() {
    let mut f = fixture();
    let (star, shape) = (f.ids.mint(), f.ids.mint());
    let (root, m) = (f.root, f.m);
    f.edit(vec![
        create(star, root, 1, frame(24.0, 24.0), "Star"),
        create(shape, star, 0, rect(), "Shape"),
        Operation::SetComponent {
            id: star,
            component: true,
        },
    ]);
    // Committed, so `owed` gives every instance of Card its copy of it.
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(star).unwrap(),
            parent: m,
            index: None,
        }],
        Default::default(),
    );
    f.commit(tx.0);
    let n = made[0];
    let i = f.i;
    let r = f.copy_of(i, n).expect("i's copy of the nested Star");
    let rs = f.kids(r)[0];
    f.edit(vec![Operation::SetOpacity {
        id: rs,
        opacity: 0.3,
    }]);
    let reset = Operation::SetOpacity {
        id: rs,
        opacity: 1.0,
    };
    let ops = push::fields(&f.doc, &[reset], &mut f.ids);
    assert!(!ops.is_empty(), "the opacity pushes");
    let predicted = push::predicted(&f.doc, r, &ops, 0x51);
    let before = f.commit(ops.clone());
    let out = push::outcome(&before, &f.doc, r, &ops);
    assert_eq!(out, Outcome { follow: 2, kept: 0 }, "j and k");
    assert_eq!(predicted, out, "the tooltip's count");
}

/// Two copies pushing one field into one node is a push that writes it twice,
/// which the doors refuse: two local Star instances in one Card instance.
/// flip: `push::writes_twice` always false → fails at "twice".
#[test]
fn two_copies_pushing_one_node_write_it_twice() {
    let mut f = fixture();
    let (star, shape) = (f.ids.mint(), f.ids.mint());
    let root = f.root;
    f.edit(vec![
        create(star, root, 1, frame(24.0, 24.0), "Star"),
        create(shape, star, 0, rect(), "Shape"),
        Operation::SetComponent {
            id: star,
            component: true,
        },
    ]);
    let i = f.i;
    let l1 = instance(&mut f.doc, &mut f.ids, star, i);
    let l2 = instance(&mut f.doc, &mut f.ids, star, i);
    let (s1, s2) = (f.kids(l1)[0], f.kids(l2)[0]);
    f.edit(vec![
        Operation::SetOpacity {
            id: s1,
            opacity: 0.3,
        },
        Operation::SetOpacity {
            id: s2,
            opacity: 0.7,
        },
    ]);
    let resets = [s1, s2].map(|id| Operation::SetOpacity { id, opacity: 1.0 });
    let ops = push::fields(&f.doc, &resets, &mut f.ids);
    assert!(push::writes_twice(&ops), "twice: {ops:?}");
    let one = push::fields(&f.doc, &resets[..1], &mut f.ids);
    assert!(!push::writes_twice(&one));
}

/// The X mark's push moves a rotated copy's main without taking its rotation:
/// the reset carries the main's matrix with the copy's X, and the push takes
/// only the parts that differ.
/// flip: `changed_from` taking the copy's whole number run → the main turns.
#[test]
fn an_x_push_keeps_the_mains_rotation() {
    use ondin_core::kurbo::Affine;
    let mut f = fixture();
    let main_t = Affine::translate((5.0, 5.0));
    let a = f.a;
    f.commit(vec![Operation::SetTransform {
        id: a,
        transform: main_t,
    }]);
    let copy_t = Affine::translate((40.0, 5.0)) * Affine::rotate(0.5);
    f.edit(vec![Operation::SetTransform {
        id: f.ia,
        transform: copy_t,
    }]);
    // The X mark's reset: the copy's own matrix with the main's X.
    let mut reset_t = copy_t.as_coeffs();
    reset_t[4] = 5.0;
    let reset = Operation::SetTransform {
        id: f.ia,
        transform: Affine::new(reset_t),
    };
    let ops = push::fields(&f.doc, &[reset], &mut f.ids);
    f.commit(ops);
    let got = f.doc.get(a).unwrap().transform().as_coeffs();
    assert_eq!(&got[..4], &main_t.as_coeffs()[..4], "no rotation taken");
    assert!((got[4] - 40.0).abs() < 1e-9, "the copy's X: {got:?}");
}

/// A ghost row's push deletes the one fill the copy removed from the main;
/// an instance that kept the main's list follows, one that edited its list
/// keeps its own.
/// flip: `item_removal`'s fills arm retaining nothing → the main loses both.
#[test]
fn a_pushed_removal_of_one_fill_leaves_the_other() {
    let mut f = fixture();
    let two = keyed_by_position([
        Fill {
            brush: solid(10).into(),
            visible: true,
        },
        Fill {
            brush: solid(20).into(),
            visible: true,
        },
    ]);
    let a = f.a;
    f.commit(vec![Operation::SetFills { id: a, fills: two }]);
    let fills = f.fills(f.ia);
    assert_eq!(fills.len(), 2, "the fixture: i's copy has both");
    let gone = fills[0].id;
    let ka = f.copy_of(f.k, a).unwrap();
    f.edit(vec![
        Operation::SetFills {
            id: f.ia,
            fills: fills[1..].to_vec(),
        },
        Operation::SetFills {
            id: ka,
            fills: fills_of(99),
        },
    ]);
    let list = Operation::SetFills {
        id: f.ia,
        fills: Vec::new(),
    };
    let tx = push::item_removal(&f.doc, f.ia, &list, gone).expect("a removal to push");
    f.commit(tx.0);
    assert_eq!(f.fills(a).len(), 1, "the main lost one");
    assert_eq!(f.fills(a)[0].id, fills[1].id, "the other stays");
    let ja = f.copy_of(f.j, a).unwrap();
    assert_eq!(f.fills(ja).len(), 1, "j follows");
    assert_eq!(f.fills(ka), fills_of(99), "k keeps its own");
}

/// Two local layers cannot be pushed, and each says why: a group wrapping a
/// linked copy, and a local instance of the very main. Neither counts as
/// pushable; both count as blocked.
#[test]
fn a_wrapping_group_and_a_nested_self_cannot_be_pushed() {
    let mut f = fixture();
    let g = f.ids.mint();
    f.edit(vec![
        create(g, f.i, 0, NodeKind::Group, "Wrap"),
        Operation::Reparent {
            id: f.ia,
            new_parent: g,
            index: 0,
        },
    ]);
    assert_eq!(
        push::layer_blocked(&f.doc, g),
        Some(LayerBlocked::WrapsLinked)
    );
    let mut ids = IdSource::new(0xB6);
    let own = instance(&mut f.doc, &mut ids, f.m, f.i);
    assert_eq!(
        push::layer_blocked(&f.doc, own),
        Some(LayerBlocked::HoldsMain)
    );
    let p = push::pushable(&f.doc, f.i);
    assert_eq!((p.layers, p.layers_blocked), (0, 2));
    assert!(push::push_layer(&f.doc, own, &mut f.ids).is_empty());
}

/// *Push properties*: a boolean property driving the title's visibility, turned
/// off on `i`, is a property and not a field; pushed, the main hides its title
/// and `j` follows.
#[test]
fn a_pushed_boolean_property_hides_the_main_layer() {
    let mut f = fixture();
    let item = f.ids.mint_item();
    let mut tx = Transaction(vec![Operation::SetProperties {
        id: f.m,
        props: vec![Keyed::new(
            item,
            Property {
                name: "Show title".into(),
                kind: PropKind::Boolean,
                bound: vec![f.t],
                filter: String::new(),
                preferred: Vec::new(),
            },
        )],
    }]);
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.doc.apply(&tx).unwrap();
    f.edit(vec![Operation::SetVisible {
        id: f.it,
        visible: false,
    }]);
    let p = push::pushable(&f.doc, f.i);
    assert_eq!((p.properties, p.fields), (1, 0));
    assert!(push::push(&f.doc, Kind::Fields, f.i, &mut f.ids).is_empty());
    f.push(Kind::Properties);
    assert!(!f.doc.get(f.t).unwrap().visible());
    let jt = f.copy_of(f.j, f.t).unwrap();
    assert!(!f.doc.get(jt).unwrap().visible(), "j followed");
}

/// *Push all* leaves the instance equal to its main but for what it removed:
/// a field, a local layer and an order pushed in one transaction.
#[test]
fn push_all_leaves_nothing_but_removals() {
    let mut f = fixture();
    let x = f.ids.mint();
    f.edit(vec![
        Operation::SetOpacity {
            id: f.ia,
            opacity: 0.5,
        },
        create(x, f.i, 2, rect(), "Badge"),
        Operation::Reorder { id: f.it, index: 0 },
    ]);
    f.push(Kind::All);
    let d = reset::drift(&f.doc, f.i);
    assert!(!d.any(), "{d:?}");
    assert_eq!(d.local, 0);
    assert_eq!(f.kids(f.m).len(), 3);
    assert_eq!(f.kids(f.j).len(), 3);
}

/// A swapped nested copy pushes its swap into its slot: the main's nested
/// instance is relinked to show what the copy showed, the copy's swap goes (it
/// equals its slot now), and the other instance shows it too.
#[test]
fn a_pushed_swap_relinks_the_mains_nested_instance() {
    let mut ids = IdSource::new(0xB7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [star, s1, heart, h1, button, label] = [(); 6].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(star, root, 0, frame(24.0, 24.0), "Icons / Star"),
        create(s1, star, 0, rect(), "Shape"),
        create(heart, root, 1, frame(24.0, 24.0), "Icons / Heart"),
        create(h1, heart, 0, rect(), "Shape"),
        create(button, root, 2, frame(120.0, 40.0), "Button"),
        create(label, button, 0, rect(), "Label"),
        Operation::SetComponent {
            id: star,
            component: true,
        },
        Operation::SetComponent {
            id: heart,
            component: true,
        },
    ]))
    .unwrap();
    let n = instance(&mut doc, &mut ids, star, button);
    doc.apply(&Transaction(vec![Operation::SetComponent {
        id: button,
        component: true,
    }]))
    .unwrap();
    let b1 = instance(&mut doc, &mut ids, button, root);
    let b2 = instance(&mut doc, &mut ids, button, root);
    let copy_in = |doc: &Document, b: NodeId| {
        doc.get(b)
            .unwrap()
            .children()
            .iter()
            .copied()
            .find(|c| doc.get(*c).and_then(|x| x.link()) == Some(n))
            .unwrap()
    };
    let r = copy_in(&doc, b1);
    let mut tx = swap::swap(&doc, r, heart).unwrap();
    ondin_core::propagate::owed(&doc, &mut tx, &mut ids);
    doc.apply(&tx).unwrap();
    assert_eq!(push::pushable(&doc, b1).fields, 1, "the swap is a field");
    // From the swapped copy itself — its Swap row's label — the swap pushes
    // into its slot's main, not into the Heart it shows.
    // flip: `push::inside` leaving out the slot's main → 0 from `r`.
    assert_eq!(push::pushable(&doc, r).fields, 1, "selected, it pushes too");
    let from_r = push::fields(&doc, &[Operation::SetSwap { id: r, swap: None }], &mut ids);
    assert!(
        from_r
            .iter()
            .any(|op| matches!(op, Operation::SetLink { id, .. } if *id == n)),
        "the slot relinked: {from_r:?}"
    );

    let mut tx = Transaction(push::push(&doc, Kind::Fields, b1, &mut ids));
    ondin_core::propagate::owed(&doc, &mut tx, &mut ids);
    doc.apply(&tx).expect("the pushed swap commits");
    io::load(&io::save(&doc).unwrap()).expect("reloads");
    assert_eq!(doc.get(n).unwrap().link(), Some(heart), "the slot relinked");
    assert_eq!(
        doc.get(r).unwrap().swap(),
        None,
        "b1's swap is the slot's now"
    );
    assert_eq!(ondin_core::component::main_of(&doc, r), Some(heart));
    let r2 = copy_in(&doc, b2);
    assert_eq!(ondin_core::component::main_of(&doc, r2), Some(heart));
    let _ = (s1, h1, label, root);
}

/// A kit instance cannot push: its main lives in the vault. Read here only as
/// the predicate's contract on a document with no vault — no kit, no block.
#[test]
fn an_ordinary_instance_is_not_blocked() {
    let f = fixture();
    assert_eq!(push::blocked(&f.doc, f.i), None);
    assert_eq!(push::home(&f.doc, f.i), Some(f.m));
    assert_eq!(push::home(&f.doc, f.root), None);
}

/// One field's push carries only what its reset would change: the width mark's
/// reset is the whole size with the copy's own height, so pushing it writes the
/// copy's width into the main and leaves the main's height. Flip: `field_push`
/// taking the copy's whole op → the main's height becomes 80.
#[test]
fn a_fields_push_carries_that_field_alone() {
    let mut f = fixture();
    f.edit(vec![Operation::SetGeometry {
        id: f.ia,
        geometry: ondin_core::GeometryPatch::Size(Size::new(30.0, 80.0)),
    }]);
    let width_reset = Operation::SetGeometry {
        id: f.ia,
        geometry: ondin_core::GeometryPatch::Size(Size::new(10.0, 80.0)),
    };
    let ops = push::fields(&f.doc, &[width_reset], &mut f.ids);
    f.commit(ops);
    match f.doc.get(f.a).unwrap().kind() {
        NodeKind::Rect { size, .. } => assert_eq!((size.width, size.height), (30.0, 10.0)),
        _ => unreachable!(),
    }
}
