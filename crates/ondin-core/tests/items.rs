//! Item ids on the five item lists (§15 D980): what agrees by value, what keeps
//! its id through a write, and the two doors that refuse a repeated one.

use ondin_core::Brush;
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Fill, IdSource, ItemId, Keyed, NodeId, NodeKind, OpError, Operation, PaintShown,
    Transaction, io, keyed_by_position, reserve_existing_ids,
};

fn solid(r: u8) -> Fill {
    Fill {
        brush: Brush::Solid(Color::from_rgba8(r, 0, 0, 255)),
        visible: true,
    }
}

/// A root, a frame and `n` rects in it, each rect given the fills `[solid(10),
/// solid(20)]` with ids minted from the session — so the rects agree in value and
/// every item id differs.
fn rects(n: usize) -> (Document, IdSource, Vec<NodeId>) {
    let mut ids = IdSource::new(0xAB);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: frame,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 400.0),
        },
        transform: None,
        name: None,
    }];
    let mut out = Vec::new();
    for i in 0..n {
        let id = ids.mint();
        ops.push(Operation::CreateNode {
            id,
            parent: frame,
            index: i,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: None,
        });
        let fills = vec![
            Keyed::new(ids.mint_item(), solid(10)),
            Keyed::new(ids.mint_item(), solid(20)),
        ];
        ops.push(Operation::SetFills { id, fills });
        out.push(id);
    }
    doc.apply(&Transaction(ops)).unwrap();
    (doc, ids, out)
}

fn fills(doc: &Document, id: NodeId) -> Vec<Keyed<Fill>> {
    doc.get(id).unwrap().paint().fills.clone()
}

/// Two layers given the same fills separately agree: the panel shows one list,
/// not *Mixed*. Flip: `paint_shown` comparing the keyed lists whole (`f != list`)
/// reads these as Mixed and fails here.
#[test]
fn a_shared_fill_list_agrees_by_value_not_by_id() {
    let (doc, _, ids) = rects(2);
    assert_ne!(fills(&doc, ids[0])[0].id, fills(&doc, ids[1])[0].id);
    match ondin_core::shared_fills(&doc, &ids) {
        PaintShown::List(l) => assert_eq!(l, fills(&doc, ids[0]), "the anchor is the first's"),
        PaintShown::Mixed => panic!("equal values read as mixed"),
    }
}

/// An edit of the shared list lands on each layer's **own** items: deleting the
/// first row and adding one leaves each layer its own id for the survivor and a
/// fresh, unshared id for the new row. Flip: writing `edited` verbatim (the
/// anchor's ids) fails the second rect's survivor assertion.
#[test]
fn an_edit_over_a_selection_keeps_each_layers_item_ids() {
    let (mut doc, mut src, ids) = rects(2);
    let anchor = fills(&doc, ids[0]);
    let (a, b) = (fills(&doc, ids[0]), fills(&doc, ids[1]));
    let mut edited = anchor.clone();
    edited.remove(0);
    edited.push(Keyed::new(src.mint_item(), solid(30)));
    let tx = ondin_core::retarget_fills_all(&doc, &ids, &anchor, &edited, &mut src);
    doc.apply(&tx).unwrap();
    let (a2, b2) = (fills(&doc, ids[0]), fills(&doc, ids[1]));
    assert_eq!(a2.len(), 2);
    assert_eq!(a2[0].id, a[1].id, "the first rect's survivor keeps its id");
    assert_eq!(
        b2[0].id, b[1].id,
        "the second rect's survivor keeps *its* id"
    );
    assert_ne!(a2[1].id, b2[1].id, "a new row is minted per layer");
    assert_eq!(a2[1].value, solid(30));
}

