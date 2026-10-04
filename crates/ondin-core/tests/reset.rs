//! Resetting an instance to its main (§5.3d's reset family, build step 5,
//! `ondin_core::reset`): overrides found by comparing a copy with its source, and
//! each reset writing the source back without deleting anything of the instance's
//! own.

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::reset::{self, Drift, ItemState};
use ondin_core::{
    Document, Fill, IdSource, Keyed, NodeId, NodeKind, Operation, Placement, TextSizing, TextStyle,
    Transaction, keyed_by_position,
};

fn solid(r: u8) -> Brush {
    Brush::Solid(Color::from_rgba8(r, 0, 0, 255))
}

struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    /// The main, its rect and its text.
    m: NodeId,
    a: NodeId,
    t: NodeId,
    /// An instance of it and its two copies.
    i: NodeId,
    ia: NodeId,
    it: NodeId,
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

fn rect_kind() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(5.0, 5.0),
        corner_radii: RoundedRectRadii::default(),
    }
}

fn create(id: NodeId, parent: NodeId, index: usize, kind: NodeKind) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: None,
        name: None,
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xAC);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [m, a, t] = [(); 3].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(
            m,
            root,
            0,
            NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
        ),
        create(
            a,
            m,
            0,
            NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
            },
        ),
        create(t, m, 1, text_kind("Label")),
        Operation::SetFills {
            id: a,
            fills: keyed_by_position([
                Fill {
                    brush: solid(10),
                    visible: true,
                },
                Fill {
                    brush: solid(20),
                    visible: true,
                },
            ]),
        },
        Operation::SetComponent {
            id: m,
            component: true,
        },
    ]))
    .expect("a main");
    let i = instance(&mut doc, &mut ids, m, root);
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

impl F {
    /// `ops` with everything `commit_inner` appends — what a reset goes through.
    fn commit_all(&mut self, ops: Vec<Operation>) {
        let mut tx = Transaction(ops);
        let s = ondin_core::propagate::propagate_structure(&self.doc, &tx, &mut self.ids);
        tx.0.extend(s);
        let l = ondin_core::component::settle_links(&self.doc, &tx);
        tx.0.extend(l);
        let p = ondin_core::propagate::propagate(&self.doc, &tx);
        tx.0.extend(p);
        self.doc
            .apply(&tx)
            .expect("the edit and everything it owes");
    }

    /// An edit applied bare — the instance's own, which owes nothing.
    fn edit(&mut self, ops: Vec<Operation>) {
        self.doc.apply(&Transaction(ops)).expect("an instance edit");
    }
}

fn kids(doc: &Document, id: NodeId) -> Vec<NodeId> {
    doc.get(id).unwrap().children().to_vec()
}

fn link(doc: &Document, id: NodeId) -> Option<NodeId> {
    doc.get(id).unwrap().link()
}

fn fills(doc: &Document, id: NodeId) -> Vec<Keyed<Fill>> {
    doc.get(id).unwrap().paint().fills.clone()
}

fn style(doc: &Document, id: NodeId) -> TextStyle {
    match doc.get(id).unwrap().kind() {
        NodeKind::Text { style, .. } => (**style).clone(),
        _ => unreachable!(),
    }
}

/// A fresh instance has drifted nowhere — the comparison is exact, and a copy is
/// exactly its source. The control every count below is read against.
#[test]
fn a_fresh_instance_has_no_drift() {
    let f = fixture();
    assert_eq!(reset::drift(&f.doc, f.i), Drift::default());
    for n in [f.i, f.ia, f.it] {
        assert!(reset::overrides(&f.doc, n).is_empty(), "{n:?}");
    }
}

