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
        brush: solid(r).into(),
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
        corner_smoothing: 0.0,
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
    /// `ops` with everything `EditorSession::commit_inner` appends
    /// (`propagate::owed`, the one copy of its order).
    fn commit(&mut self, ops: Vec<Operation>) {
        let mut tx = Transaction(ops);
        ondin_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
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
        self.doc.get(id).unwrap().paint().fills[0]
            .brush
            .literal()
            .unwrap()
            .clone()
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
                filter: String::new(),
                preferred: Vec::new(),
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
///
/// And it brings its properties: a main carrying one, dragged in, gives it to the
/// set — every route into a set (*Combine as variants* too) goes through
/// `settle`'s merge. Flip, run: the merge's `if grew {` made `if false && grew {`
/// — the variant's own list still emptied — fails the set's props, 0 for 1: the
/// definition and its binding silently gone (and
/// `combining_mains_whose_properties_share_an_id_keys_them_apart` with it, so
/// *Combine*'s route is covered too). A second flip, the `SetName` arm dropped
/// from `can_break`, fails the hand rename's assertion, `"Tiny"` for `"Small"`.
#[test]
fn a_main_moved_into_a_set_is_settled_as_a_variant() {
    let mut f = fixture();
    // A third value, so a free combination exists.
    let tx = variant::add_value(&f.doc, f.s, 0, "Huge").unwrap();
    f.commit(tx.0);
    let [m, t] = [(); 2].map(|_| f.ids.mint());
    f.commit(vec![
        create(m, f.root, 1, frame(50.0, 50.0), "Loose"),
        create(t, m, 0, text("Go"), "Label"),
        Operation::SetComponent {
            id: m,
            component: true,
        },
    ]);
    // A property of its own, bound inside it.
    let (tx, item) =
        variant::define(&f.doc, &mut f.ids, m, "Label text", PropKind::Text, vec![t]).unwrap();
    f.commit(tx.0);
    f.commit(vec![Operation::Reparent {
        id: m,
        new_parent: f.s,
        index: 2,
    }]);
    assert_eq!(f.doc.get(m).unwrap().variant(), strs(&["Huge"]));
    assert_eq!(f.name(m), "Huge");
    // Its property is the set's now, binding and id with it; the variant holds
    // none of its own.
    let props = f.doc.get(f.s).unwrap().props();
    assert_eq!(props.len(), 1, "{props:?}");
    assert_eq!(
        (props[0].id, props[0].name.as_str(), props[0].kind),
        (item, "Label text", PropKind::Text)
    );
    assert_eq!(props[0].bound, vec![t]);
    assert!(f.doc.get(m).unwrap().props().is_empty());
    // A variant renamed by hand takes its derived name back.
    f.commit(vec![Operation::SetName {
        id: f.small,
        name: "Tiny".into(),
    }]);
    assert_eq!(f.name(f.small), "Small");
    f.commit(vec![Operation::Reparent {
        id: m,
        new_parent: f.root,
        index: 0,
    }]);
    assert!(f.doc.get(m).unwrap().variant().is_empty());
}

/// Renaming a value renames the variants holding it, and through the compare
/// rule the instances still carrying the variant's name (names are copied
/// verbatim — §5.3d's rule under D979 (e), held by D982). Flip, run: `settle`
/// after `propagate` **in `propagate::owed`** — the order the app's commit runs,
/// not a copy of it — fails the instance's name, "Small".
/// Flip, run: the remap closure an identity (`if false && v.get(prop) ==
/// Some(&old)`) passes the value-0 half and fails at Large's values,
/// `["Compact"]` for `["Big"]` — Large re-valued by the fallback to the first
/// value, a clash with Small.
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
    // And a value that is not the first: renaming value 0 is the one case
    // `settle`'s first-value fallback answers the same as the remap, so a
    // `rename_value` writing the set and leaving the variants alone would pass it.
    let tx = variant::rename_value(&f.doc, f.s, 0, 1, "Big").unwrap();
    f.commit(tx.0);
    assert_eq!(f.doc.get(f.large).unwrap().variant(), strs(&["Big"]));
    assert_eq!(f.name(f.large), "Big");
    assert!(variant::clashes(&f.doc, f.s).is_empty());
}

/// Deleting a value deletes the variants holding it, and **their instances
/// detach** (§15 D979 (c)) — and keep their layers, compared whole: an `all()`
/// over the children an emptied instance satisfies. Flip, run:
/// `propagate_structure`'s deleted-parent `partition` made `true || …` (the D984
/// defect, a deleted main's children read as losses) fails the `kids == before`
/// assertion, `[]` for the two layers.
#[test]
fn deleting_a_value_deletes_its_variants_and_detaches_their_instances() {
    let mut f = fixture();
    let before = f.kids(f.i);
    assert_eq!(before.len(), 2);
    let tx = variant::delete_value(&f.doc, f.s, 0, 0).unwrap();
    f.commit(tx.0);
    assert!(f.doc.get(f.small).is_none());
    assert_eq!(f.link(f.i), None, "the instance is detached, not relinked");
    assert_eq!(f.kids(f.i), before, "and keeps its look, every layer");
    assert!(f.kids(f.i).iter().all(|k| f.link(*k).is_none()));
    assert_eq!(
        f.doc.get(f.s).unwrap().set().unwrap().props[0].values,
        strs(&["Large"])
    );
    // A property's last value cannot go.
    assert!(variant::delete_value(&f.doc, f.s, 0, 0).is_none());
}

