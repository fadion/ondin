//! Nested component properties (§15 D988, `design/Variants.dc.html` 4K–4S): a
//! main's author shows a nested instance's own properties on every instance of
//! the main, and the instance's card lists them under a sub-heading — which
//! copies are shown, through how many levels, what counts and resets as a
//! property, the nested variant choice as an override, and the carry by property
//! name across a swap.

use ondin_core::component::LinkRule;
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::variant::{self, PropKind, PropValue, VariantProp, VariantSet};
use ondin_core::{
    Document, IdSource, NodeId, NodeKind, OpError, Operation, Placement, Transaction, swap,
};

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

fn weight() -> VariantSet {
    VariantSet {
        props: vec![VariantProp {
            name: "Weight".into(),
            values: vec!["Regular".into(), "Bold".into()],
        }],
    }
}

/// Two icon sets, **Arrow** and **Check**, each a `Weight` of Regular and Bold —
/// and a **Button** main holding `icon`, an instance of Arrow · Regular renamed
/// *Icon*, beside a `Label`. `b1` is an instance of Button and `r` its copy of
/// `icon`.
struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    a_reg: NodeId,
    a_bold: NodeId,
    c_reg: NodeId,
    c_bold: NodeId,
    button: NodeId,
    icon: NodeId,
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

fn child_linked(doc: &Document, parent: NodeId, src: NodeId) -> Option<NodeId> {
    doc.get(parent)?
        .children()
        .iter()
        .copied()
        .find(|c| doc.get(*c).and_then(|n| n.link()) == Some(src))
}

impl F {
    /// `ops` with everything `EditorSession::commit_inner` appends
    /// (`propagate::owed`, the one copy of its order).
    fn commit(&mut self, ops: Vec<Operation>) {
        let mut tx = Transaction(ops);
        ondin_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        self.doc
            .apply(&tx)
            .expect("the edit and everything it owes");
    }

    fn node(&self, id: NodeId) -> &ondin_core::Node {
        self.doc.get(id).unwrap()
    }

    fn show(&mut self, slot: NodeId, on: bool) {
        let tx = variant::set_shown(&self.doc, &mut self.ids, slot, on).expect("a change");
        self.commit(tx.0);
    }

    /// Switch the nested copy `r` to `Weight: value` within the set it shows.
    fn weight(&mut self, value: &str) {
        let shown = ondin_core::component::main_of(&self.doc, self.r).unwrap();
        let to = variant::switch_target(&self.doc, shown, 0, value).unwrap();
        let tx = variant::switch(&self.doc, self.r, to, &mut self.ids).expect("a switch");
        self.commit(tx.0);
    }
}

/// A set named `name` holding two variant frames, Regular and Bold, each with a
/// `Shape`; returns the two variants.
fn icon_set(f: &mut F, name: &str, index: usize) -> (NodeId, NodeId) {
    let [set, reg, reg_shape, bold, bold_shape] = [(); 5].map(|_| f.ids.mint());
    f.commit(vec![
        create(set, f.root, index, frame(200.0, 100.0), name),
        create(reg, set, 0, frame(24.0, 24.0), "Regular"),
        create(reg_shape, reg, 0, rect(), "Shape"),
        create(bold, set, 1, frame(24.0, 24.0), "Bold"),
        create(bold_shape, bold, 0, rect(), "Shape"),
        Operation::SetComponent {
            id: reg,
            component: true,
        },
        Operation::SetComponent {
            id: bold,
            component: true,
        },
        Operation::SetVariantSet {
            id: set,
            set: Some(weight()),
        },
        Operation::SetVariant {
            id: reg,
            values: vec!["Regular".into()],
        },
        Operation::SetVariant {
            id: bold,
            values: vec!["Bold".into()],
        },
    ]);
    (reg, bold)
}

