//! IO: deterministic save, round-trip load, integrity checks, and the migration
//! chain (§5.11, invariant 9).

use ondin_core::Brush;
use ondin_core::io::{self, IoError};
use ondin_core::kurbo::{BezPath, Point, RoundedRectRadii, Size, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, DocumentMeta, Effect, EffectKind, Fill, Filters, GradientBrush, Guide, GuideAxis,
    GuideId, History, IdSource, ImageEntry, ImageFormat, ImageId, ImageSource, NodeId, NodeKind,
    Operation, Pivot, Shadow, Stroke, StrokeAlign, Transaction, image_brush,
};

/// Build a document exercising every node kind and several fields.
fn rich_document() -> (Document, NodeId) {
    let mut ids = IdSource::new(0xABCDEF);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut hist = History::new();

    let ab = ids.mint();
    let group = ids.mint();
    let r = ids.mint();
    let e = ids.mint();
    let line = ids.mint();
    let path = ids.mint();
    let text = ids.mint();

    let mut p = BezPath::new();
    p.move_to(Point::new(0.0, 0.0));
    p.line_to(Point::new(10.0, 10.0));
    p.close_path();

    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(800.0, 600.0),
                },
                transform: None,
                name: Some("Board".into()),
            },
            Operation::SetFills {
                id: ab,
                fills: vec![Fill {
                    brush: Brush::Solid(Color::from_rgba8(250, 250, 250, 255)),
                    visible: true,
                }],
            },
            Operation::CreateNode {
                id: group,
                parent: ab,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: r,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 50.0),
                    corner_radii: RoundedRectRadii::new(8.0, 4.0, 0.0, 12.0),
                },
                transform: Some(ondin_core::kurbo::Affine::translate((12.0, 34.0))),
                name: None,
            },
            Operation::CreateNode {
                id: e,
                parent: group,
                index: 1,
                kind: NodeKind::Ellipse {
                    size: Size::new(40.0, 40.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: line,
                parent: ab,
                index: 1,
                kind: NodeKind::Line {
                    end: Point::new(100.0, 0.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: path,
                parent: ab,
                index: 2,
                kind: NodeKind::Path {
                    path: p,
                    corner_radii: Vec::new(),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: text,
                parent: ab,
                index: 3,
                kind: NodeKind::Text {
                    content: "Hello".into(),
                    style: Box::new(ondin_core::TextStyle {
                        font_family: "Inter".into(),
                        font_size: 16.0,
                        weight: 500,
                        italic: false,
                        line_height: Some(ondin_core::Length::Em(1.4)),
                        ..Default::default()
                    }),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing: ondin_core::TextSizing::Fixed(Size::new(120.0, 40.0)),
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: None,
                name: None,
            },
        ]),
    )
    .unwrap();

    // Give the rect a fill and a stroke, and tweak some scalar fields.
    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::SetFills {
                id: r,
                fills: vec![Fill {
                    brush: Brush::Solid(Color::from_rgba8(200, 30, 30, 255)),
                    visible: true,
                }],
            },
            Operation::SetStrokes {
                id: r,
                strokes: vec![Stroke {
                    brush: Brush::Solid(Color::from_rgba8(20, 20, 20, 255)),
                    width: 2.5,
                    join: ondin_core::kurbo::Join::Bevel,
                    cap: ondin_core::kurbo::Cap::Square,
                    dashes: vec![4.0, 2.0],
                    dash_offset: 1.0,
                    sides: Default::default(),
                    miter_limit: 4.0,
                    dash_fit: false,
                    align: StrokeAlign::Center,
                    visible: true,
                }],
            },
            Operation::SetOpacity {
                id: r,
                opacity: 0.5,
            },
            Operation::SetVisible {
                id: e,
                visible: false,
            },
            Operation::SetLocked {
                id: group,
                locked: true,
            },
        ]),
    )
    .unwrap();

    (doc, root)
}

#[test]
fn save_load_roundtrips() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let loaded = io::load(&bytes).unwrap();
    assert_eq!(doc, loaded, "load must reconstruct the exact document");
}

#[test]
fn save_is_byte_deterministic_through_load() {
    let (doc, _root) = rich_document();
    let bytes1 = io::save(&doc).unwrap();
    let loaded = io::load(&bytes1).unwrap();
    let bytes2 = io::save(&loaded).unwrap();
    assert_eq!(
        bytes1, bytes2,
        "save must be byte-identical across a load cycle"
    );
}

#[test]
fn saved_file_records_current_schema_version() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["schema_version"],
        serde_json::json!(io::CURRENT_SCHEMA_VERSION)
    );
}

#[test]
fn migration_chain_upgrades_v0_to_current() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();

    // Synthesize a v0 file: identical bytes but with schema_version = 0. Each
    // step past v0 is a no-op on data already in the current shape, so what
    // this pins is that the *chain* runs to the end rather than stopping at the
    // first step.
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["schema_version"] = serde_json::json!(0);
    let v0_bytes = serde_json::to_vec(&value).unwrap();

    let loaded = io::load(&v0_bytes).unwrap();
    assert_eq!(loaded.schema_version(), io::CURRENT_SCHEMA_VERSION);
    assert_eq!(doc, loaded);
}

/// **The chain composes: one genuinely old file through all four steps**
/// (`[S1.1-L6-02]`, §15 D501).
///
/// ⚠️ **Nothing anywhere fed a migration step the output of the step before it.**
/// `migration_chain_upgrades_v0_to_current` above says so in its own comment —
/// its v0 file is a *current-shape* save with the version rewritten, so every
/// step past v0 has nothing to do and what it pins is that the loop runs to the
/// end. The three tests that carry real data each rewind exactly **one** version,
/// from their own step's immediate predecessor. So a `migrate_2_to_3` that
/// depended on a v1→v2 artefact would be untestable to notice, because no fixture
/// ever arrived at step 3 by way of step 2.
///
/// The fixture here is a v1 file carrying **all three** legacy shapes at once —
/// a `Rect` with a scalar `corner_radius`, a `Text` with a bare `line_height`
/// multiplier and an `align` inside `style`, and an `Artboard` with a
/// `background` brush — so the load runs 1→2→3→4 with something to do at every
/// step, and each step's own outcome is asserted rather than only the version
/// number.
///
/// **This is coverage, not a bug**: the composition works today. What it costs
/// when it stops is the failure mode the review named — *"this old file will not
/// open"*, which is data loss by a different road, landing on exactly the files
/// that have been around longest.
///
/// ⚠️ **The rewinds are the exact inverses of the migrations**, taken from the
/// three single-step tests below rather than written afresh, so this is a round
/// trip through the shape the old writers emitted rather than a guess at it.
///
/// **Flip run**, the `2 => migrate_2_to_3` arm deleted: the load fails outright
/// with `UnsupportedVersion(2)` — the chain refusing rather than silently
/// dropping the alignment, which is the predicted site and the behaviour that
/// makes a broken step loud.
#[test]
fn a_v1_file_carrying_all_three_old_shapes_migrates_through_every_step() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["schema_version"] = serde_json::json!(1);

    let (mut rects, mut texts, mut frames) = (0, 0, 0);
    for node in value["nodes"].as_array_mut().unwrap() {
        if let Some(rect) = node.get_mut("kind").and_then(|k| k.get_mut("Rect")) {
            let obj = rect.as_object_mut().unwrap();
            obj.remove("corner_radii");
            obj.insert("corner_radius".into(), serde_json::json!(6.0));
            rects += 1;
        }
        if let Some(text) = node.get_mut("kind").and_then(|k| k.get_mut("Text")) {
            let obj = text.as_object_mut().unwrap();
            obj.remove("spans");
            obj.remove("paragraph");
            obj.remove("block");
            let style = obj.get_mut("style").unwrap().as_object_mut().unwrap();
            style.insert("line_height".into(), serde_json::json!(1.4));
            style.insert("align".into(), serde_json::json!("Right"));
            texts += 1;
        }
        if node.get("kind").and_then(|k| k.get("Artboard")).is_some() {
            let fills = node["paint"]["fills"].as_array_mut().unwrap();
            assert_eq!(fills.len(), 1, "the fixture's frame has one fill");
            let brush = fills.remove(0)["brush"].clone();
            node["kind"]["Artboard"]
                .as_object_mut()
                .unwrap()
                .insert("background".into(), brush);
            frames += 1;
        }
    }
    assert_eq!(
        (rects, texts, frames),
        (1, 1, 1),
        "the fixture holds one of each, or a step below is asserting nothing"
    );

    let loaded = io::load(&serde_json::to_vec(&value).unwrap()).expect("a v1 file still opens");
    assert_eq!(loaded.schema_version(), io::CURRENT_SCHEMA_VERSION);

    // Step 2's outcome.
    let board = loaded.get(loaded.root()).unwrap().children()[0];
    let all = loaded.capture_subtree(board).unwrap();
    let rect = all
        .iter()
        .find(|n| matches!(n.kind(), NodeKind::Rect { .. }))
        .expect("the rect survives");
    assert_eq!(
        rect.kind(),
        &NodeKind::Rect {
            size: Size::new(100.0, 50.0),
            corner_radii: RoundedRectRadii::from_single_radius(6.0),
        },
        "v1→v2: the scalar radius is spread over four corners"
    );

    // Step 3's, which is the one no fixture had ever reached by way of step 2.
    let NodeKind::Text {
        style, paragraph, ..
    } = text_node(&loaded).kind()
    else {
        unreachable!("the fixture's text node is still text")
    };
    assert_eq!(
        style.line_height,
        Some(ondin_core::Length::Em(1.4)),
        "v2→v3: a bare multiplier is `Em`, not the font's own leading"
    );
    assert_eq!(
        paragraph.align,
        ondin_core::TextAlign::Right,
        "v2→v3: and `align` moved into the paragraph scope without relabelling"
    );

    // Step 4's.
    let frame = loaded.get(board).unwrap();
    assert_eq!(
        frame.kind(),
        &NodeKind::Artboard {
            size: Size::new(800.0, 600.0)
        },
        "v3→v4: the kind carries a size and nothing else"
    );
    assert_eq!(
        frame.paint().fills,
        vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(250, 250, 250, 255)),
            visible: true,
        }],
        "v3→v4: and the ground it was saved with is fill 0"
    );
}

/// v1 → v2: a rect's single `corner_radius` becomes four equal `corner_radii`.
///
/// Real data, not a stub like v0 → v1: a v1 file that still says
/// `corner_radius` must open, and must open as the rectangle it drew.
#[test]
fn migration_spreads_a_v1_corner_radius_over_four_corners() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // Rewind the one rect in the document to the v1 shape.
    value["schema_version"] = serde_json::json!(1);
    let mut rewound = 0;
    for node in value["nodes"].as_array_mut().unwrap() {
        let Some(rect) = node.get_mut("kind").and_then(|k| k.get_mut("Rect")) else {
            continue;
        };
        let obj = rect.as_object_mut().unwrap();
        obj.remove("corner_radii");
        obj.insert("corner_radius".into(), serde_json::json!(6.0));
        rewound += 1;
    }
    assert_eq!(rewound, 1, "the fixture should hold exactly one rect");

    let loaded = io::load(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(loaded.schema_version(), io::CURRENT_SCHEMA_VERSION);
    // `capture_subtree` refuses the root, so walk from the artboard under it.
    let board = loaded.get(loaded.root()).unwrap().children()[0];
    let all = loaded.capture_subtree(board).unwrap();
    let rect = all
        .iter()
        .find(|n| matches!(n.kind(), NodeKind::Rect { .. }))
        .expect("the rect survives the migration");
    assert_eq!(
        rect.kind(),
        &NodeKind::Rect {
            size: Size::new(100.0, 50.0),
            corner_radii: RoundedRectRadii::from_single_radius(6.0),
        }
    );
}

/// v3 → v4: a frame's `background` becomes the first entry of its fill list.
///
/// Real data like the v1 → v2 case above: a v3 file that still says `background`
/// must open, and must open with the frame painted the colour it was painted
/// (§15 D400). The rewind is the exact inverse of the migration — take the fill
/// back out of `fills` and put its brush in a `background` key on the kind — which
/// is what makes this a round trip through the shape the old writer emitted rather
/// than a hand-written guess at it.
///
/// ⚠️ **Two assertions, and the second is the one with teeth.** That the colour
/// survives is the obvious half. That `background` is *gone from the kind* is the
/// half a migration can fail silently: serde has no `deny_unknown_fields` here, so
/// a v3 file whose `background` key were simply ignored would load, pass every
/// integrity check, and draw a frame with no ground — the ordinary way a
/// migration that never ran looks exactly like one that did.
///
/// Flipped by deleting the `3 => migrate_3_to_4` arm: the load fails outright with
/// `UnsupportedVersion(3)`, which is the chain doing its job. Flipped by making
/// `migrate_3_to_4` remove the key without writing the fill: the colour assertion
/// fails with an empty list, which is the case above.
#[test]
fn migration_moves_a_v3_frame_background_into_its_fill_list() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // Rewind the one frame in the document to the v3 shape.
    value["schema_version"] = serde_json::json!(3);
    let mut rewound = 0;
    for node in value["nodes"].as_array_mut().unwrap() {
        if node.get("kind").and_then(|k| k.get("Artboard")).is_none() {
            continue;
        }
        let fills = node["paint"]["fills"].as_array_mut().unwrap();
        assert_eq!(fills.len(), 1, "the fixture's frame has one fill");
        let brush = fills.remove(0)["brush"].clone();
        node["kind"]["Artboard"]
            .as_object_mut()
            .unwrap()
            .insert("background".into(), brush);
        rewound += 1;
    }
    assert_eq!(rewound, 1, "the fixture should hold exactly one frame");

    let loaded = io::load(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(loaded.schema_version(), io::CURRENT_SCHEMA_VERSION);
    let board = loaded.get(loaded.root()).unwrap().children()[0];
    let frame = loaded.get(board).unwrap();
    assert_eq!(
        frame.kind(),
        &NodeKind::Artboard {
            size: Size::new(800.0, 600.0)
        },
        "the kind carries a size and nothing else"
    );
    assert_eq!(
        frame.paint().fills,
        vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(250, 250, 250, 255)),
            visible: true,
        }],
        "the ground it was saved with, visible, as fill 0"
    );
}

/// A v3 frame with **no** ground gains no fill, because `null` is what "no ground"
/// was written as and an empty list is exactly what it meant.
///
/// Worth its own test rather than a second assertion above: the migration reaches
/// the `background` key by `remove`, and a `null` that slipped past the filter
/// would be wrapped into a `Fill` whose brush fails to deserialize — a load error
/// on a file that is perfectly good, which is the worst failure a migration has.
#[test]
fn migration_leaves_a_v3_frame_with_no_background_unfilled() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    value["schema_version"] = serde_json::json!(3);
    for node in value["nodes"].as_array_mut().unwrap() {
        if node.get("kind").and_then(|k| k.get("Artboard")).is_none() {
            continue;
        }
        node["paint"]["fills"] = serde_json::json!([]);
        node["kind"]["Artboard"]
            .as_object_mut()
            .unwrap()
            .insert("background".into(), serde_json::Value::Null);
    }

    let loaded = io::load(&serde_json::to_vec(&value).unwrap()).unwrap();
    let board = loaded.get(loaded.root()).unwrap().children()[0];
    assert!(loaded.get(board).unwrap().paint().fills.is_empty());
}

