//! Kits — the vault, provenance and inserting from a kit (§5.3e, §15 D1012).
//!
//! A kit L publishes a frozen copy of its mains (`kit::freeze`); a consumer D
//! vendors one into its **vault**, a parentless second root, and places an
//! instance of it in its own tree (`kit::insert`). These tests hold the three
//! things that make that safe: the vault is never resolved, so it is never drawn
//! or picked; every rule a document keeps is kept by the vault and by the frozen
//! copy; and the component passes see a vendored main as any other.

use ondin_core::kit::{self, Origin, Published, VaultRule};
use ondin_core::kurbo::{Affine, Point, RoundedRectRadii, Size};
use ondin_core::meta::{DocumentMeta, KitMark};
use ondin_core::variant::{VariantProp, VariantSet};
use ondin_core::{
    Document, IdSource, NodeId, NodeKind, OpError, Operation, Placement, Resolved, Transaction,
    hit_test, io,
};

const L_ID: &str = "5eed0000000000000000000000000001";
const K_ID: &str = "5eed0000000000000000000000000002";

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

fn main_on(id: NodeId) -> Operation {
    Operation::SetComponent {
        id,
        component: true,
    }
}

/// An instance of `main` placed under `parent` — the duplicate verb's own path.
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

/// Kit L: a frame *Buttons* holding a set *Button* of two variants — *Large*
/// first in the list and lower on the canvas, *Small* second and on top — a lone
/// main *Icon*, and a main *Card* that nests an instance of *Icon*.
struct Kit {
    doc: Document,
    set: NodeId,
    small: NodeId,
    large: NodeId,
    icon: NodeId,
    icon_dot: NodeId,
    card: NodeId,
}

fn kit() -> Kit {
    let mut ids = IdSource::new(0xAC);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [buttons, set, large, small, icon, icon_dot, card] = [(); 7].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(buttons, root, 0, frame(600.0, 400.0), "Buttons"),
        create(set, buttons, 0, frame(300.0, 200.0), "Button"),
        create(large, set, 0, frame(120.0, 40.0), "Large"),
        create(small, set, 1, frame(80.0, 30.0), "Small"),
        Operation::SetTransform {
            id: large,
            transform: Affine::translate((0.0, 60.0)),
        },
        main_on(large),
        main_on(small),
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![VariantProp {
                    name: "Size".into(),
                    values: vec!["Large".into(), "Small".into()],
                }],
            }),
        },
        Operation::SetVariant {
            id: large,
            values: vec!["Large".into()],
        },
        Operation::SetVariant {
            id: small,
            values: vec!["Small".into()],
        },
        create(icon, root, 1, frame(24.0, 24.0), "Icon"),
        create(icon_dot, icon, 0, rect(), "Dot"),
        main_on(icon),
        create(card, root, 2, frame(200.0, 100.0), "Card"),
    ]))
    .expect("the kit's mains");
    instance(&mut doc, &mut ids, icon, card);
    doc.apply(&Transaction(vec![main_on(card)]))
        .expect("a main nesting an instance");
    doc.set_meta(DocumentMeta {
        id: Some(L_ID.into()),
        name: Some("Atlas".into()),
        kit: Some(KitMark {
            version: 4,
            ..KitMark::default()
        }),
        ..DocumentMeta::default()
    });
    let published = kit::freeze(&doc).expect("the kit has mains");
    doc.set_published(Some(published));
    Kit {
        doc,
        set,
        small,
        large,
        icon,
        icon_dot,
        card,
    }
}

fn published(k: &Kit) -> &Published {
    k.doc.published().unwrap()
}

/// Consumer D: an empty document with its own id stream.
fn consumer() -> (Document, IdSource) {
    let mut ids = IdSource::new(0xDA);
    let root = ids.mint();
    (Document::new(root), ids)
}

fn insert(doc: &mut Document, ids: &mut IdSource, k: &Kit, main: NodeId) -> NodeId {
    let root = doc.root();
    let (tx, made) = kit::insert(doc, ids, L_ID, 4, published(k), main, root).unwrap();
    doc.apply(&tx).expect("one kit insert, one transaction");
    made
}

fn vault_children(doc: &Document) -> Vec<NodeId> {
    doc.vault()
        .map(|v| doc.get(v).unwrap().children().to_vec())
        .unwrap_or_default()
}

/// What a publish freezes: the set and both lone mains, ids kept, each set's
/// variants inside it, and the frame each sat in.
#[test]
fn a_publish_freezes_every_set_and_main_with_its_frame() {
    let k = kit();
    let p = published(&k);
    assert_eq!(p.tops(), &[k.set, k.icon, k.card]);
    assert_eq!(p.group(k.set), Some("Buttons"));
    assert_eq!(p.group(k.icon), None, "outside any frame");
    assert!(p.document().get(k.small).unwrap().component());
    assert!(
        p.document().vault().is_none(),
        "nothing vendored from elsewhere"
    );
}

/// The plain insert: L's *Icon* lands in D's vault with its origin, an instance of
/// it lands in D's tree, and the whole of it is one transaction — undone in one
/// step, the vault going with it.
#[test]
fn an_insert_vendors_the_main_and_places_an_instance() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let before = d.clone();
    let root = d.root();
    let (tx, inst) = kit::insert(&d, &mut ids, L_ID, 4, published(&k), k.icon, root).unwrap();
    let undo = d.apply(&tx).unwrap().inverse;

    let [copy] = vault_children(&d)[..] else {
        panic!("one vendored main")
    };
    assert_ne!(copy, k.icon, "minted its own id here");
    assert_eq!(
        d.get(copy).unwrap().origin(),
        Some(&Origin {
            kit: L_ID.into(),
            node: k.icon,
            version: 4,
            name: Some("Atlas".into()),
        }),
        "the kit's name on the top node, for a kit that cannot be found"
    );
    let dot = d.get(copy).unwrap().children()[0];
    assert_eq!(d.get(dot).unwrap().origin().unwrap().node, k.icon_dot);
    assert_eq!(d.get(dot).unwrap().origin().unwrap().name, None, "top only");
    assert_eq!(d.get(inst).unwrap().link(), Some(copy));
    assert_eq!(d.get(inst).unwrap().parent(), Some(root));
    assert_eq!(
        d.get(inst).unwrap().origin(),
        None,
        "an instance is not vendored"
    );

    d.apply(&undo).unwrap();
    assert_eq!(d, before, "the vault closed with the undo");
}

/// **The vault is never resolved**, so nothing draws or picks it. The instance
/// sits exactly where the main does — the insert places it at the main's own
/// transform — so a pick there that answered the main would be the leak this
/// design exists to prevent. Checked after a rebuild and after an `update` from a
/// transaction that dirtied the main (an edit like a kit update makes), which is
/// the door `rebuild` does not cover.
///
/// Flip, run: `Resolved::update`'s `in_vault` dropped from `unresolved` fails the
/// second block — the main's dot gets world bounds — and the first stays green,
/// which is the rebuild/update split this test exists to name.
#[test]
fn the_vault_is_never_resolved_nor_picked() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let copy = vault_children(&d)[0];
    let dot = d.get(copy).unwrap().children()[0];

    let mut res = Resolved::rebuild(&d);
    assert!(res.world_bounds(copy).is_none() && res.world_bounds(dot).is_none());
    let hits = hit_test(&res, &d, Point::new(5.0, 5.0), 0.0);
    assert!(!hits.is_empty(), "the instance is under the point");
    assert!(hits.iter().all(|h| !d.in_vault(*h)), "{hits:?}");

    let out = d
        .apply(&Transaction(vec![Operation::SetTransform {
            id: dot,
            transform: Affine::translate((3.0, 3.0)),
        }]))
        .unwrap();
    assert!(out.dirty.0.contains(&dot), "the fixture dirtied the vault");
    res.update(&d, &out.dirty);
    assert!(res.world_bounds(dot).is_none() && res.world_bounds(copy).is_none());
    let hits = hit_test(&res, &d, Point::new(5.0, 5.0), 0.0);
    assert!(hits.contains(&inst) && hits.iter().all(|h| !d.in_vault(*h)));
}

/// A second insert of the same main reuses the vendored copy: one copy in the
/// vault, two instances of it.
#[test]
fn a_main_is_vendored_once() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let a = insert(&mut d, &mut ids, &k, k.icon);
    let b = insert(&mut d, &mut ids, &k, k.icon);
    assert_eq!(vault_children(&d).len(), 1);
    assert_eq!(d.get(a).unwrap().link(), d.get(b).unwrap().link());
}

/// A main nesting another main's instance brings that main along, and the nested
/// copy links to **D's** copy of it — not to L's id, which means nothing here.
/// Inserting the nested main afterwards reuses it.
#[test]
fn a_nested_main_is_vendored_with_the_main_that_shows_it() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    insert(&mut d, &mut ids, &k, k.card);
    let vault = vault_children(&d);
    assert_eq!(vault.len(), 2, "Card and Icon");
    let by_origin = |n: NodeId| {
        vault
            .iter()
            .copied()
            .find(|v| d.get(*v).unwrap().origin().unwrap().node == n)
            .unwrap()
    };
    let (card, icon) = (by_origin(k.card), by_origin(k.icon));
    let nested = d.get(card).unwrap().children()[0];
    assert_eq!(d.get(nested).unwrap().link(), Some(icon));
    insert(&mut d, &mut ids, &k, k.icon);
    assert_eq!(vault_children(&d).len(), 2, "Icon reused");
}

/// A set inserts its default variant — the one first in reading order, *Small*
/// here, though *Large* is first among the set's children — and is vendored
/// whole. A variant named directly is placed as itself.
#[test]
fn a_set_inserts_its_default_variant_and_is_vendored_whole() {
    let k = kit();
    assert_eq!(
        kit::default_variant(published(&k).document(), k.set),
        Some(k.small)
    );
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.set);
    let [set] = vault_children(&d)[..] else {
        panic!("the set alone at the vault's top")
    };
    assert_eq!(d.get(set).unwrap().children().len(), 2);
    let main = d.get(inst).unwrap().link().unwrap();
    assert_eq!(d.get(main).unwrap().origin().unwrap().node, k.small);

    let large = insert(&mut d, &mut ids, &k, k.large);
    let main = d.get(large).unwrap().link().unwrap();
    assert_eq!(d.get(main).unwrap().origin().unwrap().node, k.large);
    assert_eq!(vault_children(&d).len(), 1, "the set reused");
}

/// A kit that vendored a main from a third kit publishes it in its frozen vault,
/// and a consumer vendors it **as the third kit's** — its origin names K, not L,
/// so an update of K reaches it and two routes to one main meet at one copy.
#[test]
fn a_transitive_main_keeps_the_kit_it_came_from() {
    // K: one main, *Badge*.
    let mut kids = IdSource::new(0xAB);
    let kroot = kids.mint();
    let mut kdoc = Document::new(kroot);
    let badge = kids.mint();
    kdoc.apply(&Transaction(vec![
        create(badge, kroot, 0, frame(16.0, 16.0), "Badge"),
        main_on(badge),
    ]))
    .unwrap();
    kdoc.set_meta(DocumentMeta {
        name: Some("Badges".into()),
        ..DocumentMeta::default()
    });
    kdoc.set_published(kit::freeze(&kdoc));

    // L uses K's *Badge* inside its own main *Chip*, then publishes.
    let mut lids = IdSource::new(0xAC);
    let lroot = lids.mint();
    let mut ldoc = Document::new(lroot);
    let (tx, _) = kit::insert(
        &ldoc,
        &mut lids,
        K_ID,
        2,
        kdoc.published().unwrap(),
        badge,
        lroot,
    )
    .unwrap();
    ldoc.apply(&tx).unwrap();
    let placed = *ldoc.get(lroot).unwrap().children().last().unwrap();
    let chip = lids.mint();
    ldoc.apply(&Transaction(vec![create(
        chip,
        lroot,
        0,
        frame(60.0, 20.0),
        "Chip",
    )]))
    .unwrap();
    ldoc.apply(&Transaction(vec![Operation::Reparent {
        id: placed,
        new_parent: chip,
        index: 0,
    }]))
    .unwrap();
    ldoc.apply(&Transaction(vec![main_on(chip)])).unwrap();
    let p = kit::freeze(&ldoc).unwrap();
    assert_eq!(p.tops(), &[chip], "K's main is not one of L's own");
    assert!(
        p.document().vault().is_some(),
        "but it travels in the frozen vault"
    );

    let (mut d, mut ids) = consumer();
    let root = d.root();
    let (tx, _) = kit::insert(&d, &mut ids, L_ID, 7, &p, chip, root).unwrap();
    d.apply(&tx).unwrap();
    let origins: Vec<Origin> = vault_children(&d)
        .iter()
        .map(|v| d.get(*v).unwrap().origin().unwrap().clone())
        .collect();
    assert!(origins.contains(&Origin {
        kit: L_ID.into(),
        node: chip,
        version: 7,
        name: None,
    }));
    assert!(
        origins.contains(&Origin {
            kit: K_ID.into(),
            node: badge,
            version: 2,
            name: Some("Badges".into()),
        }),
        "K's own name, carried through L: {origins:?}"
    );
}

