//! A pinned document exports exactly as the same document with the pins baked in
//! (§15 D871, D868) — the first test that used geometry reaches anything outside
//! `ondin-core`.
//!
//! Container layout's step 1 routed the SVG writer, the snapshot and the scene
//! walk (which the PNG writer shares) through used geometry, and could prove it
//! only inside core: nothing could put a used value anywhere else. Insets can.
//! So this builds one frame of pinned layers, resizes the frame so every pin
//! actually moves or stretches something, and then **bakes** it — a copy where
//! each pinned layer's stored transform and size are set to where the pins put
//! it, and the pins removed. The two documents describe the same picture by two
//! routes, and every writer must say so byte for byte.
//!
//! **Why a differential and not expected numbers**: the claim is not "the writer
//! computes 390" but "the writer draws the used geometry, whatever it is" — and
//! a writer still reading the document draws the pinned copy at its stored
//! position, which the baked copy does not have.
//!
//! **Flips, each run and each failing only its own test**: `svg::emit_node`
//! composing `node.transform()` fails the SVG test; `scene::paint_node` falling
//! back to `node.transform()` fails the PNG test; `snapshot`'s `local_transform`
//! reading `node.transform()` fails the snapshot test. Each writer's routing is
//! held by exactly one test, which is the evidence step 1 owed.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Fill, GeometryPatch, IdSource, Insets, LengthPct, NodeId, NodeKind, Operation,
    Resolved, TextSizing, TextStyle, Transaction, keyed_by_position,
};
use ondin_render::Viewport;

fn px(v: f64) -> Option<LengthPct> {
    Some(LengthPct::Px(v))
}

/// The pinned document and every pinned node in it.
fn pinned() -> (Document, Vec<NodeId>) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut colour = 0u8;
    let mut add = |doc: &mut Document, parent: NodeId, kind: NodeKind, at: (f64, f64)| {
        let id = ids.mint();
        let index = doc.get(parent).unwrap().children().len();
        let text = matches!(kind, NodeKind::Text { .. });
        let mut ops = vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        }];
        colour = colour.wrapping_add(40);
        if !text {
            ops.push(Operation::SetFills {
                id,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(colour, 90, 200 - colour / 2, 255)),
                    visible: true,
                }]),
            });
        }
        doc.apply(&Transaction(ops)).unwrap();
        id
    };
    let rect = |w, h| NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
    };
    let frame = add(
        &mut doc,
        root,
        NodeKind::Artboard {
            size: Size::new(300.0, 200.0),
        },
        (20.0, 30.0),
    );
    let right = add(&mut doc, frame, rect(100.0, 40.0), (10.0, 10.0));
    let wide = add(&mut doc, frame, rect(100.0, 40.0), (10.0, 60.0));
    let words = add(
        &mut doc,
        frame,
        NodeKind::Text {
            content: "pinned text that wraps when its frame is narrower".into(),
            style: Box::new(TextStyle::default()),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        (10.0, 110.0),
    );
    let nested = add(
        &mut doc,
        frame,
        NodeKind::Artboard {
            size: Size::new(80.0, 40.0),
        },
        (10.0, 150.0),
    );
    let grandchild = add(&mut doc, nested, rect(20.0, 20.0), (0.0, 0.0));
    let turned = add(&mut doc, frame, rect(30.0, 20.0), (0.0, 0.0));
    let both = Insets {
        left: px(10.0),
        right: px(10.0),
        ..Default::default()
    };
    let mut ops = vec![
        Operation::SetInsets {
            id: right,
            insets: Insets {
                right: px(10.0),
                ..Default::default()
            },
        },
        Operation::SetInsets {
            id: wide,
            insets: both,
        },
        Operation::SetInsets {
            id: words,
            insets: both,
        },
        Operation::SetInsets {
            id: nested,
            insets: both,
        },
        Operation::SetInsets {
            id: grandchild,
            insets: Insets {
                right: px(0.0),
                bottom: Some(LengthPct::Percent(10.0)),
                ..Default::default()
            },
        },
        // Rotated, so the rotation-about-the-centre rule reaches the writers too.
        Operation::SetTransform {
            id: turned,
            transform: Affine::rotate(0.4),
        },
        Operation::SetInsets {
            id: turned,
            insets: Insets {
                right: px(20.0),
                bottom: px(20.0),
                ..Default::default()
            },
        },
    ];
    // And a frame resize, so every pin has somewhere else to put its layer.
    ops.push(Operation::SetGeometry {
        id: frame,
        geometry: GeometryPatch::Size(Size::new(460.0, 200.0)),
    });
    doc.apply(&Transaction(ops)).unwrap();
    (doc, vec![right, wide, words, nested, grandchild, turned])
}

