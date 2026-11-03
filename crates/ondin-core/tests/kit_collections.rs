//! A kit with two collections (§5.3e, §5.3f): vendored, its modes keep their
//! collections, and each kit collection reads the mode in force of the
//! document collection of its name.

use ondin_core::kit::{self};
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::meta::{DocumentMeta, KitMark};
use ondin_core::peniko::Color;
use ondin_core::variable::{Collection, CollectionId};
use ondin_core::{
    Brush, BrushSource, Cell, Document, Fill, IdSource, Mode, ModeId, NodeId, NodeKind, Operation,
    Transaction, Value, VarId, VarKind, Variable, keyed_by_position,
};

const L_ID: &str = "5eed0000000000000000000000000022";
const WHITE: Color = Color::from_rgba8(255, 255, 255, 255);
const NAVY: Color = Color::from_rgba8(10, 20, 80, 255);

fn mode(id: ModeId, name: &str) -> Mode {
    Mode {
        id,
        name: name.into(),
    }
}

fn var(id: VarId, name: &str, kind: VarKind, cells: Vec<(ModeId, Cell)>) -> Variable {
    Variable {
        id,
        name: name.into(),
        kind,
        cells: cells.into_iter().collect(),
        scope: None,
        origin: None,
        forked: None,
        description: String::new(),
    }
}

/// `ids` making a document with collections Colour (`a`, `b`) and Spacing
/// (`c`, `d`), named as given.
fn two_collections(ids: &mut IdSource, names: [&str; 4]) -> (Document, [ModeId; 4], CollectionId) {
    let mut doc = Document::new(ids.mint());
    let modes = [(); 4].map(|_| ids.mint_mode());
    let spacing = ids.mint_collection();
    doc.apply(&Transaction(vec![
        Operation::SetCollectionName {
            id: CollectionId::FIRST,
            name: "Colour".into(),
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(modes[0], names[0]),
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(modes[1], names[1]),
            index: 1,
            cells: vec![],
        },
        Operation::AddCollection {
            collection: Collection {
                id: spacing,
                name: "Spacing".into(),
                modes: vec![mode(modes[2], names[2]), mode(modes[3], names[3])],
            },
            index: 1,
        },
    ]))
    .expect("two collections");
    (doc, modes, spacing)
}

/// **A two-collection kit vendors with its collections**: its modes arrive
/// named by their kit collection, the instance's fill reads Colour's mode in
/// force and its corners Spacing's, each independently.
///
/// flip: `Variables::kit_onto` always answering the first collection → fails
/// at *"Comfortable through Spacing"*.
#[test]
fn a_kits_collections_map_by_name() {
    let mut ids = IdSource::new(0xAF);
    let (mut l, [day, night, compact, comfy], _) =
        two_collections(&mut ids, ["Day", "Night", "Compact", "Comfortable"]);
    let (brand, radius) = (ids.mint_var(), ids.mint_var());
    let badge = ids.mint();
    let root = l.first_page();
    l.apply(&Transaction(vec![
        Operation::AddVariable {
            var: var(
                brand,
                "brand",
                VarKind::Color,
                vec![(day, Cell::Color(WHITE)), (night, Cell::Color(NAVY))],
            ),
        },
        Operation::AddVariable {
            var: var(
                radius,
                "radius",
                VarKind::Number,
                vec![(compact, Cell::Number(2.0)), (comfy, Cell::Number(9.0))],
            ),
        },
        Operation::CreateNode {
            id: badge,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 16.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Badge".into()),
        },
        Operation::SetFills {
            id: badge,
            fills: keyed_by_position([Fill {
                brush: BrushSource::Bound {
                    var: brand,
                    alpha: 1.0,
                },
                visible: true,
            }]),
        },
        Operation::SetCornerVars {
            id: badge,
            vars: [Some(radius); 4],
        },
        Operation::SetComponent {
            id: badge,
            component: true,
        },
    ]))
    .expect("the kit");
    l.set_meta(DocumentMeta {
        id: Some(L_ID.into()),
        name: Some("Atlas".into()),
        kit: Some(KitMark {
            version: 0,
            ..KitMark::default()
        }),
        ..DocumentMeta::default()
    });
    assert_eq!(kit::publish(&mut l, None, 1), Some(1));

    let mut cids = IdSource::new(0xDC);
    let (mut d, [_, dark, _, comfortable], spacing) =
        two_collections(&mut cids, ["Light", "Night", "Compact", "Comfortable"]);
    let page = d.first_page();
    let (tx, inst) =
        kit::insert(&d, &mut cids, L_ID, 1, l.published().unwrap(), badge, page).unwrap();
    d.apply(&tx).expect("an insert of a two-collection kit");
    let km = &d.variables().kits()[L_ID];
    let named: Vec<(&str, &str)> = km
        .modes
        .iter()
        .map(|m| (m.collection.as_str(), m.name.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("Colour", "Day"),
            ("Colour", "Night"),
            ("Spacing", "Compact"),
            ("Spacing", "Comfortable")
        ]
    );

    let fill = |d: &Document, id: NodeId| -> Brush {
        let b = &d.get(id).unwrap().paint().fills[0].brush;
        d.resolve_brush(id, b).into_owned()
    };
    let radius_at = |d: &Document, id: NodeId| {
        let v = d.get(id).unwrap().corner_vars()[0].unwrap();
        d.resolve_var(id, v)
    };
    // Defaults: Light maps to Day by first, Compact by name.
    assert_eq!(fill(&d, inst), Brush::Solid(WHITE));
    assert_eq!(radius_at(&d, inst), Some(Value::Number(2.0)));
    d.apply(&Transaction(vec![Operation::SetModes {
        id: page,
        modes: vec![comfortable],
    }]))
    .unwrap();
    assert_eq!(fill(&d, inst), Brush::Solid(WHITE), "Colour unmoved");
    assert_eq!(
        radius_at(&d, inst),
        Some(Value::Number(9.0)),
        "Comfortable through Spacing"
    );
    d.apply(&Transaction(vec![Operation::SetModes {
        id: page,
        modes: vec![dark, comfortable],
    }]))
    .unwrap();
    assert_eq!(fill(&d, inst), Brush::Solid(NAVY), "Night by name");
    assert_eq!(d.mode_at(inst, spacing), Some(comfortable));
}