/// **A third kit's main never takes the name of the kit it came through.** K
/// published with no name — a pre-library file has none — and L, *Atlas*, nests
/// K's main in its own; a consumer's copy of K's main must not read *Atlas*,
/// which is what the picker would then call K once K is lost (3I).
///
/// Flip, run: `vendor`'s `o.kit == kit` guard removed fails the assertion, the
/// badge's copy named *Atlas*.
#[test]
fn a_third_kits_main_keeps_its_own_name_or_none() {
    let mut kids = IdSource::new(0xAB);
    let kroot = kids.mint();
    let mut kdoc = Document::new(kroot);
    let badge = kids.mint();
    kdoc.apply(&Transaction(vec![
        create(badge, kroot, 0, frame(16.0, 16.0), "Badge"),
        main_on(badge),
    ]))
    .unwrap();
    kdoc.set_published(kit::freeze(&kdoc));

    let mut lids = IdSource::new(0xAC);
    let lroot = lids.mint();
    let mut ldoc = Document::new(lroot);
    ldoc.set_meta(DocumentMeta {
        name: Some("Atlas".into()),
        ..DocumentMeta::default()
    });
    let (tx, placed) = kit::insert(
        &ldoc,
        &mut lids,
        K_ID,
        1,
        kdoc.published().unwrap(),
        badge,
        lroot,
    )
    .unwrap();
    ldoc.apply(&tx).unwrap();
    let chip = lids.mint();
    ldoc.apply(&Transaction(vec![
        create(chip, lroot, 0, frame(60.0, 20.0), "Chip"),
        Operation::Reparent {
            id: placed,
            new_parent: chip,
            index: 0,
        },
    ]))
    .unwrap();
    ldoc.apply(&Transaction(vec![main_on(chip)])).unwrap();
    let p = kit::freeze(&ldoc).unwrap();

    let (mut d, mut ids) = consumer();
    let root = d.root();
    let (tx, _) = kit::insert(&d, &mut ids, L_ID, 1, &p, chip, root).unwrap();
    d.apply(&tx).unwrap();
    let names: Vec<(String, Option<String>)> = vault_children(&d)
        .iter()
        .map(|v| {
            let o = d.get(*v).unwrap().origin().unwrap();
            (o.kit.clone(), o.name.clone())
        })
        .collect();
    assert!(
        names.contains(&(L_ID.into(), Some("Atlas".into()))),
        "{names:?}"
    );
    assert!(names.contains(&(K_ID.into(), None)), "{names:?}");
}

/// An edit to a vendored main reaches its instances through the ordinary
/// propagation — the component passes ask by id, so the vault is no different —
/// which is what an update will lean on.
#[test]
fn an_edit_to_a_vendored_main_reaches_its_instance() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let copy = vault_children(&d)[0];
    let dot = d.get(copy).unwrap().children()[0];
    let idot = d.get(inst).unwrap().children()[0];
    let mut tx = Transaction(vec![Operation::SetOpacity {
        id: dot,
        opacity: 0.5,
    }]);
    ondin_core::propagate::owed(&d, &mut tx, &mut ids);
    d.apply(&tx).unwrap();
    assert_eq!(d.get(idot).unwrap().opacity(), 0.5);
}

/// The vault's rules, each one operation from a valid document.
#[test]
fn the_vault_keeps_its_rules() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    insert(&mut d, &mut ids, &k, k.icon);
    let vault = d.vault().unwrap();
    let copy = vault_children(&d)[0];
    let refused = |d: &mut Document, ops: Vec<Operation>| match d.apply(&Transaction(ops)) {
        Err(OpError::Vault(_, rule)) => rule,
        other => panic!("expected a vault rule refused, got {other:?}"),
    };

    // A vendored main taken out of the vault keeps an origin it may not.
    let root = d.root();
    assert_eq!(
        refused(
            &mut d,
            vec![Operation::Reparent {
                id: copy,
                new_parent: root,
                index: 0,
            }]
        ),
        VaultRule::OutsideVault
    );
    // A plain layer put in the vault has no origin, and is not a main.
    let stray = ids.mint();
    assert_eq!(
        refused(
            &mut d,
            vec![create(stray, vault, 1, frame(10.0, 10.0), "Stray")]
        ),
        VaultRule::NotAMain
    );
    let inner = ids.mint();
    assert_eq!(
        refused(&mut d, vec![create(inner, copy, 0, rect(), "Inner")]),
        VaultRule::NoOrigin
    );
    // Opened twice; closed while it holds something.
    assert_eq!(
        refused(&mut d, vec![Operation::OpenVault { id: ids.mint() }]),
        VaultRule::State
    );
    assert_eq!(
        refused(&mut d, vec![Operation::CloseVault { id: vault }]),
        VaultRule::State
    );
    // A vault node pointed at a node of the document (§15 D1036 (1)).
    // An instance nested in the vault's copy, of a main of the document's own.
    let [local, nest] = [(); 2].map(|_| ids.mint());
    d.apply(&Transaction(vec![
        create(local, root, 0, frame(10.0, 10.0), "Local"),
        main_on(local),
    ]))
    .unwrap();
    let origin = Origin {
        node: nest,
        ..d.get(copy).unwrap().origin().unwrap().clone()
    };
    assert_eq!(
        refused(
            &mut d,
            vec![
                create(nest, copy, 1, frame(10.0, 10.0), "Nest"),
                Operation::SetOrigin {
                    id: nest,
                    origin: Some(origin),
                },
                Operation::SetLink {
                    id: nest,
                    link: Some(local),
                },
            ]
        ),
        VaultRule::LinksOut
    );
}

/// A consumer and a kit both save and load byte-identically (invariant 9): the
/// vault, the origins, the kit mark and the frozen copy all survive, the kit
/// mark inside the block the scan reads from the first 4 KB.
#[test]
fn a_consumer_and_a_kit_round_trip_through_a_file() {
    let k = kit();
    let kbytes = io::save(&k.doc).unwrap();
    let kback = io::load(&kbytes).unwrap();
    assert_eq!(kback, k.doc);
    assert_eq!(io::save(&kback).unwrap(), kbytes);
    match io::probe::meta_in_prefix(&kbytes[..kbytes.len().min(io::probe::PREFIX_BYTES)]) {
        io::probe::MetaProbe::Found(m) => assert_eq!(m.kit.unwrap().version, 4),
        other => panic!("the kit mark is in the prefix: {other:?}"),
    }

    let (mut d, mut ids) = consumer();
    insert(&mut d, &mut ids, &k, k.card);
    let bytes = io::save(&d).unwrap();
    let back = io::load(&bytes).unwrap();
    assert_eq!(back, d);
    assert_eq!(io::save(&back).unwrap(), bytes);
    assert_eq!(back.schema_version(), 7);
}

/// A file whose `vault` key went missing — what a v5 build would make of a v6
/// file if the version had not been bumped — is refused rather than loaded with
/// its vendored mains dropped or drawn. The vault itself is what the loader
/// trips on: a parentless node that is not the root, before reachability is
/// asked.
#[test]
fn a_file_that_lost_its_vault_key_is_refused() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    insert(&mut d, &mut ids, &k, k.icon);
    let vault = d.vault().unwrap().to_wire();
    let mut v: serde_json::Value = serde_json::from_slice(&io::save(&d).unwrap()).unwrap();
    v.as_object_mut().unwrap().remove("vault");
    let err = io::load(&serde_json::to_vec(&v).unwrap()).unwrap_err();
    assert!(
        err.to_string()
            .contains(&format!("non-root node {vault} has no parent")),
        "{err}"
    );
}

/// **A published state nested in a published state is refused** (§5.11: *the
/// nesting is one level*; the v0.5.0 release review's `[R2-L6-01]`). A hand-made
/// file whose kit's published document carries a published state of its own
/// would otherwise be loaded one level per nesting, and kept with a
/// `published` every reader assumes is never there. The guard had no test:
/// deleting it left every suite green.
///
/// Flip, run: the guard deleted fails *"the nested published state loaded"*.
#[test]
fn a_published_state_inside_a_published_state_is_refused() {
    let k = kit();
    let mut v: serde_json::Value = serde_json::from_slice(&io::save(&k.doc).unwrap()).unwrap();
    let inner = v["published"].clone();
    assert!(inner.is_object(), "fixture: the kit has published");
    v["published"]["document"]["published"] = inner;
    let Err(err) = io::load(&serde_json::to_vec(&v).unwrap()) else {
        panic!("the nested published state loaded");
    };
    assert!(
        err.to_string()
            .contains("a published kit state holds a published state of its own"),
        "{err}"
    );
}

/// A v5 file loads as the current version — two steps that are each a number
/// and nothing else (`migrate_5_to_6`, `migrate_6_to_7`).
#[test]
fn a_v5_file_loads_as_the_current_version() {
    let (d, _) = consumer();
    let mut v: serde_json::Value = serde_json::from_slice(&io::save(&d).unwrap()).unwrap();
    v["schema_version"] = 5.into();
    let back = io::load(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(back, d);
}

/// Copy a kit instance out of `from` through the clipboard's text form — the
/// door another window reads — and paste it into `to`'s root.
fn paste_across(from: &Document, inst: NodeId, to: &mut Document, ids: &mut IdSource) -> NodeId {
    let (tx, made) = try_paste_across(from, inst, to, ids).expect("the paste builds");
    to.apply(&tx).expect("a carrying paste");
    made[0]
}

/// `paste_across`'s transaction, or what the carrying paste refuses.
fn try_paste_across(
    from: &Document,
    inst: NodeId,
    to: &Document,
    ids: &mut IdSource,
) -> Result<(Transaction, Vec<NodeId>), kit::KitError> {
    let subtrees = vec![from.capture_subtree(inst).unwrap()];
    let kits = kit::carried(from, &subtrees);
    let text =
        io::clip::write_carrying(&subtrees, &kits, &[], &Default::default(), None, "Icon").unwrap();
    let payload = io::clip::read(&text).unwrap().unwrap();
    let root = to.root();
    let placements: Vec<Placement> = payload
        .subtrees
        .into_iter()
        .map(|nodes| Placement {
            nodes,
            parent: root,
            index: None,
        })
        .collect();
    kit::insert_carrying(to, ids, &payload.kits, &placements, Default::default())
}

/// **A kit instance pasted into a document that has never used the kit lands an
/// instance** (D1010's premise (d), ruled by §15 D1012): the copy carries the
/// vendored main, nested mains and all, and the paste vendors them with their
/// origins. Without the carry §15 D979 (d) cuts the link and the paste is a
/// detached copy.
///
/// Flip, run: `kit::carried` answering nothing fails *"still an instance"*, the
/// pasted Card's link cut.
#[test]
fn a_kit_instance_pasted_into_another_document_stays_an_instance() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.card);

    let mut eids = IdSource::new(0xEA);
    let eroot = eids.mint();
    let mut e = Document::new(eroot);
    let pasted = paste_across(&d, inst, &mut e, &mut eids);
    let main = e.get(pasted).unwrap().link().expect("still an instance");
    assert!(e.in_vault(main));
    assert_eq!(e.get(main).unwrap().origin().unwrap().node, k.card);
    assert_eq!(vault_children(&e).len(), 2, "Card and the Icon it nests");
    let nested = e.get(pasted).unwrap().children()[0];
    let nested_main = e.get(nested).unwrap().link().unwrap();
    assert!(
        e.in_vault(nested_main),
        "the nested copy links inside E's vault"
    );
}