fn fixture() -> F {
    let mut ids = IdSource::new(0x6E);
    let root = ids.mint();
    let doc = Document::new(root);
    let mut f = F {
        doc,
        ids,
        root,
        a_reg: root,
        a_bold: root,
        c_reg: root,
        c_bold: root,
        button: root,
        icon: root,
        b1: root,
        r: root,
    };
    (f.a_reg, f.a_bold) = icon_set(&mut f, "Arrow", 0);
    (f.c_reg, f.c_bold) = icon_set(&mut f, "Check", 1);
    let [button, label] = [(); 2].map(|_| f.ids.mint());
    f.commit(vec![
        create(button, root, 2, frame(120.0, 40.0), "Button"),
        create(label, button, 0, rect(), "Label"),
    ]);
    let (tx, icon) = instance(&f.doc, &mut f.ids, f.a_reg, button);
    f.commit(tx.0);
    f.commit(vec![
        Operation::SetName {
            id: icon,
            name: "Icon".into(),
        },
        Operation::SetComponent {
            id: button,
            component: true,
        },
    ]);
    let (tx, b1) = instance(&f.doc, &mut f.ids, button, root);
    f.commit(tx.0);
    f.r = child_linked(&f.doc, b1, icon).expect("b1's copy of Icon");
    f.button = button;
    f.icon = icon;
    f.b1 = b1;
    f
}

/// **A shown slot lists its copy on the instance**, and only once shown:
/// `shown_nested` is empty, then names `r` with the path and the name-path key
/// the card matches several instances by; and the last showing taken away takes
/// the owner's one `Nested` property with it.
#[test]
fn showing_a_slot_lists_its_copy_on_every_instance() {
    let mut f = fixture();
    assert!(variant::shown_nested(&f.doc, f.b1).is_empty());
    assert!(!variant::is_shown(&f.doc, f.r));
    f.show(f.icon, true);
    assert!(variant::slot_is_shown(&f.doc, f.icon));
    assert!(variant::is_shown(&f.doc, f.r));
    assert_eq!(
        variant::shown_nested(&f.doc, f.b1),
        vec![variant::ShownNested {
            copy: f.r,
            path: vec![f.r],
            key: vec![("Icon".to_string(), 0)],
        }]
    );
    assert_eq!(
        variant::nested_slots(&f.doc, f.button),
        vec![(f.icon, true)]
    );
    f.show(f.icon, false);
    assert!(variant::shown_nested(&f.doc, f.b1).is_empty());
    assert!(
        f.node(f.button).props().is_empty(),
        "the showing goes with its last slot"
    );
}

/// **A nested copy's variant choice is an override, marked and reset within its
/// set** (§15 D988's departure, 4N). `r` switched to Bold: its state names the
/// slot's Regular and differs; the reset swaps it back, which is no swap at all
/// since Regular is what the slot shows. A root linked straight to a main has no
/// state — *"a variant choice is never an override"* there (§15 D982).
#[test]
fn a_nested_variant_choice_is_an_override_reset_within_its_set() {
    let mut f = fixture();
    assert_eq!(
        variant::nested_variant_state(&f.doc, f.r, 0),
        Some(("Regular".to_string(), false))
    );
    f.weight("Bold");
    assert_eq!(f.node(f.r).swap(), Some(f.a_bold));
    assert_eq!(
        variant::nested_variant_state(&f.doc, f.r, 0),
        Some(("Regular".to_string(), true))
    );
    let tx = variant::nested_variant_reset(&f.doc, f.r, 0).expect("a reset");
    f.commit(tx.0);
    assert_eq!(f.node(f.r).swap(), None, "back to the slot's own main");
    assert_eq!(
        variant::nested_variant_state(&f.doc, f.icon, 0),
        None,
        "the slot itself is linked straight to a main"
    );
}

/// **A shown copy carries its values across a swap by property name** (4Q):
/// `r` at Arrow · Bold, swapped to Check · Regular, lands on Check · **Bold**;
/// not shown, it lands where it was sent. And the nested Weight row's reset after
/// that keeps the outer swap to Check — it switches within the set the copy shows
/// now. Flip: `carried_target` answering `to` always fails *"Weight carries"*.
#[test]
fn a_shown_copy_carries_its_values_across_a_swap_by_name() {
    let mut f = fixture();
    f.weight("Bold");
    assert_eq!(
        variant::carried_target(&f.doc, f.r, f.c_reg),
        f.c_reg,
        "not shown: no carry"
    );
    f.show(f.icon, true);
    assert_eq!(
        variant::carried_target(&f.doc, f.r, f.c_reg),
        f.c_bold,
        "Weight carries"
    );
    let tx = variant::swap_carrying(&f.doc, f.r, f.c_reg).unwrap();
    f.commit(tx.0);
    assert_eq!(f.node(f.r).swap(), Some(f.c_bold));
    let tx = variant::nested_variant_reset(&f.doc, f.r, 0).expect("Weight differs from the slot's");
    f.commit(tx.0);
    assert_eq!(
        f.node(f.r).swap(),
        Some(f.c_reg),
        "still Check, now Regular"
    );
}

