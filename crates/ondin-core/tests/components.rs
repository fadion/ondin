//! The component rules `Document::apply` and the loader hold a document to
//! (§15 D978, `ondin_core::component::check`): one test per rule, each against a
//! valid fixture it is one operation away from.

use ondin_core::component::LinkRule;
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::{Document, IdSource, NodeId, NodeKind, OpError, Operation, Transaction, io};

fn frame() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(100.0, 100.0),
    }
}

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(10.0, 10.0),
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

/// A main `m` (a frame holding rects `a` and `b`) and an instance `i` of it (a
/// frame holding `ia` → `a` and `ib` → `b`), both under the root.
struct Fixture {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    m: NodeId,
    a: NodeId,
    b: NodeId,
    i: NodeId,
    ia: NodeId,
    ib: NodeId,
}

fn fixture() -> Fixture {
    let mut ids = IdSource::new(0xAB);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [m, a, b, i, ia, ib] = [(); 6].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(m, root, 0, frame()),
        create(a, m, 0, rect()),
        create(b, m, 1, rect()),
        Operation::SetComponent {
            id: m,
            component: true,
        },
        create(i, root, 1, frame()),
        create(ia, i, 0, rect()),
        create(ib, i, 1, rect()),
        Operation::SetLink {
            id: i,
            link: Some(m),
        },
        Operation::SetLink {
            id: ia,
            link: Some(a),
        },
        Operation::SetLink {
            id: ib,
            link: Some(b),
        },
    ]))
    .expect("a main and an instance of it");
    Fixture {
        doc,
        ids,
        root,
        m,
        a,
        b,
        i,
        ia,
        ib,
    }
}

fn refused(doc: &mut Document, ops: Vec<Operation>) -> (NodeId, LinkRule) {
    match doc.apply(&Transaction(ops)) {
        Err(OpError::BadLink(id, rule)) => (id, rule),
        other => panic!("expected a component rule refused, got {other:?}"),
    }
}

#[test]
fn a_main_and_its_instance_round_trip_through_a_file() {
    let f = fixture();
    let loaded = io::load(&io::save(&f.doc).unwrap()).unwrap();
    assert!(loaded.get(f.m).unwrap().component());
    assert_eq!(loaded.get(f.ia).unwrap().link(), Some(f.a));
    assert_eq!(io::save(&loaded).unwrap(), io::save(&f.doc).unwrap());
}

/// Deleting a main its instance still points at is refused — and accepted once
/// the same transaction cuts the links, in **either** order, because the check is
/// after the last op (the guide owners' rule, §15 D491). Flip: checking per op
/// refuses the delete-first order.
#[test]
fn a_main_cannot_be_deleted_from_under_its_instance() {
    let mut f = fixture();
    let (_, rule) = refused(&mut f.doc, vec![Operation::DeleteNode { id: f.m }]);
    assert_eq!(rule, LinkRule::Dangling);
    let cut = [f.i, f.ia, f.ib].map(|id| Operation::SetLink { id, link: None });
    let mut ops = vec![Operation::DeleteNode { id: f.m }];
    ops.extend(cut);
    f.doc.apply(&Transaction(ops)).expect("delete, then detach");
    assert!(f.doc.get(f.m).is_none());
}

#[test]
fn only_a_frame_or_a_group_can_be_a_main() {
    let mut f = fixture();
    let (id, rule) = refused(
        &mut f.doc,
        vec![Operation::SetComponent {
            id: f.ia,
            component: true,
        }],
    );
    // `ia` is a rect *and* inside an instance; the kind is the first thing asked.
    assert_eq!((id, rule), (f.ia, LinkRule::ComponentKind));
}

#[test]
fn a_main_cannot_sit_inside_a_main_or_an_instance() {
    let mut f = fixture();
    let inner = f.ids.mint();
    for parent in [f.m, f.i] {
        let (id, rule) = refused(
            &mut f.doc,
            vec![
                create(inner, parent, 0, frame()),
                Operation::SetComponent {
                    id: inner,
                    component: true,
                },
            ],
        );
        assert_eq!(
            (id, rule),
            (inner, LinkRule::NestedMain),
            "under {parent:?}"
        );
    }
}

/// A node of an instance must be linked inside its own instance's main — here,
/// a second main's rect.
#[test]
fn a_member_links_inside_its_own_main() {
    let mut f = fixture();
    let [m2, c] = [(); 2].map(|_| f.ids.mint());
    f.doc
        .apply(&Transaction(vec![
            create(m2, f.root, 2, frame()),
            create(c, m2, 0, rect()),
            Operation::SetComponent {
                id: m2,
                component: true,
            },
        ]))
        .unwrap();
    let (id, rule) = refused(
        &mut f.doc,
        vec![Operation::SetLink {
            id: f.ib,
            link: Some(c),
        }],
    );
    assert_eq!((id, rule), (f.ib, LinkRule::Membership));
}

#[test]
fn two_nodes_of_one_instance_cannot_share_a_source() {
    let mut f = fixture();
    let (id, rule) = refused(
        &mut f.doc,
        vec![Operation::SetLink {
            id: f.ib,
            link: Some(f.a),
        }],
    );
    assert!(id == f.ia || id == f.ib);
    assert_eq!(rule, LinkRule::SharedSource);
}