/// Seven override units across three nodes — a list item, an opacity, a name, two
/// fields of one text style, the content, and the root's opacity — while the
/// root's **placement** (moved, hidden) counts nothing. *Reset fields* writes them
/// all back and leaves the placement where the instance put it. Flips, both on
/// the count: dropping the placement skip in `reset::overrides` reads 9 (the
/// transform and the visibility); counting a struct payload whole in `leaves`
/// reads 6 — which is why the style changes two of its fields.
#[test]
fn reset_fields_writes_every_override_back_and_keeps_the_placement() {
    let mut f = fixture();
    let mut mine = fills(&f.doc, f.ia);
    mine[0].brush = solid(55);
    let mut st = style(&f.doc, f.it);
    st.font_size = 30.0;
    st.font_family = "Mono".into();
    let moved = Affine::translate((200.0, 40.0));
    f.edit(vec![
        Operation::SetFills {
            id: f.ia,
            fills: mine,
        },
        Operation::SetOpacity {
            id: f.ia,
            opacity: 0.5,
        },
        Operation::SetName {
            id: f.ia,
            name: "Mine".into(),
        },
        Operation::SetTextStyle {
            id: f.it,
            style: st,
            spans: None,
        },
        Operation::SetText {
            id: f.it,
            content: "Other".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        },
        Operation::SetOpacity {
            id: f.i,
            opacity: 0.25,
        },
        Operation::SetTransform {
            id: f.i,
            transform: moved,
        },
        Operation::SetVisible {
            id: f.i,
            visible: false,
        },
    ]);
    let d = reset::drift(&f.doc, f.i);
    assert_eq!(
        d.fields, 7,
        "fill, opacity, name, two style fields, content, root opacity"
    );
    assert_eq!((d.removed, d.order, d.local), (0, 0, 0));

    let ops = reset::reset_fields(&f.doc, &[f.i]);
    f.commit_all(ops);
    assert_eq!(reset::drift(&f.doc, f.i), Drift::default());
    assert_eq!(fills(&f.doc, f.ia), fills(&f.doc, f.a));
    assert_eq!(style(&f.doc, f.it), style(&f.doc, f.t));
    let root = f.doc.get(f.i).unwrap();
    assert_eq!(
        root.transform(),
        moved,
        "the placement is the instance's own"
    );
    assert!(!root.visible(), "and so is its visibility");
    assert_eq!(root.opacity(), 1.0, "its opacity is not");
}

/// A list keeps its own items through a reset: the instance removed the main's
/// second fill and added one of its own after the first. One unit (the removed
/// item); the local item is `Local` and stays after its anchor. The ghost row is
/// the removed item.
#[test]
fn a_list_reset_restores_the_mains_items_and_keeps_its_own() {
    let mut f = fixture();
    let main = fills(&f.doc, f.a);
    let local = Keyed::new(
        f.ids.mint_item(),
        Fill {
            brush: solid(77),
            visible: true,
        },
    );
    f.edit(vec![Operation::SetFills {
        id: f.ia,
        fills: vec![main[0].clone(), local.clone()],
    }]);
    let cur = fills(&f.doc, f.ia);
    assert_eq!(
        reset::item_states(&main, &cur),
        vec![ItemState::Follows, ItemState::Local]
    );
    assert_eq!(reset::removed_items(&main, &cur), vec![main[1].clone()]);
    let o = reset::overrides(&f.doc, f.ia);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].units, 1, "the removed item, and not the local one");

    f.commit_all(reset::reset_fields(&f.doc, &[f.i]));
    assert_eq!(
        fills(&f.doc, f.ia),
        vec![main[0].clone(), local, main[1].clone()]
    );
}

/// One ghost row's *Restore*: the item lands at its anchor among what the instance
/// has, and an overridden item's reset replaces it in place.
#[test]
fn reset_item_restores_one_item_at_its_anchor() {
    let src = keyed_by_position([1u32, 2, 3]);
    let local = Keyed::new(ondin_core::ItemId::positional(9), 7u32);
    let cur = vec![src[0], local, src[2]];
    assert_eq!(
        reset::reset_item(&src, &cur, src[1].id),
        vec![src[0], src[1], local, src[2]],
        "after its preceding item's counterpart"
    );
    let changed = vec![src[0].map(|_| 5), src[1], src[2]];
    assert_eq!(reset::reset_item(&src, &changed, src[0].id), src);
    // With no counterpart on either side, first — the list rule
    // (`propagate::items`, `reset_items`), where the children's anchor falls back
    // to topmost. `arch-scribe` found the doc and the code disagreeing here.
    assert_eq!(
        reset::reset_item(&src, &[local], src[1].id),
        vec![src[1], local]
    );
}

