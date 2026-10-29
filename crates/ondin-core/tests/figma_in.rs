//! Figma import (`ondin_core::figma_in`): a link's file key, nodes placed in
//! their containing frame, geometry, text overrides, components linked through
//! instance ids with their overrides kept, styles, pictures, and a result the
//! loader accepts.

use ondin_core::figma_in::{Read, file_key, read};
use ondin_core::foreign::Archive;
use ondin_core::image::{ImageEntry, ImageFormat, ImageId, ImageSource};
use ondin_core::kurbo::{Affine, Point};
use ondin_core::peniko::{self, Color};
use ondin_core::svg_in::ImageSink;
use ondin_core::{BrushSource, Document, IdSource, NodeId, NodeKind, TextSizing};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
struct Fetched(BTreeMap<String, Vec<u8>>);

impl Archive for Fetched {
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        self.0.get(name).cloned()
    }
}

struct Pictures;

impl ImageSink for Pictures {
    fn place(&mut self, bytes: Vec<u8>, format: ImageFormat) -> Option<(ImageId, ImageEntry)> {
        Some((
            ImageId(format!("test:{}", bytes.len())),
            ImageEntry {
                source: ImageSource::Embedded(bytes.into()),
                format,
                width: 4,
                height: 4,
            },
        ))
    }
}

fn rgb(r: f64, g: f64, b: f64) -> Value {
    json!({ "r": r, "g": g, "b": b, "a": 1 })
}

fn solid(c: Value) -> Value {
    json!({ "type": "SOLID", "color": c })
}

/// A node at `(x, y)` in its containing frame, `w` by `h`.
fn node(id: &str, kind: &str, (x, y, w, h): (f64, f64, f64, f64), extra: Value) -> Value {
    let mut n = json!({
        "id": id, "name": id, "type": kind,
        "relativeTransform": [[1, 0, x], [0, 1, y]],
        "size": { "x": w, "y": h },
    });
    if let (Some(n), Some(e)) = (n.as_object_mut(), extra.as_object()) {
        n.extend(e.clone());
    }
    n
}

fn fetched(children: Vec<Value>, extra: Value, images: &[(&str, Vec<u8>)]) -> Fetched {
    let mut file = json!({
        "name": "Checkout flow",
        "document": { "id": "0:0", "type": "DOCUMENT", "children": [{
            "id": "0:1", "name": "Flows", "type": "CANVAS",
            "backgroundColor": rgb(0.9, 0.9, 0.9), "children": children,
        }]},
    });
    if let (Some(f), Some(e)) = (file.as_object_mut(), extra.as_object()) {
        f.extend(e.clone());
    }
    let mut out = Fetched::default();
    out.0
        .insert("file.json".into(), file.to_string().into_bytes());
    for (r, bytes) in images {
        out.0.insert(format!("images/{r}"), bytes.clone());
    }
    out
}

fn import(mut f: Fetched) -> Read {
    let mut ids = IdSource::new(0xF1);
    read(&mut f, &mut ids, &mut Pictures).expect("the file reads")
}

fn one(doc: &Document, name: &str) -> NodeId {
    let found: Vec<NodeId> = doc
        .page_ids()
        .flat_map(|p| ondin_core::build::subtree_nodes(doc, &[p]))
        .filter(|&id| doc.get(id).is_some_and(|n| n.name() == name))
        .collect();
    assert_eq!(found.len(), 1, "one layer named {name:?}");
    found[0]
}

fn fill(doc: &Document, id: NodeId) -> BrushSource {
    doc.get(id).unwrap().paint().fills[0].brush.clone()
}

/// The file key comes out of every shape of link Figma hands out.
#[test]
fn a_links_file_key_is_found() {
    for (link, key) in [
        (
            "https://www.figma.com/design/AbC123/Checkout-flow?node-id=1-2",
            Some("AbC123"),
        ),
        ("figma.com/file/XyZ9/Old-style", Some("XyZ9")),
        (
            "https://www.figma.com/design/AbC123/branch/Br4nch/Name",
            Some("Br4nch"),
        ),
        ("https://www.figma.com/proto/P1/Prototype", Some("P1")),
        ("https://example.com/design/AbC123", None),
        ("https://www.figma.com/community/file/123", None),
    ] {
        assert_eq!(file_key(link).as_deref(), key, "{link}");
    }
}