/// **A shown row counts and resets as a property** (4N). Before showing, `r`'s
/// swap is an override and not a property field; shown, it is one, counted once
/// by `shown_rows_overridden` and taken back by `reset_all_properties`. Flip:
/// `property_fields` without the shown copies fails *"a property now"*.
#[test]
fn a_shown_row_counts_and_resets_as_a_property() {
    let mut f = fixture();
    f.weight("Bold");
    assert!(!variant::property_fields(&f.doc, f.b1).contains(&(f.r, PropKind::Swap)));
    assert_eq!(variant::shown_rows_overridden(&f.doc, f.b1), 0);
    f.show(f.icon, true);
    assert!(
        variant::property_fields(&f.doc, f.b1).contains(&(f.r, PropKind::Swap)),
        "a property now"
    );
    assert_eq!(variant::shown_rows_overridden(&f.doc, f.b1), 1);
    let ops = variant::reset_all_properties(&f.doc, f.b1);
    assert_eq!(
        ops,
        vec![Operation::SetSwap {
            id: f.r,
            swap: None
        }]
    );
    f.commit(ops);
    assert_eq!(variant::shown_rows_overridden(&f.doc, f.b1), 0);
}

/// **A slot an outer swap property drives is counted by the property, once** —
/// 4O's Leading and Trailing: `r` swapped through Button's *Icon* property
/// counts as that property and not again as a shown row.
#[test]
fn a_swap_property_counts_its_slot_once() {
    let mut f = fixture();
    let (tx, item) = variant::define(
        &f.doc,
        &mut f.ids,
        f.button,
        "Icon",
        PropKind::Swap,
        vec![f.icon],
    )
    .unwrap();
    f.commit(tx.0);
    f.show(f.icon, true);
    let p = variant::instance_properties(&f.doc, f.b1)
        .into_iter()
        .find(|p| p.id == item)
        .unwrap()
        .value;
    let ops = variant::set_property(&f.doc, &[f.b1], &p, &PropValue::Swap(f.c_reg));
    f.commit(ops);
    assert!(f.node(f.r).swap().is_some(), "the fixture swapped the copy");
    assert_eq!(variant::shown_rows_overridden(&f.doc, f.b1), 0);
}

/// **Hidden by an outer boolean** (4R): Button's *Show icon* bound to the slot,
/// set off on `b1`, names itself as what hides the group.
#[test]
fn a_hidden_group_names_the_property_that_hides_it() {
    let mut f = fixture();
    let (tx, item) = variant::define(
        &f.doc,
        &mut f.ids,
        f.button,
        "Show icon",
        PropKind::Boolean,
        vec![f.icon],
    )
    .unwrap();
    f.commit(tx.0);
    f.show(f.icon, true);
    let g = variant::shown_nested(&f.doc, f.b1).remove(0);
    assert_eq!(variant::hidden_by(&f.doc, f.b1, &g), None);
    let p = variant::instance_properties(&f.doc, f.b1)
        .into_iter()
        .find(|p| p.id == item)
        .unwrap()
        .value;
    let ops = variant::set_property(&f.doc, &[f.b1], &p, &PropValue::Boolean(false));
    f.commit(ops);
    assert_eq!(
        variant::hidden_by(&f.doc, f.b1, &g),
        Some("Show icon".to_string())
    );
}

/// **Two levels deep** (4P): a Card main showing its nested Button, whose main
/// shows Icon, lists both groups on a Card instance — Action's, then Icon's with
/// a two-step path and key. Flip: `shown_below` not recursing loses the second.
#[test]
fn the_showing_carries_through_each_level() {
    let mut f = fixture();
    f.show(f.icon, true);
    let card = f.ids.mint();
    f.commit(vec![create(card, f.root, 3, frame(300.0, 200.0), "Card")]);
    let (tx, action) = instance(&f.doc, &mut f.ids, f.button, card);
    f.commit(tx.0);
    f.commit(vec![
        Operation::SetName {
            id: action,
            name: "Action".into(),
        },
        Operation::SetComponent {
            id: card,
            component: true,
        },
    ]);
    f.show(action, true);
    let (tx, c1) = instance(&f.doc, &mut f.ids, card, f.root);
    f.commit(tx.0);
    let a1 = child_linked(&f.doc, c1, action).unwrap();
    let shown = variant::shown_nested(&f.doc, c1);
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert_eq!(shown[0].copy, a1);
    assert_eq!(shown[1].path.len(), 2);
    assert_eq!(shown[1].path[0], a1);
    assert_eq!(
        shown[1].key,
        vec![("Action".to_string(), 0), ("Icon".to_string(), 0)]
    );
}

