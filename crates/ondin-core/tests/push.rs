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

/// A locked main takes no push: [`push::blocked`] says why and every push is
/// empty.
#[test]
fn a_locked_main_refuses_every_push() {
    let mut f = fixture();
    f.edit(vec![
        Operation::SetOpacity {
            id: f.ia,
            opacity: 0.5,
        },
        Operation::SetLocked {
            id: f.m,
            locked: true,
        },
    ]);
    assert_eq!(push::blocked(&f.doc, f.i), Some(Blocked::Locked));
    assert!(push::push(&f.doc, Kind::All, f.i, &mut f.ids).is_empty());
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