/// Pasted back into its own document, or into one already holding the main, the
/// carry adds nothing: the held copy is matched by origin and reused.
#[test]
fn a_carrying_paste_reuses_what_the_document_holds() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.card);
    let before = vault_children(&d);
    let copy = d.clone();
    let pasted = paste_across(&copy, inst, &mut d, &mut ids);
    assert_eq!(vault_children(&d), before);
    assert_eq!(d.get(pasted).unwrap().link(), d.get(inst).unwrap().link());

    // Its own actor: two documents minting from one seed would share ids, which
    // no two sessions do (`session::random_actor`).
    let mut fids = IdSource::new(0xFA);
    let froot = fids.mint();
    let mut f = Document::new(froot);
    let held = insert(&mut f, &mut fids, &k, k.icon);
    let pasted = paste_across(&d, inst, &mut f, &mut fids);
    assert_eq!(vault_children(&f).len(), 2, "Icon reused, Card added");
    // A nested copy links one level up — to the node inside the Card main — so
    // it is the vendored Card's own nested instance that must show F's Icon.
    let card = f.get(pasted).unwrap().link().unwrap();
    let nested = f.get(card).unwrap().children()[0];
    assert_eq!(
        f.get(nested).unwrap().link(),
        f.get(held).unwrap().link(),
        "the vendored Card nests the Icon F already held"
    );
}

/// **A carrying paste over an older held main asks for the update** (§15 D1036
/// (7), the v0.5.0 release review's `[X1-L1-01]`). F holds *Icon* at v4; L
/// gives *Icon* a *Ring* and publishes v5; D places *Card* at v5. Pasting D's
/// *Card* into F carried a fresh *Card* whose nested copy links to a *Ring* F's
/// *Icon* lacks — inserted into the vault dangling, and the whole paste refused
/// as a broken link. It answers `NeedsUpdate`, naming F's *Icon*, as the picker
/// does for the same main.
///
/// Flip, run: `adopt` without the check fails *"an update owed"* with `Ok`.
#[test]
fn a_carrying_paste_over_an_older_held_main_asks_for_the_update() {
    let mut k = kit();
    let (mut f, mut fids) = consumer();
    let f_icon = insert(&mut f, &mut fids, &k, k.icon);
    let f_held = f.get(f_icon).unwrap().link().unwrap();
    let (ring, icon) = (IdSource::new(0xAF).mint(), k.icon);
    edit_and_publish(&mut k, vec![create(ring, icon, 1, rect(), "Ring")]);
    let version = k.doc.meta().kit.as_ref().unwrap().version;
    let mut d = Document::new(IdSource::new(0xDD).mint());
    let mut dids = IdSource::new(0xDE);
    let root = d.root();
    let (tx, card) =
        kit::insert(&d, &mut dids, L_ID, version, published(&k), k.card, root).unwrap();
    d.apply(&tx).unwrap();

    assert!(
        matches!(
            try_paste_across(&d, card, &f, &mut fids),
            Err(kit::KitError::NeedsUpdate(h)) if h == f_held
        ),
        "an update owed, naming the Icon F holds"
    );
}

/// 🚨 **A kit built on this one is not used back in it** (§15 D1036 (2), the
/// v0.5.0 release review's `[X1-L1-02]`). M uses L's *Icon* in its *Card2*;
/// L inserting *Card2* would vendor a frozen copy linking to L's live *Icon*,
/// and from then on every structural edit of *Icon* in L was refused,
/// `Vault(.., NoOrigin)`, naming nothing. The insert answers `Circular`
/// naming *Icon*, and so does an update of a *Card2* L held before M nested
/// the *Icon* in it.
///
/// Flip, run: `vendor`'s `into_vault` resolving as `resolve` does fails
/// *"the insert names Icon"*; `Builder::resolve` taking a live node fails
/// *"and so does the update"*.
#[test]
fn a_kit_built_on_this_one_is_refused_naming_the_main() {
    let mut k = kit();
    // M: an empty *Card2*, published as v1.
    let mut mids = IdSource::new(0xAB);
    let mroot = mids.mint();
    let mut m = Document::new(mroot);
    let card2 = mids.mint();
    m.apply(&Transaction(vec![
        create(card2, mroot, 0, frame(60.0, 20.0), "Card2"),
        main_on(card2),
    ]))
    .unwrap();
    let m1 = kit::freeze(&m).unwrap();
    // L takes *Card2* while it nests nothing.
    let lroot = k.doc.root();
    let mut lids = IdSource::new(0xA9);
    let (tx, _) = kit::insert(&k.doc, &mut lids, K_ID, 1, &m1, card2, lroot).unwrap();
    k.doc.apply(&tx).unwrap();

    // M puts L's *Icon* in *Card2*, and publishes v2.
    let (tx, placed) = kit::insert(&m, &mut mids, L_ID, 4, published(&k), k.icon, mroot).unwrap();
    m.apply(&tx).unwrap();
    m.apply(&Transaction(vec![Operation::Reparent {
        id: placed,
        new_parent: card2,
        index: 0,
    }]))
    .unwrap();
    let m2 = kit::freeze(&m).unwrap();

    let named =
        |r: Result<(), kit::KitError>| matches!(r, Err(kit::KitError::Circular(i)) if i == k.icon);
    // A second L that never took *Card2* — the fixture is the same document.
    let fresh = kit().doc;
    assert!(
        named(kit::insert(&fresh, &mut lids, K_ID, 2, &m2, card2, lroot).map(|_| ())),
        "the insert names Icon"
    );
    assert!(
        named(kit::update(&k.doc, &mut lids, K_ID, 2, &m2).map(|_| ())),
        "and so does the update"
    );
}

/// **A carried main left out of the paste leaves its nester detached, not
/// dangling** (the record's reading of the D1036 (7) build). L deleted its own
/// *Icon*; D holds M's *Card2*, which nests L's *Icon*; D's *Card2* pasted into
/// L. *Icon*'s template is L's own main, no longer here — left out (§15 D1023)
/// — and the fresh *Card2* went into the vault still linking to it, so the
/// paste was refused as a broken link. Its nested copy now lands detached.
///
/// Flip, run: the cut left out fails *"the paste commits"* with
/// `BadLink(.., Dangling)`.
#[test]
fn a_carried_main_left_out_detaches_what_nests_it() {
    let mut k = kit();
    let mut mids = IdSource::new(0xAB);
    let mroot = mids.mint();
    let mut m = Document::new(mroot);
    let card2 = mids.mint();
    m.apply(&Transaction(vec![
        create(card2, mroot, 0, frame(60.0, 20.0), "Card2"),
        main_on(card2),
    ]))
    .unwrap();
    let (tx, placed) = kit::insert(&m, &mut mids, L_ID, 4, published(&k), k.icon, mroot).unwrap();
    m.apply(&tx).unwrap();
    m.apply(&Transaction(vec![Operation::Reparent {
        id: placed,
        new_parent: card2,
        index: 0,
    }]))
    .unwrap();
    let m1 = kit::freeze(&m).unwrap();
    let (mut d, mut dids) = consumer();
    let droot = d.root();
    let (tx, inst) = kit::insert(&d, &mut dids, K_ID, 1, &m1, card2, droot).unwrap();
    d.apply(&tx).unwrap();

    // L deletes its own Icon (and Card, which nests it).
    let mut tx = Transaction(vec![
        Operation::DeleteNode { id: k.card },
        Operation::DeleteNode { id: k.icon },
    ]);
    let mut lids = IdSource::new(0xA9);
    ondin_core::propagate::owed(&k.doc, &mut tx, &mut lids);
    k.doc.apply(&tx).unwrap();

    let (tx, made) = try_paste_across(&d, inst, &k.doc, &mut lids).expect("the paste builds");
    k.doc.apply(&tx).expect("the paste commits");
    let card_here = k.doc.get(made[0]).unwrap().link().unwrap();
    let nested = k.doc.get(card_here).unwrap().children()[0];
    assert_eq!(
        k.doc.get(nested).unwrap().link(),
        None,
        "detached in the vault"
    );
}

/// **A kit never vendors itself.** A kit instance pasted back into the kit, or
/// one of the kit's own mains inserted from its own published state, links to
/// the live main — not to a frozen copy in a vault, which would stop following
/// the main it was made from (`arch-scribe`'s reading of the first build). An
/// origin's node is the kit's own id, so in the kit it names the main itself.
///
/// Flip, run: `kit::find`'s own-kit arm removed fails *"links to the live main"*,
/// the paste linked into a new vault.
#[test]
fn a_kit_never_vendors_itself() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.card);

    let mut lids = IdSource::new(0xAD);
    let pasted = paste_across(&d, inst, &mut k.doc, &mut lids);
    assert_eq!(
        k.doc.get(pasted).unwrap().link(),
        Some(k.card),
        "links to the live main"
    );
    assert!(k.doc.vault().is_none(), "nothing vendored");

    let root = k.doc.root();
    let p = k.doc.published().unwrap().clone();
    let (tx, made) = kit::insert(&k.doc, &mut lids, L_ID, 4, &p, k.icon, root).unwrap();
    k.doc.apply(&tx).unwrap();
    assert_eq!(k.doc.get(made).unwrap().link(), Some(k.icon));
    assert!(
        k.doc.vault().is_none(),
        "an insert from itself opens no vault"
    );
}

/// A variant a later publish added cannot be inserted beside an older copy of
/// its set: the copy has nowhere to put it, and the error says an update is owed
/// rather than that the kit is broken.
#[test]
fn a_variant_newer_than_the_held_set_asks_for_an_update() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    insert(&mut d, &mut ids, &k, k.set);

    let mut lids = IdSource::new(0xAD);
    let medium = lids.mint();
    k.doc
        .apply(&Transaction(vec![
            create(medium, k.set, 2, frame(100.0, 35.0), "Medium"),
            main_on(medium),
            Operation::SetVariantSet {
                id: k.set,
                set: Some(VariantSet {
                    props: vec![VariantProp {
                        name: "Size".into(),
                        values: vec!["Large".into(), "Small".into(), "Medium".into()],
                    }],
                }),
            },
            Operation::SetVariant {
                id: medium,
                values: vec!["Medium".into()],
            },
        ]))
        .expect("a third variant");
    let p5 = kit::freeze(&k.doc).unwrap();
    let root = d.root();
    assert_eq!(
        kit::insert(&d, &mut ids, L_ID, 5, &p5, medium, root).unwrap_err(),
        kit::KitError::NeedsUpdate(medium)
    );
}

/// A duplicate of a kit instance is an instance of the same vendored main, and
/// carries no origin — a copy never does (`remap_subtree`).
///
/// Flip, run: `remap_subtree` carrying `origin` fails long before this test's
/// own assertions — `kit::insert` builds its instance through the same copy, so
/// every insert in this file is refused with `OutsideVault`, which is
/// `kit::check` answering for the copy that forgot.
#[test]
fn a_copy_of_a_kit_instance_is_an_instance_without_an_origin() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let root = d.root();
    let dup = instance(&mut d, &mut ids, inst, root);
    assert_eq!(d.get(dup).unwrap().link(), d.get(inst).unwrap().link());
    assert!(d.get(dup).unwrap().origin().is_none());
}

