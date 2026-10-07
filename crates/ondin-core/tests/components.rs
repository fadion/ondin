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

/// An override one ULP off its main survives a save and a reopen, and the
/// coordinate comes back bit-exact (`[R3-L5-02]`). An override *is* a value that
/// differs from the main's (§15 D979 (a)), so the loader has to read back the
/// `f64` the file wrote, not its neighbour: `serde_json`'s default float parser
/// reads the shortest text of `0x403d19fb54ced16a` (`29.101491260998422`) as
/// `…16b`, one ULP up — the copy's own value — so after a reload main and copy
/// agreed, the copy counted no override and followed the main from then on. The
/// workspace manifest turns on `float_roundtrip`, which parses exactly.
///
/// Flip, run: the feature dropped from the workspace `serde_json` line fails the
/// first assertion, the main's x back as `…16b`; the override count is the second
/// assertion and is not reached. (The fixture is asserted first: the two values
/// are distinct and the copy counts one override before the save.)
#[test]
fn an_override_one_ulp_off_its_main_survives_a_reload() {
    let mut f = fixture();
    let main_x = f64::from_bits(0x403d_19fb_54ce_d16a);
    let copy_x = f64::from_bits(0x403d_19fb_54ce_d16b);
    let at = |x: f64| ondin_core::kurbo::Affine::translate((x, 0.0));
    f.doc
        .apply(&Transaction(vec![
            Operation::SetTransform {
                id: f.a,
                transform: at(main_x),
            },
            Operation::SetTransform {
                id: f.ia,
                transform: at(copy_x),
            },
        ]))
        .unwrap();
    assert_ne!(main_x, copy_x);
    let overridden = |doc: &Document| ondin_core::reset::overrides(doc, f.ia).len();
    assert_eq!(overridden(&f.doc), 1);
    let x_of = |doc: &Document, id: NodeId| doc.get(id).unwrap().transform().translation().x;
    let loaded = io::load(&io::save(&f.doc).unwrap()).unwrap();
    assert_eq!(x_of(&loaded, f.a).to_bits(), main_x.to_bits());
    assert_eq!(x_of(&loaded, f.ia).to_bits(), copy_x.to_bits());
    assert_eq!(overridden(&loaded), 1);
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

/// `n` mains `M0…M(n-1)` under the root, each `Mk` holding one frame linked to
/// `M(k+1)` — and, with `closed`, the last holding one linked back to `M0`. A
/// valid chain is only two levels deep, so `io::MAX_TREE_DEPTH` never trips
/// while the component graph is `n` long. Built in one transaction on a thread
/// with a large stack, and saved: only the load is under test.
fn chain_of_mains(n: usize, closed: bool) -> Vec<u8> {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let mut ids = IdSource::new(0xAC);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let mains: Vec<NodeId> = (0..n).map(|_| ids.mint()).collect();
            let mut ops = Vec::new();
            for (k, &m) in mains.iter().enumerate() {
                ops.push(create(m, root, k, frame()));
                ops.push(Operation::SetComponent {
                    id: m,
                    component: true,
                });
            }
            let last = if closed { n } else { n - 1 };
            for k in 0..last {
                let inner = ids.mint();
                ops.push(create(inner, mains[k], 0, frame()));
                ops.push(Operation::SetLink {
                    id: inner,
                    link: Some(mains[(k + 1) % n]),
                });
            }
            if closed {
                // `apply` refuses the cycle, so the closed chain's file is
                // written as the open one's with the last link added by hand.
                return close_by_hand(&mut doc, ops);
            }
            doc.apply(&Transaction(ops)).expect("a chain of mains");
            io::save(&doc).unwrap()
        })
        .unwrap()
        .join()
        .unwrap()
}

