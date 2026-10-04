//! Variants and component properties (§5.3d build step 7, §15 D982,
//! `ondin_core::variant`): the rules a set holds its variants to, the commit-time
//! settling, the set's edits, the in-place switch, and properties read as views
//! over fields.

use ondin_core::component::LinkRule;
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::variant::{
    self, PropKind, PropValue, Property, VariantProp, VariantRule, VariantSet,
};
use ondin_core::{
    Brush, Document, Fill, IdSource, Keyed, NodeId, NodeKind, OpError, Operation, Placement,
    Resolved, TextSizing, TextStyle, Transaction, io, keyed_by_position,
};

fn solid(r: u8) -> Brush {
    Brush::Solid(Color::from_rgba8(r, 0, 0, 255))
}

fn fills(r: u8) -> Vec<Keyed<Fill>> {
    keyed_by_position([Fill {
        brush: solid(r),
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

fn text(content: &str) -> NodeKind {
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

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// A set `s` — `Size: Small, Large` — holding two variants. `small` holds `bg`
/// (red) and `label` ("Go"); `large` holds `lbg` (blue), `llabel` ("Go") and
/// `shine`, which Small has no counterpart of. Plus an instance `i` of Small
/// under the root, made the way the app makes one.
struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    s: NodeId,
    small: NodeId,
    bg: NodeId,
    label: NodeId,
    large: NodeId,
    lbg: NodeId,
    llabel: NodeId,
    shine: NodeId,
    i: NodeId,
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xBA);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [s, small, bg, label, large, lbg, llabel, shine] = [(); 8].map(|_| ids.mint());
    let red = fills(10);
    let blue = fills(200);
    doc.apply(&Transaction(vec![
        create(s, root, 0, frame(400.0, 200.0), "Button"),
        create(small, s, 0, frame(80.0, 30.0), "Small"),
        create(bg, small, 0, rect(), "Bg"),
        create(label, small, 1, text("Go"), "Label"),
        create(large, s, 1, frame(120.0, 40.0), "Large"),
        create(lbg, large, 0, rect(), "Bg"),
        create(llabel, large, 1, text("Go"), "Label"),
        create(shine, large, 2, rect(), "Shine"),
        Operation::SetFills { id: bg, fills: red },
        Operation::SetFills {
            id: lbg,
            fills: blue,
        },
        Operation::SetTransform {
            id: large,
            transform: Affine::translate((0.0, 60.0)),
        },
        Operation::SetComponent {
            id: small,
            component: true,
        },
        Operation::SetComponent {
            id: large,
            component: true,
        },
        Operation::SetVariantSet {
            id: s,
            set: Some(VariantSet {
                props: vec![VariantProp {
                    name: "Size".into(),
                    values: strs(&["Small", "Large"]),
                }],
            }),
        },
        Operation::SetVariant {
            id: small,
            values: strs(&["Small"]),
        },
        Operation::SetVariant {
            id: large,
            values: strs(&["Large"]),
        },
    ]))
    .expect("a set of two variants");
    let (tx, made) = ondin_core::insert_subtrees(
        &doc,
        &mut ids,
        &[Placement {
            nodes: doc.capture_subtree(small).unwrap(),
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    doc.apply(&tx).expect("an instance of Small");
    F {
        doc,
        ids,
        root,
        s,
        small,
        bg,
        label,
        large,
        lbg,
        llabel,
        shine,
        i: made[0],
    }
}

impl F {
    /// `ops` with everything `EditorSession::commit_inner` appends, in its order.
    fn commit(&mut self, ops: Vec<Operation>) {
        let mut tx = Transaction(ops);
        let s = ondin_core::propagate::propagate_structure(&self.doc, &tx, &mut self.ids);
        tx.0.extend(s);
        let l = ondin_core::component::settle_links(&self.doc, &tx);
        tx.0.extend(l);
        let v = variant::settle(&self.doc, &tx);
        tx.0.extend(v);
        let p = ondin_core::propagate::propagate(&self.doc, &tx);
        tx.0.extend(p);
        self.doc
            .apply(&tx)
            .expect("the edit and everything it owes");
    }

    fn name(&self, id: NodeId) -> &str {
        self.doc.get(id).unwrap().name()
    }

    fn kids(&self, id: NodeId) -> Vec<NodeId> {
        self.doc.get(id).unwrap().children().to_vec()
    }

    fn link(&self, id: NodeId) -> Option<NodeId> {
        self.doc.get(id).unwrap().link()
    }

    fn fill(&self, id: NodeId) -> Brush {
        self.doc.get(id).unwrap().paint().fills[0].brush.clone()
    }

    fn content(&self, id: NodeId) -> String {
        match self.doc.get(id).unwrap().kind() {
            NodeKind::Text { content, .. } => content.clone(),
            _ => panic!("not text"),
        }
    }
}

fn refused(doc: &mut Document, ops: Vec<Operation>) -> VariantRule {
    match doc.apply(&Transaction(ops)) {
        Err(OpError::BadLink(_, LinkRule::Variant(rule))) => rule,
        other => panic!("expected a variant rule refused, got {other:?}"),
    }
}

#[test]
fn a_set_and_its_instance_round_trip_through_a_file() {
    let f = fixture();
    let bytes = io::save(&f.doc).unwrap();
    let loaded = io::load(&bytes).unwrap();
    assert_eq!(loaded.get(f.small).unwrap().variant(), strs(&["Small"]));
    assert_eq!(
        loaded.get(f.s).unwrap().set().unwrap().props[0].values,
        strs(&["Small", "Large"])
    );
    assert_eq!(io::save(&loaded).unwrap(), bytes);
}

/// Each rule refused at `apply`, one op from the valid fixture.
#[test]
fn the_variant_rules_are_held_after_the_last_op() {
    let mut f = fixture();
    // A value the set does not have.
    let rule = refused(
        &mut f.doc,
        vec![Operation::SetVariant {
            id: f.small,
            values: strs(&["Huge"]),
        }],
    );
    assert_eq!(rule, VariantRule::Values);
    // Values on a layer that is not a variant.
    let rule = refused(
        &mut f.doc,
        vec![Operation::SetVariant {
            id: f.bg,
            values: strs(&["Small"]),
        }],
    );
    assert_eq!(rule, VariantRule::Values);
    // A set inside a main.
    let rule = refused(
        &mut f.doc,
        vec![Operation::SetVariantSet {
            id: f.small,
            set: Some(VariantSet::default()),
        }],
    );
    assert_eq!(rule, VariantRule::SetKind);
    // A repeated value.
    let rule = refused(
        &mut f.doc,
        vec![Operation::SetVariantSet {
            id: f.s,
            set: Some(VariantSet {
                props: vec![VariantProp {
                    name: "Size".into(),
                    values: strs(&["Small", "Small", "Large"]),
                }],
            }),
        }],
    );
    assert_eq!(rule, VariantRule::SetNames);
    // Properties on a variant, which are its set's.
    let item = f.ids.mint_item();
    let prop = |bound: Vec<NodeId>| {
        vec![Keyed::new(
            item,
            Property {
                name: "Show".into(),
                kind: PropKind::Boolean,
                bound,
            },
        )]
    };
    let rule = refused(
        &mut f.doc,
        vec![Operation::SetProperties {
            id: f.small,
            props: prop(vec![f.bg]),
        }],
    );
    assert_eq!(rule, VariantRule::PropertyOwner);
    // A binding to a variant's own root, which is its placement.
    let rule = refused(
        &mut f.doc,
        vec![Operation::SetProperties {
            id: f.s,
            props: prop(vec![f.small]),
        }],
    );
    assert_eq!(rule, VariantRule::Binding);
    // And the valid one.
    f.doc
        .apply(&Transaction(vec![Operation::SetProperties {
            id: f.s,
            props: prop(vec![f.bg, f.lbg]),
        }]))
        .expect("a set's property bound inside its variants");
}

/// A main dragged into a set takes the first free combination and the name it
/// derives; dragged out, it drops them. Flip: `settle` answering nothing refuses
/// the drag-in (`Values`).
#[test]
fn a_main_moved_into_a_set_is_settled_as_a_variant() {
    let mut f = fixture();
    // A third value, so a free combination exists.
    let tx = variant::add_value(&f.doc, f.s, 0, "Huge").unwrap();
    f.commit(tx.0);
    let m = f.ids.mint();
    f.commit(vec![
        create(m, f.root, 1, frame(50.0, 50.0), "Loose"),
        Operation::SetComponent {
            id: m,
            component: true,
        },
    ]);
    f.commit(vec![Operation::Reparent {
        id: m,
        new_parent: f.s,
        index: 2,
    }]);
    assert_eq!(f.doc.get(m).unwrap().variant(), strs(&["Huge"]));
    assert_eq!(f.name(m), "Huge");
    f.commit(vec![Operation::Reparent {
        id: m,
        new_parent: f.root,
        index: 0,
    }]);
    assert!(f.doc.get(m).unwrap().variant().is_empty());
}

/// Renaming a value renames the variants holding it, and through the compare
/// rule the instances still carrying the variant's name (§15 D982: names are
/// copied verbatim, D979 (e)'s rule held). Flip: running `settle` after
/// `propagate` leaves the instance named "Small".
#[test]
fn renaming_a_value_renames_its_variants_and_their_instances() {
    let mut f = fixture();
    assert_eq!(
        f.name(f.i),
        "Small",
        "an instance's name is its main's, copied"
    );
    let tx = variant::rename_value(&f.doc, f.s, 0, 0, "Compact").unwrap();
    f.commit(tx.0);
    assert_eq!(f.doc.get(f.small).unwrap().variant(), strs(&["Compact"]));
    assert_eq!(f.name(f.small), "Compact");
    assert_eq!(f.name(f.i), "Compact");
}

/// Deleting a value deletes the variants holding it, and **their instances
/// detach** (§15 D979 (c), held over the mockup's relink).
#[test]
fn deleting_a_value_deletes_its_variants_and_detaches_their_instances() {
    let mut f = fixture();
    let tx = variant::delete_value(&f.doc, f.s, 0, 0).unwrap();
    f.commit(tx.0);
    assert!(f.doc.get(f.small).is_none());
    assert_eq!(f.link(f.i), None, "the instance is detached, not relinked");
    assert!(f.kids(f.i).iter().all(|k| f.link(*k).is_none()));
    assert_eq!(
        f.doc.get(f.s).unwrap().set().unwrap().props[0].values,
        strs(&["Large"])
    );
    // A property's last value cannot go.
    assert!(variant::delete_value(&f.doc, f.s, 0, 0).is_none());
}

/// `Ctrl+D` on a variant makes an **instance** of it (§15 D979 (e), held over the
/// mockup's next-free-combination), which sits in the set as a plain layer.
#[test]
fn a_copy_of_a_variant_is_an_instance_of_it() {
    let mut f = fixture();
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(f.small).unwrap(),
            parent: f.s,
            index: Some(1),
        }],
        Default::default(),
    );
    f.commit(tx.0);
    let copy = made[0];
    assert_eq!(f.link(copy), Some(f.small));
    assert!(!f.doc.get(copy).unwrap().component());
    assert!(f.doc.get(copy).unwrap().variant().is_empty());
    assert_eq!(variant::variants(&f.doc, f.s), vec![f.small, f.large]);
}

/// The switch: Small → Large, **in place**. The instance's own fill on Bg is
/// kept, Label's untouched content follows, Shine — new in Large — is copied in,
/// and every id that matched survives. Flip: carrying fields whole (taking
/// `s_new`'s value regardless) loses the override, failing on `bg`'s fill.
#[test]
fn switching_a_variant_carries_overrides_and_keeps_ids() {
    let mut f = fixture();
    let kids = f.kids(f.i);
    let (ibg, ilabel) = (kids[0], kids[1]);
    // The instance's own fill.
    // The same item as the main's (by position, as an edit keeps it), recoloured.
    let own = fills(77);
    f.commit(vec![Operation::SetFills {
        id: ibg,
        fills: own,
    }]);
    let tx = variant::switch(&f.doc, f.i, f.large, &mut f.ids).unwrap();
    f.commit(tx.0);
    assert_eq!(f.link(f.i), Some(f.large));
    assert_eq!(f.link(ibg), Some(f.lbg));
    assert_eq!(f.link(ilabel), Some(f.llabel));
    assert_eq!(f.fill(ibg), solid(77), "the instance's own fill carries");
    assert_eq!(
        f.name(f.i),
        "Large",
        "an unrenamed instance takes the new name"
    );
    let after = f.kids(f.i);
    assert_eq!(after.len(), 3, "Shine is copied in");
    assert_eq!(
        &after[..2],
        &[ibg, ilabel],
        "in the new main's order, ids kept"
    );
    assert_eq!(f.link(after[2]), Some(f.shine));
    match f.doc.get(f.i).unwrap().kind() {
        NodeKind::Artboard { size } => assert_eq!(*size, Size::new(120.0, 40.0)),
        _ => unreachable!(),
    }
    // And back: Shine is untouched, so it goes with its source's absence.
    let tx = variant::switch(&f.doc, f.i, f.small, &mut f.ids).unwrap();
    f.commit(tx.0);
    assert_eq!(f.kids(f.i), vec![ibg, ilabel]);
    assert_eq!(f.fill(ibg), solid(77));
    assert_eq!(
        ondin_core::reset::drift(&f.doc, f.i).fields,
        1,
        "only the fill"
    );
}

/// A counterpart with no match in the new variant that the instance changed is
/// kept as the instance's own layer, its edit with it — the design's 4I.
#[test]
fn an_unmatched_layer_the_instance_changed_is_kept_as_its_own() {
    let mut f = fixture();
    let tx = variant::switch(&f.doc, f.i, f.large, &mut f.ids).unwrap();
    f.commit(tx.0);
    let ishine = f.kids(f.i)[2];
    f.commit(vec![Operation::SetOpacity {
        id: ishine,
        opacity: 0.4,
    }]);
    let tx = variant::switch(&f.doc, f.i, f.small, &mut f.ids).unwrap();
    f.commit(tx.0);
    assert!(f.kids(f.i).contains(&ishine), "kept");
    assert_eq!(f.link(ishine), None, "as the instance's own");
    assert_eq!(f.doc.get(ishine).unwrap().opacity(), 0.4);
}

/// A boolean and a text property are views over fields: setting one on an
/// instance is an override of the bound field, the main's value is the default,
/// and resetting it puts the field back.
#[test]
fn a_property_is_a_view_over_its_bound_fields() {
    let mut f = fixture();
    let (tx, item) = variant::define(
        &f.doc,
        &mut f.ids,
        f.s,
        "Label text",
        PropKind::Text,
        vec![f.label, f.llabel],
    )
    .unwrap();
    f.commit(tx.0);
    let (tx, _) = variant::define(
        &f.doc,
        &mut f.ids,
        f.s,
        "Show bg",
        PropKind::Boolean,
        vec![f.bg, f.lbg],
    )
    .unwrap();
    f.commit(tx.0);
    let props = variant::instance_properties(&f.doc, f.i);
    assert_eq!(props.len(), 2);
    let text_prop = props.iter().find(|p| p.id == item).unwrap().value.clone();
    assert_eq!(
        variant::property_state(&f.doc, f.i, &text_prop),
        Some((PropValue::Text("Go".into()), false))
    );
    let ops = variant::set_property(
        &f.doc,
        &[f.i],
        &text_prop,
        &PropValue::Text("Sign up".into()),
    );
    f.commit(ops);
    let ilabel = f.kids(f.i)[1];
    assert_eq!(f.content(ilabel), "Sign up");
    assert_eq!(
        variant::property_state(&f.doc, f.i, &text_prop),
        Some((PropValue::Text("Sign up".into()), true))
    );
    // The property carries through a switch, with every other override.
    let tx = variant::switch(&f.doc, f.i, f.large, &mut f.ids).unwrap();
    f.commit(tx.0);
    assert_eq!(f.content(ilabel), "Sign up");
    let ops = variant::reset_property(&f.doc, &[f.i], &text_prop);
    f.commit(ops);
    assert_eq!(f.content(ilabel), "Go");
    assert_eq!(
        variant::property_state(&f.doc, f.i, &text_prop),
        Some((PropValue::Text("Go".into()), false))
    );
    // A bound layer deleted from the main is unbound by the settling.
    f.commit(vec![Operation::DeleteNode { id: f.label }]);
    let p = &f.doc.get(f.s).unwrap().props()[0];
    assert_eq!(p.bound, vec![f.llabel]);
}

/// *Combine as variants*: one property, each main's name a value, names kept.
#[test]
fn combining_mains_makes_a_set_of_their_names() {
    let mut ids = IdSource::new(0xBB);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [a, b] = [(); 2].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(a, root, 0, frame(10.0, 10.0), "Button/Primary"),
        create(b, root, 1, frame(10.0, 10.0), "Button/Ghost"),
        Operation::SetComponent {
            id: a,
            component: true,
        },
        Operation::SetComponent {
            id: b,
            component: true,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let (tx, set) = variant::combine(&doc, &res, &mut ids, &[b, a]).unwrap();
    doc.apply(&tx).expect("a set");
    let s = doc.get(set).unwrap();
    assert_eq!(s.name(), "Button");
    assert_eq!(
        s.set().unwrap().props[0].values,
        strs(&["Button/Primary", "Button/Ghost"])
    );
    assert_eq!(doc.get(a).unwrap().variant(), strs(&["Button/Primary"]));
    assert_eq!(variant::set_of(&doc, a), Some(set));
}

/// *Add variant* takes the first free combination and extends every binding to
/// the copy.
#[test]
fn adding_a_variant_takes_the_next_free_combination() {
    let mut f = fixture();
    let tx = variant::add_value(&f.doc, f.s, 0, "Huge").unwrap();
    f.commit(tx.0);
    let (tx, _) = variant::define(
        &f.doc,
        &mut f.ids,
        f.s,
        "Show bg",
        PropKind::Boolean,
        vec![f.bg],
    )
    .unwrap();
    f.commit(tx.0);
    let res = Resolved::rebuild(&f.doc);
    let (tx, new) = variant::add_variant(&f.doc, &res, &mut f.ids, f.s, Some(f.small)).unwrap();
    f.commit(tx.0);
    assert_eq!(f.doc.get(new).unwrap().variant(), strs(&["Huge"]));
    assert_eq!(f.name(new), "Huge");
    let bound = &f.doc.get(f.s).unwrap().props()[0].bound;
    assert_eq!(bound.len(), 2);
    assert_eq!(f.doc.get(bound[1]).unwrap().parent(), Some(new));
    assert!(variant::clashes(&f.doc, f.s).is_empty());
}