/// **The change count is what consumers would receive** (§5.3e, 2A): nothing
/// right after a publish; nothing for moving a main across the kit's canvas,
/// which no consumer's copy can see; a main whose layer changed is *changed*, a
/// new one *added*, a deleted one *removed*. Publishing bumps the version, stamps
/// the date and note, and leaves nothing to publish.
///
/// Flip, run: `reach` comparing the top's own transform fails *"a move is not a
/// change"*, the moved Icon counted as changed.
#[test]
fn the_change_count_is_what_consumers_would_receive() {
    let mut k = kit();
    assert_eq!(kit::changes(&k.doc).count(), 0, "just published");

    k.doc
        .apply(&Transaction(vec![Operation::SetTransform {
            id: k.icon,
            transform: Affine::translate((500.0, 40.0)),
        }]))
        .unwrap();
    assert_eq!(kit::changes(&k.doc).count(), 0, "a move is not a change");

    k.doc
        .apply(&Transaction(vec![Operation::SetOpacity {
            id: k.icon_dot,
            opacity: 0.5,
        }]))
        .unwrap();
    let mut ids = IdSource::new(0xAD);
    let fresh = ids.mint();
    let root = k.doc.root();
    k.doc
        .apply(&Transaction(vec![
            create(fresh, root, 0, frame(30.0, 30.0), "Chip"),
            main_on(fresh),
        ]))
        .unwrap();
    let c = kit::changes(&k.doc);
    assert_eq!(c.added, vec![fresh]);
    assert_eq!(
        c.changed,
        vec![k.icon, k.card],
        "the Icon, and the Card that nests it"
    );
    assert!(c.removed.is_empty());

    // Card is detached from Icon's change by deleting it: removed, not changed.
    k.doc
        .apply(&Transaction(vec![Operation::DeleteNode { id: k.card }]))
        .unwrap();
    let c = kit::changes(&k.doc);
    assert_eq!(c.removed, vec![k.card]);
    assert_eq!(c.count(), 3);

    let v = kit::publish(&mut k.doc, Some("New chip".into()), 1_780_000_000);
    assert_eq!(v, Some(5));
    let mark = k.doc.meta().kit.clone().unwrap();
    assert_eq!(
        (mark.version, mark.date, mark.note.as_deref()),
        (5, Some(1_780_000_000), Some("New chip"))
    );
    assert_eq!(kit::changes(&k.doc).count(), 0, "nothing left to publish");
    assert!(k.doc.published().unwrap().tops().contains(&fresh));
}

/// **The count leaves out a main's whole place, and counts its order** (the
/// v0.5.0 release review's `[X2-L1-03]`, `[X2-L1-04]`). Hiding *Icon* on the
/// kit's page, pinning the *Button* set inside its frame and then moving it,
/// and giving it a layout item each changed something only the main's own
/// place holds — an instance root keeps its own (`propagate::is_placement`) —
/// and each was counted, an update for nothing. Putting *Icon* after *Card*
/// changes no node and does change the order a consumer's picker lists them
/// in, and was not counted, so it could not be published.
///
/// Flip, run: `reach` blanking the transform alone fails *"hiding a main is
/// not a change"*; `reordered` left false fails *"the order is"*.
#[test]
fn a_mains_place_is_not_a_change_and_its_order_is() {
    use ondin_core::container::{Insets, LayoutItem, LengthPct};
    let mut k = kit();
    k.doc
        .apply(&Transaction(vec![Operation::SetVisible {
            id: k.icon,
            visible: false,
        }]))
        .unwrap();
    assert_eq!(
        kit::changes(&k.doc).count(),
        0,
        "hiding a main is not a change"
    );
    let pinned = Insets {
        right: Some(LengthPct::Px(12.0)),
        ..Default::default()
    };
    k.doc
        .apply(&Transaction(vec![
            Operation::SetInsets {
                id: k.set,
                insets: pinned,
            },
            Operation::SetLayoutItem {
                id: k.set,
                item: LayoutItem {
                    grow: 1.0,
                    ..Default::default()
                },
            },
        ]))
        .unwrap();
    assert_eq!(
        kit::changes(&k.doc).count(),
        0,
        "nor pinning one, nor its item"
    );
    let c = kit::changes(&k.doc);
    assert!(!c.any(), "nothing to publish");

    let root = k.doc.root();
    let last = k.doc.get(root).unwrap().children().len() - 1;
    k.doc
        .apply(&Transaction(vec![Operation::Reorder {
            id: k.icon,
            index: last,
        }]))
        .unwrap();
    let c = kit::changes(&k.doc);
    assert_eq!(c.count(), 0, "no component changed");
    assert!(c.reordered && c.any(), "the order is");
}

/// **A third kit's version alone is not a change** (the v0.5.0 release
/// review's `[X2-L1-03]`): L's *Chip* nests K's *Badge*; K publishes a change
/// to another main, and L takes K's update — which moves *Badge*'s recorded
/// version and nothing else. L counted *Chip* changed, and every consumer of L
/// got an update that drew nothing new.
///
/// Flip, run: `reach` comparing the origin's version fails *"nothing L draws
/// changed"*.
#[test]
fn a_third_kits_version_alone_is_not_a_change() {
    let mut kids = IdSource::new(0xAB);
    let kroot = kids.mint();
    let mut kdoc = Document::new(kroot);
    let [badge, star] = [(); 2].map(|_| kids.mint());
    kdoc.apply(&Transaction(vec![
        create(badge, kroot, 0, frame(16.0, 16.0), "Badge"),
        main_on(badge),
        create(star, kroot, 1, frame(16.0, 16.0), "Star"),
        main_on(star),
    ]))
    .unwrap();
    let k2 = kit::freeze(&kdoc).unwrap();

    let mut lids = IdSource::new(0xAC);
    let lroot = lids.mint();
    let mut ldoc = Document::new(lroot);
    let (tx, placed) = kit::insert(&ldoc, &mut lids, K_ID, 2, &k2, badge, lroot).unwrap();
    ldoc.apply(&tx).unwrap();
    let chip = lids.mint();
    ldoc.apply(&Transaction(vec![
        create(chip, lroot, 0, frame(60.0, 20.0), "Chip"),
        Operation::Reparent {
            id: placed,
            new_parent: chip,
            index: 0,
        },
        main_on(chip),
    ]))
    .unwrap();
    ldoc.set_published(kit::freeze(&ldoc));
    assert_eq!(kit::changes(&ldoc).count(), 0, "fixture: just published");

    kdoc.apply(&Transaction(vec![Operation::SetOpacity {
        id: star,
        opacity: 0.5,
    }]))
    .unwrap();
    let k3 = kit::freeze(&kdoc).unwrap();
    let u = kit::update(&ldoc, &mut lids, K_ID, 3, &k3).unwrap();
    let mut tx = u.tx;
    ondin_core::propagate::owed(&ldoc, &mut tx, &mut lids);
    ldoc.apply(&tx).unwrap();
    assert_eq!(kit::changes(&ldoc).count(), 0, "nothing L draws changed");
}

/// **A regroup and a rename are worth a publish** (§15 D1015's fix): moving
/// the Button set out of its *Buttons* frame changes no node a consumer copies,
/// but it changes the picker's grouping downstream, so the set counts as
/// changed; renaming the kit changes no component and is still something to
/// publish — what a consumer calls the kit once it is lost.
///
/// Flip, run: `changes` without the group comparison fails *"the regrouped
/// set"*, the count 0.
#[test]
fn a_regroup_and_a_rename_are_worth_a_publish() {
    let mut k = kit();
    let root = k.doc.root();
    k.doc
        .apply(&Transaction(vec![Operation::Reparent {
            id: k.set,
            new_parent: root,
            index: 0,
        }]))
        .unwrap();
    let c = kit::changes(&k.doc);
    assert_eq!(c.changed, vec![k.set], "the regrouped set");
    assert!(!c.renamed);

    kit::publish(&mut k.doc, None, 1).unwrap();
    let mut meta = k.doc.meta().clone();
    meta.name = Some("Atlas 2".into());
    k.doc.set_meta(meta);
    let c = kit::changes(&k.doc);
    assert_eq!(c.count(), 0, "no component changed");
    assert!(c.renamed && c.any(), "but the name did");
}

/// Accept `k`'s current publish into `d`, through the commit's own passes
/// (`propagate::owed`), as the app's commit does. Answers the update and the
/// inverse that undoes it.
fn accept(d: &mut Document, ids: &mut IdSource, k: &Kit) -> (kit::Update, Transaction) {
    let version = k.doc.meta().kit.as_ref().unwrap().version;
    let u = kit::update(d, ids, L_ID, version, k.doc.published().unwrap()).unwrap();
    let mut tx = u.tx.clone();
    ondin_core::propagate::owed(d, &mut tx, ids);
    let undo = d.apply(&tx).expect("an accepted update commits").inverse;
    (u, undo)
}

/// **An update is a diff plus an ordinary commit** (§5.3e, 5B; §15 D1016): the
/// kit changes Icon's dot and gives Icon a second layer; accepting the new
/// publish rewrites the vendored Icon in place, and the main-edit passes carry
/// it to the instances — the one that matched the main follows, **the one with
/// an override keeps it** — and the new layer reaches both. Every origin moves to
/// the new version. One undo takes it all back.
///
/// Flip, run: `update` leaving out `state_ops` fails *"the vendored main took the
/// new value"*, the dot still at full opacity.
#[test]
fn an_update_reaches_the_instances_and_keeps_their_overrides() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let a = insert(&mut d, &mut ids, &k, k.icon);
    let b = insert(&mut d, &mut ids, &k, k.icon);
    let b_dot = d.get(b).unwrap().children()[0];
    d.apply(&Transaction(vec![Operation::SetOpacity {
        id: b_dot,
        opacity: 0.3,
    }]))
    .unwrap();
    let before = d.clone();

    let mut lids = IdSource::new(0xAD);
    let ring = lids.mint();
    k.doc
        .apply(&Transaction(vec![
            Operation::SetOpacity {
                id: k.icon_dot,
                opacity: 0.5,
            },
            create(ring, k.icon, 1, rect(), "Ring"),
        ]))
        .unwrap();
    assert_eq!(kit::publish(&mut k.doc, None, 1), Some(5));

    let (u, undo) = accept(&mut d, &mut ids, &k);
    let main = d.get(a).unwrap().link().unwrap();
    assert_eq!(u.updated, vec![main]);
    let dot = d.get(main).unwrap().children()[0];
    assert_eq!(
        d.get(dot).unwrap().opacity(),
        0.5,
        "the vendored main took the new value"
    );
    let a_dot = d.get(a).unwrap().children()[0];
    assert_eq!(
        d.get(a_dot).unwrap().opacity(),
        0.5,
        "the plain instance follows"
    );
    assert_eq!(d.get(b_dot).unwrap().opacity(), 0.3, "the override stays");
    for i in [a, b] {
        assert_eq!(
            d.get(i).unwrap().children().len(),
            2,
            "the new layer reached {i:?}"
        );
    }
    let versions: Vec<u32> = d
        .capture_subtree(main)
        .unwrap()
        .iter()
        .map(|n| n.origin().unwrap().version)
        .collect();
    assert_eq!(versions, vec![5, 5, 5]);

    d.apply(&undo).unwrap();
    assert_eq!(d, before, "one undo");
}