/// A wholesale replacement keeps each layer's ids by position and mints past its
/// end — the paste that lets an instance's child read as overriding its main's
/// items rather than deleting them.
#[test]
fn a_replacement_keeps_ids_by_position_and_mints_past_the_end() {
    let (mut doc, mut src, ids) = rects(1);
    let before = fills(&doc, ids[0]);
    let tx = ondin_core::set_fills_all(&doc, &ids, &[solid(1), solid(2), solid(3)], &mut src);
    doc.apply(&tx).unwrap();
    let after = fills(&doc, ids[0]);
    assert_eq!(after[0].id, before[0].id);
    assert_eq!(after[1].id, before[1].id);
    assert!(before.iter().all(|k| k.id != after[2].id));
    // The same values again is nothing to do.
    let again = ondin_core::set_fills_all(&doc, &ids, &[solid(1), solid(2), solid(3)], &mut src);
    assert!(again.0.is_empty());
}

/// `apply` refuses a list repeating an item id — and checks after the last op, so
/// a transaction that passes through a duplicate on its way to a valid list is
/// accepted.
#[test]
fn apply_refuses_an_item_id_twice_in_one_list() {
    let (mut doc, _, ids) = rects(1);
    let mut dup = fills(&doc, ids[0]);
    dup[1].id = dup[0].id;
    let id = ids[0];
    let err = doc
        .apply(&Transaction(vec![Operation::SetFills {
            id,
            fills: dup.clone(),
        }]))
        .unwrap_err();
    assert!(
        matches!(err, OpError::DuplicateItemId(n, _) if n == id),
        "{err:?}"
    );
    let fixed = keyed_by_position([solid(1), solid(2)]);
    doc.apply(&Transaction(vec![
        Operation::SetFills { id, fills: dup },
        Operation::SetFills { id, fills: fixed },
    ]))
    .expect("the duplicate is gone by the last op");
}

/// The loader refuses the same thing at the other door.
#[test]
fn a_file_repeating_an_item_id_is_refused() {
    let (doc, _, ids) = rects(1);
    let f = fills(&doc, ids[0]);
    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    let first = format!("\"{}\"", f[0].id.0.to_wire());
    let second = format!("\"{}\"", f[1].id.0.to_wire());
    assert!(text.contains(&second));
    let broken = text.replace(&second, &first);
    let err = io::load(broken.as_bytes()).unwrap_err();
    assert!(err.to_string().contains("twice"), "{err}");
}

/// A v4 file — items with no ids — loads with positional ids, `0:0`, `0:1`, and
/// saves to the same bytes twice. Flip: dropping the migration's numbering fails
/// the load on the missing `id`.
#[test]
fn a_v4_file_migrates_to_positional_item_ids() {
    let (doc, _, ids) = rects(1);
    let mut v: serde_json::Value = serde_json::from_slice(&io::save(&doc).unwrap()).unwrap();
    v["schema_version"] = 4.into();
    for node in v["nodes"].as_array_mut().unwrap() {
        if let Some(list) = node["paint"]["fills"].as_array_mut() {
            for item in list {
                item.as_object_mut().unwrap().remove("id");
            }
        }
    }
    let loaded = io::load(serde_json::to_vec(&v).unwrap().as_slice()).unwrap();
    let got: Vec<ItemId> = fills(&loaded, ids[0]).iter().map(|k| k.id).collect();
    assert_eq!(got, vec![ItemId::positional(0), ItemId::positional(1)]);
    assert_eq!(io::save(&loaded).unwrap(), io::save(&loaded).unwrap());
}

/// A session whose actor matches an item id's starts minting past it — including
/// actor 0, which a migration writes and a live session can (improbably) draw.
/// Flip: dropping the item sweep from `reserve_existing_ids` mints `0:1` here,
/// which the migrated list already holds.
#[test]
fn reserving_ids_skips_past_item_ids() {
    let mut ids = IdSource::new(0xAB);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(10.0, 10.0),
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: frame,
            fills: keyed_by_position([solid(1), solid(2), solid(3)]),
        },
    ]))
    .unwrap();
    let mut session = IdSource::new(0);
    reserve_existing_ids(&doc, &mut session);
    assert_eq!(session.mint_item(), ItemId::positional(3));
}