/// A frame holds a rect and a group; the group's child is placed by Figma in the
/// frame, not the group, and lands where Figma draws it.
#[test]
fn nodes_are_placed_in_their_containing_frame() {
    let rect = node(
        "1:3",
        "RECTANGLE",
        (10.0, 20.0, 40.0, 30.0),
        json!({
            "fills": [solid(rgb(1.0, 0.0, 0.0))], "rectangleCornerRadii": [4, 4, 0, 0],
        }),
    );
    let inner = node("1:5", "RECTANGLE", (60.0, 70.0, 10.0, 10.0), json!({}));
    let group = node(
        "1:4",
        "GROUP",
        (50.0, 60.0, 30.0, 30.0),
        json!({ "children": [inner] }),
    );
    let frame = node(
        "1:2",
        "FRAME",
        (100.0, 100.0, 300.0, 200.0),
        json!({
            "clipsContent": true, "fills": [solid(rgb(1.0, 1.0, 1.0))], "children": [rect, group],
        }),
    );
    let r = import(fetched(vec![frame], json!({}), &[]));
    let doc = &r.doc;
    assert_eq!(r.name, "Checkout flow");
    let f = one(doc, "1:2");
    assert!(doc.get(f).unwrap().clip());
    let rect = doc.get(one(doc, "1:3")).unwrap();
    assert_eq!(rect.transform(), Affine::translate((10.0, 20.0)));
    assert!(
        matches!(rect.kind(), NodeKind::Rect { corner_radii, .. } if corner_radii.top_left == 4.0)
    );
    // In the frame at (60, 70): the group sits at (50, 60), so (10, 10) in it.
    let inner = doc.get(one(doc, "1:5")).unwrap();
    assert_eq!(inner.transform(), Affine::translate((10.0, 10.0)));
    let page = doc.first_page();
    assert_eq!(doc.get(page).unwrap().name(), "Flows");
}

/// A vector is its fill geometry; an open one with only stroke geometry is that
/// outline filled with the stroke's paint.
#[test]
fn vectors_come_from_their_geometry() {
    let closed = node(
        "2:1",
        "VECTOR",
        (0.0, 0.0, 10.0, 10.0),
        json!({
            "fills": [solid(rgb(0.0, 0.0, 1.0))],
            "fillGeometry": [{ "path": "M0 0L10 0L10 10Z", "windingRule": "NONZERO" }],
        }),
    );
    let open = node(
        "2:2",
        "VECTOR",
        (0.0, 0.0, 10.0, 0.0),
        json!({
            "strokes": [solid(rgb(0.0, 1.0, 0.0))], "strokeWeight": 2,
            "fillGeometry": [], "strokeGeometry": [{ "path": "M0 -1L10 -1L10 1L0 1Z" }],
        }),
    );
    let r = import(fetched(vec![closed, open], json!({}), &[]));
    let doc = &r.doc;
    let NodeKind::Path { path, .. } = doc.get(one(doc, "2:1")).unwrap().kind() else {
        panic!("a path");
    };
    assert_eq!(path.to_svg(), "M0,0 L10,0 L10,10 Z");
    let open = doc.get(one(doc, "2:2")).unwrap();
    assert!(matches!(open.kind(), NodeKind::Path { .. }));
    assert!(open.paint().strokes.is_empty());
    assert_eq!(
        fill(doc, one(doc, "2:2")),
        BrushSource::Literal(peniko::Brush::Solid(Color::new([0.0, 1.0, 0.0, 1.0])))
    );
    let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
    assert_eq!(rows, ["Open paths"]);
}