/// **A component the kit removed is made local, and a layer it dropped goes**
/// (D1011 (3)): the kit deletes Card and Icon's dot. Card's vendored copy leaves
/// the vault as an ordinary main of the document — no origin anywhere in it,
/// its instance still linked to it — and Icon, which Card nests, stays vendored
/// with its dot gone.
#[test]
fn a_removed_component_is_made_local_and_a_dropped_layer_goes() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.card);
    let card = d.get(inst).unwrap().link().unwrap();
    k.doc
        .apply(&Transaction(vec![
            Operation::DeleteNode { id: k.card },
            Operation::DeleteNode { id: k.icon_dot },
        ]))
        .unwrap();
    kit::publish(&mut k.doc, None, 1).unwrap();

    let (u, _) = accept(&mut d, &mut ids, &k);
    assert_eq!(u.made_local, vec![card]);
    let n = d.get(card).unwrap();
    assert_eq!(n.parent(), Some(d.root()), "out of the vault");
    assert!(n.component(), "an ordinary main");
    assert!(
        d.capture_subtree(card)
            .unwrap()
            .iter()
            .all(|n| n.origin().is_none()),
        "no origin left in it"
    );
    assert_eq!(
        d.get(inst).unwrap().link(),
        Some(card),
        "its instance follows it"
    );
    let icon = vault_children(&d)
        .into_iter()
        .find(|v| d.get(*v).unwrap().origin().unwrap().node == k.icon)
        .expect("Icon stays vendored");
    assert!(
        d.get(icon).unwrap().children().is_empty(),
        "the dot is gone"
    );
}

/// **A main the new publish reaches and this document does not hold is vendored
/// first**: the kit gives Icon a nested instance of a new main, Pip; accepting
/// brings Pip into the vault and Icon's nested copy links to it.
#[test]
fn an_update_vendors_a_main_it_newly_reaches() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    insert(&mut d, &mut ids, &k, k.icon);
    let mut lids = IdSource::new(0xAD);
    let pip = lids.mint();
    let root = k.doc.root();
    k.doc
        .apply(&Transaction(vec![
            create(pip, root, 0, frame(6.0, 6.0), "Pip"),
            main_on(pip),
        ]))
        .unwrap();
    instance(&mut k.doc, &mut lids, pip, k.icon);
    kit::publish(&mut k.doc, None, 1).unwrap();

    accept(&mut d, &mut ids, &k);
    let held: Vec<NodeId> = vault_children(&d)
        .iter()
        .map(|v| d.get(*v).unwrap().origin().unwrap().node)
        .collect();
    assert!(held.contains(&pip), "{held:?}");
    let icon = vault_children(&d)
        .into_iter()
        .find(|v| d.get(*v).unwrap().origin().unwrap().node == k.icon)
        .unwrap();
    let nested = *d.get(icon).unwrap().children().last().unwrap();
    let pip_here = vault_children(&d)
        .into_iter()
        .find(|v| d.get(*v).unwrap().origin().unwrap().node == pip)
        .unwrap();
    assert_eq!(d.get(nested).unwrap().link(), Some(pip_here));
}

/// ***Make local* reveals the copy and promotes no instance** (D1011 (3)): the
/// vendored Card leaves the vault as an ordinary main with no origin left in it;
/// both instances still link to it and the one with an override keeps it, by
/// value; Icon, which Card nests, stays vendored. One undo puts it back.
///
/// Flip, run: `make_local` without the `SetOrigin`s is refused at the commit —
/// `kit::check`'s `OutsideVault` — so the transaction never lands.
#[test]
fn make_local_reveals_the_copy_and_promotes_no_instance() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let a = insert(&mut d, &mut ids, &k, k.card);
    let b = insert(&mut d, &mut ids, &k, k.card);
    let card = d.get(a).unwrap().link().unwrap();
    d.apply(&Transaction(vec![Operation::SetOpacity {
        id: b,
        opacity: 0.4,
    }]))
    .unwrap();
    let before = d.clone();

    let tx = kit::make_local(&d, &mut ids, &[card]);
    let undo = d.apply(&tx).expect("make local commits").inverse;
    assert_eq!(d.get(card).unwrap().parent(), Some(d.root()));
    assert!(d.get(card).unwrap().component());
    assert!(
        d.capture_subtree(card)
            .unwrap()
            .iter()
            .all(|n| n.origin().is_none())
    );
    assert_eq!(d.get(a).unwrap().link(), Some(card));
    assert_eq!(d.get(b).unwrap().link(), Some(card));
    assert_eq!(d.get(b).unwrap().opacity(), 0.4, "the override stays");
    assert_eq!(vault_children(&d).len(), 1, "Icon stays vendored");

    d.apply(&undo).unwrap();
    assert_eq!(d, before, "one undo");
}

/// 🚨 ***Make local* on a main the kit's own components nest leaves them the
/// kit's** (§15 D1036 (1), the v0.5.0 release review's `[R1-L2-02]`, with
/// `[R3-L5-02]`'s probe). D holds *Card*, so the vault holds *Card* and the
/// *Icon* it nests; D makes *Icon* local. Before, the vault's *Card* went on
/// linking to the revealed *Icon*: an edit of the local *Icon* reached the
/// kit's *Card* and its instances, and the next update — even one changing
/// only *Card*'s name — vendored a fresh *Icon* and put them back, while the
/// toast said overrides were kept. Now the same transaction copies *Icon* back
/// into the vault for *Card*, so the local *Icon* is the user's own fork.
///
/// Flip, run — **two defences**: `make_local` leaving the vault's references as
/// they were is refused at the commit by `kit::check`'s `LinksOut` (*"make local
/// commits"*); with that rule off too it fails *"Card's Icon is the kit's"*.
#[test]
fn make_local_on_a_nested_kit_main_leaves_the_kits_components_the_kits() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let card_inst = insert(&mut d, &mut ids, &k, k.card);
    let card = d.get(card_inst).unwrap().link().unwrap();
    let nested = d.get(card).unwrap().children()[0];
    let icon = d.get(nested).unwrap().link().unwrap();
    let before = d.clone();

    let mut tx = kit::make_local(&d, &mut ids, &[icon]);
    ondin_core::propagate::owed(&d, &mut tx, &mut ids);
    d.apply(&tx).expect("make local commits");
    assert_eq!(
        d.get(icon).unwrap().parent(),
        Some(d.root()),
        "Icon revealed"
    );
    let kits_icon = d.get(nested).unwrap().link().unwrap();
    assert!(
        kits_icon != icon && d.in_vault(kits_icon),
        "Card's Icon is the kit's"
    );
    assert_eq!(
        d.get(kits_icon).unwrap().origin().map(|o| o.node),
        Some(k.icon),
        "a copy of the kit's Icon, origin and all"
    );

    // An edit of the local Icon is the user's alone.
    let local_dot = d.get(icon).unwrap().children()[0];
    let mut tx = Transaction(vec![Operation::SetOpacity {
        id: local_dot,
        opacity: 0.25,
    }]);
    ondin_core::propagate::owed(&d, &mut tx, &mut ids);
    d.apply(&tx).unwrap();
    let page_dot = d
        .get(d.get(card_inst).unwrap().children()[0])
        .unwrap()
        .children()[0];
    assert_eq!(
        d.get(page_dot).unwrap().opacity(),
        1.0,
        "Card's instances keep the kit's"
    );

    // And an update of Card changes nothing of that.
    let card_id = k.card;
    edit_and_publish(
        &mut k,
        vec![Operation::SetName {
            id: card_id,
            name: "Card 2".into(),
        }],
    );
    accept(&mut d, &mut ids, &k);
    assert_eq!(
        d.get(nested).unwrap().link(),
        Some(kits_icon),
        "nothing re-vendored"
    );
    assert_eq!(d.get(page_dot).unwrap().opacity(), 1.0);
    assert_eq!(
        d.get(local_dot).unwrap().opacity(),
        0.25,
        "the fork is kept"
    );

    // One undo of the make-local is the state before it.
    let mut d2 = before.clone();
    let mut tx = kit::make_local(&d2, &mut ids, &[icon]);
    ondin_core::propagate::owed(&d2, &mut tx, &mut ids);
    let undo2 = d2.apply(&tx).unwrap().inverse;
    d2.apply(&undo2).unwrap();
    assert_eq!(d2, before, "one undo");
}

/// **An update says which mains it changed, and which it removed** (§15 D1036
/// (4), (5); the v0.5.0 release review's `[X3.2-L1-02]` and `[R3-L5-03]`). D
/// holds *Icon* and the *Button* set; the kit fades *Icon*'s dot and deletes
/// the *Small* variant D placed. Every synced main moves to the new version,
/// so `updated` holds both — the toast counted that, and named a version-only
/// publish as N components changed. `changed` holds *Icon* alone; `removed`
/// holds *Small*, whose instance the commit detaches.
///
/// Flip, run: `changed` taking every synced top fails *"only Icon changed"*.
#[test]
fn an_update_says_what_it_changed_and_what_it_removed() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let icon_inst = insert(&mut d, &mut ids, &k, k.icon);
    let small_inst = insert(&mut d, &mut ids, &k, k.small);
    let icon = d.get(icon_inst).unwrap().link().unwrap();
    let small = d.get(small_inst).unwrap().link().unwrap();
    let set = d.get(small).unwrap().parent().unwrap();
    let (dot, gone) = (k.icon_dot, k.small);
    edit_and_publish(
        &mut k,
        vec![
            Operation::SetOpacity {
                id: dot,
                opacity: 0.5,
            },
            Operation::DeleteNode { id: gone },
        ],
    );
    let (u, _) = accept(&mut d, &mut ids, &k);
    assert!(u.updated.contains(&set), "fixture: the set was synced");
    assert_eq!(u.changed, vec![icon], "only Icon changed");
    assert_eq!(u.removed, vec![small], "Small was removed");
    assert_eq!(
        d.get(small_inst).unwrap().link(),
        None,
        "and its instance cut loose"
    );

    // A main whose only change is a layer deleted — dropped after every sync,
    // outside each top's count — is changed too (the record's reading of
    // `3b9cfc3`). Flip, run: the drop not counted fails *"a deleted layer is
    // a change"*.
    edit_and_publish(&mut k, vec![Operation::DeleteNode { id: dot }]);
    let (u, _) = accept(&mut d, &mut ids, &k);
    assert!(
        d.get(icon).unwrap().children().is_empty(),
        "fixture: the dot went"
    );
    assert_eq!(u.changed, vec![icon], "a deleted layer is a change");
}

/// A picture, and a fill wearing it.
fn picture(
    key: &str,
) -> (
    ondin_core::ImageId,
    ondin_core::ImageEntry,
    ondin_core::Fill,
) {
    use ondin_core::{Fill, ImageEntry, ImageFormat, ImageId, ImageSource, image_brush};
    let id = ImageId(key.into());
    let entry = ImageEntry {
        source: ImageSource::Embedded(vec![1, 2, 3, 4].into()),
        format: ImageFormat::Png,
        width: 20,
        height: 10,
    };
    let fill = Fill {
        brush: image_brush(id.clone()).into(),
        visible: true,
    };
    (id, entry, fill)
}