/// A child the instance deleted is a removed child: counted once, restored at its
/// anchor — after the first copy, before the instance's own layer — linked to its
/// source, and the restore passes every link rule. The local layer stays. Flip:
/// inserting topmost in `restore_children` fails the anchor assertion (the local
/// layer is at index 1). ⚠️ `reset_all_leaves_nothing_but_local_layers` stays
/// green under that flip — its order reset re-slots the restored child — so this
/// test is the one that pins the anchor.
#[test]
fn a_deleted_child_is_restored_at_its_anchor_and_a_local_layer_stays() {
    let mut f = fixture();
    let own = f.ids.mint();
    f.edit(vec![
        Operation::DeleteNode { id: f.it },
        create(own, f.i, 1, rect_kind()),
    ]);
    let d = reset::drift(&f.doc, f.i);
    assert_eq!((d.removed, d.local, d.fields, d.order), (1, 1, 0, 0));

    let ops = reset::restore_children(&f.doc, &[f.i], &mut f.ids);
    f.commit_all(ops);
    let k = kids(&f.doc, f.i);
    assert_eq!(k.len(), 3);
    assert_eq!(k[0], f.ia);
    assert_eq!(
        link(&f.doc, k[1]),
        Some(f.t),
        "restored after its preceding sibling"
    );
    assert_eq!(
        k[2], own,
        "the instance's own layer stays, and stays topmost"
    );
    assert_eq!(reset::drift(&f.doc, f.i).removed, 0);
}

/// A child the instance moved elsewhere inside itself is not missing, and a
/// restore of its removed parent does not copy it a second time. Flip: no pruning
/// in `restore_children` fails *"the rect is not copied again"* — and **not** by a
/// refused commit: two nodes of one instance on one source is
/// `LinkRule::SharedSource`, but `settle_links` cuts the copy the transaction
/// touched first, so the unpruned restore would land an unlinked duplicate.
#[test]
fn a_child_moved_elsewhere_is_not_restored_twice() {
    let mut f = fixture();
    // The main gets a group holding a rect; the instance follows.
    let (g, r) = (f.ids.mint(), f.ids.mint());
    let m = f.m;
    f.commit_all(vec![
        create(g, m, 2, NodeKind::Group),
        create(r, g, 0, rect_kind()),
    ]);
    let ig = kids(&f.doc, f.i)[2];
    let ir = kids(&f.doc, ig)[0];
    assert_eq!((link(&f.doc, ig), link(&f.doc, ir)), (Some(g), Some(r)));
    // The instance lifts the rect out and deletes the group.
    f.edit(vec![
        Operation::Reparent {
            id: ir,
            new_parent: f.i,
            index: 0,
        },
        Operation::DeleteNode { id: ig },
    ]);
    assert_eq!(reset::drift(&f.doc, f.i).removed, 1, "the group alone");
    let ops = reset::restore_children(&f.doc, &[f.i], &mut f.ids);
    f.commit_all(ops);
    let restored = *kids(&f.doc, f.i).last().unwrap();
    assert_eq!(link(&f.doc, restored), Some(g));
    assert!(
        kids(&f.doc, restored).is_empty(),
        "the rect is not copied again"
    );
    assert_eq!(
        f.doc.get(ir).unwrap().parent(),
        Some(f.i),
        "it stays where it went"
    );
}

/// The instance reordered its two copies around a layer of its own: *Reset order*
/// puts the copies back in the main's order in the slots they hold, and the own
/// layer keeps its slot — bottommost here. Flip: rebuilding the order as the
/// main's with the own layers after it reads `[ia, it, own]`.
#[test]
fn reset_order_puts_linked_children_back_around_local_ones() {
    let mut f = fixture();
    let own = f.ids.mint();
    f.edit(vec![
        create(own, f.i, 1, rect_kind()),
        Operation::Reorder { id: f.ia, index: 2 },
    ]);
    assert_eq!(kids(&f.doc, f.i), vec![own, f.it, f.ia], "the fixture");
    assert_eq!(reset::drift(&f.doc, f.i).order, 1);
    f.commit_all(reset::reset_order(&f.doc, &[f.i]));
    assert_eq!(kids(&f.doc, f.i), vec![own, f.ia, f.it]);
}

/// *Reset all* over every kind of drift at once comes back with none — and the
/// instance's own layer is still there.
#[test]
fn reset_all_leaves_nothing_but_local_layers() {
    let mut f = fixture();
    let own = f.ids.mint();
    f.edit(vec![
        create(own, f.i, 0, rect_kind()),
        Operation::SetName {
            id: f.ia,
            name: "x".into(),
        },
        Operation::DeleteNode { id: f.it },
    ]);
    // And the main gains a child the instance then puts below the copy.
    let extra = f.ids.mint();
    let m = f.m;
    f.commit_all(vec![create(extra, m, 2, rect_kind())]);
    let iextra = *kids(&f.doc, f.i).last().unwrap();
    f.edit(vec![Operation::Reorder {
        id: iextra,
        index: 0,
    }]);
    let d = reset::drift(&f.doc, f.i);
    assert_eq!((d.fields, d.removed, d.order, d.local), (1, 1, 1, 1));

    let ops = reset::reset_all(&f.doc, &[f.i], &mut f.ids);
    f.commit_all(ops);
    assert_eq!(
        reset::drift(&f.doc, f.i),
        Drift {
            local: 1,
            ..Default::default()
        }
    );
    let linked: Vec<_> = kids(&f.doc, f.i)
        .into_iter()
        .filter_map(|k| link(&f.doc, k))
        .collect();
    assert_eq!(
        linked,
        kids(&f.doc, f.m),
        "the main's children, in its order"
    );
    assert!(kids(&f.doc, f.i).contains(&own));
}