/// A text's style is its default and each run of overrides a span, at bytes
/// though Figma counts UTF-16 units.
#[test]
fn text_overrides_become_spans() {
    let text = node(
        "3:1",
        "TEXT",
        (0.0, 0.0, 200.0, 40.0),
        json!({
            "characters": "😀 Pay now",
            "fills": [solid(rgb(0.0, 0.0, 0.0))],
            "style": { "fontFamily": "Inter", "fontWeight": 400, "fontSize": 16,
                       "textAutoResize": "HEIGHT", "textAlignHorizontal": "CENTER",
                       "lineHeightUnit": "PIXELS", "lineHeightPx": 24 },
            "characterStyleOverrides": [0, 0, 0, 7, 7, 7],
            "styleOverrideTable": { "7": { "fontWeight": 700 } },
        }),
    );
    let r = import(fetched(vec![text], json!({}), &[]));
    let NodeKind::Text {
        content,
        style,
        spans,
        sizing,
        ..
    } = r.doc.get(one(&r.doc, "3:1")).unwrap().kind()
    else {
        panic!("a text");
    };
    assert_eq!(content, "😀 Pay now");
    assert_eq!(style.weight, 400);
    assert_eq!(style.line_height, Some(ondin_core::Length::Px(24.0)));
    assert_eq!(*sizing, TextSizing::AutoHeight(200.0));
    let bold: Vec<_> = spans
        .as_slice()
        .iter()
        .filter(|s| matches!(s.attr, ondin_core::CharAttr::Weight(700)))
        .map(|s| s.range())
        .collect();
    // The emoji is two UTF-16 units and four bytes: units 3..6 are bytes 5..8.
    assert_eq!(bold, vec![5..8]);
}

/// A component is a main and its instance a linked copy, node by node, keeping a
/// fill it overrides; an instance of a component the file lacks stays plain.
/// flip: skip the copies' links in `components` → fails at the copy's link.
#[test]
fn an_instance_is_linked_through_its_ids() {
    let main = node(
        "4:1",
        "COMPONENT",
        (0.0, 0.0, 100.0, 40.0),
        json!({ "children": [
            node("4:2", "RECTANGLE", (10.0, 10.0, 20.0, 20.0), json!({ "fills": [solid(rgb(0.0, 1.0, 0.0))] })),
        ]}),
    );
    let inst = node(
        "5:1",
        "INSTANCE",
        (200.0, 0.0, 100.0, 40.0),
        json!({
            "componentId": "4:1", "children": [
                node("I5:1;4:2", "RECTANGLE", (10.0, 10.0, 20.0, 20.0), json!({ "fills": [solid(rgb(0.0, 0.0, 1.0))] })),
            ],
        }),
    );
    let remote = node(
        "6:1",
        "INSTANCE",
        (0.0, 100.0, 10.0, 10.0),
        json!({ "componentId": "99:9" }),
    );
    let r = import(fetched(vec![main, inst, remote], json!({}), &[]));
    let doc = &r.doc;
    let (m, i) = (one(doc, "4:1"), one(doc, "5:1"));
    assert!(doc.get(m).unwrap().component());
    assert_eq!(doc.get(i).unwrap().link(), Some(m));
    assert_eq!(
        doc.get(one(doc, "I5:1;4:2")).unwrap().link(),
        Some(one(doc, "4:2"))
    );
    assert_eq!(
        fill(doc, one(doc, "I5:1;4:2")),
        BrushSource::Literal(peniko::Brush::Solid(Color::new([0.0, 0.0, 1.0, 1.0]))),
        "the override survives the link"
    );
    assert_eq!(doc.get(one(doc, "6:1")).unwrap().link(), None);
    let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
    assert_eq!(rows, ["Instances of components not in the file"]);
}

/// A component holding another's instance, and an instance of it: every copy
/// links to what it copies, the nested copy to the nested instance in the
/// component, not straight to its main. flip: link a nested root to its main in
/// `components` too → the commit refuses and fails at the outer link.
#[test]
fn a_nested_instance_links_to_the_instance_it_copies() {
    let icon = node(
        "3:1",
        "COMPONENT",
        (0.0, 0.0, 10.0, 10.0),
        json!({ "children": [
            node("3:2", "RECTANGLE", (0.0, 0.0, 10.0, 10.0), json!({})),
        ]}),
    );
    let button = node(
        "4:1",
        "COMPONENT",
        (50.0, 0.0, 40.0, 20.0),
        json!({ "children": [
            node("4:3", "INSTANCE", (2.0, 2.0, 10.0, 10.0), json!({ "componentId": "3:1", "children": [
                node("I4:3;3:2", "RECTANGLE", (0.0, 0.0, 10.0, 10.0), json!({})),
            ]})),
        ]}),
    );
    let outer = node(
        "5:1",
        "INSTANCE",
        (200.0, 0.0, 40.0, 20.0),
        json!({ "componentId": "4:1", "children": [
            node("I5:1;4:3", "INSTANCE", (2.0, 2.0, 10.0, 10.0), json!({ "componentId": "3:1", "children": [
                node("I5:1;4:3;3:2", "RECTANGLE", (0.0, 0.0, 10.0, 10.0), json!({})),
            ]})),
        ]}),
    );
    let r = import(fetched(vec![icon, button, outer], json!({}), &[]));
    let doc = &r.doc;
    let link = |name: &str| doc.get(one(doc, name)).unwrap().link();
    assert_eq!(link("5:1"), Some(one(doc, "4:1")));
    assert_eq!(link("I5:1;4:3"), Some(one(doc, "4:3")));
    assert_eq!(link("I5:1;4:3;3:2"), Some(one(doc, "I4:3;3:2")));
    assert_eq!(link("4:3"), Some(one(doc, "3:1")));
    assert!(r.report.is_empty(), "{:?}", r.report);
}