#[test]
fn load_rejects_future_version() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["schema_version"] = serde_json::json!(9999);
    let future = serde_json::to_vec(&value).unwrap();

    let err = io::load(&future).unwrap_err();
    assert!(matches!(err, IoError::UnsupportedVersion(9999)));
}

/// A `schema_version` too large for a `u32` is refused as the number the file
/// actually claims, rather than truncated into a version the chain will migrate
/// (§15 D638).
///
/// Three headers, one per measured row of the finding. `8589934592` is 2³³,
/// whose low 32 bits are **zero**: cast first, it read as v0 and ran the whole
/// v0→v4 chain, so the file opened. `4294967300` is 2³²+4, which truncated to
/// `CURRENT_SCHEMA_VERSION` and returned early, and the failure then arrived
/// from the DTO as a serde error about a `u32` — an answer to the wrong
/// question. `u64::MAX` truncated to `4294967295` and was refused under a
/// version the file does not claim.
///
/// ⚠️ **The assertion is on the number inside the error, not on the variant.**
/// `UnsupportedVersion(_)` alone is satisfied by a comparison that is widened
/// and a payload that is re-narrowed, which is the half of this fix a later
/// reader is likeliest to undo — the error type carrying a `u32` is what made
/// the cast look free.
///
/// `9999` is deliberately left to `load_rejects_future_version` above: it fits a
/// `u32`, so it was green before this fix and says nothing about it.
///
/// Flip: the cast put back on the read and both comparisons narrowed, with the
/// payload widened again at the two `Err` sites so it still compiles — i.e. the
/// shipped code before D638, not a deletion. Red, on the 2³³ row, with
/// *"8589934592: opened as though it were an old file"*. ⚠️ **The predicted
/// failure site was the `assert_eq!` and it is the `Ok` arm**: that header does
/// not merely get reported under the wrong number, it opens the file, so the
/// loop never reaches a comparison. The row that fails on the *number* is
/// `u64::MAX`, and it never runs because the first row panics.
///
/// ⚠️ **Widening the error's payload is what makes the narrowing visible at
/// all.** The flip did not compile on its first attempt: with the variant at
/// `u64`, re-introducing the cast leaves the `match`'s `other` arm handing a
/// `u32` to it, and rustc names that line. That is a mechanical objection to
/// half of the old shape and it exists only because the payload moved — while
/// the variant was `u32`, casting at the read was free everywhere, which is how
/// the guard came to be written on a number nobody had checked.
#[test]
fn load_rejects_a_schema_version_wider_than_u32_without_truncating_it() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();

    for claimed in [8_589_934_592u64, 4_294_967_300, u64::MAX] {
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["schema_version"] = serde_json::json!(claimed);
        let file = serde_json::to_vec(&value).unwrap();

        match io::load(&file) {
            Err(IoError::UnsupportedVersion(v)) => assert_eq!(
                v, claimed,
                "refused under a version the file does not claim"
            ),
            Err(other) => panic!("{claimed}: refused, but not as a version question: {other}"),
            Ok(_) => panic!("{claimed}: opened as though it were an old file"),
        }
    }
}

#[test]
fn load_rejects_dangling_child_reference() {
    // Hand-built file whose root lists a child that does not exist.
    let ghost = NodeId::from_wire("1:2").unwrap().to_wire();
    let json = format!(
        r#"{{
          "schema_version": 1,
          "root": "1:1",
          "nodes": [
            {{"id":"1:1","parent":null,"children":["{ghost}"],"kind":"Root",
              "transform":[1.0,0.0,0.0,1.0,0.0,0.0],"name":"Root",
              "visible":true,"locked":false,"opacity":1.0,
              "paint":{{"fills":[],"strokes":[]}}}}
          ]
        }}"#
    );
    let err = io::load(json.as_bytes()).unwrap_err();
    assert!(matches!(err, IoError::Integrity(_)));
}

/// Corrupt a freshly saved document by mutating its JSON, then load it.
fn load_corrupted(mutate: impl FnOnce(&mut serde_json::Value)) -> IoError {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    mutate(&mut value);
    io::load(&serde_json::to_vec(&value).unwrap()).expect_err("corrupted document must be rejected")
}

fn loose_group(id: &str, parent: &str, child: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "parent": parent, "children": [child], "kind": "Group",
        "transform": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], "name": "Loop",
        "visible": true, "locked": false, "opacity": 1.0,
        "paint": {"fills": [], "strokes": []}
    })
}

#[test]
fn load_rejects_unreachable_cycle() {
    // Two nodes naming each other as parent and child: parent/child links are
    // locally consistent, but the pair is a cycle unreachable from the root.
    // The tree walks (`subtree_ids`, `paint_path`) have no visited set, so
    // accepting this would let a corrupt file hang the app.
    let err = load_corrupted(|value| {
        let nodes = value["nodes"].as_array_mut().unwrap();
        nodes.push(loose_group("ffff:1", "ffff:2", "ffff:2"));
        nodes.push(loose_group("ffff:2", "ffff:1", "ffff:1"));
    });
    assert!(matches!(err, IoError::Integrity(msg) if msg.contains("reachable")));
}

#[test]
fn load_rejects_duplicate_child_listing() {
    // One node listed twice in the same parent's children: consistent by the
    // old checks, but it makes the node appear twice in every tree walk.
    let err = load_corrupted(|value| {
        for node in value["nodes"].as_array_mut().unwrap() {
            let kids = node["children"].as_array_mut().unwrap();
            if let Some(first) = kids.first().cloned() {
                kids.push(first);
                return;
            }
        }
        panic!("fixture has no node with children");
    });
    assert!(matches!(err, IoError::Integrity(msg) if msg.contains("twice")));
}

#[test]
fn load_enforces_the_same_kind_rules_as_operations() {
    // Promote the rect inside the group to an Artboard: a frame nests inside a
    // *frame* but never inside a group (§5.3), a rule `apply` enforces and the
    // loader must not silently allow.
    let err = load_corrupted(|value| {
        let groups: Vec<serde_json::Value> = value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["kind"] == serde_json::json!("Group"))
            .map(|n| n["id"].clone())
            .collect();
        assert!(!groups.is_empty(), "fixture has no group");
        for node in value["nodes"].as_array_mut().unwrap() {
            if groups.contains(&node["parent"]) {
                node["kind"] = serde_json::json!({
                    "Artboard": { "size": {"width": 10.0, "height": 10.0}, "background": null }
                });
                return;
            }
        }
        panic!("fixture has nothing inside a group");
    });
    assert!(matches!(err, IoError::Integrity(_)));
}

/// `Stroke::align` was added after v1 files existed. It is defaulted rather than
/// schema-versioned (§5.11), and this is the guarantee that buys: a saved file
/// with no `align` field still loads, as the centred stroke it was drawn as.
#[test]
fn a_saved_file_without_stroke_align_loads_as_centred() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();

    // Strip every `align` field, reproducing a file written before it existed.
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut stripped = 0;
    for node in value["nodes"].as_array_mut().expect("nodes array") {
        if let Some(strokes) = node["paint"]["strokes"].as_array_mut() {
            for s in strokes {
                if s.as_object_mut().and_then(|o| o.remove("align")).is_some() {
                    stripped += 1;
                }
            }
        }
    }
    assert!(stripped > 0, "fixture must contain a stroke to strip");

    let old = serde_json::to_vec(&value).unwrap();
    let loaded = io::load(&old).expect("a v1 file must still load");
    // Equality covers it: the fixture's strokes are all `Center`, so a document
    // that compares equal is one whose strokes defaulted back to centred.
    assert_eq!(doc, loaded, "and reconstruct the same document");
}

/// `clip` is additive like `align` was, so a file written before it existed has
/// to load as what it drew: frames clipping, and nothing else claiming to.
#[test]
fn a_saved_file_without_clip_loads_with_frames_clipping_and_nothing_else() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut stripped = 0;
    for node in value["nodes"].as_array_mut().expect("nodes array") {
        if node
            .as_object_mut()
            .and_then(|o| o.remove("clip"))
            .is_some()
        {
            stripped += 1;
        }
    }
    assert!(stripped > 1, "every node carries the field");

    let old = serde_json::to_vec(&value).unwrap();
    let loaded = io::load(&old).expect("a file without `clip` must still load");
    assert_eq!(doc, loaded, "and reconstruct the same document");
    // Spelled out, because "equal" only means the default matched what the
    // fixture happened to hold: frames clip, everything else does not.
    let mut frames = 0;
    for id in [loaded.root()].into_iter().chain(
        loaded
            .get(loaded.root())
            .unwrap()
            .children()
            .iter()
            .copied(),
    ) {
        let node = loaded.get(id).unwrap();
        let is_frame = matches!(node.kind(), NodeKind::Artboard { .. });
        frames += usize::from(is_frame);
        assert_eq!(node.clip(), is_frame, "{:?} clips wrongly", node.name());
    }
    assert!(frames > 0, "fixture must contain a frame");
}

/// The canvas ground is additive in exactly the same way: a file written while
/// the ground was still session state was drawn on the default, so that is what
/// it has to load as.
#[test]
fn a_saved_file_without_a_canvas_ground_loads_with_the_default() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        value
            .as_object_mut()
            .and_then(|o| o.remove("canvas_background"))
            .is_some(),
        "the field is written at the top level, not per node"
    );

    let old = serde_json::to_vec(&value).unwrap();
    let loaded = io::load(&old).expect("a file without a ground must still load");
    assert_eq!(
        loaded.canvas_background(),
        ondin_core::DEFAULT_CANVAS_BACKGROUND
    );
    assert_eq!(doc, loaded, "and reconstruct the same document");
}

/// A ground the user chose survives the round trip, and undo puts the old one
/// back — it is a document edit like any other now, not a view preference.
#[test]
fn the_canvas_ground_round_trips_and_undoes() {
    let (mut doc, _root) = rich_document();
    let mut hist = History::new();
    let chosen = Color::from_rgba8(90, 20, 40, 255);

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetCanvasBackground { background: chosen }]),
    )
    .unwrap();
    assert_eq!(doc.canvas_background(), chosen);

    let loaded = io::load(&io::save(&doc).unwrap()).unwrap();
    assert_eq!(loaded.canvas_background(), chosen);
    assert_eq!(doc, loaded);

    hist.undo(&mut doc).unwrap();
    assert_eq!(
        doc.canvas_background(),
        ondin_core::DEFAULT_CANVAS_BACKGROUND
    );
}

/// And a `clip` the user turned *off* survives the round trip — the default is
/// only for files that predate the field.
#[test]
fn clipping_turned_off_round_trips() {
    let (mut doc, root) = rich_document();
    let frame = doc.get(root).unwrap().children()[0];
    doc.apply(&Transaction(vec![Operation::SetClip {
        id: frame,
        clip: false,
    }]))
    .unwrap();

    let loaded = io::load(&io::save(&doc).unwrap()).unwrap();
    assert!(!loaded.get(frame).unwrap().clip());
    assert_eq!(doc, loaded);
}

/// The proportion lock is additive in the same way, and its default is the one
/// that needs no argument: while the lock was session state no file recorded it,
/// so every node in every existing file meant "unlocked".
#[test]
fn a_saved_file_without_the_proportion_lock_loads_with_nothing_locked() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut stripped = 0;
    for node in value["nodes"].as_array_mut().expect("nodes array") {
        if node
            .as_object_mut()
            .and_then(|o| o.remove("proportions_locked"))
            .is_some()
        {
            stripped += 1;
        }
    }
    assert!(stripped > 1, "every node carries the field");

    let old = serde_json::to_vec(&value).unwrap();
    let loaded = io::load(&old).expect("a file without the lock must still load");
    assert_eq!(doc, loaded, "and reconstruct the same document");
    // Equality only says the default matched the fixture; say what it matched.
    // Every kind, not just the resizable ones — there is no kind gate.
    let mut stack = vec![loaded.root()];
    let mut seen = 0;
    while let Some(id) = stack.pop() {
        let node = loaded.get(id).unwrap();
        assert!(
            !node.proportions_locked(),
            "{:?} came back locked",
            node.name()
        );
        seen += 1;
        stack.extend(node.children().iter().copied());
    }
    assert_eq!(seen, stripped, "and every node in the file was checked");
}

/// And a lock the user turned *on* survives the round trip — losing it on reopen
/// is the whole reason it stopped being session state (§15 D41).
#[test]
fn a_proportion_lock_round_trips() {
    let (mut doc, root) = rich_document();
    let frame = doc.get(root).unwrap().children()[0];
    doc.apply(&Transaction(vec![Operation::SetProportionsLocked {
        id: frame,
        locked: true,
    }]))
    .unwrap();

    let loaded = io::load(&io::save(&doc).unwrap()).unwrap();
    assert!(loaded.get(frame).unwrap().proportions_locked());
    assert_eq!(doc, loaded);
}

/// Guides are additive too: a file written before they existed had none, and
/// loading it must not invent any.
#[test]
fn a_saved_file_without_guides_loads_with_none() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        value
            .as_object_mut()
            .and_then(|o| o.remove("guides"))
            .is_some(),
        "the list is written at the top level, not per node"
    );

    let old = serde_json::to_vec(&value).unwrap();
    let loaded = io::load(&old).expect("a file without guides must still load");
    assert!(loaded.guides().is_empty());
    assert_eq!(doc, loaded, "and reconstruct the same document");
}

/// Guides round-trip, keep their ids, and reserve those ids against the id
/// source — otherwise the next guide drawn in a reopened file collides with one
/// already in it.
#[test]
fn guides_round_trip_and_reserve_their_ids() {
    let (mut doc, _root) = rich_document();
    let mut ids = IdSource::new(0xABCDEF); // the actor `rich_document` uses
    ondin_core::reserve_existing_ids(&doc, &mut ids);

    let recoloured = Color::from_rgba8(0x2d, 0x7a, 0xa3, 255);
    let horizontal = Guide {
        id: GuideId(ids.mint()),
        axis: GuideAxis::Horizontal,
        position: 332.0,
        color: None,
        owner: None,
    };
    let vertical = Guide {
        id: GuideId(ids.mint()),
        axis: GuideAxis::Vertical,
        position: -48.25,
        color: Some(recoloured),
        owner: None,
    };
    doc.apply(&Transaction(vec![
        Operation::AddGuide { guide: horizontal },
        Operation::AddGuide { guide: vertical },
    ]))
    .unwrap();

    let loaded = io::load(&io::save(&doc).unwrap()).unwrap();
    assert_eq!(loaded.guide(horizontal.id), Some(&horizontal));
    assert_eq!(loaded.guide(vertical.id), Some(&vertical));
    assert_eq!(doc, loaded);

    // A fresh session over the loaded file must mint past both guides.
    let mut fresh = IdSource::new(0xABCDEF);
    ondin_core::reserve_existing_ids(&loaded, &mut fresh);
    let next = fresh.mint();
    assert!(
        next.seq > vertical.id.0.seq,
        "next mint {next:?} collides with an existing guide"
    );
}