/// A local addition — an unlinked layer inside an instance — is legal, and so is
/// one that is itself an instance of another main (free structure, §15 D979 (b)).
#[test]
fn an_instance_may_hold_local_layers_and_other_instances() {
    let mut f = fixture();
    let [m2, local, other] = [(); 3].map(|_| f.ids.mint());
    f.doc
        .apply(&Transaction(vec![
            create(m2, f.root, 2, frame()),
            Operation::SetComponent {
                id: m2,
                component: true,
            },
            create(local, f.i, 2, rect()),
            create(other, f.i, 3, frame()),
            Operation::SetLink {
                id: other,
                link: Some(m2),
            },
        ]))
        .expect("local additions, one of them an instance");
}

/// A main holding an instance of itself could never be expanded. Flip: dropping
/// the cycle search accepts this.
#[test]
fn a_main_cannot_contain_an_instance_of_itself() {
    let mut f = fixture();
    let inner = f.ids.mint();
    let (_, rule) = refused(
        &mut f.doc,
        vec![
            create(inner, f.m, 2, frame()),
            Operation::SetLink {
                id: inner,
                link: Some(f.m),
            },
        ],
    );
    assert_eq!(rule, LinkRule::ComponentCycle);
}

/// The nested chain §5.3d describes: main `outer` holds an instance `n` of `m`;
/// an instance of `outer` holds `cn` → `n` (an instance root by its chain) whose
/// children link to `n`'s children, one level up. Valid as built; a link from `cn`'s
/// child straight to `m`'s child skips a level and is refused.
#[test]
fn a_nested_instance_links_one_level_up() {
    let mut f = fixture();
    let [outer, n, na, nb, io_, cn, cna, cnb] = [(); 8].map(|_| f.ids.mint());
    f.doc
        .apply(&Transaction(vec![
            create(outer, f.root, 2, frame()),
            create(n, outer, 0, frame()),
            create(na, n, 0, rect()),
            create(nb, n, 1, rect()),
            Operation::SetComponent {
                id: outer,
                component: true,
            },
            Operation::SetLink {
                id: n,
                link: Some(f.m),
            },
            Operation::SetLink {
                id: na,
                link: Some(f.a),
            },
            Operation::SetLink {
                id: nb,
                link: Some(f.b),
            },
            create(io_, f.root, 3, frame()),
            create(cn, io_, 0, frame()),
            create(cna, cn, 0, rect()),
            create(cnb, cn, 1, rect()),
            Operation::SetLink {
                id: io_,
                link: Some(outer),
            },
            Operation::SetLink {
                id: cn,
                link: Some(n),
            },
            Operation::SetLink {
                id: cna,
                link: Some(na),
            },
            Operation::SetLink {
                id: cnb,
                link: Some(nb),
            },
        ]))
        .expect("a nested instance copied with its outer main");
    let (id, rule) = refused(
        &mut f.doc,
        vec![Operation::SetLink {
            id: cna,
            link: Some(f.a),
        }],
    );
    assert_eq!((id, rule), (cna, LinkRule::Membership));
}

/// The loader refuses a link to nothing rather than dropping it — dropping would
/// turn an instance into a detached copy without a word.
#[test]
fn a_file_with_a_dangling_link_is_refused() {
    let f = fixture();
    let text = String::from_utf8(io::save(&f.doc).unwrap()).unwrap();
    let a = format!("\"link\": \"{}\"", f.a.to_wire());
    assert!(text.contains(&a), "the fixture writes the link");
    let broken = text.replace(&a, "\"link\": \"ff:999\"");
    let err = io::load(broken.as_bytes()).unwrap_err();
    assert!(err.to_string().contains("not in the document"), "{err}");
}

/// Copying a subtree that holds a main and its instance remaps the instance's
/// links onto the copied main (§5.3d, D979 (e)); a link pointing outside the copy
/// is kept. Flip: copying `link` verbatim leaves the copied instance on the
/// original main.
#[test]
fn a_copied_main_and_instance_stay_linked_to_each_other() {
    let mut f = fixture();
    let g = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            create(g, f.root, 2, NodeKind::Group),
            Operation::Reparent {
                id: f.m,
                new_parent: g,
                index: 0,
            },
            Operation::Reparent {
                id: f.i,
                new_parent: g,
                index: 1,
            },
        ]))
        .unwrap();
    let template = f.doc.capture_subtree(g).unwrap();
    let (copy, _) = ondin_core::remap_subtree(&template, &mut f.ids).unwrap();
    let new_of = |old: NodeId| {
        let at = template.iter().position(|n| n.id() == old).unwrap();
        copy[at].id()
    };
    let copied_instance = &copy[template.iter().position(|n| n.id() == f.i).unwrap()];
    assert_eq!(copied_instance.link(), Some(new_of(f.m)));
    let copied_ia = &copy[template.iter().position(|n| n.id() == f.ia).unwrap()];
    assert_eq!(copied_ia.link(), Some(new_of(f.a)));

    // An instance copied alone keeps its link to the original main.
    let alone = f.doc.capture_subtree(f.i).unwrap();
    let (copy, _) = ondin_core::remap_subtree(&alone, &mut f.ids).unwrap();
    assert_eq!(copy[0].link(), Some(f.m));
}