/// **An update brings the pictures it introduces** (the v0.5.0 release review's
/// `[R3-L5-01]`): the kit puts a picture on Icon's dot and adds a layer wearing
/// a second one. Accepting it adds both to the consumer's image table, in the
/// update's own transaction — so they survive a save and a load, and one undo
/// takes them out with the layers. Before, only `vendor` added images: the
/// instance showed the missing-picture placeholder and the file kept a
/// reference with no bytes, for good.
///
/// Flip, run: `update_all` without its image ops fails *"the changed fill's
/// picture"*.
#[test]
fn an_update_brings_the_pictures_it_introduces() {
    use ondin_core::keyed_by_position;
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let before = d.clone();

    let (on_dot, dot_entry, dot_fill) = picture("hash-of-the-dot");
    let (on_ring, ring_entry, ring_fill) = picture("hash-of-the-ring");
    let ring = IdSource::new(0xAD).mint();
    k.doc
        .apply(&Transaction(vec![
            Operation::AddImage {
                id: on_dot.clone(),
                entry: dot_entry,
            },
            Operation::AddImage {
                id: on_ring.clone(),
                entry: ring_entry,
            },
            Operation::SetFills {
                id: k.icon_dot,
                fills: keyed_by_position([dot_fill]),
            },
            create(ring, k.icon, 1, rect(), "Ring"),
            Operation::SetFills {
                id: ring,
                fills: keyed_by_position([ring_fill]),
            },
        ]))
        .unwrap();
    assert_eq!(kit::publish(&mut k.doc, None, 1), Some(5));

    let (_, undo) = accept(&mut d, &mut ids, &k);
    assert!(d.has_image(&on_dot), "the changed fill's picture");
    assert!(d.has_image(&on_ring), "the new layer's picture");
    assert_eq!(
        d.get(inst).unwrap().children().len(),
        2,
        "fixture: the ring arrived"
    );

    let loaded = io::load(&io::save(&d).unwrap()).unwrap();
    assert!(
        loaded.has_image(&on_dot) && loaded.has_image(&on_ring),
        "through a file"
    );

    d.apply(&undo).unwrap();
    assert_eq!(d, before, "one undo, the pictures with it");
}

/// **A kit's rename arrives with its update** (§15 D1016's fix (b)): the kit is
/// renamed and publishes nothing else; accepting moves the vendored top's
/// recorded name to the new one — what the picker calls the kit once it is
/// lost — and leaves the nodes below it without one.
///
/// Flip, run: `sync` keeping each copy's first name fails *"the new name"*,
/// still *Atlas*.
#[test]
fn a_kit_rename_arrives_with_its_update() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    insert(&mut d, &mut ids, &k, k.icon);
    let mut meta = k.doc.meta().clone();
    meta.name = Some("Atlas 2".into());
    k.doc.set_meta(meta);
    assert!(kit::changes(&k.doc).renamed);
    kit::publish(&mut k.doc, None, 1).unwrap();

    accept(&mut d, &mut ids, &k);
    let top = vault_children(&d)[0];
    assert_eq!(
        d.get(top).unwrap().origin().unwrap().name.as_deref(),
        Some("Atlas 2"),
        "the new name"
    );
    let dot = d.get(top).unwrap().children()[0];
    assert_eq!(d.get(dot).unwrap().origin().unwrap().name, None, "top only");
}

/// **A swap to a kit main the document has never used** (§15 D1019): D's own
/// *Row* nests a *Glyph*; in an instance of *Row*, that nested copy is offered
/// every frame main the kit published — none held yet — and picking *Icon*
/// vendors it and swaps to it in one transaction, through the commit's own
/// passes, the copy then showing the vendored *Icon* with its dot. Afterwards
/// *Icon* is no longer offered past the vault, being held; one undo takes the
/// vault away with the swap.
///
/// Flip, run: `swap_offers` without its `find` filter fails *"a held main is
/// the vault's to list"*, *Icon* still offered.
#[test]
fn a_swap_to_an_unused_kit_main_vendors_it_at_the_swap() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let root = d.root();
    let [glyph, glyph_dot, row] = [(); 3].map(|_| ids.mint());
    d.apply(&Transaction(vec![
        create(glyph, root, 0, frame(24.0, 24.0), "Glyph"),
        create(glyph_dot, glyph, 0, rect(), "Square"),
        main_on(glyph),
        create(row, root, 1, frame(200.0, 40.0), "Row"),
    ]))
    .unwrap();
    instance(&mut d, &mut ids, glyph, row);
    d.apply(&Transaction(vec![main_on(row)])).unwrap();
    let row_copy = instance(&mut d, &mut ids, row, root);
    let nested = d.get(row_copy).unwrap().children()[0];
    assert!(ondin_core::swap::can_swap(&d, nested), "the fixture's slot");

    let offers = kit::swap_offers(&d, L_ID, published(&k), nested, "", "");
    assert_eq!(
        offers,
        vec![k.card, k.icon, k.large, k.small],
        "every frame main, by name"
    );
    assert_eq!(
        kit::swap_offers(&d, L_ID, published(&k), nested, "ic", ""),
        vec![k.icon],
        "the property's prefix filter"
    );

    let (mut ops, scratch, shown) =
        kit::vendored(&d, &mut ids, L_ID, 4, published(&k), k.icon).unwrap();
    let swap = ondin_core::variant::swap_carrying(&scratch, nested, shown).expect("a swap");
    ops.extend(swap.0);
    let mut tx = Transaction(ops);
    ondin_core::propagate::owed(&d, &mut tx, &mut ids);
    let undo = d.apply(&tx).expect("vendor and swap commit as one").inverse;

    assert!(d.in_vault(shown), "vendored");
    assert_eq!(
        ondin_core::component::main_of(&d, nested),
        Some(shown),
        "the copy shows the kit's Icon"
    );
    let kids = d.get(nested).unwrap().children();
    assert_eq!(kids.len(), 1, "Icon's one layer");
    assert_eq!(d.get(kids[0]).unwrap().name(), "Dot", "rewritten to Icon's");
    assert!(
        !kit::swap_offers(&d, L_ID, published(&k), nested, "", "").contains(&k.icon),
        "a held main is the vault's to list"
    );

    d.apply(&undo).unwrap();
    assert!(d.vault().is_none(), "one undo takes the vault away");
    assert_eq!(ondin_core::component::main_of(&d, nested), Some(glyph));
}

/// **An update does not depend on the vault's order** (§15 D1022, the kits QA
/// review's finding). A picker insert vendors dependencies first — *Icon*, then
/// *Card* — but a paste lands them the other way round, and so does an update
/// that newly reaches a main. The kit then gives *Icon* a ring, which reaches
/// *Card*'s nested *Icon* too. **Two defects, one per order**:
///
/// - Out of order, syncing *Card* first met a new layer linked to a ring the
///   scratch did not hold yet, and the update failed to build, `Incomplete`, on
///   every accept, for good. The link is now pointed once both are synced.
/// - **In order too** — which the review did not reach, its probe stopping at
///   the build — the commit was refused: the update writes *Card*'s nested ring
///   itself, as published, and the structural pass, seeing *Icon* gain a layer,
///   copied it into that same nested copy again, with no origin, so the vault
///   refused it. The pass now skips a copy the edit already supplied.
///
/// Each instance ends with one ring.
///
/// Flips, run: the structural pass without its `supplied` skip fails at the
/// in-order commit, `accept`'s *"an accepted update commits"*,
/// `Vault(.., NoOrigin)`; `point_or_defer` written to record nothing — the
/// cleared link left empty — fails *"the update commits"* out of order, also
/// `Vault(.., NoOrigin)`: the ring's copy with no link reads to the structural
/// pass as no counterpart, and it copies one in.
#[test]
fn an_update_does_not_depend_on_the_vaults_order() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let card_copy = insert(&mut d, &mut ids, &k, k.card);
    let (mut e, mut eids) = (
        Document::new(IdSource::new(0xEA).mint()),
        IdSource::new(0xEB),
    );
    let pasted = paste_across(&d, card_copy, &mut e, &mut eids);
    let names = |doc: &Document| -> Vec<String> {
        vault_children(doc)
            .iter()
            .map(|c| doc.get(*c).unwrap().name().to_string())
            .collect()
    };
    assert_eq!(
        names(&d),
        ["Icon", "Card"],
        "the insert: dependencies first"
    );
    assert_eq!(
        names(&e),
        ["Card", "Icon"],
        "the paste: the other way round"
    );

    let mut kids = IdSource::new(0xAD);
    let ring = kids.mint();
    let mut tx = Transaction(vec![create(ring, k.icon, 1, rect(), "Ring")]);
    ondin_core::propagate::owed(&k.doc, &mut tx, &mut kids);
    k.doc.apply(&tx).unwrap();
    kit::publish(&mut k.doc, None, 2).unwrap();

    let layers = |doc: &Document, card: NodeId| -> Vec<String> {
        let nested = doc.get(card).unwrap().children()[0];
        doc.get(nested)
            .unwrap()
            .children()
            .iter()
            .map(|c| doc.get(*c).unwrap().name().to_string())
            .collect()
    };
    // In order: `accept` asserts the commit.
    accept(&mut d, &mut ids, &k);
    assert_eq!(layers(&d, card_copy), ["Dot", "Ring"], "in order: one ring");

    let u =
        kit::update(&e, &mut eids, L_ID, 5, k.doc.published().unwrap()).expect("the update builds");
    let mut tx = u.tx;
    ondin_core::propagate::owed(&e, &mut tx, &mut eids);
    e.apply(&tx).expect("the update commits");
    assert_eq!(
        layers(&e, pasted),
        ["Dot", "Ring"],
        "out of order: one ring"
    );
}

/// Commit `ops` in the kit through the commit's passes, and publish.
fn edit_and_publish(k: &mut Kit, ops: Vec<Operation>) {
    let mut kids = IdSource::new(0xAE);
    let mut tx = Transaction(ops);
    ondin_core::propagate::owed(&k.doc, &mut tx, &mut kids);
    k.doc.apply(&tx).expect("the kit's edit");
    kit::publish(&mut k.doc, None, 2).unwrap();
}

/// **A variant the kit takes out of its set stays the main its instances
/// follow** (§15 D1022, the kits QA review's finding). D placed *Small*; the kit
/// drags *Small* out of *Button* to the top level and publishes. The update
/// lifts D's copy out of its set copy to the vault's top level — the same node,
/// so the instance is still its instance — and nothing is made local.
///
/// Flip, run: without the lift, the set's sync drops *Small* and nothing moves
/// it, so it is deleted — fails *"the same main, still followed"*, the instance
/// cut loose.
#[test]
fn a_variant_taken_out_of_its_set_keeps_its_instances() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.small);
    let main = d.get(inst).unwrap().link().unwrap();
    let (root, small) = (k.doc.root(), k.small);
    edit_and_publish(
        &mut k,
        vec![Operation::Reparent {
            id: small,
            new_parent: root,
            index: 1,
        }],
    );
    let (u, _) = accept(&mut d, &mut ids, &k);
    assert!(u.made_local.is_empty(), "nothing made local");
    assert_eq!(
        d.get(inst).unwrap().link(),
        Some(main),
        "the same main, still followed"
    );
    assert_eq!(d.get(main).unwrap().parent(), d.vault(), "a vault top now");
}

/// **A lone main the kit combines into a set stays the main its instances
/// follow** (§15 D1022). D placed *Icon*; the kit moves *Icon* into *Button* as
/// a variant and publishes. D held no copy of *Button*, so the update begins one
/// as an empty shell and its sync moves D's *Icon* in beside fresh copies of
/// *Large* and *Small* — the instance still on the same node, no second *Icon*,
/// nothing made local.
///
/// Flip, run: `update_all` making such a top local, as it did — no `moving`
/// set, the old `still` test alone — fails *"nothing made local"*.
#[test]
fn a_main_combined_into_a_set_keeps_its_instances() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let main = d.get(inst).unwrap().link().unwrap();
    let (icon, set) = (k.icon, k.set);
    edit_and_publish(
        &mut k,
        vec![Operation::Reparent {
            id: icon,
            new_parent: set,
            index: 2,
        }],
    );
    let (u, _) = accept(&mut d, &mut ids, &k);
    assert!(u.made_local.is_empty(), "nothing made local");
    assert_eq!(d.get(inst).unwrap().link(), Some(main), "the same main");
    let set = d.get(main).unwrap().parent().unwrap();
    assert_eq!(
        d.get(set).unwrap().parent(),
        d.vault(),
        "inside the set's copy"
    );
    let names: Vec<&str> = d
        .get(set)
        .unwrap()
        .children()
        .iter()
        .map(|c| d.get(*c).unwrap().name())
        .collect();
    assert_eq!(names.len(), 3, "Large, Small and the one Icon: {names:?}");
    assert_eq!(vault_children(&d), [set], "no second Icon beside the set");
}