/// Apply `ops` but the last `SetLink` (the one closing the chain), save, and
/// write that link into the file's JSON — the crafted file `apply` would refuse.
fn close_by_hand(doc: &mut Document, mut ops: Vec<Operation>) -> Vec<u8> {
    let Some(Operation::SetLink {
        id,
        link: Some(target),
    }) = ops.pop()
    else {
        panic!("the closing link is the last op");
    };
    doc.apply(&Transaction(ops)).expect("the open chain");
    let mut json: serde_json::Value = serde_json::from_slice(&io::save(doc).unwrap()).unwrap();
    let wire = |id: NodeId| serde_json::Value::String(id.to_wire());
    let node = json["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["id"] == wire(id))
        .expect("the closing frame is in the file");
    node["link"] = wire(target);
    serde_json::to_vec(&json).unwrap()
}

/// Load `bytes` on a thread with a small stack: 256 KB in a debug build, 64 KB
/// under `--release`. A debug load of even a short document needs more than
/// 64 KB, and a release build's frames are small enough that the recursive
/// search survived 2,000 mains on 256 KB — so each profile gets the size at which
/// the flip below bites (measured both ways).
fn load_on_a_small_stack(bytes: Vec<u8>) -> Result<Document, String> {
    let stack = if cfg!(debug_assertions) { 256 } else { 64 } << 10;
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || io::load(&bytes).map_err(|e| e.to_string()))
        .unwrap()
        .join()
        .unwrap()
}

/// A crafted file holding a long chain of mains loads on a small stack, and the
/// same chain closed into a loop is refused as a cycle rather than aborting the
/// process (`[R1-L2-04]`). `component::check`'s cycle search runs on every load
/// and every `Document::apply`, and it recursed once per main along the chain: a
/// stack overflow is an abort, not a panic, so nothing caught it — the cover
/// worker's `catch_unwind` included (the rule `io::MAX_TREE_DEPTH`'s doc states
/// for §15 D416, arriving by a new axis). The search is an explicit stack now.
///
/// Flip, run: the recursive search put back aborts the test binary on the open
/// chain's load (`thread '<unknown>' has overflowed its stack`, exit
/// `STATUS_STACK_OVERFLOW`), in debug on 256 KB and in release on 64 KB. In debug
/// the recursion loaded 200 mains on 256 KB and overflowed at 500; the fixed
/// search loads 20,000 there (slowly — ~200 s debug, the load being
/// super-linear in mains elsewhere, so the test keeps to 2,000).
#[test]
fn a_deep_chain_of_mains_loads_on_a_small_stack() {
    const N: usize = 2_000;
    let open = load_on_a_small_stack(chain_of_mains(N, false)).expect("a chain loads");
    assert_eq!(open.get(open.root()).unwrap().children().len(), N);
    let closed = load_on_a_small_stack(chain_of_mains(N, true)).expect_err("a loop is refused");
    assert!(
        closed.contains("contains an instance of itself"),
        "{closed}"
    );
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

/// Only a link **straight to a main** is placed freely. A layer at the canvas root
/// linked to the nested instance *inside* another main is an instance root by its
/// chain, but it was never copied with that main, so it belongs nowhere and is
/// refused. Flip: exempting every instance root from membership (the first build,
/// `is_root(n)` → `continue`) accepts it — `arch-scribe` found that looseness by
/// reading the rule against §5.3d.
#[test]
fn a_link_to_a_nested_copy_is_held_to_membership() {
    let mut f = fixture();
    let [outer, n, stray] = [(); 3].map(|_| f.ids.mint());
    f.doc
        .apply(&Transaction(vec![
            create(outer, f.root, 2, frame()),
            create(n, outer, 0, frame()),
            Operation::SetComponent {
                id: outer,
                component: true,
            },
            Operation::SetLink {
                id: n,
                link: Some(f.m),
            },
        ]))
        .expect("a main holding a nested instance");
    let (id, rule) = refused(
        &mut f.doc,
        vec![
            create(stray, f.root, 3, frame()),
            Operation::SetLink {
                id: stray,
                link: Some(n),
            },
        ],
    );
    assert_eq!((id, rule), (stray, LinkRule::Membership));
}

// ── The verbs (build step 2) ───────────────────────────────────────────────────

/// Main `outer` holding a nested instance `n` (of `m`, children `na`/`nb`), and
/// an instance `io_` of `outer` holding the copy `cn` (children `cna`/`cnb`).
fn nested(f: &mut Fixture) -> [NodeId; 8] {
    let ids @ [outer, n, na, nb, io_, cn, cna, cnb] = [(); 8].map(|_| f.ids.mint());
    let link = |id, to| Operation::SetLink { id, link: Some(to) };
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
            link(n, f.m),
            link(na, f.a),
            link(nb, f.b),
            create(io_, f.root, 3, frame()),
            create(cn, io_, 0, frame()),
            create(cna, cn, 0, rect()),
            create(cnb, cn, 1, rect()),
            link(io_, outer),
            link(cn, n),
            link(cna, na),
            link(cnb, nb),
        ]))
        .expect("the nested fixture");
    ids
}

