//! Penpot import (`ondin_core::penpot_in`): shapes placed from page space into
//! their parents', paints, text, library colours, components linked by
//! `shapeRef` with their overrides kept, and what has no counterpart reported.

use ondin_core::foreign::Archive;
use ondin_core::image::{ImageEntry, ImageFormat, ImageId, ImageSource};
use ondin_core::kurbo::Affine;
use ondin_core::peniko::{self, Color};
use ondin_core::penpot_in::{Read, read};
use ondin_core::svg_in::ImageSink;
use ondin_core::{BrushSource, Document, IdSource, NodeId, NodeKind, TextSizing};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const ROOT: &str = "00000000-0000-0000-0000-000000000000";
const FILE: &str = "f1";
const PAGE: &str = "p1";

#[derive(Default)]
struct Files(BTreeMap<String, Vec<u8>>);

impl Archive for Files {
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        self.0.get(name).cloned()
    }
    fn names(&mut self) -> Vec<String> {
        self.0.keys().cloned().collect()
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

fn shape(id: &str, kind: &str, parent: &str, (x, y, w, h): (f64, f64, f64, f64)) -> Value {
    json!({
        "id": id, "name": id, "type": kind, "parentId": parent,
        "x": x, "y": y, "width": w, "height": h,
        "selrect": { "x": x, "y": y, "width": w, "height": h },
        "transform": { "a": 1, "b": 0, "c": 0, "d": 1, "e": 0, "f": 0 },
    })
}

/// An export of one file and one page whose root holds `top`, every shape in
/// `shapes`, with `extra` entries beside them.
fn export(top: &[&str], shapes: Vec<Value>, extra: &[(&str, Value)]) -> Files {
    let mut files = Files::default();
    let mut put = |name: String, v: Value| {
        files.0.insert(name, v.to_string().into_bytes());
    };
    put(
        "manifest.json".into(),
        json!({ "type": "penpot/export-files", "version": 1,
                "files": [{ "id": FILE, "name": "Brand kit" }] }),
    );
    put(
        format!("files/{FILE}.json"),
        json!({ "id": FILE, "name": "Brand kit" }),
    );
    put(
        format!("files/{FILE}/pages/{PAGE}.json"),
        json!({ "id": PAGE, "name": "Main", "index": 0 }),
    );
    let mut root = shape(ROOT, "frame", ROOT, (0.0, 0.0, 0.01, 0.01));
    root["shapes"] = json!(top);
    put(format!("files/{FILE}/pages/{PAGE}/{ROOT}.json"), root);
    for s in shapes {
        let id = s["id"].as_str().unwrap().to_owned();
        put(format!("files/{FILE}/pages/{PAGE}/{id}.json"), s);
    }
    for (name, v) in extra {
        put((*name).to_owned(), v.clone());
    }
    files
}

fn import(mut files: Files) -> Read {
    let mut ids = IdSource::new(0x9E);
    read(&mut files, &mut ids, &mut Pictures).expect("the export reads")
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

fn solid(doc: &Document, id: NodeId) -> BrushSource {
    doc.get(id).unwrap().paint().fills[0].brush.clone()
}

/// A board on the page and a rect in it, both stored in page space, land with
/// the rect placed in the board's space; a turned rect turns about its centre.
#[test]
fn shapes_are_placed_in_their_parents_space() {
    let mut board = shape("B", "frame", ROOT, (100.0, 50.0, 300.0, 200.0));
    board["shapes"] = json!(["R", "T"]);
    board["fills"] = json!([{ "fillColor": "#0000ff", "fillOpacity": 1 }]);
    let mut rect = shape("R", "rect", "B", (110.0, 70.0, 40.0, 20.0));
    rect["r1"] = json!(6);
    rect["fills"] = json!([{ "fillColor": "#ff0000", "fillOpacity": 0.5 }]);
    let mut turned = shape("T", "rect", "B", (200.0, 100.0, 20.0, 20.0));
    turned["transform"] = json!({ "a": 0, "b": 1, "c": -1, "d": 0, "e": 0, "f": 0 });
    let r = import(export(&["B"], vec![board, rect, turned], &[]));
    let doc = &r.doc;
    assert_eq!(r.name, "Brand kit");
    let b = one(doc, "B");
    assert!(matches!(
        doc.get(b).unwrap().kind(),
        NodeKind::Artboard { .. }
    ));
    assert!(
        doc.get(b).unwrap().clip(),
        "a board hides what passes its edge"
    );
    let rn = doc.get(one(doc, "R")).unwrap();
    assert_eq!(rn.parent(), Some(b));
    assert_eq!(rn.transform(), Affine::translate((10.0, 20.0)));
    assert!(
        matches!(rn.kind(), NodeKind::Rect { corner_radii, .. } if corner_radii.top_left == 6.0)
    );
    assert_eq!(
        solid(doc, one(doc, "R")),
        BrushSource::Literal(peniko::Brush::Solid(
            Color::from_rgba8(255, 0, 0, 255).multiply_alpha(0.5)
        ))
    );
    // Turned a quarter about (210, 110), its corner lands at (220, 100) on the page.
    let t = doc.get(one(doc, "T")).unwrap().transform();
    let corner = Affine::translate((100.0, 50.0)) * t * ondin_core::kurbo::Point::ZERO;
    assert!(
        (corner.x - 220.0).abs() < 1e-9 && (corner.y - 100.0).abs() < 1e-9,
        "{corner:?}"
    );
}

/// A path's page-space outline is moved into its own space at its box's corner.
#[test]
fn a_path_is_read_from_page_space() {
    let mut path = shape("P", "path", ROOT, (0.0, 0.0, 0.0, 0.0));
    path["selrect"] = json!({ "x": 10, "y": 20, "width": 30, "height": 40 });
    path["content"] = json!("M10,20L40,60ZZ");
    let r = import(export(&["P"], vec![path], &[]));
    let node = r.doc.get(one(&r.doc, "P")).unwrap();
    assert_eq!(node.transform(), Affine::translate((10.0, 20.0)));
    let NodeKind::Path { path, .. } = node.kind() else {
        panic!("a path");
    };
    assert_eq!(path.to_svg(), "M0,0 L30,40 Z");
}

/// A text's paragraphs are joined by line breaks; its first leaf is the default
/// and a bolder leaf a span; its box grows as Penpot's says.
#[test]
fn text_leaves_become_a_default_and_spans() {
    let mut t = shape("T", "text", ROOT, (0.0, 0.0, 200.0, 40.0));
    t["growType"] = json!("auto-height");
    let leaf = |text: &str, weight: &str| {
        json!({ "text": text, "fontFamily": "Work Sans", "fontSize": "16", "fontWeight": weight,
                "fills": [{ "fillColor": "#112233", "fillOpacity": 1 }] })
    };
    t["content"] = json!({ "type": "root", "children": [{ "type": "paragraph-set", "children": [
        { "type": "paragraph", "textAlign": "center", "children": [leaf("Hello ", "400"), leaf("world", "700")] },
        { "type": "paragraph", "children": [leaf("Second", "400")] },
    ]}]});
    let r = import(export(&["T"], vec![t], &[]));
    let NodeKind::Text {
        content,
        style,
        spans,
        sizing,
        paragraph,
        ..
    } = r.doc.get(one(&r.doc, "T")).unwrap().kind()
    else {
        panic!("a text");
    };
    assert_eq!(content, "Hello world\nSecond");
    assert_eq!(
        (style.font_family.as_str(), style.font_size),
        ("Work Sans", 16.0)
    );
    assert_eq!(*sizing, TextSizing::AutoHeight(200.0));
    assert_eq!(paragraph.align, ondin_core::TextAlign::Center);
    let bold: Vec<_> = spans
        .as_slice()
        .iter()
        .filter(|s| matches!(s.attr, ondin_core::CharAttr::Weight(700)))
        .map(|s| s.range())
        .collect();
    assert_eq!(bold, vec![6..11]);
    assert_eq!(r.families.get("Work Sans"), Some(&3));
}

/// A library colour is a colour variable a fill naming it is bound to.
#[test]
fn a_library_colour_is_a_variable_its_fills_are_bound_to() {
    let mut rect = shape("R", "rect", ROOT, (0.0, 0.0, 10.0, 10.0));
    rect["fills"] = json!([{ "fillColor": "#ff0000", "fillOpacity": 1, "fillColorRefId": "C1" }]);
    let colour = json!({ "id": "C1", "name": "Red", "path": "Brand", "color": "#ff0000" });
    let r = import(export(
        &["R"],
        vec![rect],
        &[(&format!("files/{FILE}/colors/C1.json"), colour)],
    ));
    let var = r.doc.variables().iter().next().expect("the colour");
    assert_eq!(var.name, "Brand/Red");
    assert_eq!(
        solid(&r.doc, one(&r.doc, "R")),
        BrushSource::Bound {
            var: var.id,
            alpha: 1.0
        }
    );
}

/// A half-clear library colour, whose opacity Penpot copies into the fill, is
/// drawn half clear once, by the variable. flip: bind with the fill's own
/// opacity in `brush` → fails at the alpha.
#[test]
fn a_library_colours_opacity_is_not_applied_twice() {
    let mut rect = shape("R", "rect", ROOT, (0.0, 0.0, 10.0, 10.0));
    rect["fills"] = json!([{ "fillColor": "#ff0000", "fillOpacity": 0.5, "fillColorRefId": "C1" }]);
    let colour = json!({ "id": "C1", "name": "Veil", "color": "#ff0000", "opacity": 0.5 });
    let r = import(export(
        &["R"],
        vec![rect],
        &[(&format!("files/{FILE}/colors/C1.json"), colour)],
    ));
    let BrushSource::Bound { alpha, .. } = solid(&r.doc, one(&r.doc, "R")) else {
        panic!("bound to the colour");
    };
    assert_eq!(alpha, 1.0);
}

/// A main is a component, and its instance — each shape naming the main shape
/// it copies — is linked to it node by node, keeping a fill it overrides.
/// flip: skip the links in `components` → fails at the root's link.
#[test]
fn an_instance_is_linked_to_its_main_and_keeps_its_overrides() {
    let mut main = shape("M", "frame", ROOT, (0.0, 0.0, 100.0, 40.0));
    main["mainInstance"] = json!(true);
    main["componentRoot"] = json!(true);
    main["shapes"] = json!(["MR"]);
    let mut mr = shape("MR", "rect", "M", (10.0, 10.0, 20.0, 20.0));
    mr["fills"] = json!([{ "fillColor": "#00ff00", "fillOpacity": 1 }]);
    let mut inst = shape("I", "frame", ROOT, (200.0, 0.0, 100.0, 40.0));
    inst["componentRoot"] = json!(true);
    inst["shapeRef"] = json!("M");
    inst["shapes"] = json!(["IR"]);
    let mut ir = shape("IR", "rect", "I", (210.0, 10.0, 20.0, 20.0));
    ir["shapeRef"] = json!("MR");
    ir["fills"] = json!([{ "fillColor": "#0000ff", "fillOpacity": 1 }]);
    let r = import(export(&["M", "I"], vec![main, mr, inst, ir], &[]));
    let doc = &r.doc;
    let (m, i) = (one(doc, "M"), one(doc, "I"));
    assert!(doc.get(m).unwrap().component());
    assert_eq!(doc.get(i).unwrap().link(), Some(m));
    assert_eq!(
        doc.get(one(doc, "IR")).unwrap().link(),
        Some(one(doc, "MR"))
    );
    assert_eq!(
        solid(doc, one(doc, "IR")),
        BrushSource::Literal(peniko::Brush::Solid(Color::from_rgba8(0, 0, 255, 255))),
        "the override survives the link"
    );
    assert!(r.report.is_empty(), "{:?}", r.report);
}

/// An instance whose main is not in the export is kept as plain layers.
#[test]
fn an_instance_of_a_missing_main_is_kept_plain() {
    let mut inst = shape("I", "frame", ROOT, (0.0, 0.0, 10.0, 10.0));
    inst["componentRoot"] = json!(true);
    inst["shapeRef"] = json!("elsewhere");
    let r = import(export(&["I"], vec![inst], &[]));
    assert_eq!(r.doc.get(one(&r.doc, "I")).unwrap().link(), None);
    let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
    assert_eq!(rows, ["Instances whose main is not in the file"]);
    let bytes = ondin_core::io::save(&r.doc).unwrap();
    ondin_core::io::load(&bytes).expect("the document loads");
}

/// A layer left out (an SVG layer, a shape missing from the export) does not
/// leave a gap its later siblings are placed past, on the page or in a group.
/// flip: place each child at its position in `shapes` → the read fails.
#[test]
fn a_left_out_layer_does_not_push_its_siblings_out_of_range() {
    let mut board = shape("B", "frame", ROOT, (0.0, 0.0, 100.0, 100.0));
    board["shapes"] = json!(["S", "gone", "R"]);
    let svg = shape("S", "svg-raw", "B", (0.0, 0.0, 10.0, 10.0));
    let rect = shape("R", "rect", "B", (0.0, 0.0, 10.0, 10.0));
    let after = shape("A", "rect", ROOT, (0.0, 0.0, 10.0, 10.0));
    let r = import(export(
        &["S2", "B", "A"],
        vec![board, svg.clone(), rect, after, {
            let mut s = svg;
            s["id"] = json!("S2");
            s
        }],
        &[],
    ));
    let doc = &r.doc;
    let b = one(doc, "B");
    assert_eq!(doc.get(b).unwrap().children(), [one(doc, "R")]);
    let page = doc.page_ids().next().unwrap();
    assert_eq!(doc.get(page).unwrap().children(), [b, one(doc, "A")]);
}

/// Penpot 1's binary export is told apart from a zip, and an archive with no
/// manifest is refused.
#[test]
fn an_old_or_foreign_archive_is_refused() {
    assert!(ondin_core::penpot_in::is_old_format(&[
        0x01, 0x0b, 0x32, 0x06
    ]));
    assert!(!ondin_core::penpot_in::is_old_format(b"PK\x03\x04rest"));
    let mut ids = IdSource::new(1);
    assert!(read(&mut Files::default(), &mut ids, &mut Pictures).is_err());
}