/// **A main's instances are the document's, never the vault's** (§15 D1023):
/// D places *Card*, so the vault's *Card* holds a copy of *Icon* and D's own
/// *Card* another. *Icon*'s instances are the one in D's tree.
///
/// Flip, run: `instances_of` without its vault filter fails *"the vault's copy
/// is no one's"*, 2 against 1.
#[test]
fn a_mains_instances_leave_out_the_vault() {
    let k = kit();
    let (mut d, mut ids) = consumer();
    let card = insert(&mut d, &mut ids, &k, k.card);
    let icon = vault_children(&d)[0];
    let nested = d.get(card).unwrap().children()[0];
    assert_eq!(
        ondin_core::component::instances_of(&d, icon),
        [nested],
        "the vault's copy is no one's"
    );
}

/// **A kit never vendors its own main by paste** (§15 D1023, the kits QA
/// review's finding): the kit deletes *Icon*, then an *Icon* instance copied from
/// a consumer is pasted back in. No vault opens in the kit; the paste lands
/// detached, as any copy of a main that is gone does.
///
/// Flip, run: `adopt` without its own-kit skip fails *"no vault in the kit"*.
#[test]
fn a_kit_never_vendors_its_own_main_by_paste() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let mut kids = IdSource::new(0xAF);
    let mut tx = Transaction(vec![Operation::DeleteNode { id: k.icon }]);
    ondin_core::propagate::owed(&k.doc, &mut tx, &mut kids);
    k.doc.apply(&tx).unwrap();

    let pasted = paste_across(&d, inst, &mut k.doc, &mut kids);
    assert!(k.doc.vault().is_none(), "no vault in the kit");
    assert_eq!(k.doc.get(pasted).unwrap().link(), None, "detached");
}

/// The app's commit door, less its history: the two layout rewrites, the
/// component passes, and the apply.
fn door(d: &mut Document, ids: &mut IdSource, tx: Transaction) {
    let res = Resolved::rebuild(d);
    let tx = ondin_core::build::keep_insets(d, &res, tx);
    let mut tx = ondin_core::build::keep_flex_sizes(d, &res, tx);
    ondin_core::propagate::owed(d, &mut tx, ids);
    d.apply(&tx).expect("the door commits");
}

/// A kit of one main, *Card*, built by `ops` after its root and marked; and a
/// consumer that has placed it and overridden one layer's opacity — the layer
/// `overridden` picks from the instance.
struct Grouping {
    kit: Document,
    d: Document,
    ids: IdSource,
    inst: NodeId,
}

fn placed_card(
    ops: impl FnOnce(NodeId) -> (Vec<Operation>, NodeId),
    overridden: impl Fn(&Document, NodeId) -> NodeId,
) -> Grouping {
    let mut kids = IdSource::new(0xB0);
    let root = kids.mint();
    let mut kit_doc = Document::new(root);
    let (ops, card) = ops(root);
    kit_doc.apply(&Transaction(ops)).unwrap();
    kit_doc.set_meta(DocumentMeta {
        id: Some(L_ID.into()),
        name: Some("Atlas".into()),
        kit: Some(KitMark::default()),
        ..DocumentMeta::default()
    });
    kit::publish(&mut kit_doc, None, 1).unwrap();
    let (mut d, mut ids) = consumer();
    let droot = d.root();
    let (tx, inst) = kit::insert(
        &d,
        &mut ids,
        L_ID,
        1,
        kit_doc.published().unwrap(),
        card,
        droot,
    )
    .unwrap();
    door(&mut d, &mut ids, tx);
    let layer = overridden(&d, inst);
    door(
        &mut d,
        &mut ids,
        Transaction(vec![Operation::SetOpacity {
            id: layer,
            opacity: 0.3,
        }]),
    );
    Grouping {
        kit: kit_doc,
        d,
        ids,
        inst,
    }
}

/// Publish `g.kit` again after `ops`, and accept it in `g.d` through the door.
fn republish_and_accept(g: &mut Grouping, ops: Vec<Operation>) {
    g.kit.apply(&Transaction(ops)).unwrap();
    kit::publish(&mut g.kit, None, 2).unwrap();
    let u = kit::update(&g.d, &mut g.ids, L_ID, 2, g.kit.published().unwrap())
        .expect("the update builds");
    door(&mut g.d, &mut g.ids, u.tx);
}

/// Each child of `id`, by name and opacity.
fn shape(d: &Document, id: NodeId) -> Vec<(String, f32)> {
    d.get(id)
        .unwrap()
        .children()
        .iter()
        .map(|c| {
            let n = d.get(*c).unwrap();
            (n.name().to_string(), n.opacity())
        })
        .collect()
}

/// **An ungroup in the kit keeps the layers it lifts out** (§15 D1024, the kits
/// QA review's second pass, reproduced by its probe): *Card* > *G* > *A*, the
/// instance's *A* overridden; the kit ungroups *G*. The vault's *A* is the same
/// node after, and the instance holds the one *A*, its override kept. Deleting
/// *G* at once took *A* with it, and the instance kept the old *A* detached in
/// the old *G* beside a fresh one.
///
/// Flip, run: a dropped child deleted at once, as it was, fails *"A kept its
/// identity"*.
#[test]
fn an_ungroup_in_the_kit_keeps_the_layers_and_their_overrides() {
    let mut ids = IdSource::new(0xB1);
    let [card, g, a] = [(); 3].map(|_| ids.mint());
    let mut gr = placed_card(
        |root| {
            (
                vec![
                    create(card, root, 0, frame(200.0, 100.0), "Card"),
                    create(g, card, 0, NodeKind::Group, "G"),
                    create(a, g, 0, rect(), "A"),
                    main_on(card),
                ],
                card,
            )
        },
        |d, inst| {
            d.get(d.get(inst).unwrap().children()[0])
                .unwrap()
                .children()[0]
        },
    );
    let main = gr.d.get(gr.inst).unwrap().link().unwrap();
    let vault_a =
        gr.d.get(gr.d.get(main).unwrap().children()[0])
            .unwrap()
            .children()[0];
    republish_and_accept(
        &mut gr,
        vec![
            Operation::Reparent {
                id: a,
                new_parent: card,
                index: 0,
            },
            Operation::DeleteNode { id: g },
        ],
    );
    assert!(gr.d.contains(vault_a), "A kept its identity");
    assert_eq!(
        shape(&gr.d, gr.inst),
        [("A".to_string(), 0.3)],
        "the override on A stays"
    );
}

/// **A group or a frame the kit makes around a layer keeps it** (§15 D1024):
/// *Card* > *A*, the instance's *A* overridden; the kit groups *A* (*Group
/// selection*). The vault's *A* moves into the new group, and the instance's
/// *A* into the group's copy, override kept. A whole copy of the new group minted
/// a second *A* and dropped the first, the instance keeping it detached. This
/// also found **every sync of a group failing**: `state_ops` wrote a group's
/// `SetFills`, which `apply` refuses by kind.
///
/// Flips, run: `sync` never moving a held node in — so the group's shell
/// copies *A* fresh, which is what a whole copy did — fails *"A kept its
/// identity"* (and the ungroup test and the set test with it); the group's
/// fields written without the kind skip fails *"the update builds"*,
/// `Incomplete`.
#[test]
fn a_group_in_the_kit_keeps_the_layer_and_its_override() {
    let mut ids = IdSource::new(0xB2);
    let [card, a, g] = [(); 3].map(|_| ids.mint());
    let mut gr = placed_card(
        |root| {
            (
                vec![
                    create(card, root, 0, frame(200.0, 100.0), "Card"),
                    create(a, card, 0, rect(), "A"),
                    main_on(card),
                ],
                card,
            )
        },
        |d, inst| d.get(inst).unwrap().children()[0],
    );
    let main = gr.d.get(gr.inst).unwrap().link().unwrap();
    let vault_a = gr.d.get(main).unwrap().children()[0];
    republish_and_accept(
        &mut gr,
        vec![
            create(g, card, 0, NodeKind::Group, "G"),
            Operation::Reparent {
                id: a,
                new_parent: g,
                index: 0,
            },
        ],
    );
    assert!(gr.d.contains(vault_a), "A kept its identity");
    assert_eq!(shape(&gr.d, gr.inst).len(), 1, "one child, the group");
    let ig = gr.d.get(gr.inst).unwrap().children()[0];
    assert_eq!(
        shape(&gr.d, ig),
        [("A".to_string(), 0.3)],
        "the override on A stays"
    );
}

/// **An accept leaves a pinned layer where the kit draws it** (§15 D1024, the
/// kits QA review's second pass, reproduced): *Badge* pinned `right: 10` in a
/// *Card* widened since its transform was stored, so drawn at x 170 from a
/// stored 70. The kit changes *Badge*'s height and publishes; the commit door's
/// `keep_insets`, with no resolved place for a vault node, read the stored
/// transform as where it is drawn and re-pinned it `right: 110`, and the
/// instance's badge jumped to x 70.
///
/// Flip, run: `keep_insets` without its vault skip fails *"the kit's inset
/// arrived unchanged"*.
#[test]
fn an_accept_leaves_a_pinned_layer_where_the_kit_draws_it() {
    use ondin_core::{GeometryPatch, Insets, LengthPct};
    let mut ids = IdSource::new(0xB3);
    let [card, badge] = [(); 2].map(|_| ids.mint());
    let mut gr = placed_card(
        |root| {
            (
                vec![
                    create(card, root, 0, frame(200.0, 100.0), "Card"),
                    create(badge, card, 0, rect(), "Badge"),
                    Operation::SetTransform {
                        id: badge,
                        transform: Affine::translate((70.0, 10.0)),
                    },
                    Operation::SetInsets {
                        id: badge,
                        insets: Insets {
                            right: Some(LengthPct::Px(10.0)),
                            top: Some(LengthPct::Px(10.0)),
                            ..Insets::default()
                        },
                    },
                    main_on(card),
                ],
                card,
            )
        },
        |d, inst| d.get(inst).unwrap().children()[0],
    );
    let ib = gr.d.get(gr.inst).unwrap().children()[0];
    let before = Resolved::rebuild(&gr.d).world_bounds(ib).unwrap();
    republish_and_accept(
        &mut gr,
        vec![Operation::SetGeometry {
            id: badge,
            geometry: GeometryPatch::Size(Size::new(10.0, 14.0)),
        }],
    );
    let main = gr.d.get(gr.inst).unwrap().link().unwrap();
    let vb = gr.d.get(main).unwrap().children()[0];
    assert_eq!(
        gr.d.get(vb).unwrap().insets().right,
        Some(LengthPct::Px(10.0)),
        "the kit's inset arrived unchanged"
    );
    let after = Resolved::rebuild(&gr.d).world_bounds(ib).unwrap();
    assert_eq!(after.x0, before.x0, "the instance's badge did not move");
}

/// **A removed main is made local whole** (§15 D1024): the kit moves *Icon*'s
/// *Dot* into *Card* and deletes *Icon*. D's *Icon* is made local — kept as a
/// copy where used — with its *Dot* still in it; *Card* takes a fresh dot of its
/// own. A sync run before the classification moved D's *Dot* out of the copy
/// being made local, and its instances lost it.
///
/// Flip, run: the move-in without its `keep_out` guard fails *"the local Icon
/// keeps its Dot"*.
#[test]
fn a_removed_main_is_made_local_whole() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    insert(&mut d, &mut ids, &k, k.card);
    let icon_here = vault_children(&d)[0];
    let (card, dot, icon) = (k.card, k.icon_dot, k.icon);
    edit_and_publish(
        &mut k,
        vec![
            Operation::Reparent {
                id: dot,
                new_parent: card,
                index: 1,
            },
            Operation::DeleteNode { id: icon },
        ],
    );
    let (u, _) = accept(&mut d, &mut ids, &k);
    assert_eq!(u.made_local, [icon_here], "Icon made local");
    assert_eq!(
        shape(&d, icon_here).len(),
        1,
        "the local Icon keeps its Dot"
    );
}