/// **Two nested instances of one name are two groups** — an instance takes its
/// main's name verbatim (§15 D982), so a second Arrow dropped into the Button is
/// *Regular* beside the renamed *Icon*'s twin until renamed. Both shown, their
/// keys are the first and second sibling of that name, and each group names its
/// own copy. Flip: `name_path` counting every sibling as the 0th gives both one
/// key and fails *"two keys"* — the card then drew the first's rows twice.
#[test]
fn two_same_named_nested_instances_are_two_groups() {
    let mut f = fixture();
    let (tx, second) = instance(&f.doc, &mut f.ids, f.a_reg, f.button);
    f.commit(tx.0);
    f.commit(vec![Operation::SetName {
        id: second,
        name: "Icon".into(),
    }]);
    f.show(f.icon, true);
    f.show(second, true);
    let r2 = child_linked(&f.doc, f.b1, second).expect("b1's copy of the second Icon");
    let shown = variant::shown_nested(&f.doc, f.b1);
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert_ne!(shown[0].key, shown[1].key, "two keys");
    let copies: Vec<NodeId> = shown.iter().map(|s| s.copy).collect();
    assert!(copies.contains(&f.r) && copies.contains(&r2));
}

/// **The rules**: one showing per owner, and a showing binds only a nested
/// instance — two `Nested` properties, or one bound to the plain `Label`, are
/// refused at the commit.
#[test]
fn a_showing_is_one_per_owner_and_binds_only_a_nested_instance() {
    let mut f = fixture();
    let label = f.node(f.button).children()[0];
    let nested = |item, bound| {
        ondin_core::Keyed::new(
            item,
            variant::Property {
                name: String::new(),
                kind: PropKind::Nested,
                bound,
                filter: String::new(),
            },
        )
    };
    let two = vec![
        nested(f.ids.mint_item(), vec![f.icon]),
        nested(f.ids.mint_item(), vec![]),
    ];
    match f.doc.apply(&Transaction(vec![Operation::SetProperties {
        id: f.button,
        props: two,
    }])) {
        Err(OpError::BadLink(_, LinkRule::Variant(variant::VariantRule::PropertyName))) => {}
        other => panic!("expected two showings refused, got {other:?}"),
    }
    let plain = vec![nested(f.ids.mint_item(), vec![label])];
    match f.doc.apply(&Transaction(vec![Operation::SetProperties {
        id: f.button,
        props: plain,
    }])) {
        Err(OpError::BadLink(_, LinkRule::Variant(variant::VariantRule::Binding))) => {}
        other => panic!("expected the binding refused, got {other:?}"),
    }
}

/// **A deleted slot leaves the showing** — `variant::settle`'s pruning, which
/// the showing shares with every property — and the showing, empty, goes with
/// it. Flip: dropping `settle`'s empty-showing `retain` leaves `Some([])` and
/// fails here.
#[test]
fn deleting_a_shown_slot_unbinds_it() {
    let mut f = fixture();
    f.show(f.icon, true);
    f.commit(vec![Operation::DeleteNode { id: f.icon }]);
    let showing = f
        .node(f.button)
        .props()
        .iter()
        .find(|p| p.kind == PropKind::Nested)
        .map(|p| p.bound.clone());
    assert_eq!(
        showing, None,
        "a showing with nothing left goes, as `set_shown` takes it with its last slot"
    );
}

/// The fixture is what it says: `r` is a nested copy that can swap, its slot a
/// root linked straight to Arrow · Regular, and the two sets' Bold variants are
/// where `Weight: Bold` goes.
#[test]
fn the_fixture_is_what_it_says() {
    let f = fixture();
    assert_eq!(f.node(f.r).link(), Some(f.icon));
    assert_eq!(f.node(f.icon).link(), Some(f.a_reg));
    assert!(swap::can_swap(&f.doc, f.r));
    assert_eq!(
        variant::switch_target(&f.doc, f.a_reg, 0, "Bold"),
        Some(f.a_bold)
    );
    assert_eq!(
        variant::switch_target(&f.doc, f.c_reg, 0, "Bold"),
        Some(f.c_bold)
    );
}