/// The fixture's set given a second property, `State: Default, Hover`, with
/// Large on Hover — the shape the set's property verbs are told apart on.
fn two_properties(f: &mut F) {
    let tx = variant::add_property(&f.doc, f.s, "State", "Default").unwrap();
    f.commit(tx.0);
    let tx = variant::add_value(&f.doc, f.s, 1, "Hover").unwrap();
    f.commit(tx.0);
    let tx = variant::set_values(&f.doc, f.large, strs(&["Large", "Hover"])).unwrap();
    f.commit(tx.0);
}

/// *Add property* gives every variant its one value, renaming them and the
/// instances that carry their names; a name a variant property or a component
/// property already has is refused.
///
/// Flip, run: `add_property`'s remap inserting the value first
/// (`v.insert(0, value)` for `v.push(value)`) passes Small — `settle` repairs
/// its misfit `["Default", "Small"]` position by position to the first values,
/// which are Small's — and fails at Large's, `["Small", "Default"]` for
/// `["Large", "Default"]`: a clash, and Large renamed "Small, Default".
#[test]
fn adding_a_property_gives_every_variant_its_value() {
    let mut f = fixture();
    let tx = variant::add_property(&f.doc, f.s, "State", "Default").unwrap();
    f.commit(tx.0);
    let set = f.doc.get(f.s).unwrap().set().unwrap().clone();
    assert_eq!(set.props[1].name, "State");
    assert_eq!(set.props[1].values, strs(&["Default"]));
    assert_eq!(
        f.doc.get(f.small).unwrap().variant(),
        strs(&["Small", "Default"])
    );
    assert_eq!(
        f.doc.get(f.large).unwrap().variant(),
        strs(&["Large", "Default"])
    );
    assert_eq!(f.name(f.small), "Small, Default");
    assert_eq!(f.name(f.i), "Small, Default", "the instance follows");
    assert!(variant::add_property(&f.doc, f.s, "Size", "X").is_none());
    let (tx, _) = variant::define(
        &f.doc,
        &mut f.ids,
        f.s,
        "Label text",
        PropKind::Text,
        vec![f.label, f.llabel],
    )
    .unwrap();
    f.commit(tx.0);
    assert!(variant::add_property(&f.doc, f.s, "Label text", "X").is_none());
}

/// *Delete property* drops exactly that position from every variant, and is
/// refused for the last property. Deleting the **second** one, so that dropping
/// the wrong position shows.
///
/// Flip, run: `delete_property`'s remap `v.remove(0)` for `v.remove(prop)`
/// passes Small (its `["Default"]` misfits and `settle` falls back to the first
/// value, Small) and fails at Large's, `["Small"]` for `["Large"]`.
#[test]
fn deleting_a_property_drops_its_position_from_every_variant() {
    let mut f = fixture();
    two_properties(&mut f);
    assert_eq!(f.name(f.large), "Large, Hover");
    let tx = variant::delete_property(&f.doc, f.s, 1).unwrap();
    f.commit(tx.0);
    assert_eq!(f.doc.get(f.small).unwrap().variant(), strs(&["Small"]));
    assert_eq!(f.doc.get(f.large).unwrap().variant(), strs(&["Large"]));
    assert_eq!(f.name(f.large), "Large");
    assert_eq!(f.name(f.i), "Small");
    let set = f.doc.get(f.s).unwrap().set().unwrap().clone();
    assert_eq!(set.props.len(), 1);
    assert_eq!(set.props[0].name, "Size");
    assert!(
        variant::delete_property(&f.doc, f.s, 0).is_none(),
        "the last"
    );
}

/// *Rename property* renames it in the set and leaves every variant's values
/// alone; a name another variant property or one of the set's component
/// properties has is refused.
///
/// Flip, run: the component-property half of the clash test dropped (`|| false`
/// for `|| s.props.iter().any(..)`) fails the last assertion — the rename to
/// *Label text* is offered.
#[test]
fn renaming_a_property_refuses_a_name_taken_either_way() {
    let mut f = fixture();
    two_properties(&mut f);
    let tx = variant::rename_property(&f.doc, f.s, 1, "Mode").unwrap();
    f.commit(tx.0);
    assert_eq!(f.doc.get(f.s).unwrap().set().unwrap().props[1].name, "Mode");
    assert_eq!(
        f.doc.get(f.large).unwrap().variant(),
        strs(&["Large", "Hover"])
    );
    assert!(variant::rename_property(&f.doc, f.s, 1, "Size").is_none());
    assert!(variant::rename_property(&f.doc, f.s, 1, "  ").is_none());
    let (tx, _) = variant::define(
        &f.doc,
        &mut f.ids,
        f.s,
        "Label text",
        PropKind::Text,
        vec![f.label, f.llabel],
    )
    .unwrap();
    f.commit(tx.0);
    assert!(variant::rename_property(&f.doc, f.s, 1, "Label text").is_none());
}

/// *Move value* reorders the set's values and leaves the variants' alone. Three
/// values, so a move is not a swap.
///
/// Flip, run: `p.values.swap(from, to)` for the remove-and-insert fails the
/// set's order, `["Huge", "Large", "Small"]` for `["Huge", "Small", "Large"]`.
#[test]
fn moving_a_value_reorders_the_set_and_not_the_variants() {
    let mut f = fixture();
    let tx = variant::add_value(&f.doc, f.s, 0, "Huge").unwrap();
    f.commit(tx.0);
    let tx = variant::move_value(&f.doc, f.s, 0, 2, 0).unwrap();
    f.commit(tx.0);
    assert_eq!(
        f.doc.get(f.s).unwrap().set().unwrap().props[0].values,
        strs(&["Huge", "Small", "Large"])
    );
    assert_eq!(f.doc.get(f.small).unwrap().variant(), strs(&["Small"]));
    assert_eq!(f.doc.get(f.large).unwrap().variant(), strs(&["Large"]));
    assert_eq!(f.name(f.small), "Small");
    assert!(
        variant::move_value(&f.doc, f.s, 0, 1, 1).is_none(),
        "no move"
    );
    assert!(
        variant::move_value(&f.doc, f.s, 0, 0, 3).is_none(),
        "past the end"
    );
}