/// **An update reaching a newer version of a third kit's main asks for that
/// kit's update, and takes it together with it** (§15 D1022's `resolve`, D1024):
/// L's *Chip* nests K's *Badge*; D placed *Chip* with *Badge* at K's v2. K gives
/// *Badge* a dot; L takes K's update and publishes again. Accepting L alone
/// cannot point *Chip*'s nested dot at a dot D's *Badge* lacks — `NeedsUpdate`,
/// which the app answers by accepting K's waiting update with it — and the two
/// together commit, in either order.
///
/// Flip, run: `resolve` answering `Incomplete` for a held top too fails
/// *"an update owed, named as one"*.
#[test]
fn a_third_kits_newer_main_asks_for_its_update() {
    // K: *Badge*, at v2.
    let mut kids = IdSource::new(0xAB);
    let kroot = kids.mint();
    let mut kdoc = Document::new(kroot);
    let badge = kids.mint();
    kdoc.apply(&Transaction(vec![
        create(badge, kroot, 0, frame(16.0, 16.0), "Badge"),
        main_on(badge),
    ]))
    .unwrap();
    kdoc.set_published(kit::freeze(&kdoc));
    let k2 = kdoc.published().unwrap().clone();

    // L: *Chip* nesting K's *Badge*, published as v7.
    let mut lids = IdSource::new(0xAC);
    let lroot = lids.mint();
    let mut ldoc = Document::new(lroot);
    let (tx, placed) = kit::insert(&ldoc, &mut lids, K_ID, 2, &k2, badge, lroot).unwrap();
    ldoc.apply(&tx).unwrap();
    let chip = lids.mint();
    ldoc.apply(&Transaction(vec![
        create(chip, lroot, 0, frame(60.0, 20.0), "Chip"),
        Operation::Reparent {
            id: placed,
            new_parent: chip,
            index: 0,
        },
        main_on(chip),
    ]))
    .unwrap();
    let l7 = kit::freeze(&ldoc).unwrap();

    let (mut d, mut ids) = consumer();
    let root = d.root();
    let (tx, _) = kit::insert(&d, &mut ids, L_ID, 7, &l7, chip, root).unwrap();
    d.apply(&tx).unwrap();

    // K v3: a dot in *Badge*. L takes it, and publishes v8.
    let dot = kids.mint();
    kdoc.apply(&Transaction(vec![create(dot, badge, 0, rect(), "Dot")]))
        .unwrap();
    kdoc.set_published(kit::freeze(&kdoc));
    let k3 = kdoc.published().unwrap().clone();
    let u = kit::update(&ldoc, &mut lids, K_ID, 3, &k3).unwrap();
    let mut tx = u.tx;
    ondin_core::propagate::owed(&ldoc, &mut tx, &mut lids);
    ldoc.apply(&tx).unwrap();
    let l8 = kit::freeze(&ldoc).unwrap();

    assert!(
        matches!(
            kit::update(&d, &mut ids, L_ID, 8, &l8).map(|_| ()),
            Err(kit::KitError::NeedsUpdate(_))
        ),
        "an update owed, named as one"
    );
    for kits in [
        [(K_ID, 3, &k3), (L_ID, 8, &l8)],
        [(L_ID, 8, &l8), (K_ID, 3, &k3)],
    ] {
        let mut d = d.clone();
        let u = kit::update_all(&d, &mut ids, &kits).expect("both together build");
        let mut tx = u.tx;
        ondin_core::propagate::owed(&d, &mut tx, &mut ids);
        d.apply(&tx).expect("and commit");
        // What it commits (`[X2-L6-01]`): one *Dot* in *Chip*'s *Badge*, the
        // copy of *Badge*'s own.
        let nested = d.get(held(&d, L_ID, chip)).unwrap().children()[0];
        let badge_dot = d.get(held(&d, K_ID, badge)).unwrap().children()[0];
        let dots = d.get(nested).unwrap().children();
        assert_eq!(dots.len(), 1, "one dot in Chip's Badge");
        assert_eq!(d.get(dots[0]).unwrap().link(), Some(badge_dot));
    }

    // 🚨 **K's update taken while L's publish is older** (the v0.5.0 release
    // review's `[X2-L1-01]`, route 2): D's *Badge* gains the dot, and *Chip*'s
    // nested *Badge* — L's v7, which never saw it — was given a copy of it with
    // no origin, and the vault refused the commit. K alone, and K with L's v7
    // again, *Accept all*'s case, both commit, *Chip* as L published it.
    for kits in [vec![(K_ID, 3, &k3)], vec![(K_ID, 3, &k3), (L_ID, 7, &l7)]] {
        let mut d = d.clone();
        let mut tx = kit::update_all(&d, &mut ids, &kits).unwrap().tx;
        ondin_core::propagate::owed(&d, &mut tx, &mut ids);
        d.apply(&tx)
            .expect("K's update commits before L has taken it");
        let nested = d.get(held(&d, L_ID, chip)).unwrap().children()[0];
        assert!(
            d.get(nested).unwrap().children().is_empty(),
            "Chip as L published it"
        );
        assert_eq!(d.get(held(&d, K_ID, badge)).unwrap().children().len(), 1);
    }
}

/// The vault node holding `kit`'s `node`.
fn held(d: &Document, kit: &str, node: NodeId) -> NodeId {
    let mut stack = vec![d.vault().expect("a vault")];
    while let Some(id) = stack.pop() {
        let n = d.get(id).unwrap();
        if n.origin().is_some_and(|o| o.kit == kit && o.node == node) {
            return id;
        }
        stack.extend(n.children());
    }
    panic!("held")
}

/// 🚨 **A layer the kit deletes from a nested copy stays deleted through the
/// update** (the v0.5.0 release review's `[X2-L1-01]`, route 1). L gives *Icon*
/// a *Ring*, then deletes *Ring*'s copy from *Card*'s nested *Icon* — instance
/// structure is free (§5.3d) — and publishes. D, holding *Card*, accepts: the
/// commit's structural pass gave the vault's nested copy the ring again, with
/// no origin, and the vault refused the update every time it was asked.
///
/// Flip, run: `follows_nothing` answering `false` fails *"an accepted update
/// commits"* with `Vault(.., NoOrigin)`.
#[test]
fn a_layer_the_kit_cut_from_a_nested_copy_stays_cut() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.card);
    let (ring, icon) = (IdSource::new(0xAF).mint(), k.icon);
    edit_and_publish(&mut k, vec![create(ring, icon, 1, rect(), "Ring")]);
    let nested = k.doc.get(k.card).unwrap().children()[0];
    let cut = *k
        .doc
        .get(nested)
        .unwrap()
        .children()
        .iter()
        .find(|c| k.doc.get(**c).unwrap().link() == Some(ring))
        .expect("fixture: the kit's door copied the ring into Card");
    edit_and_publish(&mut k, vec![Operation::DeleteNode { id: cut }]);

    accept(&mut d, &mut ids, &k);
    let vault_nested = d.get(held(&d, L_ID, k.card)).unwrap().children()[0];
    assert_eq!(
        shape(&d, vault_nested),
        shape(published(&k).document(), nested)
    );
    assert_eq!(d.get(held(&d, L_ID, k.icon)).unwrap().children().len(), 2);
    let page_nested = d.get(inst).unwrap().children()[0];
    assert_eq!(
        d.get(page_nested).unwrap().children().len(),
        1,
        "nor on the page"
    );
}

/// **A consumer's copy is the publish, value and order** (the v0.5.0 release
/// review's `[X2-L1-02]`). The kit changes *Icon*'s dot and holds *Card*'s
/// nested dot at the old opacity. Accepting wrote the publish into the vault —
/// no op for the nested dot, already 1 — and then the commit's field pass made
/// the copy follow its main again: 0.5 against the published 1, so D drew Card
/// unlike a consumer that inserted the same version fresh.
///
/// Flip, run: `follows_nothing` answering `false` fails *"the value is the
/// publish's"*, 0.5 against 1.
#[test]
fn an_accepted_copy_is_the_publish_value() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.card);
    let nested = k.doc.get(k.card).unwrap().children()[0];
    let nested_dot = k.doc.get(nested).unwrap().children()[0];
    let icon_dot = k.icon_dot;
    edit_and_publish(
        &mut k,
        vec![Operation::SetOpacity {
            id: icon_dot,
            opacity: 0.5,
        }],
    );
    edit_and_publish(
        &mut k,
        vec![Operation::SetOpacity {
            id: nested_dot,
            opacity: 1.0,
        }],
    );
    accept(&mut d, &mut ids, &k);
    let vault_nested = d.get(held(&d, L_ID, k.card)).unwrap().children()[0];
    let want = shape(published(&k).document(), nested);
    assert_eq!(
        want,
        vec![("Dot".to_string(), 1.0)],
        "fixture: the kit kept Card's dot"
    );
    assert_eq!(shape(&d, vault_nested), want, "the value is the publish's");
    let page_nested = d.get(inst).unwrap().children()[0];
    assert_eq!(shape(&d, page_nested), want, "and the page's");
}

/// **…and its order** (`[X2-L1-02]`'s second case): with D holding *Icon*
/// {Dot, Ring}, the kit puts a layer of *Card*'s own at the front of its nested
/// *Icon* and moves *Ring* before *Dot* in *Icon*. The sync put the nested copy
/// in the published order and the structural pass's reorder arm then applied
/// *Icon*'s reorder to it again.
///
/// Flip, run: `follows_nothing` answering `false` fails *"the order is the
/// publish's"* with `[Ring, Dot, Own]` against `[Own, Ring, Dot]`. ⚠️ **Its
/// first fixture took all three edits in one accept and passed under the
/// flip**: a consumer gaining *Ring* and the reorder together has no reorder of
/// its own to replay, so D accepts *Ring* first.
///
/// 🚨 **And with the vault fixed, the page was still wrong**: the structural
/// pass's reorder arm took only a parent whose children were the same set, and
/// the vault's nested copy had gained *Own* in the same transaction, so the
/// instance got *Own* and never the new order. Flip, run: the arm skipping a
/// parent that gained or lost a child again fails *"and the page's"* with
/// `[Dot, Own, Ring]`.
#[test]
fn an_accepted_copy_is_the_publish_order() {
    let mut k = kit();
    let (mut d, mut ids) = consumer();
    let inst = insert(&mut d, &mut ids, &k, k.card);
    let nested = k.doc.get(k.card).unwrap().children()[0];
    let icon = k.icon;
    let vault_nested = d.get(held(&d, L_ID, k.card)).unwrap().children()[0];
    let page_nested = d.get(inst).unwrap().children()[0];
    let mut lids = IdSource::new(0xAF);
    let (ring, own) = (lids.mint(), lids.mint());
    // D holds *Icon* {Dot, Ring} first, so the reorder is a reorder here too.
    edit_and_publish(&mut k, vec![create(ring, icon, 1, rect(), "Ring")]);
    accept(&mut d, &mut ids, &k);
    edit_and_publish(&mut k, vec![create(own, nested, 0, rect(), "Own")]);
    edit_and_publish(&mut k, vec![Operation::Reorder { id: ring, index: 0 }]);
    accept(&mut d, &mut ids, &k);
    let want = shape(published(&k).document(), nested);
    let names: Vec<&str> = want.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Own", "Ring", "Dot"], "fixture: the kit's order");
    assert_eq!(shape(&d, vault_nested), want, "the order is the publish's");
    assert_eq!(shape(&d, page_nested), want, "and the page's");
}