fn link_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    doc.get(id).unwrap().link()
}

/// Deleting a main detaches its instances (§15 D979 (c)) and a nested instance
/// copied with them **climbs to its own main** rather than detaching. Flip:
/// `relink_past` cutting every link into the deleted set instead of climbing
/// leaves `cn` unlinked, and the nested instance is lost.
#[test]
fn deleting_a_main_detaches_its_instances_and_keeps_nested_ones() {
    let mut f = fixture();
    let [outer, _, _, _, io_, cn, cna, cnb] = nested(&mut f);
    let mut ops = vec![Operation::DeleteNode { id: outer }];
    ops.extend(ondin_core::component::relink_for_delete(&f.doc, &[outer]));
    f.doc
        .apply(&Transaction(ops))
        .expect("the delete with its relinks");
    assert_eq!(link_of(&f.doc, io_), None, "the instance detaches");
    assert_eq!(
        link_of(&f.doc, cn),
        Some(f.m),
        "the nested copy climbs to m"
    );
    assert_eq!(link_of(&f.doc, cna), Some(f.a));
    assert_eq!(link_of(&f.doc, cnb), Some(f.b));
}

/// Deleting a node inside a main leaves its counterpart as the instance's own
/// layer, never deleted with it — until build step 4 can tell untouched from
/// changed.
#[test]
fn deleting_a_mains_child_keeps_its_counterpart_as_a_local_layer() {
    let mut f = fixture();
    let mut ops = vec![Operation::DeleteNode { id: f.b }];
    ops.extend(ondin_core::component::relink_for_delete(&f.doc, &[f.b]));
    f.doc.apply(&Transaction(ops)).unwrap();
    assert!(f.doc.get(f.ib).is_some());
    assert_eq!(link_of(&f.doc, f.ib), None);
    assert_eq!(link_of(&f.doc, f.ia), Some(f.a), "the rest still follow");
}

/// Detaching an instance cuts its own links and lets a nested instance climb.
#[test]
fn detaching_an_instance_keeps_a_nested_one() {
    let mut f = fixture();
    let [_, _, _, _, io_, cn, cna, _] = nested(&mut f);
    let tx = ondin_core::component::detach(&f.doc, io_).unwrap();
    f.doc.apply(&tx).unwrap();
    assert_eq!(link_of(&f.doc, io_), None);
    assert_eq!(link_of(&f.doc, cn), Some(f.m));
    assert_eq!(link_of(&f.doc, cna), Some(f.a));
}

/// Detaching the **nested** instance alone cuts it and its members — it does not
/// climb them, which would hand each member a link with no instance root above
/// it. Flip: climbing every link past the source (the first draft) is refused by
/// `apply` as `Membership`.
#[test]
fn detaching_a_nested_instance_cuts_its_members_too() {
    let mut f = fixture();
    let [_, _, _, _, io_, cn, cna, cnb] = nested(&mut f);
    let tx = ondin_core::component::detach(&f.doc, cn).unwrap();
    f.doc.apply(&tx).expect("a valid detach");
    for id in [cn, cna, cnb] {
        assert_eq!(link_of(&f.doc, id), None, "{id:?}");
    }
    assert!(
        link_of(&f.doc, io_).is_some(),
        "the outer instance is untouched"
    );
}

fn duplicate(f: &mut Fixture, id: NodeId) -> (Transaction, NodeId) {
    duplicate_as(f, id, ondin_core::component::MainCopy::Instance)
}

fn duplicate_as(
    f: &mut Fixture,
    id: NodeId,
    mains: ondin_core::component::MainCopy,
) -> (Transaction, NodeId) {
    let parent = f.doc.get(id).unwrap().parent().unwrap();
    let placements = [ondin_core::Placement {
        nodes: f.doc.capture_subtree(id).unwrap(),
        parent,
        index: None,
    }];
    let (tx, created) = ondin_core::build::insert_subtrees_as(
        &f.doc,
        &mut f.ids,
        &placements,
        Default::default(),
        mains,
    );
    (tx, created[0])
}