/// Save is byte-stable whatever order the guides were drawn in (invariant 9).
#[test]
fn guide_order_does_not_change_the_bytes() {
    let (base, _root) = rich_document();
    let mut ids = IdSource::new(0xABCDEF);
    ondin_core::reserve_existing_ids(&base, &mut ids);
    let a = Guide {
        id: GuideId(ids.mint()),
        axis: GuideAxis::Horizontal,
        position: 10.0,
        color: None,
        owner: None,
    };
    let b = Guide {
        id: GuideId(ids.mint()),
        axis: GuideAxis::Vertical,
        position: 20.0,
        color: None,
        owner: None,
    };

    let saved = |first: Guide, second: Guide| {
        let mut doc = base.clone();
        doc.apply(&Transaction(vec![
            Operation::AddGuide { guide: first },
            Operation::AddGuide { guide: second },
        ]))
        .unwrap();
        io::save(&doc).unwrap()
    };
    assert_eq!(saved(a, b), saved(b, a));
}

/// A hand-edited file with a NaN guide is rejected rather than loaded into a
/// line that no hit test can ever match.
#[test]
fn a_guide_with_a_non_finite_position_is_rejected() {
    let (doc, _root) = rich_document();
    let mut value: serde_json::Value = serde_json::from_slice(&io::save(&doc).unwrap()).unwrap();
    value["guides"] = serde_json::json!([{
        "id": "abcdef:900",
        "axis": "horizontal",
        "position": "NaN",
    }]);
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(io::load(&bytes).is_err());
}

/// The pivot is additive in the strongest sense: it is `skip_serializing_if`, so a
/// document nobody has moved a pivot in saves the **same bytes it always did**.
/// That is what keeps the field out of every existing file rather than merely
/// defaulting correctly in one.
#[test]
fn a_document_with_no_moved_pivot_writes_no_pivot_field() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        !text.contains("pivot"),
        "an untouched pivot must not reach the file:\n{text}"
    );
}

/// And once one *is* moved it survives the round trip — both variants, because
/// which one a kind gets is a rule the loader must not quietly re-decide.
#[test]
fn a_moved_pivot_round_trips_in_both_variants() {
    use ondin_core::Pivot;
    use ondin_core::kurbo::Vec2;

    let (mut doc, root) = rich_document();
    // A rect has an authored box, so its pivot is a fraction of it; a group's is
    // its children's union, so its pivot is an absolute local point.
    let (mut rect, mut group) = (None, None);
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let node = doc.get(id).unwrap();
        match node.kind() {
            NodeKind::Rect { .. } if rect.is_none() => rect = Some(id),
            NodeKind::Group if group.is_none() => group = Some(id),
            _ => {}
        }
        stack.extend(node.children().iter().copied());
    }
    let (rect, group) = (rect.expect("a rect"), group.expect("a group"));

    doc.apply(&Transaction(vec![
        Operation::SetPivot {
            id: rect,
            pivot: Some(Pivot::Normalized(Vec2::new(0.0, 1.0))),
        },
        Operation::SetPivot {
            id: group,
            pivot: Some(Pivot::Local(Point::new(-7.5, 12.25))),
        },
    ]))
    .unwrap();

    let loaded = io::load(&io::save(&doc).unwrap()).expect("a file with pivots must load");
    assert_eq!(doc, loaded);
    assert_eq!(
        loaded.get(rect).unwrap().pivot(),
        Some(Pivot::Normalized(Vec2::new(0.0, 1.0)))
    );
    assert_eq!(
        loaded.get(group).unwrap().pivot(),
        Some(Pivot::Local(Point::new(-7.5, 12.25)))
    );
    // And it is undoable like any other field.
    let inverse = doc
        .apply(&Transaction(vec![Operation::SetPivot {
            id: rect,
            pivot: None,
        }]))
        .unwrap()
        .inverse;
    assert_eq!(doc.get(rect).unwrap().pivot(), None);
    doc.apply(&inverse).unwrap();
    assert_eq!(
        doc.get(rect).unwrap().pivot(),
        Some(Pivot::Normalized(Vec2::new(0.0, 1.0))),
        "undo must put the pivot back"
    );
}

/// `sides` and `dash_fit` are additive in the strongest form 5.11 allows:
/// `skip_serializing_if`, so a stroke on every side and a pattern that is not
/// fitted never reach the file at all. Adding the fields therefore changed no
/// existing document's bytes, which is invariant 9 read forwards.
///
/// `miter_limit` is the weaker kind — it is always written, because there is no
/// "absent" reading of a ratio that is not 4 — so this only asserts the two that
/// claim to vanish.
#[test]
fn a_stroke_on_every_side_writes_no_sides_field() {
    let (doc, _root) = rich_document();
    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    assert!(
        !text.contains("sides"),
        "a stroke on the whole outline must not reach the file:\n{text}"
    );
    assert!(
        !text.contains("dash_fit"),
        "an unfitted dash pattern must not reach the file:\n{text}"
    );
}

/// A file written before per-side strokes, the mitre limit and dash fitting
/// existed has to load as what it drew: the whole outline, kurbo's own mitre
/// ratio of 4, and no fitting.
#[test]
fn a_saved_file_without_the_new_stroke_fields_loads_as_it_was_drawn() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();

    // Strip every field a pre-existing file would not have carried.
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut stripped = 0;
    for node in value["nodes"].as_array_mut().expect("nodes array") {
        if let Some(strokes) = node["paint"]["strokes"].as_array_mut() {
            for s in strokes {
                let Some(o) = s.as_object_mut() else { continue };
                for key in ["sides", "miter_limit", "dash_fit"] {
                    if o.remove(key).is_some() {
                        stripped += 1;
                    }
                }
            }
        }
    }
    assert!(stripped > 0, "fixture must contain a stroke to strip");

    let old = serde_json::to_vec(&value).unwrap();
    let loaded = io::load(&old).expect("a file without the new fields must load");
    // Equality covers it: the fixture's strokes are all whole-outline, unfitted
    // and at the default ratio, so a document that compares equal is one whose
    // strokes defaulted back to exactly that.
    assert_eq!(doc, loaded, "and reconstruct the same document");
}

/// And once set they survive the round trip, including the per-side widths, which
/// live inside the enum variant rather than beside it.
#[test]
fn per_side_widths_and_a_mitre_limit_round_trip() {
    use ondin_core::{Operation, StrokeSides, Transaction};

    let (mut doc, root) = rich_document();
    let mut painted = None;
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let node = doc.get(id).expect("node");
        if !node.paint().strokes.is_empty() {
            painted = Some(id);
            break;
        }
        stack.extend(node.children().iter().copied());
    }
    let id = painted.expect("fixture must contain a stroked node");

    let mut strokes = doc.get(id).unwrap().paint().strokes.clone();
    strokes[0].sides = StrokeSides::Custom {
        top: 1.5,
        right: 0.0,
        bottom: 4.0,
        left: 2.0,
    };
    strokes[0].miter_limit = 1.8;
    strokes[0].dash_fit = true;
    strokes[0].dashes = vec![0.0, 6.0];
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id,
        strokes: strokes.clone(),
    }]))
    .expect("set strokes");

    let bytes = io::save(&doc).unwrap();
    let loaded = io::load(&bytes).expect("reload");
    assert_eq!(loaded.get(id).unwrap().paint().strokes, strokes);
    // Byte-stable re-save, as invariant 9 requires of every field.
    assert_eq!(io::save(&loaded).unwrap(), bytes);
}

// --- v2 → v3: the text attribute model -------------------------------------

/// Find the one text node in a loaded fixture.
fn text_node(doc: &Document) -> &ondin_core::Node {
    let board = doc.get(doc.root()).unwrap().children()[0];
    doc.get(board)
        .unwrap()
        .children()
        .iter()
        .map(|id| doc.get(*id).unwrap())
        .find(|n| matches!(n.kind(), NodeKind::Text { .. }))
        .expect("the fixture holds one text node")
}

/// Rewind a saved document to the v2 text shape: a bare `line_height` multiplier
/// and an `align` inside the character defaults.
fn as_v2_text(doc: &Document, align: &str, line_height: f64) -> Vec<u8> {
    let bytes = io::save(doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["schema_version"] = serde_json::json!(2);
    let mut rewound = 0;
    for node in value["nodes"].as_array_mut().unwrap() {
        let Some(text) = node.get_mut("kind").and_then(|k| k.get_mut("Text")) else {
            continue;
        };
        let obj = text.as_object_mut().unwrap();
        obj.remove("spans");
        obj.remove("paragraph");
        obj.remove("block");
        let style = obj.get_mut("style").unwrap().as_object_mut().unwrap();
        style.insert("line_height".into(), serde_json::json!(line_height));
        style.insert("align".into(), serde_json::json!(align));
        rewound += 1;
    }
    assert_eq!(rewound, 1, "the fixture should hold exactly one text node");
    serde_json::to_vec(&value).unwrap()
}

/// A v2 file's bare multiplier meant "this many times the font size", which is
/// exactly `Length::Em`. Left to serde the key would be dropped and the field
/// defaulted to `None` — silently retyping every document ever saved to the
/// font's own leading.
#[test]
fn migration_reads_a_v2_line_height_as_a_multiple_of_the_font_size() {
    let (doc, _root) = rich_document();
    let loaded = io::load(&as_v2_text(&doc, "Left", 1.4)).unwrap();
    assert_eq!(loaded.schema_version(), io::CURRENT_SCHEMA_VERSION);
    let NodeKind::Text { style, .. } = text_node(&loaded).kind() else {
        unreachable!()
    };
    assert_eq!(style.line_height, Some(ondin_core::Length::Em(1.4)));
}

/// `align` moved from the character defaults to the paragraph scope, and the
/// **physical** value is preserved rather than mapped to `Start`. An old file said
/// *left*; turning that into "the start edge, whichever that is" would flip the
/// alignment of the first RTL document anyone opened.
#[test]
fn migration_moves_align_into_the_paragraph_scope_without_relabelling_it() {
    let (doc, _root) = rich_document();
    for (written, expected) in [
        ("Left", ondin_core::TextAlign::Left),
        ("Center", ondin_core::TextAlign::Center),
        ("Right", ondin_core::TextAlign::Right),
        ("Justify", ondin_core::TextAlign::Justify),
    ] {
        let loaded = io::load(&as_v2_text(&doc, written, 1.4)).unwrap();
        let NodeKind::Text { paragraph, .. } = text_node(&loaded).kind() else {
            unreachable!()
        };
        assert_eq!(paragraph.align, expected, "v2 {written:?}");
    }
}

/// Every attribute the new model added round-trips, and the re-save is
/// byte-identical (invariant 9).
#[test]
fn the_text_attributes_round_trip_defaults_spans_and_all_three_scopes() {
    let (mut doc, _root) = rich_document();
    let id = text_node(&doc).id();
    let style = ondin_core::TextStyle {
        font_family: "Inter".into(),
        font_size: 18.0,
        weight: 650,
        italic: true,
        variations: vec![ondin_core::AxisSetting::new(
            ondin_core::Tag::parse("wdth").unwrap(),
            87.5,
        )],
        features: vec![ondin_core::FeatureSetting {
            tag: ondin_core::Tag::parse("tnum").unwrap(),
            value: 1,
        }],
        locale: Some("tr".into()),
        line_height: Some(ondin_core::Length::Px(22.0)),
        letter_spacing: ondin_core::Length::Em(0.02),
        word_spacing: ondin_core::Length::Px(1.5),
        underline: Some(ondin_core::Decoration {
            thickness: Some(ondin_core::Length::Px(2.0)),
            offset: Some(ondin_core::Length::Em(-0.1)),
            style: ondin_core::LineStyle::Wavy,
            color: Some(Color::from_rgba8(200, 30, 30, 255)),
            // The non-default, so the round trip covers the state that is actually
            // written: `skip_ink` is on unless somebody switched it off, and
            // `skip_serializing_if` means `true` puts nothing in the file to
            // round-trip. The strikethrough below keeps the default.
            skip_ink: false,
        }),
        strikethrough: Some(ondin_core::Decoration::default()),
        case: ondin_core::TextCase::Upper,
        baseline_shift: ondin_core::Length::Px(-3.0),
        color: Some(Color::from_rgba8(30, 90, 200, 255)),
    };
    let mut spans = ondin_core::CharSpans::default();
    spans.set(0..3, ondin_core::CharAttr::Weight(900), &style);
    spans.set(
        1..4,
        ondin_core::CharAttr::LetterSpacing(ondin_core::Length::Em(0.1)),
        &style,
    );
    let paragraph = ondin_core::ParagraphStyle {
        align: ondin_core::TextAlign::Justify,
        spacing: ondin_core::Length::Px(12.0),
        indent: ondin_core::Length::Em(1.5),
        hanging: true,
        indent_start: ondin_core::Length::Px(24.0),
        indent_end: ondin_core::Length::Em(0.75),
        justify_last: ondin_core::JustifyLast::Center,
        wrap: ondin_core::WrapMode::NoWrap,
        word_break: ondin_core::WordBreak::BreakAll,
        overflow_wrap: ondin_core::OverflowWrap::Anywhere,
        direction: ondin_core::TextDirection::Rtl,
        marker: Some(ondin_core::ListMarker::UpperRoman),
        level: 3,
        optical_margins: true,
    };
    // The paragraph *overrides*, against those defaults — "all three scopes" is
    // four span-able lists now, and the one added last is the one a round-trip
    // test forgets.
    let mut para_spans = ondin_core::ParaSpans::default();
    para_spans.set(
        0..5,
        ondin_core::ParaAttr::IndentStart(ondin_core::Length::Px(48.0)),
        &paragraph,
    );
    para_spans.set(2..5, ondin_core::ParaAttr::Hanging(false), &paragraph);
    // **`None` is a value, so a span that turns a marker *off* has to survive the
    // round trip too** — it is the only way one paragraph opts out of a node-level
    // marker, and `skip_serializing_if = "Option::is_none"` is on the *defaults*,
    // where the absence means the same thing. A span's is not skippable.
    para_spans.set(0..2, ondin_core::ParaAttr::Marker(None), &paragraph);
    // And a nesting level, which is the newest of the seven.
    para_spans.set(2..5, ondin_core::ParaAttr::Level(1), &paragraph);
    let block = ondin_core::BlockStyle {
        vertical_align: ondin_core::VerticalAlign::Middle,
        trim: ondin_core::BoxTrim::CapToBaseline,
        overflow: ondin_core::TextOverflow::Ellipsis,
        max_lines: 3,
    };
    doc.apply(&Transaction(vec![
        Operation::SetTextStyle {
            id,
            style: style.clone(),
            spans: None,
        },
        Operation::SetTextSpans {
            id,
            spans: spans.clone(),
        },
        Operation::SetParagraphStyle {
            id,
            paragraph: paragraph.clone(),
            spans: None,
        },
        Operation::SetParagraphSpans {
            id,
            spans: para_spans.clone(),
        },
        Operation::SetBlockStyle { id, block },
    ]))
    .expect("set every text scope");

    let bytes = io::save(&doc).unwrap();
    let loaded = io::load(&bytes).expect("reload");
    let NodeKind::Text {
        style: s,
        spans: sp,
        para_spans: psp,
        paragraph: p,
        block: b,
        ..
    } = loaded.get(id).unwrap().kind()
    else {
        unreachable!()
    };
    assert_eq!(**s, style);
    assert_eq!(*sp, spans);
    assert_eq!(*psp, para_spans);
    assert_eq!(*p, paragraph);
    assert_eq!(*b, block);
    assert_eq!(io::save(&loaded).unwrap(), bytes, "byte-stable re-save");
}

/// **A unit chosen at amount zero survives the file**, which is the test
/// `[S6.2-L1-03]` asks for and which nothing had (§15 D603).
///
/// A designer who works in percent sets the unit before the number, so one click
/// on the `%` chip at the stock `Px(0.0)` is an ordinary first move. The finding
/// measured that click costing an undo step and then being *silently reverted by
/// the next save*: reopen the file and the chip reads `px` again.
///
/// ⚠️ **That half no longer reproduces, and this test is the proof rather than a
/// new fix.** `[S6.1-L1-03]`'s core repair moved all seven of these fields from
/// `skip_serializing_if = "Length::is_zero"` — which elides `Em(0.0)` and
/// `Px(0.0)` alike — to `Length::is_default_zero`, which elides only the `Px(0.0)`
/// an absent key reads back as. The unit is a value now, at zero as at any other
/// amount, and this is the guard that says so.
///
/// **All seven fields, because they were changed together and would regress
/// together.** Three on `TextStyle` and four on `ParagraphStyle`.
///
/// Flip-check, run: any one of the seven back to `Length::is_zero` fails here on
/// that field's assertion, `Px(0.0)` against `Em(0.0)` — the key is simply not in
/// the file and `#[serde(default)]` supplies the `Px` zero.
///
/// Plain backticks throughout: this is a `crates/*/tests/` file, which `cargo doc`
/// documents no target for, so a link here is checked by nothing.
#[test]
fn a_unit_chosen_at_zero_is_not_thrown_away_by_the_next_save() {
    let (mut doc, _root) = rich_document();
    let id = text_node(&doc).id();
    let zero = ondin_core::Length::Em(0.0);
    let style = ondin_core::TextStyle {
        letter_spacing: zero,
        word_spacing: zero,
        baseline_shift: zero,
        ..ondin_core::TextStyle::default()
    };
    let paragraph = ondin_core::ParagraphStyle {
        spacing: zero,
        indent: zero,
        indent_start: zero,
        indent_end: zero,
        ..ondin_core::ParagraphStyle::default()
    };
    doc.apply(&Transaction(vec![
        Operation::SetTextStyle {
            id,
            style: style.clone(),
            spans: None,
        },
        Operation::SetParagraphStyle {
            id,
            paragraph: paragraph.clone(),
            spans: None,
        },
    ]))
    .expect("set the two scopes");

    // The fixture has to actually be in the state the test is about, or the
    // assertions below are about nothing: `Em(0.0)` is only interesting because
    // it is a *different value* from the `Px(0.0)` an absent key reads back as.
    assert_ne!(zero, ondin_core::Length::Px(0.0));

    let loaded = io::load(&io::save(&doc).unwrap()).expect("reload");
    let NodeKind::Text {
        style: s,
        paragraph: p,
        ..
    } = loaded.get(id).unwrap().kind()
    else {
        unreachable!()
    };
    for (got, name) in [
        (s.letter_spacing, "letter_spacing"),
        (s.word_spacing, "word_spacing"),
        (s.baseline_shift, "baseline_shift"),
        (p.spacing, "paragraph spacing"),
        (p.indent, "indent"),
        (p.indent_start, "indent_start"),
        (p.indent_end, "indent_end"),
    ] {
        assert_eq!(got, zero, "{name} lost the unit the user chose");
    }
}

/// A node with no overrides and every scope on its default writes none of the
/// optional keys — the economy `skip_serializing_if` is there for, and what keeps
/// the file from growing by three objects per text node.
#[test]
fn a_uniform_text_node_writes_no_spans_paragraph_or_block() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    for key in [
        "\"spans\"",
        "\"para_spans\"",
        "\"paragraph\"",
        "\"block\"",
        "\"case\"",
    ] {
        assert!(
            !text.contains(key),
            "a default text node should not write {key}"
        );
    }
}