/// `doc` with each pinned layer's used placement written into the document and
/// its pins taken away — the same picture, reached without layout.
fn baked(doc: &Document, pinned: &[NodeId]) -> Document {
    let res = Resolved::rebuild(doc);
    let mut ops = Vec::new();
    for &id in pinned {
        ops.push(Operation::SetTransform {
            id,
            transform: res.used_local(doc, id).unwrap(),
        });
        match res.used_kind(doc, id).unwrap() {
            NodeKind::Rect { size, .. } | NodeKind::Artboard { size } => {
                ops.push(Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Size(*size),
                })
            }
            NodeKind::Text { sizing, .. } => ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::TextSizing(*sizing),
            }),
            other => panic!("the fixture has no {other:?}"),
        }
        ops.push(Operation::SetInsets {
            id,
            insets: Insets::default(),
        });
    }
    let mut out = doc.clone();
    out.apply(&Transaction(ops)).unwrap();
    out
}

/// The fixture pins something that actually moved: without this every
/// comparison below would be two unpinned documents agreeing.
fn assert_the_pins_moved_things(doc: &Document, pinned: &[NodeId]) {
    let res = Resolved::rebuild(doc);
    for &id in pinned {
        let node = doc.get(id).unwrap();
        // Moved, or stretched in place — a layer pinned on both sides keeps its
        // left edge and changes only its size.
        assert!(
            res.used_local(doc, id) != Some(node.transform())
                || res.used_kind(doc, id) != Some(node.kind()),
            "{id:?} is drawn exactly as its document says — the fixture tests nothing"
        );
    }
}

#[test]
fn a_pinned_document_writes_the_same_svg_as_its_baked_twin() {
    let (doc, pinned) = pinned();
    assert_the_pins_moved_things(&doc, &pinned);
    let bake = baked(&doc, &pinned);
    let a = ondin_export::svg::svg(&doc, &Resolved::rebuild(&doc), None);
    let b = ondin_export::svg::svg(&bake, &Resolved::rebuild(&bake), None);
    assert_eq!(a, b);
}

#[test]
fn a_pinned_document_rasterizes_to_the_same_png_as_its_baked_twin() {
    let (doc, pinned) = pinned();
    let bake = baked(&doc, &pinned);
    let vp = Viewport {
        view: ondin_core::kurbo::Rect::new(0.0, 0.0, 520.0, 260.0),
        pixel_size: (520, 260),
    };
    let a = ondin_export::png::png(&doc, &Resolved::rebuild(&doc), &vp);
    let b = ondin_export::png::png(&bake, &Resolved::rebuild(&bake), &vp);
    assert!(a == b, "the two rasters differ");
}

#[test]
fn a_pinned_document_snapshots_the_same_as_its_baked_twin() {
    let (doc, pinned) = pinned();
    let bake = baked(&doc, &pinned);
    let a = serde_json::to_string(&ondin_export::snapshot::snapshot(
        &doc,
        &Resolved::rebuild(&doc),
    ))
    .unwrap();
    let b = serde_json::to_string(&ondin_export::snapshot::snapshot(
        &bake,
        &Resolved::rebuild(&bake),
    ))
    .unwrap();
    assert_eq!(a, b);
}