/// A variant card's dropdown: values the set has are written — a clash allowed,
/// which the set shows — and a value it lacks, or a list of the wrong length,
/// is refused.
///
/// Flip, run: the `p.values.contains(v)` test dropped from `set_values` fails
/// the refusal of `Huge`.
#[test]
fn setting_a_variants_values_refuses_one_the_set_lacks() {
    let mut f = fixture();
    assert!(variant::set_values(&f.doc, f.small, strs(&["Huge"])).is_none());
    assert!(variant::set_values(&f.doc, f.small, strs(&["Small", "Large"])).is_none());
    let tx = variant::set_values(&f.doc, f.small, strs(&["Large"])).unwrap();
    f.commit(tx.0);
    assert_eq!(f.doc.get(f.small).unwrap().variant(), strs(&["Large"]));
    assert_eq!(f.name(f.small), "Large");
    assert!(!variant::clashes(&f.doc, f.s).is_empty(), "the clash shows");
}

/// `edit_property` rewrites, or with `None` removes, exactly the item it was
/// given, by id. The second of two, so a position-0 removal shows.
///
/// Flip, run: `props.remove(0)` for `props.remove(at)` fails the last
/// assertion, the survivor *Show bg* for *Caption*.
#[test]
fn editing_a_property_touches_only_its_item() {
    let mut f = fixture();
    let define = |f: &mut F, name: &str, kind, bound: Vec<NodeId>| {
        let (tx, item) = variant::define(&f.doc, &mut f.ids, f.s, name, kind, bound).unwrap();
        f.commit(tx.0);
        item
    };
    let (texts, bgs) = (vec![f.label, f.llabel], vec![f.bg, f.lbg]);
    let text_item = define(&mut f, "Label text", PropKind::Text, texts);
    let bool_item = define(&mut f, "Show bg", PropKind::Boolean, bgs);
    let tx = variant::edit_property(&f.doc, f.s, text_item, |p| {
        Some(Property {
            name: "Caption".into(),
            ..p.clone()
        })
    })
    .unwrap();
    f.commit(tx.0);
    let names = |f: &F| -> Vec<(ondin_core::ItemId, String)> {
        f.doc
            .get(f.s)
            .unwrap()
            .props()
            .iter()
            .map(|p| (p.id, p.name.clone()))
            .collect()
    };
    assert_eq!(
        names(&f),
        vec![(text_item, "Caption".into()), (bool_item, "Show bg".into())]
    );
    let tx = variant::edit_property(&f.doc, f.s, bool_item, |_| None).unwrap();
    f.commit(tx.0);
    assert_eq!(names(&f), vec![(text_item, "Caption".into())]);
}

/// `Ctrl+D` on a variant makes an **instance** of it (§15 D979 (e)), not a new
/// combination, which sits in the set as a plain layer.
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

/// **Variants made apart share no item ids**, and a switch matches their fills by
/// position (`rekey_lists`). A fixture keyed `keyed_by_position` gives Small's
/// and Large's `Bg` fill one id already, so the rekeying is a no-op there. Here
/// each variant's fill is minted, as the inspector mints one: the instance's
/// recoloured fill carries as its one fill, and an untouched instance takes
/// Large's.
///
/// Flip, run: `rekey_lists` returning `(old, cur)` unchanged fails the
/// one-fill assertion — Large's blue arrives as a second item beside the
/// instance's own 77, a list override the user never made.
#[test]
fn a_switch_carries_a_fill_override_between_variants_made_apart() {
    let mut f = fixture();
    let minted = |ids: &mut IdSource, r: u8| {
        vec![Keyed::new(
            ids.mint_item(),
            Fill {
                brush: solid(r).into(),
                visible: true,
            },
        )]
    };
    let (small_fill, large_fill) = (minted(&mut f.ids, 10), minted(&mut f.ids, 200));
    assert_ne!(small_fill[0].id, large_fill[0].id, "made apart");
    f.commit(vec![
        Operation::SetFills {
            id: f.bg,
            fills: small_fill.clone(),
        },
        Operation::SetFills {
            id: f.lbg,
            fills: large_fill,
        },
    ]);
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(f.small).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.commit(tx.0);
    let untouched = made[0];
    let ibg = f.kids(f.i)[0];
    assert_eq!(
        f.doc.get(ibg).unwrap().paint().fills[0].id,
        small_fill[0].id,
        "the copy follows the main's item"
    );
    // The instance's own colour, on the main's item.
    f.commit(vec![Operation::SetFills {
        id: ibg,
        fills: vec![Keyed::new(
            small_fill[0].id,
            Fill {
                brush: solid(77).into(),
                visible: true,
            },
        )],
    }]);
    for root in [f.i, untouched] {
        let tx = variant::switch(&f.doc, root, f.large, &mut f.ids).unwrap();
        f.commit(tx.0);
    }
    let fills_of = |f: &F, id: NodeId| -> Vec<Brush> {
        f.doc
            .get(id)
            .unwrap()
            .paint()
            .fills
            .iter()
            .map(|k| k.value.brush.literal().unwrap().clone())
            .collect()
    };
    assert_eq!(
        fills_of(&f, ibg),
        vec![solid(77)],
        "the override carries, one fill"
    );
    let ubg = f.kids(untouched)[0];
    assert_eq!(
        fills_of(&f, ubg),
        vec![solid(200)],
        "an untouched one takes Large's"
    );
}

