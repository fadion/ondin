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
    assert_eq!(back.schema_version(), 6);
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

/// A v5 file loads as v6 — a number and nothing else.
#[test]
fn a_v5_file_loads_as_v6() {
    let (d, _) = consumer();
    let mut v: serde_json::Value = serde_json::from_slice(&io::save(&d).unwrap()).unwrap();
    v["schema_version"] = 5.into();
    let back = io::load(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(back, d);
}

/// Copy a kit instance out of `from` through the clipboard's text form — the
/// door another window reads — and paste it into `to`'s root.
fn paste_across(from: &Document, inst: NodeId, to: &mut Document, ids: &mut IdSource) -> NodeId {
    let subtrees = vec![from.capture_subtree(inst).unwrap()];
    let kits = kit::carried(from, &subtrees);
    let text = io::clip::write_carrying(&subtrees, &kits, &[], None, "Icon").unwrap();
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
    let (tx, made) = kit::insert_carrying(to, ids, &payload.kits, &placements, Default::default());
    to.apply(&tx).expect("a carrying paste");
    made[0]
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

    let tx = kit::make_local(&d, &[card]);
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