/// Scope: an outer main holds an instance of `m`; an instance of the outer main
/// holds a copy of that, and a local instance of `m` the outer instance added. A
/// reset of the outer instance reaches the nested copy's members — they are the
/// outer main's content — and leaves the local instance alone, counting it as a
/// local layer.
#[test]
fn a_reset_reaches_a_nested_copy_and_not_a_local_instance() {
    let mut f = fixture();
    let outer = f.ids.mint();
    f.edit(vec![
        create(
            outer,
            f.root,
            0,
            NodeKind::Artboard {
                size: Size::new(300.0, 300.0),
            },
        ),
        Operation::Reparent {
            id: f.i,
            new_parent: outer,
            index: 0,
        },
        Operation::SetComponent {
            id: outer,
            component: true,
        },
    ]);
    let oi = instance(&mut f.doc, &mut f.ids, outer, f.root);
    let nested = kids(&f.doc, oi)[0];
    let deep = kids(&f.doc, nested)[0];
    let local_inst = instance(&mut f.doc, &mut f.ids, f.m, oi);
    let local_member = kids(&f.doc, local_inst)[0];
    f.edit(vec![
        Operation::SetName {
            id: deep,
            name: "deep".into(),
        },
        Operation::SetName {
            id: local_member,
            name: "local".into(),
        },
    ]);
    assert!(!reset::scope_nodes(&f.doc, oi).contains(&local_inst));
    let d = reset::drift(&f.doc, oi);
    assert_eq!((d.fields, d.local), (1, 1));
    let ops = reset::reset_all(&f.doc, &[oi], &mut f.ids);
    f.commit_all(ops);
    assert_eq!(
        f.doc.get(deep).unwrap().name(),
        f.doc.get(f.a).unwrap().name()
    );
    assert_eq!(
        f.doc.get(local_member).unwrap().name(),
        "local",
        "the local instance is the outer instance's own"
    );
}

/// The outer main from the two tests above: `outer` holds the instance `f.i` of
/// `m`, and the answer is an instance of `outer` with its nested copy of `f.i`.
fn outer_instance(f: &mut F) -> (NodeId, NodeId) {
    let outer = f.ids.mint();
    f.edit(vec![
        create(
            outer,
            f.root,
            0,
            NodeKind::Artboard {
                size: Size::new(300.0, 300.0),
            },
        ),
        Operation::Reparent {
            id: f.i,
            new_parent: outer,
            index: 0,
        },
        Operation::SetComponent {
            id: outer,
            component: true,
        },
    ]);
    let oi = instance(&mut f.doc, &mut f.ids, outer, f.root);
    let nested = kids(&f.doc, oi)[0];
    (oi, nested)
}

/// **A member dragged out of a nested copy into the outer instance is not
/// missing.** `settle_links` keeps its link — the outer root's source contains
/// its source — so a restore that looked for it only under the nested copy (the
/// nearest instance root) found it absent and inserted a second node on the same
/// source, under another root, where `LinkRule::SharedSource` cannot see it.
/// Found by `arch-scribe` reading `missing` against `settle_links`; failed before
/// the held links were read under the outermost root.
#[test]
fn a_member_dragged_out_of_a_nested_copy_is_not_restored_twice() {
    let mut f = fixture();
    let (oi, nested) = outer_instance(&mut f);
    let deep = kids(&f.doc, nested)[0];
    f.commit_all(vec![Operation::Reparent {
        id: deep,
        new_parent: oi,
        index: 0,
    }]);
    assert_eq!(
        link(&f.doc, deep),
        Some(f.ia),
        "the fixture: it kept its link"
    );
    assert_eq!(reset::drift(&f.doc, oi).removed, 0);
    assert!(reset::restore_children(&f.doc, &[oi], &mut f.ids).is_empty());
}

