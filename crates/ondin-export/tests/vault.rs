//! A document holding a kit's vault exports none of it (§5.3e step 1, §15
//! D1012): the vault is a second root, and every walk here starts at the
//! document's own.
//!
//! `Resolved` and hit-testing pin the claim elsewhere; without a document with a
//! vault here, an export or snapshot change iterating the node map, the shape
//! §5.3e names as the one to suspect, would put a kit's hidden mains in every
//! consumer's SVG and JSON with every suite green.

use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::meta::{DocumentMeta, KitMark};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Fill, IdSource, NodeKind, Operation, Resolved, Transaction, keyed_by_position,
};

const KIT: &str = "5eed00000000000000000000000000e1";

/// A consumer holding an instance of a kit's *Badge*, whose *Secret* layer —
/// filled `#010203` — the instance has deleted, so it lives in the vault alone;
/// and the vault's node ids.
fn consumer() -> (Document, Vec<String>) {
    let mut ids = IdSource::new(0xE1);
    let root = ids.mint();
    let mut kit = Document::new(root);
    let [badge, secret, shown] = [(); 3].map(|_| ids.mint());
    let rect = |id, index, name: &str| Operation::CreateNode {
        id,
        parent: badge,
        index,
        kind: NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: Some(name.into()),
    };
    kit.apply(&Transaction(vec![
        Operation::CreateNode {
            id: badge,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 20.0),
            },
            transform: None,
            name: Some("Badge".into()),
        },
        rect(secret, 0, "Secret"),
        rect(shown, 1, "Shown"),
        Operation::SetFills {
            id: secret,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(1, 2, 3, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetComponent {
            id: badge,
            component: true,
        },
    ]))
    .unwrap();
    kit.set_meta(DocumentMeta {
        id: Some(KIT.into()),
        kit: Some(KitMark::default()),
        ..Default::default()
    });
    let published = ondin_core::kit::freeze(&kit).unwrap();

    let mut dids = IdSource::new(0xE2);
    let mut d = Document::new(dids.mint());
    let droot = d.first_page();
    let (tx, inst) =
        ondin_core::kit::insert(&d, &mut dids, KIT, 1, &published, badge, droot).unwrap();
    d.apply(&tx).unwrap();
    let copy = d.get(inst).unwrap().children()[0];
    d.apply(&Transaction(vec![Operation::DeleteNode { id: copy }]))
        .unwrap();
    let vault = d.vault().expect("a vault");
    let in_vault: Vec<String> = d
        .capture_subtree(vault)
        .unwrap()
        .iter()
        .map(|n| n.id().to_wire())
        .collect();
    (d, in_vault)
}

/// **Neither the SVG nor the snapshot shows a node of the vault.** The
/// instance's own layer is in both; the vault's *Secret*, which only the vault
/// holds, is in neither, and no vault id appears in the snapshot.
///
/// Flip, run: `snapshot` collecting from the vault's root too — the
/// map-iterating shape — fails *"no vault node in the snapshot"*.
#[test]
fn a_vault_is_in_no_export() {
    let (d, in_vault) = consumer();
    let res = Resolved::rebuild(&d);

    let svg = ondin_export::svg::svg(&d, &res, d.first_page(), None);
    assert!(svg.contains("<rect"), "fixture: the instance draws: {svg}");
    assert!(!svg.contains("#010203"), "the vault's Secret is not drawn");

    let snap = ondin_export::snapshot::snapshot(&d, &res);
    let json = serde_json::to_string(&snap).unwrap();
    assert!(
        json.contains("Shown"),
        "fixture: the instance's layer is there"
    );
    for id in &in_vault {
        assert!(
            !snap.nodes.iter().any(|n| &n.id == id),
            "no vault node in the snapshot: {id}"
        );
    }
    assert!(!json.contains("Secret"), "nor its name");
}