/// A copy of a main is an **instance** of it (§15 D979 (e)): every copied node
/// links to its original and the names are the main's. Flip: `settle_copy`'s
/// instance arm removed leaves a second main, refused nowhere and wrong.
#[test]
fn duplicating_a_main_makes_an_instance_of_it() {
    let mut f = fixture();
    let m = f.m;
    let (tx, copy) = duplicate(&mut f, m);
    f.doc.apply(&tx).unwrap();
    let c = f.doc.get(copy).unwrap();
    assert!(!c.component());
    assert_eq!(c.link(), Some(f.m));
    assert_eq!(
        c.name(),
        f.doc.get(f.m).unwrap().name(),
        "no copy numbering"
    );
    let kids: Vec<_> = c.children().iter().map(|k| link_of(&f.doc, *k)).collect();
    assert_eq!(kids, vec![Some(f.a), Some(f.b)]);
    assert_eq!(ondin_core::component::instances_of(&f.doc, f.m).len(), 2);
}

/// *Duplicate as component* makes a new, unlinked main.
#[test]
fn duplicating_as_component_makes_a_new_main() {
    let mut f = fixture();
    let m = f.m;
    let (tx, copy) = duplicate_as(&mut f, m, ondin_core::component::MainCopy::NewMain);
    f.doc.apply(&tx).unwrap();
    let c = f.doc.get(copy).unwrap();
    assert!(c.component());
    assert_eq!(c.link(), None);
}

/// A member duplicated inside its own instance becomes the instance's own layer:
/// keeping the link would give two nodes of one instance one source. Flip: keeping
/// a lone member's link is refused as `SharedSource`.
#[test]
fn duplicating_a_member_inside_its_instance_makes_a_local_layer() {
    let mut f = fixture();
    let ia = f.ia;
    let (tx, copy) = duplicate(&mut f, ia);
    f.doc.apply(&tx).expect("a local copy");
    assert_eq!(link_of(&f.doc, copy), None);
    assert_eq!(link_of(&f.doc, f.ia), Some(f.a));
}

/// An instance duplicated is another instance of the same main.
#[test]
fn duplicating_an_instance_makes_another_instance() {
    let mut f = fixture();
    let i = f.i;
    let (tx, copy) = duplicate(&mut f, i);
    f.doc.apply(&tx).unwrap();
    assert_eq!(link_of(&f.doc, copy), Some(f.m));
    let kid = f.doc.get(copy).unwrap().children()[0];
    assert_eq!(link_of(&f.doc, kid), Some(f.a));
}