/// A hand-edited file whose spans outrun its content loads with them clamped
/// rather than refusing to open: a stale range is a rounding error, not a corrupt
/// tree, and clamping loses nothing a designer would notice.
#[test]
fn a_span_reaching_past_the_content_is_clamped_on_load() {
    let (mut doc, _root) = rich_document();
    let id = text_node(&doc).id();
    let NodeKind::Text { style, .. } = doc.get(id).unwrap().kind().clone() else {
        unreachable!()
    };
    let mut spans = ondin_core::CharSpans::default();
    spans.set(0..3, ondin_core::CharAttr::Weight(900), &style);
    doc.apply(&Transaction(vec![Operation::SetTextSpans { id, spans }]))
        .unwrap();

    let bytes = io::save(&doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for node in value["nodes"].as_array_mut().unwrap() {
        if let Some(text) = node.get_mut("kind").and_then(|k| k.get_mut("Text"))
            && let Some(spans) = text.get_mut("spans").and_then(|s| s.as_array_mut())
        {
            spans[0]["end"] = serde_json::json!(9999);
        }
    }
    let loaded = io::load(&serde_json::to_vec(&value).unwrap()).expect("loads anyway");
    let NodeKind::Text { content, spans, .. } = loaded.get(id).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(spans.as_slice()[0].end, content.len());
}

/// **A path's corner radii save, and their absence is what an old file means.**
///
/// The field is `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, which
/// is what lets it be purely additive rather than a schema bump: a file written
/// before radii existed has no key, an unrounded path still writes none, and
/// "no key" and "no radii" are the same thing — so every existing file stays
/// byte-identical (invariant 9) and loads to exactly what it meant.
#[test]
fn path_corner_radii_survive_a_save_and_an_old_file_still_loads() {
    let mut ids = IdSource::new(9);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let id = ids.mint();
    let mut p = BezPath::new();
    p.move_to(Point::ZERO);
    p.line_to(Point::new(40.0, 0.0));
    p.line_to(Point::new(40.0, 40.0));
    p.close_path();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id,
        parent: root,
        index: 0,
        kind: NodeKind::Path {
            path: p,
            corner_radii: vec![0.0, 6.0],
        },
        transform: None,
        name: None,
    }]))
    .unwrap();

    let bytes = io::save(&doc).unwrap();
    assert_eq!(io::load(&bytes).unwrap(), doc, "the radii came back");

    // An unrounded path writes no key at all, so nothing already on disk grows
    // one — the whole reason this is additive.
    // Per *path node*, not by substring: `Rect` has a `corner_radii` of its own
    // and always writes it, so a whole-file grep answers the wrong question.
    let path_kind = |doc: &Document| -> serde_json::Value {
        let bytes = io::save(doc).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|n| n["kind"].get("Path").cloned())
            .expect("a path node")
    };
    assert!(
        path_kind(&doc).get("corner_radii").is_some(),
        "a rounded path stores them"
    );
    let (plain, _) = rich_document();
    assert!(
        path_kind(&plain).get("corner_radii").is_none(),
        "an unrounded path must add nothing to the file"
    );

    // And a file that predates the field: strip the key and it still loads, as
    // the unrounded path it always was.
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for node in value["nodes"].as_array_mut().unwrap() {
        if let Some(kind) = node.get_mut("kind").and_then(|k| k.as_object_mut())
            && let Some(path) = kind.get_mut("Path").and_then(|p| p.as_object_mut())
        {
            path.remove("corner_radii");
        }
    }
    let old = serde_json::to_vec(&value).unwrap();
    let loaded = io::load(&old).expect("a file without the field must still load");
    let node = loaded.get(id).unwrap();
    assert!(
        matches!(node.kind(), NodeKind::Path { corner_radii, .. } if corner_radii.is_empty()),
        "absent means unrounded"
    );
}

/// A guide's `owner` is additive in the strongest sense — `skip_serializing_if`,
/// like the pivot — so a document with only canvas-wide guides writes the bytes
/// it always did and the field reaches no existing file (invariant 9).
#[test]
fn a_document_with_no_scoped_guide_writes_no_owner_field() {
    let (mut doc, _root) = rich_document();
    let mut ids = IdSource::new(0xABCDEF);
    ondin_core::reserve_existing_ids(&doc, &mut ids);
    doc.apply(&Transaction(vec![Operation::AddGuide {
        guide: Guide {
            id: GuideId(ids.mint()),
            axis: GuideAxis::Horizontal,
            position: 12.0,
            color: None,
            owner: None,
        },
    }]))
    .unwrap();
    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    assert!(
        !text.contains("owner"),
        "a canvas-wide guide must not carry an owner key:\n{text}"
    );
}

/// A frame-scoped guide round-trips with its owner, and the owner is written as
/// the same wire id a node reference uses.
#[test]
fn a_frame_scoped_guide_round_trips_with_its_owner() {
    let (mut doc, root) = rich_document();
    let frame = *doc
        .get(root)
        .unwrap()
        .children()
        .iter()
        .find(|c| {
            matches!(
                doc.get(**c).map(|n| n.kind()),
                Some(NodeKind::Artboard { .. })
            )
        })
        .expect("rich_document has a frame at the root");
    let mut ids = IdSource::new(0xABCDEF);
    ondin_core::reserve_existing_ids(&doc, &mut ids);

    let scoped = Guide {
        id: GuideId(ids.mint()),
        axis: GuideAxis::Vertical,
        // A local offset, not a world coordinate — the distinction the field
        // exists to make.
        position: 120.0,
        color: None,
        owner: Some(frame),
    };
    doc.apply(&Transaction(vec![Operation::AddGuide { guide: scoped }]))
        .unwrap();

    let bytes = io::save(&doc).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        text.contains("owner"),
        "the scope must reach the file:\n{text}"
    );
    let loaded = io::load(&bytes).unwrap();
    assert_eq!(loaded.guide(scoped.id), Some(&scoped));
    assert_eq!(doc, loaded);
}

/// A hand-edited file whose guide is scoped to something that is not a frame in
/// it is **rejected**, not quietly promoted to a canvas-wide guide: a scoped
/// guide's position is read in its owner's space, so promoting it would move the
/// line to a coordinate nobody chose.
#[test]
fn a_guide_scoped_to_a_missing_or_non_frame_owner_is_rejected() {
    let (doc, root) = rich_document();
    let base: serde_json::Value = serde_json::from_slice(&io::save(&doc).unwrap()).unwrap();
    // A node that is in the document but is not a frame — the group inside the
    // board.
    let frame = doc.get(root).unwrap().children()[0];
    let group = doc.get(frame).unwrap().children()[0];

    for owner in ["abcdef:9999", &group.to_wire()] {
        let mut value = base.clone();
        value["guides"] = serde_json::json!([{
            "id": "abcdef:900",
            "axis": "horizontal",
            "position": 10.0,
            "owner": owner,
        }]);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(
            io::load(&bytes).is_err(),
            "a guide scoped to {owner} loaded without complaint"
        );
    }

    // And the frame itself still loads, so the check is about the owner's kind
    // rather than about the key being present.
    let mut value = base.clone();
    value["guides"] = serde_json::json!([{
        "id": "abcdef:900",
        "axis": "horizontal",
        "position": 10.0,
        "owner": frame.to_wire(),
    }]);
    let bytes = serde_json::to_vec(&value).unwrap();
    let loaded = io::load(&bytes).expect("a guide on a real frame must load");
    assert_eq!(loaded.guides()[0].owner, Some(frame));
}

// --- the image table on disk (§5.5a) ----------------------------------------

fn with_images() -> Document {
    let (mut doc, _root) = rich_document();
    doc.apply(&Transaction(vec![
        Operation::AddImage {
            // Deliberately out of sorted order against the second one, so the
            // sort on save is doing something.
            id: ImageId("sha256:ffff".into()),
            entry: ImageEntry {
                // A zero byte and a 0xFF, because the point of base64 here is
                // that the file is text and these are what text cannot hold.
                source: ImageSource::Embedded(vec![0xFF, 0xD8, 0x00, 0x01, 0x80].into()),
                format: ImageFormat::Jpeg,
                width: 2000,
                height: 1500,
            },
        },
        Operation::AddImage {
            id: ImageId("sha256:0001".into()),
            entry: ImageEntry {
                source: ImageSource::Linked(r"C:\photos\hero.png".into()),
                format: ImageFormat::Png,
                width: 800,
                height: 600,
            },
        },
    ]))
    .unwrap();
    doc
}

#[test]
fn an_image_table_round_trips_and_stays_byte_stable() {
    let doc = with_images();
    let bytes1 = io::save(&doc).unwrap();
    let loaded = io::load(&bytes1).unwrap();
    assert_eq!(doc, loaded, "the table must survive the round trip exactly");
    let bytes2 = io::save(&loaded).unwrap();
    assert_eq!(
        bytes1, bytes2,
        "a `FxHashMap` has no order of its own, so the sort on save is the whole \
         of invariant 9 for the table"
    );
}

/// **The bytes are base64, the table is last, and neither is cosmetic.**
///
/// `Vec<u8>` through serde_json is an array of numbers — four to six text bytes
/// per byte, one per line when pretty-printed — which is the difference between
/// a photograph costing 1.33× in the file and costing twenty times that across a
/// hundred thousand lines. And the table sits below the tree so that a geometry
/// edit produces a diff nowhere near it.
#[test]
fn embedded_bytes_are_base64_and_the_table_is_written_last() {
    let bytes = io::save(&with_images()).unwrap();
    let text = String::from_utf8(bytes).unwrap();

    // 0xFF 0xD8 0x00 0x01 0x80 in base64, on one line.
    assert!(
        text.contains(r#""data": "/9gAAYA=""#),
        "expected the encoded bytes as one base64 string:\n{text}"
    );
    assert!(
        !text.contains("255,"),
        "an array of numbers means serde's own `Vec<u8>` shape got out"
    );

    let nodes_at = text.find(r#""nodes""#).expect("nodes are written");
    let images_at = text.find(r#""images""#).expect("images are written");
    assert!(
        nodes_at < images_at,
        "the tree must come first: the image lines are the ones that never change"
    );

    // Sorted by id, which is what makes the bytes stable.
    let first = text.find("sha256:0001").expect("the linked image");
    let second = text.find("sha256:ffff").expect("the embedded image");
    assert!(first < second, "the table is written sorted by id");
}

/// **A document with no images is written exactly as it was before the field
/// existed.**
///
/// The `skip_serializing_if` half of `DocumentDto::images`, and the same
/// guarantee `GuideDto::owner` documents: adding a table must not rewrite every
/// existing file. This is the assertion that would fail if someone dropped the
/// attribute for tidiness.
#[test]
fn a_document_with_no_images_writes_no_images_key() {
    let (doc, _root) = rich_document();
    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    assert!(
        !text.contains("images"),
        "an imageless document grew a key:\n{text}"
    );
    // And a file written before the field existed still loads, which is the
    // other half of the same claim.
    io::load(text.as_bytes()).expect("an imageless file must load");
}

/// **A fill pointing at an image the table does not carry still loads.**
///
/// A linked file that has moved is a state every image feature has to survive —
/// it draws the placeholder and is repaired by the popover's *Replace…* (§15
/// D179; *Relink*, which repoints the path instead of embedding, is still owed)
/// — so refusing the document would turn a picture that needs repairing into a
/// file that cannot be opened. The dangling reference is deliberately not an
/// integrity error, and the reference surviving is what the repair aims at.
#[test]
fn a_dangling_image_reference_loads_rather_than_failing() {
    let (mut doc, root) = rich_document();
    // Frame → group → the rectangle. Walked rather than assumed, and *asserted*:
    // a fixture that landed on the group instead would fail `SetFills` with
    // `WrongKindForOp` and say nothing about images.
    let frame = doc.get(root).unwrap().children()[0];
    let group = doc.get(frame).unwrap().children()[0];
    let rect = doc.get(group).unwrap().children()[0];
    assert!(
        matches!(doc.get(rect).unwrap().kind(), NodeKind::Rect { .. }),
        "the fixture must reach a shape that can carry a fill"
    );
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: rect,
        fills: vec![Fill {
            brush: image_brush(ImageId("sha256:gone".into())),
            visible: true,
        }],
    }]))
    .unwrap();

    let bytes = io::save(&doc).unwrap();
    let loaded = io::load(&bytes).expect("a dangling reference must not fail the load");
    assert_eq!(loaded.image_count(), 0);
    assert_eq!(
        loaded.get(rect).unwrap().paint().fills[0].brush,
        image_brush(ImageId("sha256:gone".into())),
        "and the reference survives, so a relink has something to repair"
    );
}