/// **A local instance's removed child is not masked by the instance around it.**
/// An instance of `m` holds a local instance of `m` too; the local one deletes its
/// copy of the rect. The outer instance's own copy of the rect links to the same
/// source, so a restore that looked for held links under the outer root found it
/// "held" and restored nothing. The climb to the outermost root is for a nested
/// copy's members; it stops at a root linked straight to a main. Found by
/// `arch-scribe` reading the fix for the test above; failed with `removed` 0 before.
#[test]
fn a_local_instances_removed_child_is_not_masked_by_its_host() {
    let mut f = fixture();
    let local = instance(&mut f.doc, &mut f.ids, f.m, f.i);
    let local_rect = kids(&f.doc, local)[0];
    assert_eq!(link(&f.doc, local_rect), Some(f.a), "the fixture");
    f.edit(vec![Operation::DeleteNode { id: local_rect }]);
    assert_eq!(reset::drift(&f.doc, local).removed, 1);
    let ops = reset::restore_children(&f.doc, &[local], &mut f.ids);
    f.commit_all(ops);
    assert_eq!(link(&f.doc, kids(&f.doc, local)[0]), Some(f.a));
}

/// **Overlapping scopes reset once.** An outer instance and the nested copy
/// inside it, scoped together, with a child of the nested copy deleted: one
/// restore, not two (the second would be cut by `settle_links` and land an
/// unlinked duplicate). Found by `arch-scribe`; failed before `reset` took the
/// outermost of its scopes. ⚠️ The scope inside must be a **parent of the missing
/// child** — the first spelling of this test scoped a childless rect beside it
/// and passed against the defect.
#[test]
fn overlapping_scopes_restore_a_child_once() {
    let mut f = fixture();
    let (oi, nested) = outer_instance(&mut f);
    let deep = kids(&f.doc, nested)[0];
    f.edit(vec![Operation::DeleteNode { id: deep }]);
    let ops = reset::reset(&f.doc, reset::Kind::All, &[oi, nested], &mut f.ids);
    let inserts = ops
        .iter()
        .filter(|op| matches!(op, Operation::InsertSubtree { .. }))
        .count();
    assert_eq!(inserts, 1);
}

/// A reset scoped to one child resets that child and nothing beside it — the
/// context menu's *Reset Label*.
#[test]
fn a_reset_scoped_to_a_child_leaves_its_siblings() {
    let mut f = fixture();
    f.edit(vec![
        Operation::SetName {
            id: f.ia,
            name: "a".into(),
        },
        Operation::SetName {
            id: f.it,
            name: "t".into(),
        },
    ]);
    assert_eq!(reset::drift(&f.doc, f.it).fields, 1);
    let ops = reset::reset_all(&f.doc, &[f.it], &mut f.ids);
    f.commit_all(ops);
    assert_eq!(
        f.doc.get(f.it).unwrap().name(),
        f.doc.get(f.t).unwrap().name()
    );
    assert_eq!(f.doc.get(f.ia).unwrap().name(), "a");
}

/// A reset of a nested copy **inside a main** is an edit to that main, so it
/// reaches the main's instances like any other — through the ordinary commit.
#[test]
fn resetting_a_nested_copy_inside_a_main_reaches_the_mains_instances() {
    let mut f = fixture();
    let outer = f.ids.mint();
    f.edit(vec![
        create(
            outer,
            f.root,
            0,
            NodeKind::Artboard {
                size: Size::new(300.0, 300.0),
            },
        ),
        Operation::Reparent {
            id: f.i,
            new_parent: outer,
            index: 0,
        },
        Operation::SetComponent {
            id: outer,
            component: true,
        },
    ]);
    let oi = instance(&mut f.doc, &mut f.ids, outer, f.root);
    let deep = kids(&f.doc, kids(&f.doc, oi)[0])[0];
    // The outer main's nested copy overrides a name, and its instance follows.
    f.commit_all(vec![Operation::SetName {
        id: f.ia,
        name: "in outer".into(),
    }]);
    assert_eq!(f.doc.get(deep).unwrap().name(), "in outer", "the fixture");
    f.commit_all(reset::reset_fields(&f.doc, &[f.i]));
    let a_name = f.doc.get(f.a).unwrap().name().to_string();
    assert_eq!(f.doc.get(f.ia).unwrap().name(), a_name);
    assert_eq!(f.doc.get(deep).unwrap().name(), a_name, "carried on");
}