/// A paste from another document drops links it cannot resolve (§15 D979 (d)):
/// the instance arrives as plain layers.
#[test]
fn pasting_an_instance_into_another_document_drops_its_links() {
    let f = fixture();
    let template = f.doc.capture_subtree(f.i).unwrap();
    let mut ids = IdSource::new(0xDA);
    let root = ids.mint();
    let mut other = Document::new(root);
    let (tx, created) = ondin_core::insert_subtrees(
        &other,
        &mut ids,
        &[ondin_core::Placement {
            nodes: template,
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    other.apply(&tx).expect("plain layers");
    assert_eq!(link_of(&other, created[0]), None);
}

/// `tx` plus the links its structural edits owe, as `commit_inner` assembles it.
fn settled(doc: &Document, ops: Vec<Operation>) -> Transaction {
    let mut tx = Transaction(ops);
    let cuts = ondin_core::component::settle_links(doc, &tx);
    tx.0.extend(cuts);
    tx
}

/// A member dragged out of its instance becomes its own layer: the bare move is
/// refused (`Membership`), the settled one goes through with the link cut. Flip:
/// `settle_links` answering nothing leaves the settled move refused too.
#[test]
fn a_member_moved_out_of_its_instance_loses_its_link() {
    let mut f = fixture();
    let out = Operation::Reparent {
        id: f.ia,
        new_parent: f.root,
        index: 0,
    };
    refused(&mut f.doc, vec![out.clone()]);
    let tx = settled(&f.doc, vec![out]);
    f.doc.apply(&tx).expect("the move with its cut");
    assert_eq!(link_of(&f.doc, f.ia), None);
    assert_eq!(
        link_of(&f.doc, f.ib),
        Some(f.b),
        "the one left behind still follows"
    );
}

/// Regrouping inside its own instance moves nothing out of it: no cut.
#[test]
fn a_member_regrouped_inside_its_instance_keeps_its_link() {
    let mut f = fixture();
    let g = f.ids.mint();
    let tx = settled(
        &f.doc,
        vec![
            create(g, f.i, 2, NodeKind::Group),
            Operation::Reparent {
                id: f.ia,
                new_parent: g,
                index: 0,
            },
        ],
    );
    assert_eq!(tx.0.len(), 2, "nothing cut");
    f.doc.apply(&tx).expect("a local group holding a member");
    assert_eq!(link_of(&f.doc, f.ia), Some(f.a));
}

/// A nested copy moved out of its outer instance is cut, and its members with it
/// — they are asked after it, with its link already gone.
#[test]
fn a_nested_copy_moved_out_takes_its_members_links_with_it() {
    let mut f = fixture();
    let [_, _, _, _, _, cn, cna, cnb] = nested(&mut f);
    let tx = settled(
        &f.doc,
        vec![Operation::Reparent {
            id: cn,
            new_parent: f.root,
            index: 0,
        }],
    );
    f.doc.apply(&tx).expect("the move with its cuts");
    for id in [cn, cna, cnb] {
        assert_eq!(link_of(&f.doc, id), None, "{id:?}");
    }
}

/// A nested instance duplicated on its own climbs to its own main — it was never
/// going to belong to the outer instance a second time (`SharedSource`), nor to
/// nothing (`Membership`). Flip: dropping `settle_copy`'s climb is refused.
#[test]
fn a_nested_instance_copied_alone_becomes_an_instance_of_its_own_main() {
    let mut f = fixture();
    let [_, _, _, _, _, cn, _, _] = nested(&mut f);
    let (tx, copy) = duplicate(&mut f, cn);
    f.doc.apply(&tx).expect("a plain instance of m");
    assert_eq!(link_of(&f.doc, copy), Some(f.m));
    let kids: Vec<_> = f
        .doc
        .get(copy)
        .unwrap()
        .children()
        .iter()
        .map(|k| link_of(&f.doc, *k))
        .collect();
    assert_eq!(kids, vec![Some(f.a), Some(f.b)]);
}

/// **Ungrouping a main** by any door — its children lifted out, the main itself
/// deleted — leaves its instances unlinked rather than refused: the root's link
/// climbs to nothing and its members, with no instance around them, are cut. No
/// verb relinked here; `settle_links` did. Flip: dropping the climb leaves the
/// instance linked to a node that is gone (`Dangling`).
#[test]
fn ungrouping_a_main_by_raw_ops_detaches_its_instances() {
    let mut f = fixture();
    let tx = settled(
        &f.doc,
        vec![
            Operation::Reparent {
                id: f.a,
                new_parent: f.root,
                index: 0,
            },
            Operation::Reparent {
                id: f.b,
                new_parent: f.root,
                index: 1,
            },
            Operation::DeleteNode { id: f.m },
        ],
    );
    f.doc.apply(&tx).expect("the ungroup with its relinks");
    for id in [f.i, f.ia, f.ib] {
        assert_eq!(link_of(&f.doc, id), None, "{id:?}");
    }
}

/// A member moved into another instance of the same main, where that instance's
/// own counterpart already holds the source, is the one cut — not the counterpart
/// the transaction never touched.
#[test]
fn moving_a_member_into_a_sibling_instance_cuts_the_mover() {
    let mut f = fixture();
    let i = f.i;
    let (tx, other) = duplicate(&mut f, i);
    f.doc.apply(&tx).unwrap();
    let theirs = f.doc.get(other).unwrap().children()[0];
    let tx = settled(
        &f.doc,
        vec![Operation::Reparent {
            id: f.ia,
            new_parent: other,
            index: 0,
        }],
    );
    f.doc.apply(&tx).expect("the move with its cut");
    assert_eq!(link_of(&f.doc, f.ia), None, "the mover is cut");
    assert_eq!(
        link_of(&f.doc, theirs),
        Some(f.a),
        "the counterpart keeps its source"
    );
}