/// **A malformed table is rejected**, which is the other side of that: a
/// reference to nothing is a repairable document, and bytes that are not bytes
/// are not.
#[test]
fn bad_base64_and_an_empty_intrinsic_size_are_integrity_errors() {
    let good = io::save(&with_images()).unwrap();
    let base: serde_json::Value = serde_json::from_slice(&good).unwrap();
    // Sorted by id, so `[1]` is `sha256:ffff` — the *embedded* one. Asserted,
    // because writing to the wrong entry inserts an `Embedded` key beside the
    // `Linked` one and fails as a malformed enum, which would pass this test for
    // entirely the wrong reason.
    assert!(
        base["images"][1]["source"]["Embedded"]["data"].is_string(),
        "expected the embedded entry second:\n{base:#}"
    );

    let mut broken = base.clone();
    broken["images"][1]["source"]["Embedded"]["data"] = serde_json::json!("not base64!!");
    let err = io::load(&serde_json::to_vec(&broken).unwrap());
    assert!(matches!(err, Err(IoError::Integrity(_))), "{err:?}");

    let mut zero = base;
    zero["images"][0]["width"] = serde_json::json!(0);
    let err = io::load(&serde_json::to_vec(&zero).unwrap());
    assert!(matches!(err, Err(IoError::Integrity(_))), "{err:?}");
}

/// Export settings are the point of the feature only if they survive the file
/// (§7) — a list that had to be rebuilt on every open would be the dialog again
/// with extra steps. Round trip, undo, and the invariant-9 half: a document that
/// exports nothing must write exactly the bytes it wrote before the field
/// existed.
#[test]
fn export_settings_round_trip_and_cost_nothing_when_empty() {
    use ondin_core::{ExportBackground, ExportFormat, ExportScale, ExportSpec};

    let (mut doc, root) = rich_document();
    let plain = io::save(&doc).unwrap();
    assert!(
        !String::from_utf8(plain.clone())
            .unwrap()
            .contains("exports"),
        "a document nobody has set up to export must not carry the key at all"
    );

    let rect = *doc.get(root).unwrap().children().first().expect("a child");
    let specs = vec![
        ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0)),
        ExportSpec {
            background: ExportBackground::Frame,
            trim: true,
            ..ExportSpec::new(ExportFormat::Jpeg, ExportScale::Width(512))
        },
    ];
    let inverse = doc
        .apply(&Transaction(vec![Operation::SetExports {
            id: rect,
            exports: specs.clone(),
        }]))
        .unwrap()
        .inverse;

    let loaded = io::load(&io::save(&doc).unwrap()).expect("a file with exports must load");
    assert_eq!(doc, loaded);
    assert_eq!(loaded.get(rect).unwrap().exports(), specs.as_slice());

    doc.apply(&inverse).unwrap();
    assert!(
        doc.get(rect).unwrap().exports().is_empty(),
        "undo must take the list back off"
    );
    assert_eq!(
        io::save(&doc).unwrap(),
        plain,
        "and the bytes must come back to what they were, not to something equivalent"
    );
}

/// A layout grid survives the file, and a document with none writes the bytes it
/// always did (invariant 9).
///
/// **The same three questions the export test above asks**, because the field is
/// the same shape — a `Vec` on the node that almost every layer leaves empty —
/// and the answers have to be the same three or the format has grown a cost for
/// documents that never used the feature.
///
/// ⚠️ **The `contains("grids")` assertion is the one that would rot quietly.**
/// It passes today for a reason that has nothing to do with the grid code:
/// `skip_serializing_if`. Drop that attribute and every `.ondin` ever written
/// gains a `"grids": []` on every node, which is invariant 9 broken for a feature
/// nobody in that document used — and no other test in this suite would notice,
/// because every one of them compares a document against itself.
#[test]
fn layout_grids_round_trip_and_cost_nothing_when_empty() {
    use ondin_core::{GridAlign, GridAxis, LayoutGrid};

    let (mut doc, root) = rich_document();
    let plain = io::save(&doc).unwrap();
    assert!(
        !String::from_utf8(plain.clone()).unwrap().contains("grids"),
        "a document with no layout grid must not carry the key at all"
    );

    let frame = *doc.get(root).unwrap().children().first().expect("a child");
    let grids = vec![
        LayoutGrid::new(GridAxis::Columns),
        LayoutGrid {
            align: GridAlign::Center,
            count: 3,
            visible: false,
            ..LayoutGrid::new(GridAxis::Rows)
        },
    ];
    let inverse = doc
        .apply(&Transaction(vec![Operation::SetLayoutGrids {
            id: frame,
            grids: grids.clone(),
        }]))
        .unwrap()
        .inverse;

    let loaded = io::load(&io::save(&doc).unwrap()).expect("a file with grids must load");
    assert_eq!(doc, loaded);
    assert_eq!(loaded.get(frame).unwrap().grids(), grids.as_slice());
    // ⚠️ **Named rather than left to `assert_eq!(doc, loaded)`**: the whole
    // document comparing equal proves the *list* survived, and would still pass
    // if `visible` or `align` were dropped on both sides of the trip by a DTO
    // that skipped them. Reading one field back off the second grid is what says
    // the fields inside the struct made it.
    assert!(!loaded.get(frame).unwrap().grids()[1].visible);

    doc.apply(&inverse).unwrap();
    assert!(
        doc.get(frame).unwrap().grids().is_empty(),
        "undo must take the grids back off"
    );
    assert_eq!(
        io::save(&doc).unwrap(),
        plain,
        "and the bytes must come back to what they were, not to something equivalent"
    );
}

/// `mask` is additive on the terms invariant 9 sets: a document with no mask in
/// it must save to the bytes it always did, key and all.
///
/// **Both directions are asserted against the same `plain`**, because the two
/// failures are different. A field serialized unconditionally would put
/// `"mask": false` on every node in every file ever written — caught by the first
/// assertion. A `skip_serializing_if` that also swallowed `true` would save a
/// mask and load it back as an ordinary layer — caught by the round trip, and by
/// nothing else, since the document would still equal itself.
#[test]
fn the_mask_flag_is_absent_from_a_document_that_has_none() {
    let (mut doc, root) = rich_document();
    let plain = io::save(&doc).unwrap();
    assert!(
        !String::from_utf8(plain.clone()).unwrap().contains("mask"),
        "a document with no mask must not carry the key at all"
    );

    let frame = doc.get(root).unwrap().children()[0];
    let group = doc.get(frame).unwrap().children()[0];
    let rect = doc.get(group).unwrap().children()[0];
    let inverse = doc
        .apply(&Transaction(vec![Operation::SetMask {
            id: rect,
            mask: true,
        }]))
        .unwrap()
        .inverse;

    let masked = io::save(&doc).unwrap();
    assert!(
        String::from_utf8(masked.clone()).unwrap().contains("mask"),
        "and must carry it once something is one"
    );
    let loaded = io::load(&masked).expect("a file with a mask must load");
    assert_eq!(doc, loaded);
    assert!(loaded.get(rect).unwrap().mask(), "and it is still the mask");

    doc.apply(&inverse).unwrap();
    assert_eq!(
        io::save(&doc).unwrap(),
        plain,
        "releasing it takes the key back out rather than writing a false"
    );
}

/// The mask **mode** is additive on the same terms: absent at its default, so a
/// document whose masks are all shape masks — which is every document written
/// before the mode existed — saves to the bytes it always did.
///
/// The round trip is the other half, and it is the one that would catch a
/// `skip_serializing_if` that swallowed `Alpha` too: the document would still
/// equal itself, and every alpha mask in every saved file would come back a hard
/// clip.
#[test]
fn the_mask_mode_is_absent_until_something_is_not_a_shape_mask() {
    use ondin_core::MaskMode;
    let (mut doc, root) = rich_document();
    let frame = doc.get(root).unwrap().children()[0];
    let group = doc.get(frame).unwrap().children()[0];
    let rect = doc.get(group).unwrap().children()[0];

    doc.apply(&Transaction(vec![Operation::SetMask {
        id: rect,
        mask: true,
    }]))
    .unwrap();
    let shape_masked = io::save(&doc).unwrap();
    assert!(
        !String::from_utf8(shape_masked.clone())
            .unwrap()
            .contains("mask_mode"),
        "a shape mask is the default and writes no mode key"
    );

    let inverse = doc
        .apply(&Transaction(vec![Operation::SetMaskMode {
            id: rect,
            mode: MaskMode::Alpha,
        }]))
        .unwrap()
        .inverse;
    let bytes = io::save(&doc).unwrap();
    assert!(
        String::from_utf8(bytes.clone()).unwrap().contains("Alpha"),
        "and carries it once one is not a shape mask"
    );
    let loaded = io::load(&bytes).expect("a file with an alpha mask must load");
    assert_eq!(doc, loaded);
    assert_eq!(
        loaded.get(rect).unwrap().mask_mode(),
        MaskMode::Alpha,
        "and it is still an alpha mask, not a clip"
    );

    doc.apply(&inverse).unwrap();
    assert_eq!(
        io::save(&doc).unwrap(),
        shape_masked,
        "back to the default takes the key out again"
    );
}

/// The **fill rule** is additive on exactly the same terms (§15 D239), and one
/// thing more: an `Exclude` boolean must write **no key at all**.
///
/// ⚠️ **That last part is the whole reason the rule is derived rather than stored.**
/// A symmetric difference is even-odd by definition, so `Node::fill_rule` answers
/// `EvenOdd` for one without the field being set — which means every `.ondin` written
/// before this existed is already correct, with no migration and no byte moving. Had
/// the DTO been written from that accessor instead of from the stored field, opening
/// and saving any document with an exclusion in it would have rewritten the file.
#[test]
fn the_fill_rule_is_absent_until_a_path_is_given_one_and_never_for_a_boolean() {
    use ondin_core::{BoolOp, FillRule};
    let (mut doc, root) = rich_document();
    let frame = doc.get(root).unwrap().children()[0];
    let group = doc.get(frame).unwrap().children()[0];
    let rect = doc.get(group).unwrap().children()[0];

    let plain = io::save(&doc).unwrap();
    assert!(
        !String::from_utf8(plain.clone())
            .unwrap()
            .contains("fill_rule"),
        "non-zero is the default and writes no key"
    );

    let inverse = doc
        .apply(&Transaction(vec![Operation::SetFillRule {
            id: rect,
            rule: FillRule::EvenOdd,
        }]))
        .unwrap()
        .inverse;
    let bytes = io::save(&doc).unwrap();
    assert!(
        String::from_utf8(bytes.clone())
            .unwrap()
            .contains("EvenOdd"),
        "and carries it once something is not non-zero"
    );
    let loaded = io::load(&bytes).expect("a file with an even-odd shape must load");
    assert_eq!(doc, loaded);
    assert_eq!(
        loaded.get(rect).unwrap().fill_rule(),
        FillRule::EvenOdd,
        "and it comes back even-odd rather than silently filling solid"
    );

    doc.apply(&inverse).unwrap();
    assert_eq!(
        io::save(&doc).unwrap(),
        plain,
        "back to the default takes the key out again"
    );

    // An `Exclude` reports even-odd and stores nothing, so the bytes do not move.
    let (tx, b) = ondin_core::build::boolean(
        &doc,
        &mut IdSource::new(0x0E0D),
        doc.get(group).unwrap().children(),
        BoolOp::Exclude,
        None,
    )
    .expect("a boolean over the group's children");
    doc.apply(&tx).unwrap();
    assert_eq!(
        doc.get(b).unwrap().fill_rule(),
        FillRule::EvenOdd,
        "an exclusion is even-odd by definition"
    );
    assert!(
        !String::from_utf8(io::save(&doc).unwrap())
            .unwrap()
            .contains("fill_rule"),
        "and stores nothing for it, which is what keeps existing files byte-identical"
    );
}

/// A mask flag on a kind that cannot honour it is dropped on load, the way an
/// inert `clip` is — a hand-edited or future file must not arrive with a frame
/// claiming to mask, because the scene walk would then have to decide whether it
/// paints.
#[test]
fn a_mask_on_a_frame_does_not_survive_being_loaded() {
    let (doc, root) = rich_document();
    let frame = doc.get(root).unwrap().children()[0];
    let wire = frame.to_wire();

    let mut value: serde_json::Value =
        serde_json::from_slice(&io::save(&doc).unwrap()).expect("saved json");
    let mut planted = false;
    for node in value["nodes"].as_array_mut().expect("nodes array") {
        let obj = node.as_object_mut().expect("node object");
        if obj["id"].as_str() == Some(wire.as_str()) {
            obj.insert("mask".into(), serde_json::Value::Bool(true));
            planted = true;
        }
    }
    assert!(planted, "the fixture's frame was found and written to");

    let loaded = io::load(&serde_json::to_vec(&value).unwrap()).expect("it still loads");
    assert!(
        !loaded.get(frame).unwrap().mask(),
        "the flag is kept inert on a kind that cannot be a mask"
    );
    assert_eq!(doc, loaded, "and nothing else about the file changed");
}