/// A fill style is made from its first user, named from the file's styles, and
/// a later user is bound only while it shows the same fill.
#[test]
fn styles_are_made_from_their_first_user() {
    let style = json!({ "fill": "S:1" });
    let a = node(
        "7:1",
        "RECTANGLE",
        (0.0, 0.0, 10.0, 10.0),
        json!({ "fills": [solid(rgb(1.0, 0.0, 0.0))], "styles": style }),
    );
    let b = node(
        "7:2",
        "RECTANGLE",
        (0.0, 0.0, 10.0, 10.0),
        json!({ "fills": [solid(rgb(1.0, 0.0, 0.0))], "styles": style }),
    );
    let c = node(
        "7:3",
        "RECTANGLE",
        (0.0, 0.0, 10.0, 10.0),
        json!({ "fills": [solid(rgb(0.0, 0.0, 1.0))], "styles": style }),
    );
    let styles = json!({ "styles": { "S:1": { "name": "Brand/Red", "styleType": "FILL" } } });
    let r = import(fetched(vec![a, b, c], styles, &[]));
    let doc = &r.doc;
    let made = doc.styles().iter().next().expect("the style");
    assert_eq!(made.name, "Brand/Red");
    assert_eq!(
        doc.get(one(doc, "7:2")).unwrap().styles().fills,
        Some(made.id)
    );
    assert_eq!(doc.get(one(doc, "7:3")).unwrap().styles().fills, None);
}

/// An image fill reads the picture Figma sent for its reference.
#[test]
fn an_image_fill_shows_its_fetched_picture() {
    let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
    let photo = node(
        "8:1",
        "RECTANGLE",
        (0.0, 0.0, 10.0, 10.0),
        json!({ "fills": [{ "type": "IMAGE", "imageRef": "abc", "scaleMode": "FIT" }] }),
    );
    let r = import(fetched(vec![photo], json!({}), &[("abc", png)]));
    assert_eq!(r.doc.image_count(), 1);
    assert!(matches!(
        fill(&r.doc, one(&r.doc, "8:1")).literal(),
        Some(peniko::Brush::Image(b)) if b.image.fit == ondin_core::ImageFit::Fit
    ));
}

/// What the reader builds is a document the loader accepts.
#[test]
fn an_imported_file_saves_and_loads() {
    let turned = node(
        "9:1",
        "ELLIPSE",
        (0.0, 0.0, 20.0, 10.0),
        json!({
            "relativeTransform": [[0, -1, 5], [1, 0, 5]],
            "strokes": [solid(rgb(0.0, 0.0, 0.0))], "strokeWeight": 1,
            "effects": [{ "type": "DROP_SHADOW", "radius": 4, "offset": { "x": 0, "y": 2 },
                          "color": { "r": 0, "g": 0, "b": 0, "a": 0.25 } }],
        }),
    );
    let r = import(fetched(vec![turned], json!({}), &[]));
    let t = r.doc.get(one(&r.doc, "9:1")).unwrap().transform();
    assert_eq!(
        t * Point::new(1.0, 0.0),
        Point::new(5.0, 6.0),
        "turned a quarter"
    );
    let bytes = ondin_core::io::save(&r.doc).unwrap();
    ondin_core::io::load(&bytes).expect("the document loads");
}