/// A counterpart with no match in the new variant that the instance changed is
/// kept as the instance's own layer, its edit with it.
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
///
/// Both kinds, so the `Boolean` arm of `property_reset_ops` is covered as well
/// as the text one. Flip, run: that arm made
/// `false && matches!(op, SetVisible { .. })` fails the boolean's
/// `property_state` after it is set off — `(false, false)` for `(false, true)`,
/// no override seen — before the reset is reached.
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
    let (tx, bool_item) = variant::define(
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
    // The boolean half: set off, it is an override of the bound
    // layer's visibility — the dot — and its reset puts the layer back.
    let bool_prop = variant::instance_properties(&f.doc, f.i)
        .into_iter()
        .find(|p| p.id == bool_item)
        .unwrap()
        .value;
    let ibg = f.kids(f.i)[0];
    assert_eq!(
        variant::property_state(&f.doc, f.i, &bool_prop),
        Some((PropValue::Boolean(true), false))
    );
    let ops = variant::set_property(&f.doc, &[f.i], &bool_prop, &PropValue::Boolean(false));
    f.commit(ops);
    assert!(!f.doc.get(ibg).unwrap().visible());
    assert_eq!(
        variant::property_state(&f.doc, f.i, &bool_prop),
        Some((PropValue::Boolean(false), true))
    );
    let ops = variant::reset_property(&f.doc, &[f.i], &bool_prop);
    f.commit(ops);
    assert!(
        f.doc.get(ibg).unwrap().visible(),
        "the reset shows Bg again"
    );
    assert_eq!(
        variant::property_state(&f.doc, f.i, &bool_prop),
        Some((PropValue::Boolean(true), false))
    );
    // A bound layer deleted from the main is unbound by the settling.
    f.commit(vec![Operation::DeleteNode { id: f.label }]);
    let p = &f.doc.get(f.s).unwrap().props()[0];
    assert_eq!(p.bound, vec![f.llabel]);
}

/// **A main's or a set's description is its own**: written by one op and undone
/// by its inverse, saved and loaded, never propagated to an instance, and never
/// copied onto a new one.
///
/// flip: `make_copy_of` keeping `description` → fails at "a new instance".
#[test]
fn a_description_stays_on_what_it_describes() {
    use ondin_core::Described;
    let mut f = fixture();
    let (main, set) = (f.small, f.s);
    f.commit(vec![
        Operation::SetDescription {
            of: Described::Node(main),
            text: "The compact button".into(),
        },
        Operation::SetDescription {
            of: Described::Node(set),
            text: "Buttons".into(),
        },
    ]);
    assert_eq!(f.doc.get(main).unwrap().description(), "The compact button");
    assert_eq!(f.doc.get(f.i).unwrap().description(), "", "not propagated");
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(main).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(made[0]).unwrap().description(),
        "",
        "a new instance"
    );

    let loaded = io::load(&io::save(&f.doc).unwrap()).unwrap();
    assert_eq!(loaded.get(set).unwrap().description(), "Buttons");

    let before = f.doc.clone();
    let out = f
        .doc
        .apply(&Transaction(vec![Operation::SetDescription {
            of: Described::Node(set),
            text: String::new(),
        }]))
        .unwrap();
    assert_eq!(f.doc.get(set).unwrap().description(), "");
    f.doc.apply(&out.inverse).unwrap();
    assert_eq!(f.doc, before);
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

/// *Combine as variants* reads `Key=Value` names as properties: keys in the order
/// first written, a key a name leaves out reading `Default`, the shared path
/// naming the set. One name that is not all pairs keeps the whole-name property.
///
/// flip: `named_values` answering `None` always → fails at the property names.
#[test]
fn combining_mains_named_as_properties_makes_those_properties() {
    let combined = |names: &[&str]| {
        let mut ids = IdSource::new(0xBD);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mains: Vec<NodeId> = names.iter().map(|_| ids.mint()).collect();
        let mut ops = Vec::new();
        for (i, (m, n)) in mains.iter().zip(names).enumerate() {
            ops.push(create(*m, root, i, frame(10.0, 10.0), n));
            ops.push(Operation::SetComponent {
                id: *m,
                component: true,
            });
        }
        doc.apply(&Transaction(ops)).unwrap();
        let res = Resolved::rebuild(&doc);
        let (tx, set) = variant::combine(&doc, &res, &mut ids, &mains).unwrap();
        doc.apply(&tx).expect("a set");
        (doc, set, mains)
    };
    let (doc, set, m) = combined(&[
        "Button/Size=L, State=Hover",
        "Button/Size=S,State=Rest",
        "Button/ Size = S",
    ]);
    let s = doc.get(set).unwrap();
    assert_eq!(s.name(), "Button");
    let props = &s.set().unwrap().props;
    assert_eq!(
        props.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Size", "State"]
    );
    assert_eq!(props[0].values, strs(&["L", "S"]));
    assert_eq!(props[1].values, strs(&["Hover", "Rest", "Default"]));
    assert_eq!(doc.get(m[0]).unwrap().variant(), strs(&["L", "Hover"]));
    assert_eq!(doc.get(m[2]).unwrap().variant(), strs(&["S", "Default"]));

    let (doc, set, _) = combined(&["Size=L", "Large"]);
    let s = doc.get(set).unwrap();
    assert_eq!(s.set().unwrap().props[0].name, "Property 1");
    assert_eq!(s.set().unwrap().props[0].values, strs(&["Size=L", "Large"]));
}