/// **An inert `clip` never reaches the file, so a document does not change on
/// disk the first time it is opened and saved** — §15 D663, `[S1.1-L1-06]`.
///
/// 🚨 **Two halves that are each right and were wrong together.** `op_set_clip`
/// deliberately accepts `clip: true` on a `Group` — §15 D282: *"an inert `clip`
/// is a switch nothing reads"*, so dragging a shape through a frame and back
/// keeps its setting, where `SetMask` refuses the same shape — and
/// `into_document` zeroes it on load. So the writer wrote a flag the reader threw
/// away: `load(save(d)) != d`, and `save(load(save(d))) != save(d)`, one byte
/// **longer** — `clip` carries a named serde default and no
/// `skip_serializing_if`, so the key is always written and `"clip":true` becomes
/// `"clip":false` — on **every** group carrying it, with no edit made. (⚠️ This
/// said *shorter* until `arch-scribe` read the DTO; the direction is not what
/// makes it a defect, which is why it went unchecked.) Neither §5.11 nor
/// D282 says the model therefore holds a state the format discards; each
/// documents its own end.
///
/// **Fixed at the writer, and the loader's normalization stays.** That one is the
/// guard against a hand-edited or foreign file and is what
/// `a_mask_on_a_frame_does_not_survive_being_loaded` above pins for the other
/// flag; this is the projection saying what the format's `clip` means, which is
/// *"clips its children"*. The model is untouched, so D282's property is
/// untouched — which is the first assertion below.
///
/// 🚨 **The finding's third fix option would have been a live bug and the entry
/// records it**: *"the loader stops normalizing and the readers keep asking the
/// kind — which they already do"*. They do not. `scene.rs`'s and `canvas.rs`'s
/// `bounds_cover_subtree` are `node.children().is_empty() || node.clip() ||
/// matches!(kind, Group | Root | Boolean)` — **not** kind-gated on the `clip`
/// term — so a group arriving with the flag live would have had its children
/// culled against its own bounds. `menu.rs`'s `clips:` row is the same shape.
/// Two of the three options were safe and the one that reads as cheapest was not.
///
/// ⚠️ **Flip run**, the writer's `&& n.kind().clips_children()` removed.
/// Predicted failing assertion: the byte-stability one. It fails one earlier, on
/// the **group's own `clip` record**, `Bool(true)` against `Bool(false)` — which
/// is the better site and is why that assertion was added: it names the flag
/// rather than the file.
#[test]
fn an_inert_clip_flag_never_reaches_the_file() {
    let (mut doc, root) = rich_document();
    let ab = doc.get(root).unwrap().children()[0];
    let group = doc.get(ab).unwrap().children()[0];
    assert!(
        matches!(doc.get(group).unwrap().kind(), NodeKind::Group),
        "fixture: the subject has to be a kind that cannot clip"
    );

    doc.apply(&Transaction(vec![Operation::SetClip {
        id: group,
        clip: true,
    }]))
    .expect("§15 D282: the operation accepts an inert clip on purpose");
    assert!(
        doc.get(group).unwrap().clip(),
        "the model still holds it — D282's 'drag it through a frame and back' \
         property is what this must not break"
    );

    let once = io::save(&doc).unwrap();
    // The loss first, and in a form a failure can print: the group's own record
    // is what must not carry the flag. ⚠️ **Read out of the node rather than
    // grepped for `"clip": true`** — the *artboard* clips, legitimately and by
    // default (`Document::apply`'s `clip: kind.clips_children()`), so the
    // substring is in every file this fixture produces and a grep for it asserts
    // nothing. ⚠️ **And `assert!` rather than `assert_eq!` on the buffers below**:
    // two byte vectors in a panic message are 53 KB of decimal that nobody reads,
    // which is what the first run of this test produced.
    let value: serde_json::Value = serde_json::from_slice(&once).expect("saved json");
    let wire = group.to_wire();
    let saved = value["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .find(|n| n["id"].as_str() == Some(wire.as_str()))
        .expect("the group is in the file");
    assert_eq!(
        saved["clip"],
        serde_json::Value::Bool(false),
        "a group cannot clip, so its record must not say it does"
    );
    let reloaded = io::load(&once).expect("it loads");
    assert!(
        !reloaded.get(group).unwrap().clip(),
        "and the file never carried it, so nothing is there to drop"
    );
    let twice = io::save(&reloaded).unwrap();
    assert_eq!(
        once.len(),
        twice.len(),
        "the re-save is a different length — `true` became `false`"
    );
    assert!(
        once == twice,
        "opening and saving with no edit made must not move a byte"
    );
}

/// The effect stack is additive on invariant 9's terms, exactly as `exports` is:
/// a document nobody has put an effect on must save to the bytes it always did.
///
/// **The stack in the fixture is deliberately two entries of the same kind**, in
/// an order that is not the order they would sort into. A `Vec` written as a set,
/// or read back through anything that dedupes by kind, still round-trips a
/// one-entry stack and still equals itself — it fails here, which is the whole
/// reason the repeatable list was chosen over one slot per effect. (Checked by
/// flipping the loader to `dedup_by` on the kind's discriminant: the second
/// shadow vanishes and this fails.)
///
/// **The subject is the artboard**, which is the other half of what needs
/// pinning: effects live on a node that has no `Paint` of its own, so a field
/// folded into `Paint` — or an op gated on `is_paintable` — would fail here and
/// nowhere else in this file.
#[test]
fn an_effect_stack_round_trips_in_order_and_costs_nothing_when_empty() {
    let (mut doc, root) = rich_document();
    let plain = io::save(&doc).unwrap();
    assert!(
        !String::from_utf8(plain.clone())
            .unwrap()
            .contains("effects"),
        "a document with no effect on it must not carry the key at all"
    );

    let board = *doc.get(root).unwrap().children().first().expect("a child");
    assert!(
        matches!(doc.get(board).unwrap().kind(), NodeKind::Artboard { .. }),
        "the fixture's subject is the container it is claimed to be"
    );
    let stack = vec![
        Effect::new(EffectKind::DropShadow(Shadow {
            blur: 24.0,
            ..Shadow::default()
        })),
        Effect {
            kind: EffectKind::DropShadow(Shadow {
                blur: 2.0,
                ..Shadow::default()
            }),
            visible: false,
        },
        Effect::new(EffectKind::Filters(Filters {
            saturation: 0.0,
            ..Filters::default()
        })),
    ];
    let inverse = doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: board,
            effects: stack.clone(),
        }]))
        .unwrap()
        .inverse;

    let loaded = io::load(&io::save(&doc).unwrap()).expect("a file with effects must load");
    assert_eq!(doc, loaded);
    assert_eq!(
        loaded.get(board).unwrap().effects(),
        stack.as_slice(),
        "three entries, two of one kind, in the order they were written"
    );

    doc.apply(&inverse).unwrap();
    assert!(
        doc.get(board).unwrap().effects().is_empty(),
        "undo must take the whole stack back off"
    );
    assert_eq!(
        io::save(&doc).unwrap(),
        plain,
        "and the bytes must come back to what they were, not to something equivalent"
    );
}

/// The library block is absent from a document that has never been through the
/// library — the half that makes it purely additive (§5.11a, invariant 9).
///
/// ⚠️ **Both halves, because they fail differently and independently.** Dropping
/// the container's `skip_serializing_if` writes `"meta": {}` into every existing
/// file and fails the first assertion; dropping the per-field ones writes four
/// nulls inside it and fails the second. The predicted failing assertion for the
/// second flip was the substring `"name"` — but `name` is also a *node* field,
/// so that one is green on every fixture and `"project"` is what actually bites.
/// Written down because a document-level absence test walks into exactly that
/// sort of substring collision.
#[test]
fn a_document_that_has_never_been_in_the_library_writes_no_meta_block() {
    let (doc, _root) = rich_document();
    assert!(doc.meta().is_empty(), "the fixture must start empty");
    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    assert!(
        !text.contains("meta"),
        "an unfiled document must not carry a meta key:\n{text}"
    );
    assert!(!text.contains("project"), "nor any of its fields:\n{text}");
}

/// A filed document round-trips every field, and the typed name survives
/// unslugged — the property the whole scheme exists for, since a filename cannot
/// give a capital or a space back.
#[test]
fn a_filed_document_round_trips_its_library_identity() {
    let (mut doc, _root) = rich_document();
    doc.set_meta(DocumentMeta {
        // ⚠️ **32 lowercase hex, because that is the only shape the app mints
        // and the loader now drops anything else** — the id is joined as a path
        // component by three writers, so a file is not allowed to assert an
        // arbitrary one (`DocumentMeta::is_wellformed_id`). This fixture read
        // `"0f9c2b7a4e"` for as long as nothing checked.
        id: Some("0f9c2b7a4e1d6c83b2f5a9074e1c3d68".into()),
        name: Some("My cool design".into()),
        project: Some("kestrel".into()),
        created: Some(1_774_483_200),
    });
    let bytes = io::save(&doc).unwrap();
    let loaded = io::load(&bytes).unwrap();
    assert_eq!(loaded.meta(), doc.meta());
    assert_eq!(loaded.meta().name.as_deref(), Some("My cool design"));
    // Re-saving is byte-identical, which is the condition the block is allowed
    // to exist under at all: nothing in it is written by a clock, so a save that
    // changed no drawing changes no bytes (invariant 9).
    assert_eq!(io::save(&loaded).unwrap(), bytes);
}

/// A document filed under only *some* of the block writes only those keys.
///
/// The case that matters is a document imported from outside the library: it
/// gets an id and a creation stamp on the way in and no project until the user
/// picks one. A per-field default of `Some(String::new())` would make "no
/// project" and "a project whose id is empty" the same thing in the file.
#[test]
fn only_the_library_fields_that_were_set_are_written() {
    let (mut doc, _root) = rich_document();
    doc.set_meta(DocumentMeta {
        // ⚠️ **32 lowercase hex, because that is the only shape the app mints
        // and the loader now drops anything else** — the id is joined as a path
        // component by three writers, so a file is not allowed to assert an
        // arbitrary one (`DocumentMeta::is_wellformed_id`). This fixture read
        // `"0f9c2b7a4e"` for as long as nothing checked.
        id: Some("0f9c2b7a4e1d6c83b2f5a9074e1c3d68".into()),
        created: Some(1_774_483_200),
        ..Default::default()
    });
    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    assert!(text.contains("1774483200"), "{text}");
    assert!(
        !text.contains("project"),
        "an unfiled project must leave no key:\n{text}"
    );
    let loaded = io::load(text.as_bytes()).unwrap();
    assert_eq!(loaded.meta().project, None);
    assert_eq!(loaded.meta().name, None);
}

/// The prefix probe reads a **real** save, not a hand-written fixture — the
/// assertion that ties `io::probe` to what `schema.rs` actually emits.
///
/// ⚠️ **This is the test that fails if the `meta` key is ever moved back down
/// the DTO.** `probe`'s own unit tests are written against a literal, so they
/// would all stay green while every folder scan silently fell back to reading
/// whole documents — which on a synced base folder means downloading the entire
/// library to draw a list of names. The fixture here is deliberately fat (a
/// document with an embedded image) so that "fits in 4 KB" is a claim about
/// *where the block is* rather than about the document being small.
#[test]
fn the_first_four_kilobytes_of_a_real_save_carry_the_library_block() {
    let (mut doc, _root) = rich_document();
    doc.set_meta(DocumentMeta {
        // ⚠️ **32 lowercase hex, because that is the only shape the app mints
        // and the loader now drops anything else** — the id is joined as a path
        // component by three writers, so a file is not allowed to assert an
        // arbitrary one (`DocumentMeta::is_wellformed_id`). This fixture read
        // `"0f9c2b7a4e"` for as long as nothing checked.
        id: Some("0f9c2b7a4e1d6c83b2f5a9074e1c3d68".into()),
        name: Some("My cool design".into()),
        project: Some("kestrel".into()),
        created: Some(1_774_483_200),
    });
    let bytes = io::save(&doc).unwrap();
    let prefix = &bytes[..bytes.len().min(io::probe::PREFIX_BYTES)];
    assert!(
        bytes.len() > io::probe::PREFIX_BYTES,
        "the fixture must be larger than the prefix or this proves nothing: {} bytes",
        bytes.len()
    );
    match io::probe::meta_in_prefix(prefix) {
        io::probe::MetaProbe::Found(m) => assert_eq!(&m, doc.meta()),
        other => panic!("a real save must be readable from its prefix, got {other:?}"),
    }
}

/// And the negative: a real save with no block is reported `Absent` from its
/// prefix rather than sending the caller off to read the whole file. This is
/// what keeps a folder of pre-library documents from being read in full on every
/// scan.
#[test]
fn a_real_unfiled_save_is_absent_from_its_prefix() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let prefix = &bytes[..bytes.len().min(io::probe::PREFIX_BYTES)];
    assert_eq!(
        io::probe::meta_in_prefix(prefix),
        io::probe::MetaProbe::Absent
    );
}

/// A file written before the block existed loads unfiled, and **`load` does not
/// invent an id** — the property that keeps loading deterministic, so reading
/// the same bytes twice cannot yield two documents that disagree about who they
/// are. Minting belongs to the app, on the first library save.
#[test]
fn a_pre_library_file_loads_unfiled_and_is_not_given_an_identity() {
    let (doc, _root) = rich_document();
    let bytes = io::save(&doc).unwrap();
    let first = io::load(&bytes).unwrap();
    let second = io::load(&bytes).unwrap();
    assert!(first.meta().is_empty());
    assert_eq!(first.meta(), second.meta());
}

/// A tree nested past `io::MAX_TREE_DEPTH` is refused at load, and one inside
/// it still opens.
///
/// ⚠️ **This is the check that is about the *stack* rather than about the
/// tree.** §5.11 delegates structural safety to the loader on the argument that
/// "a document produced by `apply` cannot contain a cycle" — so the in-memory
/// walks (`subtree_ids`, `paint_path`, `resolve`) carry no visited set and are
/// recursive. Every integrity check bounded a *shape* and none bounded a
/// *depth*, so a ~2,000-deep document of nested groups (about 900 KB, one line
/// to generate) passed all five and then killed the process inside
/// `Resolved::rebuild`: 1,000 levels in a debug build, 2,000 in release.
///
/// **A stack overflow is not a panic.** `catch_unwind` cannot see it, no
/// `Covers::Failed` memo records it, and the session's unsaved work goes with
/// the process. And nothing is clicked to reach it — `library::cover::rasterize`
/// loads and rebuilds *every* document the dashboard grid draws, and the
/// dashboard is the landing screen, so one such file in a synced base folder
/// crashes the app at launch with nothing on screen naming it.
///
/// **The accepted half is the one that keeps the refusal honest.** A new refusal
/// on the load path can lock a user out of a real document, so the test asserts
/// both ends: 250 deep loads *and rebuilds*, 300 deep is an `Integrity` error.
///
/// Flip-check, run: with check (6) removed the 300-deep assertion fails on
/// `unwrap_err`. ⚠️ **Raising `MAX_TREE_DEPTH` instead is the more interesting
/// flip and it is the one the fix sketch asked for** — at 4,096 the refusal goes
/// away *and the process survives this test*, because 300 is nowhere near the
/// abort. That is the distinction worth naming: this test proves the limit is
/// **enforced**, not that it is **low enough**. The second half is arithmetic
/// against the measured 1,000 above, not something a test can hold, because a
/// test that reached the abort would take the harness with it.
#[test]
fn a_document_nested_past_the_depth_limit_is_refused_by_the_loader() {
    fn nested(levels: usize) -> Vec<u8> {
        let mut ids = IdSource::new(0x5EED);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut hist = History::new();
        let mut parent = root;
        let mut ops = Vec::with_capacity(levels);
        for _ in 0..levels {
            let id = ids.mint();
            ops.push(Operation::CreateNode {
                id,
                parent,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            });
            parent = id;
        }
        hist.commit(&mut doc, Transaction(ops))
            .expect("the operation layer builds it happily — which is the point");
        io::save(&doc).unwrap()
    }

    // Well inside the bound: loads, and the recursive walk it certifies runs.
    let shallow = io::load(&nested(250)).expect("250 levels is an ordinary document");
    let _ = ondin_core::Resolved::rebuild(&shallow);

    // ⚠️ **Not `unwrap_err`.** On the flip that raises the constant, `unwrap_err`
    // prints the whole loaded `Document` — 300 nodes of `Debug` — and buries the
    // one sentence a reader needs. The `let else` says what happened instead.
    let Err(err) = io::load(&nested(300)) else {
        panic!(
            "a 300-deep document loaded; the walks that resolve it are recursive \
             and abort the process at 1,000"
        );
    };
    assert!(
        matches!(err, IoError::Integrity(ref m) if m.contains("nested deeper than")),
        "a document past the depth limit is refused with a reason: {err}"
    );
}

/// A document is not allowed to assert an arbitrary `meta.id`, because the app
/// joins it as a **path component**.
///
/// ⚠️ **Three writers join it and none of them examined it**: the crash
/// snapshot (`library/recovery.rs`), the version pin (`library/store.rs`) and
/// the cover cache (`library/cover.rs`). So a `.ondin` whose block reads
/// `"id": "../my-cool-design"` made the snapshot writer build
/// `root/.recovery/../my-cool-design.ondin` and the app's atomic writer rename
/// attacker-chosen bytes over the user's real document — every ten seconds,
/// silently, with nothing torn to notice. On Windows an *absolute* id is worse
/// still: `Path::join` discards the base outright.
///
/// **Dropped rather than refused**, and that is the whole design of the fix:
/// `None` is an ordinary state — every pre-library file loads that way — so the
/// document still opens and the library mints a fresh id the next time it
/// writes. Refusing the load would turn a repairable file into an unopenable
/// one, for a field the drawing does not depend on.
///
/// ⚠️ **`name` is the control, and it is the assertion that stops this becoming
/// over-sanitizing.** A name is free text by design and never reaches the
/// filesystem un-slugged; a fix that scrubbed "anything path-shaped" would
/// quietly rewrite what the user typed.
///
/// Flip-check, run: removing the `.sanitized()` from `into_document` fails on
/// the first assertion, printing `Some("../my-cool-design")`.
#[test]
fn a_documents_id_must_have_the_shape_the_app_mints() {
    let good = "0f9c2b7a4e1d6c83b2f5a9074e1c3d68";
    for bad in [
        "../my-cool-design",
        r"C:\Users\someone\AppData\Roaming\ondin\pwn",
        "/etc/passwd",
        // Right length, wrong alphabet: uppercase is not what `mint` writes,
        // and on a case-insensitive filesystem it would collide with the
        // lowercase one rather than being a second document.
        "0F9C2B7A4E1D6C83B2F5A9074E1C3D68",
        // Right alphabet, wrong length.
        "0f9c2b7a4e",
        "",
    ] {
        let (mut doc, _root) = rich_document();
        doc.set_meta(DocumentMeta {
            id: Some(bad.into()),
            name: Some("../not touched".into()),
            ..Default::default()
        });
        let bytes = io::save(&doc).unwrap();
        let loaded = io::load(&bytes).expect("the document still opens");
        assert_eq!(loaded.meta().id, None, "id {bad:?} must not survive");
        assert_eq!(
            loaded.meta().name.as_deref(),
            Some("../not touched"),
            "and the name, which never becomes a path, is left exactly as typed"
        );

        // The scan's reader is a **second** boundary and never loads the
        // document: `library::cover::key` is fed from here, for every card the
        // dashboard draws.
        let prefix = &bytes[..bytes.len().min(io::probe::PREFIX_BYTES)];
        match io::probe::meta_in_prefix(prefix) {
            io::probe::MetaProbe::Found(m) => {
                assert_eq!(m.id, None, "the prefix probe sanitizes too: {bad:?}");
            }
            other => panic!("the fixture must be readable from its prefix, got {other:?}"),
        }
    }

    // The control: a real id survives both readers untouched, or the check is a
    // data-loss bug of its own.
    let (mut doc, _root) = rich_document();
    doc.set_meta(DocumentMeta {
        id: Some(good.into()),
        ..Default::default()
    });
    let bytes = io::save(&doc).unwrap();
    assert_eq!(io::load(&bytes).unwrap().meta().id.as_deref(), Some(good));
}

/// The `.ondin`-that-never-opens class, end to end and from the top: an
/// ordinary SVG in, a document out, and the document opens again.
///
/// ⚠️ **`serde_json` writes a non-finite `f64` as `null`, and the reader has no
/// arm for it**, so `apply` → `Ok`, `save` → `Ok`, `load` → *"invalid type:
/// null, expected f64"*, permanently and with nothing reported anywhere in
/// between. The corrupting write is the one autosave and the crash snapshot both
/// make, so the user need never press Ctrl+S.
///
/// **Three doors reached it and this test drives the two that were reachable
/// without hand-built operations:**
///
/// - `d="M 0 0 L 1e999 10 Z"` — `1e999` is a legal `f64` literal that overflows
///   to infinity. `svg_in::parse_len` filtered for finite, so `<rect
///   width="1e999">` was harmless and the module read as guarded; `numbers()`
///   and `BezPath::from_svg` are the *geometry* paths and went round it.
/// - `transform="matrix(0,0,0,0,0,0)"` — finite, so nothing refused it, and
///   invisible on canvas but reachable from the layers panel. Moving the child
///   ran `build::local_for_world`, whose `parent_world.inverse()` divides by a
///   zero determinant: six `NaN`s straight into `SetTransform`.
///
/// **The import is asserted to succeed, not merely to be refused**, and that is
/// the assertion with teeth. `Document::apply` refuses a non-finite coordinate
/// now, so a fix that only added the op guard would turn "the file opens wrong"
/// into "the file will not import at all" — a whole drawing lost to one bad
/// number. The skip channel is what makes the degradation legible.
#[test]
fn an_svg_with_an_infinite_coordinate_still_saves_a_document_that_opens() {
    let mut ids = IdSource::new(0xF1);
    let root = ids.mint();
    let mut doc = Document::new(root);

    let svg = concat!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"100\">",
        "<path d=\"M 0 0 L 1e999 10 Z\" fill=\"red\"/>",
        "<polygon points=\"0,0 1e999,10 20,20\"/>",
        "<g transform=\"matrix(0,0,0,0,0,0)\"><rect width=\"10\" height=\"10\"/></g>",
        "<rect width=\"20\" height=\"20\"/>",
        "</svg>"
    );
    let out = ondin_core::svg_in::import(svg, &mut ids, root, 0, None).expect("the SVG imports");
    doc.apply(&out.tx)
        .expect("and every operation in it is accepted");

    let bytes = io::save(&doc).unwrap();
    // ⚠️ **`load` is the assertion, not a scan for `null` in the text.** The
    // first spelling of this test asserted the saved JSON held no `null` at all
    // and failed on `"parent": null`, which is the *root's* parent and entirely
    // correct — so the crude version was wrong in the direction that costs an
    // afternoon. Re-opening the document is the property the finding is about
    // and it is exact.
    let loaded = io::load(&bytes).unwrap_or_else(|e| {
        panic!(
            "the saved document must open again, got {e}\n{}",
            String::from_utf8_lossy(&bytes)
        )
    });

    // And the rest of the drawing survived — the file degraded, it was not lost.
    assert!(
        loaded.get(out.root).is_some(),
        "the import's own group is there"
    );
    let _ = ondin_core::Resolved::rebuild(&loaded);
}

/// The operation layer refuses a non-finite **number**, at every door that
/// writes one.
///
/// ⚠️ **Invariant 8 puts the check here rather than at the loader, and the
/// reason is a matter of *when*:** the operation is the last moment the value
/// can be rejected while the user still has their work. A loader check alone
/// means the document is already on disk and already unopenable.
///
/// **Every arm is a door somebody found, not a sweep for symmetry**:
/// `SetTransform` (`[A1-L2-01]`, reached from a singular SVG matrix),
/// `SetGeometry` and `CreateNode` (`[S2.1-L2-01]`, reached from an SVG path),
/// and the three guide writers (`[A1-L2-06]`, where the loader had refused the
/// value since it was written and the operation never had), `SetPivot`
/// (`[S2.2-L1-06]`, §15 D639) and `SetEffects` (`[S10.1-L1-05]`, §15 D641 —
/// the one value on this list that does damage before the file is written).
///
/// ⚠️ **"At every door" is a claim about the code and it has been false twice.**
/// D451 found paint outside it; D639 found the pivot, which is a *coordinate* —
/// inside the rule as this test's own name states it from the day the rule was
/// written, and outside the code the whole time. A sentence like the one above
/// is exactly what no gate reads.
///
/// Flip-check, run: removing the guard from `apply_geometry_patch` fails on the
/// `SetGeometry` assertion; removing it from `op_create` fails on the
/// `CreateNode` one. **They do not cover each other** — which is the whole
/// reason both are here, since `[A1-L2-01]`'s fix as originally sketched was
/// scoped to `op_set_transform` and would have closed neither.
///
/// ⚠️ **Renamed from `…_a_coordinate_…` on 2026-09-07, because the rule was
/// wider than the name and the code was narrower than the rule** (§15 D451).
/// `OpError::NonFinite`'s doc scoped itself to *"a node's geometry, its
/// transform, or a guide's position"*, and paint went unchecked: a `NaN`
/// gradient stop offset, a gradient transform, a stroke width and a dash length
/// all reached the model, were written as `null`, and killed the document on
/// reload. The four new arms are that door. Flipped per term — the fills guard
/// fails on *"SetFills (gradient stop offset) must be refused"*, and dropping
/// only the `dashes` clause fails on *"SetStrokes (dashes)"* rather than on an
/// earlier case, which is what says each arm is carrying its own weight.
#[test]
fn no_operation_may_write_a_number_that_is_not_finite() {
    use ondin_core::{GeometryPatch, OpError};

    let mut ids = IdSource::new(0xF2);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let rect = ids.mint();
    let mut hist = History::new();
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]),
    )
    .unwrap();

    let guide = GuideId(ids.mint());
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::AddGuide {
            guide: Guide {
                id: guide,
                axis: GuideAxis::Vertical,
                position: 10.0,
                color: None,
                owner: None,
            },
        }]),
    )
    .unwrap();

    let refused: Vec<(&str, Operation)> = vec![
        (
            "SetTransform",
            Operation::SetTransform {
                id: rect,
                transform: ondin_core::kurbo::Affine::new([f64::NAN; 6]),
            },
        ),
        (
            "SetGeometry (size)",
            Operation::SetGeometry {
                id: rect,
                geometry: GeometryPatch::Size(Size::new(f64::INFINITY, 10.0)),
            },
        ),
        (
            "SetGeometry (path)",
            Operation::SetGeometry {
                id: rect,
                geometry: GeometryPatch::Path {
                    path: {
                        let mut p = BezPath::new();
                        p.move_to(Point::new(0.0, 0.0));
                        // A *control* point, not an endpoint: the arm a check
                        // written against `LineTo` alone would miss.
                        p.curve_to(
                            Point::new(f64::NAN, 0.0),
                            Point::new(1.0, 1.0),
                            Point::new(2.0, 2.0),
                        );
                        p
                    },
                    corner_radii: Vec::new(),
                },
            },
        ),
        (
            "CreateNode (geometry)",
            Operation::CreateNode {
                id: ids.mint(),
                parent: root,
                index: 0,
                kind: NodeKind::Line {
                    end: Point::new(f64::INFINITY, 0.0),
                },
                transform: None,
                name: None,
            },
        ),
        (
            "CreateNode (transform)",
            Operation::CreateNode {
                id: ids.mint(),
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(ondin_core::kurbo::Affine::new([f64::NAN; 6])),
                name: None,
            },
        ),
        (
            "SetGuidePosition",
            Operation::SetGuidePosition {
                id: guide,
                position: f64::NAN,
            },
        ),
        (
            "SetGuideScope",
            Operation::SetGuideScope {
                id: guide,
                owner: None,
                position: f64::INFINITY,
            },
        ),
        (
            "AddGuide",
            Operation::AddGuide {
                guide: Guide {
                    id: GuideId(ids.mint()),
                    axis: GuideAxis::Vertical,
                    position: f64::NAN,
                    color: None,
                    owner: None,
                },
            },
        ),
        // ⚠️ **The paint half, added 2026-09-07 (§15 D451).** Until then
        // `OpError::NonFinite` covered *coordinates* — geometry, transforms,
        // guide positions — and `op_set_fills`/`op_set_strokes` stored whatever
        // brush they were handed. §2's invariant 8 calls the operation *"the last
        // place such a value can be refused while the user still has their
        // work"*, and it was true of coordinates and not of paint: a `NaN` here
        // reached the model, `serde_json` wrote it as `null`, and the file
        // answered *"invalid type: null, expected f32"* on the way back in.
        //
        // §15 D448 closed the SVG importer's route to this. These four are the
        // *operation* route, which every other author of a gradient — an MCP
        // call, a future scrub — comes through.
        (
            "SetFills (gradient stop offset)",
            Operation::SetFills {
                id: rect,
                fills: vec![Fill {
                    brush: Brush::Gradient(GradientBrush {
                        gradient: peniko::Gradient::new_linear((0.0, 0.0), (1.0, 0.0)).with_stops(
                            [
                                peniko::ColorStop {
                                    offset: f32::NAN,
                                    color: peniko::color::DynamicColor::from_alpha_color(
                                        peniko::color::palette::css::RED,
                                    ),
                                },
                                peniko::ColorStop {
                                    offset: 1.0,
                                    color: peniko::color::DynamicColor::from_alpha_color(
                                        peniko::color::palette::css::BLUE,
                                    ),
                                },
                            ],
                        ),
                        transform: ondin_core::kurbo::Affine::IDENTITY,
                        opacity: 1.0,
                    }),
                    visible: true,
                }],
            },
        ),
        (
            "SetFills (gradient transform)",
            Operation::SetFills {
                id: rect,
                fills: vec![Fill {
                    brush: Brush::Gradient(GradientBrush {
                        gradient: peniko::Gradient::new_linear((0.0, 0.0), (1.0, 0.0)),
                        transform: ondin_core::kurbo::Affine::new([f64::INFINITY; 6]),
                        opacity: 1.0,
                    }),
                    visible: true,
                }],
            },
        ),
        (
            "SetStrokes (width)",
            Operation::SetStrokes {
                id: rect,
                strokes: vec![Stroke {
                    brush: Brush::Solid(peniko::color::palette::css::BLACK),
                    width: f64::NAN,
                    ..Default::default()
                }],
            },
        ),
        (
            // `stroke-dasharray="1e999 5"` was one of `[S7.2-L5-01]`'s five sinks
            // and is closed at the importer; this is the same value arriving by
            // the door the importer is not.
            "SetStrokes (dashes)",
            Operation::SetStrokes {
                id: rect,
                strokes: vec![Stroke {
                    brush: Brush::Solid(peniko::color::palette::css::BLACK),
                    width: 1.0,
                    dashes: vec![f64::INFINITY, 5.0],
                    ..Default::default()
                }],
            },
        ),
        // ⚠️ **The pivot half, added 2026-09-09 (§15 D639), and it is a
        // *coordinate*** — so it was inside the rule this test's name states
        // from the day the rule was written, and outside the code the whole
        // time. `[S2.2-L1-06]`. Both variants are here because the check has to
        // read through the enum: a `Normalized` fraction and a `Local` point are
        // different types (`Vec2` and `Point`) and a guard written against one
        // does not see the other.
        (
            "SetPivot (normalized)",
            Operation::SetPivot {
                id: rect,
                pivot: Some(Pivot::Normalized(Vec2::new(f64::NAN, 0.0))),
            },
        ),
        (
            "SetPivot (local)",
            Operation::SetPivot {
                id: rect,
                pivot: Some(Pivot::Local(Point::new(0.0, f64::INFINITY))),
            },
        ),
        // ⚠️ **The effects half, added 2026-09-09 (§15 D641).** These are the
        // only arm on this list whose value does damage *before* the file is
        // saved: an infinite blur radius reaches `effect::escaped` through
        // `stack_escape`, and `f64::min` swallows it there, so the layer reports
        // an ink box identical to its geometry and the cull, the export plan and
        // both buffer boxes believe an unbounded blur reaches nowhere
        // (`[S10.1-L1-05]`).
        (
            "SetEffects (blur radius)",
            Operation::SetEffects {
                id: rect,
                effects: vec![Effect::new(EffectKind::LayerBlur {
                    radius: f64::INFINITY,
                })],
            },
        ),
        (
            "SetEffects (shadow offset)",
            Operation::SetEffects {
                id: rect,
                effects: vec![Effect::new(EffectKind::DropShadow(Shadow {
                    offset: Vec2::new(f64::NAN, 0.0),
                    ..Shadow::default()
                }))],
            },
        ),
        (
            // The arm a check written by reading the word "distance" leaves out,
            // and the same argument `brush_is_finite`'s colour clause carries:
            // the question is what `serde_json` refuses to write.
            "SetEffects (shadow colour)",
            Operation::SetEffects {
                id: rect,
                effects: vec![Effect::new(EffectKind::DropShadow(Shadow {
                    color: Color::new([f32::NAN, 0.0, 0.0, 1.0]),
                    ..Shadow::default()
                }))],
            },
        ),
        (
            "SetEffects (filter factor)",
            Operation::SetEffects {
                id: rect,
                effects: vec![Effect::new(EffectKind::Filters(Filters {
                    saturation: f64::NAN,
                    ..Filters::default()
                }))],
            },
        ),
    ];

    for (name, op) in refused {
        let before = io::save(&doc).unwrap();
        assert!(
            matches!(doc.apply(&Transaction(vec![op])), Err(OpError::NonFinite)),
            "{name} must be refused"
        );
        // ⚠️ **And refused without leaving anything behind.** `apply` mutates a
        // working copy as it goes, so "returns Err" and "changed nothing" are two
        // claims; the bytes are the only witness to the second.
        assert_eq!(
            io::save(&doc).unwrap(),
            before,
            "{name} left the document alone"
        );
    }

    // ⚠️ **`InsertSubtree` is a second door for the same value and it cannot be
    // driven from here.** Every field of `Node` is `pub(crate)`, so a test
    // outside the crate can only obtain a `Vec<Node>` from `capture_subtree`,
    // i.e. from a document that has already passed the guards above — the arm
    // lives in `document.rs`'s own `mod tests` as
    // `insert_subtree_refuses_a_non_finite_pivot` (§15 D639).
    //
    // The control: the same operations with finite values are accepted, or the
    // guard is a feature removal rather than a check.
    doc.apply(&Transaction(vec![
        Operation::SetTransform {
            id: rect,
            transform: ondin_core::kurbo::Affine::translate((5.0, 5.0)),
        },
        Operation::SetGeometry {
            id: rect,
            geometry: GeometryPatch::Size(Size::new(20.0, 20.0)),
        },
        Operation::SetGuidePosition {
            id: guide,
            position: 42.0,
        },
        Operation::SetPivot {
            id: rect,
            pivot: Some(Pivot::Normalized(Vec2::new(0.25, 0.75))),
        },
        Operation::SetEffects {
            id: rect,
            effects: vec![Effect::new(EffectKind::LayerBlur { radius: 8.0 })],
        },
    ]))
    .expect("finite values are ordinary");
}