/// **A new set leaves `SET_PAD` of air round its variants** (§15 D998): one 40×20
/// main at (100, 50) becomes a 60×40 set at (90, 40), and the main stays where it
/// was on the page — 10 in from each of the set's edges. The maintainer's report:
/// a set that hugged its one main could not be picked on the canvas, its tag under
/// the main's.
///
/// **Flip run**, the padding loop in `combine` deleted: fails at the set's size
/// with 40×20, the predicted site.
#[test]
fn a_new_set_pads_its_variants_and_moves_nothing() {
    let mut ids = IdSource::new(0xBC);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let a = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: a,
            parent: root,
            index: 0,
            kind: frame(40.0, 20.0),
            transform: Some(Affine::translate((100.0, 50.0))),
            name: Some("Card".into()),
        },
        Operation::SetComponent {
            id: a,
            component: true,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let before = res.world_bounds(a).unwrap();
    let (tx, set) = variant::combine(&doc, &res, &mut ids, &[a]).unwrap();
    doc.apply(&tx).expect("a set");
    let res = Resolved::rebuild(&doc);
    let s = res.world_bounds(set).unwrap();
    assert_eq!(
        (s.width(), s.height()),
        (40.0 + 2.0 * variant::SET_PAD, 20.0 + 2.0 * variant::SET_PAD),
        "the set's size"
    );
    assert_eq!((s.x0, s.y0), (90.0, 40.0), "the set's place");
    assert_eq!(
        res.world_bounds(a).unwrap(),
        before,
        "the main did not move"
    );

    // **And a second variant keeps the same air under it** (§15 D1000): the copy
    // goes `VARIANT_GAP` below the first, and the set grows to `SET_PAD` past it —
    // not `VARIANT_GAP`, which read as the padding doubling. Flip run: the old
    // `bottom + VARIANT_GAP` fails *"the air under the last variant"* with 24.
    let (tx, new) = variant::add_variant(&doc, &res, &mut ids, set, Some(a)).unwrap();
    doc.apply(&tx).expect("a second variant");
    let res = Resolved::rebuild(&doc);
    let s = res.world_bounds(set).unwrap();
    let n = res.world_bounds(new).unwrap();
    assert_eq!(
        s.y1 - n.y1,
        variant::SET_PAD,
        "the air under the last variant"
    );
    assert_eq!(
        s.height(),
        20.0 * 2.0 + variant::VARIANT_GAP + 2.0 * variant::SET_PAD
    );
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

/// **Duplicating a whole set makes a new set of new variants**, not a set of
/// instances: `component::settle_copy` turns a copy into an instance only when
/// the copied root is itself a main, and a set is a frame. (`arch-scribe` read it
/// the other way from the code; this is the measurement.)
#[test]
fn a_duplicated_set_is_a_new_set_of_new_variants() {
    let mut f = fixture();
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(f.s).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.commit(tx.0);
    let copy = made[0];
    let vs = variant::variants(&f.doc, copy);
    assert_eq!(vs.len(), 2);
    for v in vs {
        let n = f.doc.get(v).unwrap();
        assert!(n.component() && n.link().is_none(), "a new main");
        assert!(!n.variant().is_empty(), "with its values");
    }
}

// ── The switch's matching rules ────────────────────────────────────────────────

impl F {
    /// Commit `ops` creating layers in the two variants — the instance takes its
    /// copies through the structural pass.
    fn add(&mut self, layers: &[(NodeId, NodeId, NodeKind, &str)]) {
        let mut added: std::collections::HashMap<NodeId, usize> = Default::default();
        let ops = layers
            .iter()
            .map(|(id, parent, kind, name)| {
                let n = added.entry(*parent).or_default();
                let at = self.kids(*parent).len() + *n;
                *n += 1;
                create(*id, *parent, at, kind.clone(), name)
            })
            .collect();
        self.commit(ops);
    }

    fn switch(&mut self, to: NodeId) {
        let tx = variant::switch(&self.doc, self.i, to, &mut self.ids).expect("a switch");
        self.commit(tx.0);
    }

    /// The instance's layers linked to `src`, anywhere below it.
    fn copies_of(&self, src: NodeId) -> Vec<NodeId> {
        ondin_core::build::subtree_nodes(&self.doc, &[self.i])
            .into_iter()
            .filter(|id| self.link(*id) == Some(src))
            .collect()
    }

    /// The instance's child linked to `src`.
    fn child_of(&self, src: NodeId) -> NodeId {
        self.kids(self.i)
            .into_iter()
            .find(|k| self.link(*k) == Some(src))
            .expect("a copy")
    }
}

/// **A layer matches only under a matched parent**: Small's Group "Box" and
/// Large's Frame "Box" are different kinds, so neither they nor the "Box/Shape"
/// under them match. The child matched by itself, under a parent that did not,
/// and the switch was refused (`NoSuchNode`, the Shape relinked after its
/// untouched parent was deleted) — or, the parent changed, left the instance with
/// both Boxes and a stale link.
///
/// Flip: the matching loop without its matched-parent test fails the first
/// switch's commit, `NoSuchNode`.
#[test]
fn a_switch_matches_a_layer_only_under_a_matched_parent() {
    for touched in [false, true] {
        let mut f = fixture();
        let [b1, s1, b2, s2] = [(); 4].map(|_| f.ids.mint());
        let (small, large) = (f.small, f.large);
        f.add(&[
            (b1, small, NodeKind::Group, "Box"),
            (b2, large, frame(20.0, 20.0), "Box"),
        ]);
        f.add(&[(s1, b1, rect(), "Shape"), (s2, b2, rect(), "Shape")]);
        if touched {
            let bc = f.child_of(b1);
            f.doc
                .apply(&Transaction(vec![Operation::SetOpacity {
                    id: bc,
                    opacity: 0.5,
                }]))
                .unwrap();
        }
        f.switch(large);
        let shapes = f.copies_of(s2);
        assert_eq!(shapes.len(), 1, "one Shape following Large's ({touched})");
        let host = f.doc.get(shapes[0]).unwrap().parent().unwrap();
        assert_eq!(f.link(host), Some(b2), "inside a copy of Large's Box");
        let boxes = f
            .kids(f.i)
            .into_iter()
            .filter(|k| f.name(*k) == "Box")
            .count();
        assert_eq!(boxes, if touched { 2 } else { 1 }, "the changed Box kept");
    }
}

/// **The k-th sibling of a name matches the k-th** (`switch`'s doc): two Dots a
/// side. Flip: `paths_by` numbering every sibling 0 fails the count, three
/// children against two — the first Dot's slot matched twice, Large's second
/// copied in again.
#[test]
fn a_switch_matches_the_kth_same_named_sibling_to_the_kth() {
    let mut f = fixture();
    let [d1, d2, e1, e2] = [(); 4].map(|_| f.ids.mint());
    let (small, large) = (f.small, f.large);
    f.add(&[
        (d1, small, rect(), "Dot"),
        (d2, small, rect(), "Dot"),
        (e1, large, rect(), "Dot"),
        (e2, large, rect(), "Dot"),
    ]);
    let (c1, c2) = (f.child_of(d1), f.child_of(d2));
    f.switch(large);
    let dots: Vec<NodeId> = f
        .kids(f.i)
        .into_iter()
        .filter(|k| f.name(*k) == "Dot")
        .collect();
    assert_eq!(dots, vec![c1, c2], "two Dots, the same layers");
    assert_eq!((f.link(c1), f.link(c2)), (Some(e1), Some(e2)));
}

/// **Only within one kind** (`switch`'s doc): a Rect "Mark" and a Text "Mark"
/// do not match, so the rect goes and the text comes in. Flip: `same_kind`
/// answering `true` keeps the rect, relinked to a text.
#[test]
fn a_switch_matches_only_within_one_kind() {
    let mut f = fixture();
    let [m1, m2] = [(); 2].map(|_| f.ids.mint());
    let (small, large) = (f.small, f.large);
    f.add(&[(m1, small, rect(), "Mark"), (m2, large, text("M"), "Mark")]);
    let old = f.child_of(m1);
    f.switch(large);
    assert!(f.doc.get(old).is_none(), "the untouched rect went");
    let new = f.child_of(m2);
    assert!(matches!(
        f.doc.get(new).unwrap().kind(),
        NodeKind::Text { .. }
    ));
}

/// **The order follows where the instance still had the old main's, and only
/// there** (`switch`'s doc). Large puts Label under Bg; an instance in Small's
/// order takes it, and one that reordered its own keeps it. Flips: the reorder
/// guard always false fails the first ("Large's order"); always true fails the
/// second ("its own order").
#[test]
fn a_switch_takes_the_new_order_only_where_the_instance_had_the_old() {
    let mut f = fixture();
    let (large, llabel) = (f.large, f.llabel);
    f.commit(vec![Operation::Reorder {
        id: llabel,
        index: 0,
    }]);
    let (bg, label) = (f.child_of(f.bg), f.child_of(f.label));
    f.switch(large);
    let kids = f.kids(f.i);
    let at = |k: NodeId| kids.iter().position(|x| *x == k).unwrap();
    assert!(at(label) < at(bg), "Large's order");

    let mut f = fixture();
    let (large, llabel) = (f.large, f.llabel);
    f.commit(vec![Operation::Reorder {
        id: llabel,
        index: 0,
    }]);
    let (bg, label) = (f.child_of(f.bg), f.child_of(f.label));
    // The instance puts its Label under its Bg itself — and Large wants it
    // under too, so give Large Small's order back to make the two disagree.
    f.doc
        .apply(&Transaction(vec![Operation::Reorder {
            id: label,
            index: 0,
        }]))
        .unwrap();
    f.commit(vec![Operation::Reorder {
        id: llabel,
        index: 1,
    }]);
    f.switch(large);
    let kids = f.kids(f.i);
    let at = |k: NodeId| kids.iter().position(|x| *x == k).unwrap();
    assert!(at(label) < at(bg), "its own order");
}

/// **One whose match the instance had removed stays removed** (`switch`'s
/// doc). Flip: the insert guard reading `new_to_c` instead of `matched_new`
/// copies Large's Label back in.
#[test]
fn a_switch_does_not_bring_back_a_layer_the_instance_removed() {
    let mut f = fixture();
    let label = f.child_of(f.label);
    f.doc
        .apply(&Transaction(vec![Operation::DeleteNode { id: label }]))
        .unwrap();
    let large = f.large;
    f.switch(large);
    assert!(f.copies_of(f.llabel).is_empty(), "still removed");
}

/// **No switch to a main of another kind** (§15 D1003 (5)): a Frame instance
/// relinked to a Group main could never draw as it. Flip: `switch` without its
/// `same_kind` test answers `Some`.
#[test]
fn a_switch_refuses_a_main_of_another_kind() {
    let mut f = fixture();
    let g = f.ids.mint();
    let root = f.root;
    f.commit(vec![
        create(g, root, 1, NodeKind::Group, "Group main"),
        Operation::SetComponent {
            id: g,
            component: true,
        },
    ]);
    assert!(variant::switch(&f.doc, f.i, g, &mut f.ids).is_none());
}

/// **A variant pasted into a document with no set commits, a plain main**
/// — `settle`'s gate read only the document before the paste, so with no set
/// left the pass never ran and `check` refused the copy's values. Flip: the gate
/// without its `inserts` test fails the paste's commit, `Variant(Values)`.
#[test]
fn a_variant_pasted_where_no_set_is_lands_a_plain_main() {
    let mut f = fixture();
    let clip = f.doc.capture_subtree(f.small).unwrap();
    let (s, i) = (f.s, f.i);
    let mut ops = ondin_core::component::relink_for_delete(&f.doc, &[s, i]);
    ops.extend([
        Operation::DeleteNode { id: s },
        Operation::DeleteNode { id: i },
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
    let n = f.doc.get(made[0]).unwrap();
    assert!(n.component(), "a main");
    assert!(n.variant().is_empty(), "with no values");
}

/// **A set given a property of no values is refused, not a panic**
/// (invariant 8): `settle` read the property's first value before `apply` could
/// refuse. Flip: `settle` without its empty-values guard panics, *"index out of
/// bounds: the len is 0"*.
#[test]
fn an_empty_property_is_refused_rather_than_panicking() {
    let mut f = fixture();
    let mut tx = Transaction(vec![Operation::SetVariantSet {
        id: f.s,
        set: Some(VariantSet {
            props: vec![VariantProp {
                name: "Size".into(),
                values: Vec::new(),
            }],
        }),
    }]);
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    assert!(f.doc.clone().apply(&tx).is_err());
}

/// Two mains, *Button/Primary* and *Button/Ghost*, each with a text layer bound
/// to a Text property — *Label* and *Title* — **under one item id**, the shape
/// *Duplicate as component* leaves (D980's copy rule keeps a copied item's id)
/// once the copy's property is renamed. Answers the mains, their text layers and
/// the shared id.
fn mains_sharing_a_property_id() -> (Document, IdSource, [NodeId; 4], ondin_core::ItemId) {
    let mut ids = IdSource::new(0xBD);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [a, b, la, lb] = [(); 4].map(|_| ids.mint());
    let shared = ids.mint_item();
    let prop = |name: &str, bound| {
        vec![Keyed::new(
            shared,
            Property {
                name: name.into(),
                kind: PropKind::Text,
                bound: vec![bound],
                filter: String::new(),
                preferred: Vec::new(),
            },
        )]
    };
    doc.apply(&Transaction(vec![
        create(a, root, 0, frame(10.0, 10.0), "Button/Primary"),
        create(b, root, 1, frame(10.0, 10.0), "Button/Ghost"),
        create(la, a, 0, text("Go"), "Label"),
        create(lb, b, 0, text("Go"), "Label"),
        Operation::SetComponent {
            id: a,
            component: true,
        },
        Operation::SetComponent {
            id: b,
            component: true,
        },
        Operation::SetProperties {
            id: a,
            props: prop("Label", la),
        },
        Operation::SetProperties {
            id: b,
            props: prop("Title", lb),
        },
    ]))
    .expect("two mains, one property id between them");
    (doc, ids, [a, b, la, lb], shared)
}

/// **Combining mains whose properties share an item id keys them apart**
/// (§15 D980's *unique within one list*, D982's merge). `settle` moves each
/// variant's properties onto the set, merging by name and kind; the names differ,
/// so both are pushed — and the set held `[(P, "Label"), (P, "Title")]`, every
/// edit by id (`edit_property`, the Properties card's rename and delete) then
/// hitting the *first*. The merge re-keys a pushed property whose id the set
/// already holds, and `apply` refuses a repeated property id as it does a
/// repeated fill's.
///
/// Flips, run: the merge's re-key removed fails the commit itself — *"the set:
/// DuplicateItemId"*, `apply`'s check refusing the combine, the second defence;
/// with `props` taken back out of `check_item_ids` as well, it fails at *"the
/// set's properties, one id each"*.
#[test]
fn combining_mains_whose_properties_share_an_id_keys_them_apart() {
    let (mut doc, mut ids, [a, b, ..], shared) = mains_sharing_a_property_id();
    let res = Resolved::rebuild(&doc);
    let (mut tx, set) = variant::combine(&doc, &res, &mut ids, &[a, b]).unwrap();
    ondin_core::propagate::owed(&doc, &mut tx, &mut ids);
    doc.apply(&tx).expect("the set");
    let props = doc.get(set).unwrap().props().to_vec();
    let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Label", "Title"], "both properties on the set");
    assert_ne!(
        props[0].id, props[1].id,
        "the set's properties, one id each"
    );
    assert_eq!(props[0].id, shared, "the first keeps its id");
    // And an edit by id now names the one it means: renaming *Title* leaves
    // *Label* alone.
    let tx = variant::edit_property(&doc, set, props[1].id, |p| {
        Some(Property {
            name: "Heading".into(),
            ..p.clone()
        })
    })
    .expect("the property is there");
    doc.apply(&tx).expect("a rename");
    let names: Vec<String> = doc
        .get(set)
        .unwrap()
        .props()
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(names, strs(&["Label", "Heading"]), "the rename hit *Title*");
}

/// `apply` refuses a property list repeating an item id — `check_item_ids`' sixth
/// list (§15 D980). Flip, run: `props` left out of
/// `check_item_ids` fails *"refused"* with `Ok`.
#[test]
fn apply_refuses_a_property_id_twice_in_one_list() {
    let (mut doc, _, [a, ..], _) = mains_sharing_a_property_id();
    let mut props = doc.get(a).unwrap().props().to_vec();
    // Bound to nothing, or the check refusing it would be the binding rule's
    // (one property per field) rather than this one.
    props.push(props[0].map(|p| Property {
        name: "Other".into(),
        bound: Vec::new(),
        ..p.clone()
    }));
    let err = doc.apply(&Transaction(vec![Operation::SetProperties {
        id: a,
        props,
    }]));
    assert!(
        matches!(err, Err(OpError::DuplicateItemId(n, _)) if n == a),
        "refused: {err:?}"
    );
}

/// **A file repeating a property id loads, the repeat re-keyed.** Older builds
/// saved such files — *Combine as variants* made them — so refusing one on load
/// would lose the document over a defect the app itself wrote. The loader gives
/// each repeat the lowest positional id the list does not hold
/// (`item::rekey_repeats`), a function of the bytes alone, so loading the same
/// bytes twice still gives one document (invariant 9), and the re-save is stable
/// from then on.
///
/// Flip, run: the loader's re-key removed (`props: self.props`) fails the `load`
/// itself, *"has item id … twice in one list"* — the check refusing it, which is
/// the outcome this test exists to rule out. (A re-key by a minted id would not be
/// a function of the bytes; `rekey_repeats` takes no `IdSource` to make that
/// mistake with, so there is no flip for it.)
#[test]
fn a_file_repeating_a_property_id_loads_with_the_repeat_rekeyed() {
    let (mut doc, mut ids, [a, ..], shared) = mains_sharing_a_property_id();
    let other = ids.mint_item();
    let mut props = doc.get(a).unwrap().props().to_vec();
    props.push(Keyed::new(
        other,
        Property {
            name: "Caption".into(),
            kind: PropKind::Text,
            bound: Vec::new(),
            filter: String::new(),
            preferred: Vec::new(),
        },
    ));
    doc.apply(&Transaction(vec![Operation::SetProperties {
        id: a,
        props,
    }]))
    .expect("two properties, two ids");
    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    let (s, o) = (
        format!("\"{}\"", shared.0.to_wire()),
        format!("\"{}\"", other.0.to_wire()),
    );
    assert_eq!(
        text.matches(&o).count(),
        1,
        "the fixture: the id is written once"
    );
    let broken = text.replace(&o, &s);
    let loaded = io::load(broken.as_bytes()).expect("a repeated property id loads");
    let got = loaded.get(a).unwrap().props().to_vec();
    let names: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Label", "Caption"], "both properties, in order");
    assert_eq!(got[0].id, shared, "the first keeps its id");
    assert_ne!(got[1].id, shared, "the repeat re-keyed");
    let again = io::load(broken.as_bytes()).unwrap();
    assert_eq!(again, loaded, "the same bytes, the same document");
    let saved = io::save(&loaded).unwrap();
    assert_eq!(
        io::save(&io::load(&saved).unwrap()).unwrap(),
        saved,
        "and stable once re-saved"
    );
}

/// **A styled layer through a switch and a structural add** (§15 D1062,
/// `propagate::owed`'s second `style::settle`): Small's `Bg` on style *Red*,
/// Large's on *Blue*. An untouched instance switched to Large takes *Blue*
/// and its look; an instance whose `Bg` is on a third style keeps it across
/// the switch, an override `drift` reads; and a styled layer added to Small
/// reaches the instance bound, holding the style's look.
///
/// flips: either `settle` call's `extend` removed alone stays green — here the
/// two cover each other; both removed fails at *"the fixture: its own Green"*.
#[test]
fn a_styled_layer_crosses_a_switch_and_a_structural_add() {
    use ondin_core::style::{Style, StyleId, StyleSlot, StyleValue, bind_op};
    let mut f = fixture();
    let style = |ids: &mut IdSource, name: &str, r: u8| {
        let id = StyleId(ids.mint());
        (
            id,
            Operation::AddStyle {
                style: Style {
                    id,
                    name: name.into(),
                    value: StyleValue::Fills(fills(r)),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        )
    };
    let (red, add_red) = style(&mut f.ids, "Red", 10);
    let (blue, add_blue) = style(&mut f.ids, "Blue", 200);
    let (green, add_green) = style(&mut f.ids, "Green", 99);
    f.commit(vec![
        add_red,
        add_blue,
        add_green,
        bind_op(f.bg, StyleSlot::Fills, Some(red)),
        bind_op(f.lbg, StyleSlot::Fills, Some(blue)),
    ]);
    let ibg = f.kids(f.i)[0];
    let styled = |f: &F, id| f.doc.get(id).unwrap().styles().fills;
    assert_eq!(
        styled(&f, ibg),
        Some(red),
        "the fixture: the copy follows Red"
    );

    let tx = variant::switch(&f.doc, f.i, f.large, &mut f.ids).unwrap();
    f.commit(tx.0);
    assert_eq!(
        (styled(&f, ibg), f.fill(ibg)),
        (Some(blue), solid(200)),
        "an untouched copy takes Large's style"
    );

    // A second instance whose Bg is on a style of its own.
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(f.small).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.commit(tx.0);
    let (j, jbg) = (made[0], f.kids(made[0])[0]);
    f.commit(vec![bind_op(jbg, StyleSlot::Fills, Some(green))]);
    assert_eq!(f.fill(jbg), solid(99), "the fixture: its own Green");
    let tx = variant::switch(&f.doc, j, f.large, &mut f.ids).unwrap();
    f.commit(tx.0);
    assert_eq!(
        (styled(&f, jbg), f.fill(jbg)),
        (Some(green), solid(99)),
        "its own style carries"
    );
    assert!(
        ondin_core::reset::drift(&f.doc, j).any(),
        "and reads as an override"
    );

    // A styled layer added to Small reaches the instance switched back to it.
    let tx = variant::switch(&f.doc, f.i, f.small, &mut f.ids).unwrap();
    f.commit(tx.0);
    let dot = f.ids.mint();
    f.commit(vec![
        create(dot, f.small, 2, rect(), "Dot"),
        bind_op(dot, StyleSlot::Fills, Some(blue)),
    ]);
    let idot = *f.kids(f.i).last().unwrap();
    assert_eq!(f.link(idot), Some(dot), "the fixture: the copy arrived");
    assert_eq!(
        (styled(&f, idot), f.fill(idot)),
        (Some(blue), solid(200)),
        "the added copy is bound and holds the look"
    );
}