/// A file already holding a non-finite number does not load — and **the reason
/// is not a check anybody wrote.**
///
/// ⚠️ **This test exists to record a measurement that contradicts the fix it was
/// written for.** The plan was a matching refusal in `verify_integrity`, so a
/// document damaged by a build with no operation guard would be *reported* by
/// node and field rather than dying inside serde. That check was written, and
/// then this test showed it could never run: **no JSON input can carry a
/// non-finite `f64` at all.** `serde_json` writes one as `null` and then refuses
/// `null` where an `f64` is wanted, and it refuses an overflowing literal like
/// `1e999` with *"number out of range"* — both at `from_value`, one function
/// above `verify_integrity`. The check was removed as unreachable.
///
/// So the whole of the protection is at the operation (`OpError::NonFinite`),
/// which is where invariant 8 puts it anyway. What is left over is the *message*:
/// a user with such a file gets serde's wording, which names neither the node
/// nor the field. Improving that means a custom deserializer for every float on
/// the DTO, and is a decision rather than a fix.
///
/// The assertion is deliberately about the **shape** of the refusal rather than
/// its text, so it pins the behaviour without pinning serde's wording.
#[test]
fn a_file_already_holding_a_non_finite_number_is_reported_by_name() {
    let mut ids = IdSource::new(0xF3);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let rect = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: rect,
        parent: root,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(10.0, 20.0),
            corner_radii: Default::default(),
        },
        transform: None,
        name: None,
    }]))
    .unwrap();

    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    // A number `serde_json` will read as `f64` and that is not finite: the
    // literal a hand-edit or a pre-guard writer leaves behind.
    let damaged = text.replace("10.0", "1e999");
    assert_ne!(damaged, text, "the fixture must actually be damaged");

    let Err(err) = io::load(damaged.as_bytes()) else {
        panic!("a document with an infinite coordinate must not load");
    };
    assert!(
        matches!(err, IoError::Serde(_)),
        "it is serde that refuses it, one step before any integrity check: {err}"
    );

    // And the same document as an old writer would really have left it: the
    // number written out as `null`.
    let as_written = text.replace("10.0", "null");
    assert!(
        matches!(io::load(as_written.as_bytes()), Err(IoError::Serde(_))),
        "`null` in a float's place is refused at the same seam"
    );
}

/// A span boundary **inside** a multi-byte character is snapped, not carried:
/// the byte ranges in this list are used to slice the content.
///
/// ⚠️ **The unfixed version crashed the app at every launch, and never opened
/// the file at all.** Content `"aéb"` with a span of `0..2` — byte 2 is inside
/// `é`, which occupies bytes 1..3 — loaded with no complaint and then panicked
/// on every render: `text::cased_text` slices `content[range]` directly and
/// parley's style builder does the same one line later, so the early return for
/// `TextCase::Original` moves which of the two fires rather than guarding
/// either. And `library::cover::rasterize` loads and rebuilds *every* document
/// the dashboard grid draws, synchronously inside the egui pass with no
/// `catch_unwind` on the path — so one such file in the base folder took the app
/// down at launch, with `Cover::Failed` never recording it because a panic does
/// not get that far.
///
/// **`Resolved::rebuild` is the assertion that matters**, not the indices.
/// Asserting only the numbers would pass against a snap to the *wrong* side —
/// `end` walked down to 1 rather than up to 3 is still a boundary and still
/// changes the value the test would read.
///
/// ⚠️ **The op door is driven as well as the loader**, because they are the same
/// function and only one of them was in the finding's first fix sketch:
/// `op_set_text` calls `Spans::clamped` too, so a repair placed at
/// `io::schema` alone would have left live editing open — retype the content
/// under an existing span and the same panic comes back with no file involved.
///
/// Flip-check, run: reverting `clamped` to `start.min(len)`/`end.min(len)` fails
/// on the `rebuild` assertion — it *panics* rather than failing an assert, with
/// "byte index 2 is not a char boundary; it is inside 'é'", which is the
/// reported symptom in the harness output.
#[test]
fn a_span_boundary_inside_a_character_is_snapped_on_load() {
    let (mut doc, _root) = rich_document();
    let id = text_node(&doc).id();
    let NodeKind::Text { style, .. } = doc.get(id).unwrap().kind().clone() else {
        unreachable!()
    };

    // "aéb": a=0..1, é=1..3, b=3..4.
    let mut spans = ondin_core::CharSpans::default();
    spans.set(0..1, ondin_core::CharAttr::Weight(900), &style);
    doc.apply(&Transaction(vec![
        Operation::SetText {
            id,
            content: "aéb".into(),
            spans: spans.clone(),
            para_spans: Default::default(),
        },
        Operation::SetTextSpans { id, spans },
    ]))
    .unwrap();

    let bytes = io::save(&doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for node in value["nodes"].as_array_mut().unwrap() {
        if let Some(text) = node.get_mut("kind").and_then(|k| k.get_mut("Text"))
            && let Some(spans) = text.get_mut("spans").and_then(|s| s.as_array_mut())
        {
            // Straight into the middle of the `é`.
            spans[0]["end"] = serde_json::json!(2);
        }
    }
    let loaded = io::load(&serde_json::to_vec(&value).unwrap()).expect("loads anyway");

    let NodeKind::Text { content, spans, .. } = loaded.get(id).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(content, "aéb", "the fixture is the content this is about");
    let end = spans.as_slice()[0].end;

    // ⚠️ **The rebuild goes first, deliberately.** Both this and the index
    // assertion below bite under the flip, and the order decides which one a
    // future reader is shown: the rebuild *panics* with "byte index 2 is not a
    // char boundary; it is inside 'é'", which is the reported symptom, where the
    // index assertion says only `left: 2  right: 3`. Written the other way round
    // first, and corrected after running it.
    let _ = ondin_core::Resolved::rebuild(&loaded);
    assert_eq!(
        end, 3,
        "the end snaps outward, over the whole character it half-named"
    );

    // And the live-editing door, which never touches the loader.
    let mut live = loaded;
    let NodeKind::Text { style, .. } = live.get(id).unwrap().kind().clone() else {
        unreachable!()
    };
    let mut spans = ondin_core::CharSpans::default();
    spans.set(0..4, ondin_core::CharAttr::Weight(900), &style);
    live.apply(&Transaction(vec![Operation::SetText {
        id,
        // Shorter, and ending mid-character relative to the span above.
        content: "aé".into(),
        spans,
        para_spans: Default::default(),
    }]))
    .unwrap();
    let _ = ondin_core::Resolved::rebuild(&live);
}

// --- the clipboard's wire form (`io::clip`, §15 D823) -----------------------

/// **The clipboard is the document format with a different envelope**, and this
/// is what says so: every node kind `rich_document` builds goes out through
/// `io::clip::write` and comes back `PartialEq`-identical to the nodes captured
/// from the document. A second projection beside `NodeDto` would drift from it
/// silently — a field added to one and not the other compiles, saves and loads —
/// so the test that matters is not "a clip round-trips" but "a clip round-trips
/// *through the same DTO a file does*".
///
/// **Two subtrees rather than one, and that is the flip.** Copying a
/// multi-selection is the case `Clip::subtrees` is a `Vec` for, and the plausible
/// wrong version is a payload that carries *a* subtree: `write` taking
/// `subtrees.first()` and wrapping it. ⚠️ **Flipped exactly that way: red on the
/// subtree-count assertion at 1 against 2**, the predicted site, before any node
/// comparison runs — which is why the count is asserted before the contents.
#[test]
fn a_clipboard_payload_round_trips_every_node_kind() {
    let (doc, root) = rich_document();
    let frame = doc.get(root).unwrap().children()[0];
    let kids = doc.get(frame).unwrap().children().to_vec();
    assert!(
        kids.len() >= 2,
        "the fixture is not in the state this test is about — one child cannot \
         show a multi-selection crossing"
    );
    let captured: Vec<Vec<ondin_core::Node>> = kids
        .iter()
        .take(2)
        .map(|id| doc.capture_subtree(*id).unwrap())
        .collect();

    let text = io::clip::write(&captured, &[], None, "Two layers").unwrap();
    let payload = io::clip::read(&text)
        .expect("the fence is ours")
        .expect("and the payload reads");

    assert_eq!(
        payload.subtrees.len(),
        2,
        "a copy of two layers crosses as two subtrees"
    );
    assert_eq!(
        payload.subtrees, captured,
        "and each node comes back exactly as the document held it"
    );
}

/// **The pictures cross too**, which is the half a payload of nodes alone gets
/// wrong in silence: a fill stores an image *key* and the bytes live in a table
/// on the document, so a picture whose entry stayed behind arrives in the other
/// window as the missing-picture placeholder with nothing saying why (`Clip`).
///
/// ⚠️ **Flipped** by writing `images: Vec::new()` in `ClipDto`, which is the
/// plausible wrong version — "the nodes are the copy" — and is what the in-app
/// clipboard looked like before §15 D280 taught it otherwise: **red on the
/// entry-count assertion**, the predicted site, at 0 against 1. (That citation
/// read D183 for a day; D183 is the drop and paste *doors*, and the entry about
/// a picture arriving in another document as the missing-picture placeholder is
/// D280.)
#[test]
fn a_clipboard_payload_carries_the_image_table_entries_it_keys_into() {
    let (doc, root) = rich_document();
    let frame = doc.get(root).unwrap().children()[0];
    let captured = vec![doc.capture_subtree(frame).unwrap()];
    let id = ImageId("pic".to_string());
    let entry = ImageEntry {
        source: ImageSource::Embedded(vec![1, 2, 3, 4].into()),
        format: ImageFormat::Png,
        width: 2,
        height: 2,
    };

    let text = io::clip::write(&captured, &[(id.clone(), entry.clone())], None, "One").unwrap();
    let payload = io::clip::read(&text).unwrap().unwrap();

    assert_eq!(
        payload.images.len(),
        1,
        "the table entry the nodes key into crosses with them"
    );
    assert_eq!(payload.images[0].0, id);
    assert_eq!(payload.images[0].1, entry, "bytes and all");
}

/// **Three answers, and the middle one is the whole reason `read` is not a
/// `bool`.** Ordinary text is somebody else's and the caller has other arms for
/// it; text carrying our fence is ours, and an unreadable one has to be said out
/// loud rather than fallen through on — falling through pastes the payload as a
/// text layer, which is the one outcome nobody wants.
///
/// ⚠️ **Flipped** by deleting `parse`'s `schema_version` comparison, which is the
/// plausible wrong version: the DTO's *shape* has not changed between versions,
/// so serde parses a payload from an unknown build perfectly happily. **Red on
/// the third assertion**, the predicted site, with an `Ok` where the refusal
/// belongs — and green on the first two, which is what makes the version check a
/// separate claim rather than a consequence of the fence.
#[test]
fn clipboard_text_is_foreign_ours_or_refused() {
    let (doc, root) = rich_document();
    let frame = doc.get(root).unwrap().children()[0];
    let captured = vec![doc.capture_subtree(frame).unwrap()];
    let text = io::clip::write(&captured, &[], None, "One").unwrap();

    assert!(
        io::clip::read("a sentence copied in a browser").is_none(),
        "no fence, so it is not ours and the caller's other arms get it"
    );
    assert!(
        io::clip::read(&text).unwrap().is_ok(),
        "the fixture is not in the state this test is about"
    );

    let bumped = text.replace(
        &format!("\"schema_version\":{}", io::CURRENT_SCHEMA_VERSION),
        "\"schema_version\":9999",
    );
    assert_ne!(bumped, text, "the version really was rewritten");
    assert!(
        matches!(
            io::clip::read(&bumped),
            Some(Err(IoError::UnsupportedVersion(9999)))
        ),
        "a copy from a build we do not know is refused by name, not ignored"
    );
}

/// **A layer named like the fence does not break the payload**, which is why
/// `read` splits on the *last* occurrence rather than the first.
///
/// ⚠️ **Flipped** to `split_once`: **red on the `is_ok`**, the predicted site —
/// the heading's copy of the fence wins, and everything after it (the real fence
/// and the JSON) is handed to `serde_json` as one string.
#[test]
fn a_layer_named_like_the_fence_still_crosses() {
    let (doc, root) = rich_document();
    let frame = doc.get(root).unwrap().children()[0];
    let captured = vec![doc.capture_subtree(frame).unwrap()];

    let text = io::clip::write(&captured, &[], None, io::clip::FENCE).unwrap();
    assert!(
        io::clip::read(&text).unwrap().is_ok(),
        "the heading is not part of the format and cannot make it unreadable"
    );
}
