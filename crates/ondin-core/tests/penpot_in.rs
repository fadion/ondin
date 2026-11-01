//! Penpot import (`ondin_core::penpot_in`): shapes placed from page space into
//! their parents', paints, text, library colours, components linked by
//! `shapeRef` with their overrides kept, and what has no counterpart reported.

use ondin_core::foreign::Archive;
use ondin_core::image::{ImageEntry, ImageFit, ImageFormat, ImageId, ImageSource};
use ondin_core::kurbo::{Affine, Cap};
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

/// Penpot lists fills and strokes topmost first, so they come in reversed, and
/// a dashed or dotted stroke takes Penpot's pattern for its width.
/// flip: drop the `.rev()` on the fills in `style` → fails at the fill order.
#[test]
fn paints_stack_as_penpot_draws_them() {
    let mut rect = shape("R", "rect", ROOT, (0.0, 0.0, 10.0, 10.0));
    rect["fills"] = json!([
        { "fillColor": "#ff0000", "fillOpacity": 1 },
        { "fillColor": "#0000ff", "fillOpacity": 1 },
    ]);
    rect["strokes"] = json!([
        { "strokeColor": "#000000", "strokeWidth": 2, "strokeStyle": "dotted" },
        { "strokeColor": "#ffffff", "strokeWidth": 2, "strokeStyle": "dashed" },
    ]);
    let r = import(export(&["R"], vec![rect], &[]));
    let paint = r.doc.get(one(&r.doc, "R")).unwrap().paint();
    let colour = |b: &BrushSource| match b {
        BrushSource::Literal(peniko::Brush::Solid(c)) => {
            let c = c.to_rgba8();
            [c.r, c.g, c.b]
        }
        other => panic!("{other:?}"),
    };
    let fills: Vec<[u8; 3]> = paint.fills.iter().map(|f| colour(&f.brush)).collect();
    assert_eq!(
        fills,
        [[0, 0, 255], [255, 0, 0]],
        "the first listed is on top"
    );
    let strokes: Vec<([u8; 3], Vec<f64>, Cap)> = paint
        .strokes
        .iter()
        .map(|s| (colour(&s.brush), s.dashes.clone(), s.cap))
        .collect();
    assert_eq!(
        strokes,
        [
            ([255, 255, 255], vec![12.0, 12.0], Cap::Butt),
            ([0, 0, 0], vec![0.0, 7.0], Cap::Round)
        ],
        "Penpot's dots are round"
    );
    assert!(r.report.is_empty(), "{:?}", r.report);
}

/// A background blur lands as one, hidden as Penpot hid it, and a layer blur
/// beside it stays a layer blur at twice its value, Penpot's value being the
/// deviation itself. flip: the `"background-blur"` arm in `penpot_in` back to
/// a report → fails at the length of `G`'s stack.
#[test]
fn a_background_blur_lands_as_one() {
    let mut glass = shape("G", "rect", ROOT, (0.0, 0.0, 10.0, 10.0));
    glass["blur"] = json!({ "type": "background-blur", "value": 8, "hidden": true });
    let mut soft = shape("S", "rect", ROOT, (20.0, 0.0, 10.0, 10.0));
    soft["blur"] = json!({ "type": "layer-blur", "value": 4, "hidden": false });
    let r = import(export(&["G", "S"], vec![glass, soft], &[]));
    assert!(r.report.is_empty(), "{:?}", r.report);
    let kind = |name| {
        let fx = r.doc.get(one(&r.doc, name)).unwrap().effects().to_vec();
        assert_eq!(fx.len(), 1, "{fx:?}");
        (fx[0].kind.clone(), fx[0].visible)
    };
    assert!(matches!(
        kind("G"),
        (ondin_core::EffectKind::BackgroundBlur { radius }, false) if radius == 8.0
    ));
    assert!(matches!(
        kind("S"),
        (ondin_core::EffectKind::LayerBlur { radius }, true) if radius == 8.0
    ));
}

/// A group that lists itself, and two groups that list each other, read with
/// each shape placed once and the repeat reported. flip: drop the
/// `nodes.contains_key` test in `shape` → fails at `one(doc, "G")`, the group
/// placed again at every level down to the depth limit.
#[test]
fn a_shape_listed_inside_itself_is_placed_once() {
    let mut own = shape("G", "group", ROOT, (0.0, 0.0, 10.0, 10.0));
    own["shapes"] = json!(["G"]);
    let mut a = shape("A", "group", ROOT, (0.0, 0.0, 10.0, 10.0));
    a["shapes"] = json!(["B"]);
    let mut b = shape("B", "group", "A", (0.0, 0.0, 10.0, 10.0));
    b["shapes"] = json!(["A"]);
    let r = import(export(&["G", "A"], vec![own, a, b], &[]));
    let doc = &r.doc;
    let (g, a, b) = (one(doc, "G"), one(doc, "A"), one(doc, "B"));
    assert!(doc.get(g).unwrap().children().is_empty());
    assert_eq!(doc.get(a).unwrap().children(), [b]);
    assert!(doc.get(b).unwrap().children().is_empty());
    let rows: Vec<(&str, &str)> = r
        .report
        .iter()
        .map(|u| (u.what, u.count.as_str()))
        .collect();
    assert_eq!(rows, [("Layers listed inside themselves or twice", "2")]);
}

/// A chain of groups thousands deep is cut off and reported, and what came in
/// saves and loads. flip: pass `depth` unchanged to the children in `shape` →
/// the test binary aborts with a stack overflow.
#[test]
fn a_chain_nested_past_the_limit_is_cut_off() {
    let n = 5_000;
    let shapes: Vec<Value> = (0..n)
        .map(|i| {
            let parent = if i == 0 {
                ROOT.to_owned()
            } else {
                format!("G{}", i - 1)
            };
            let mut g = shape(&format!("G{i}"), "group", &parent, (0.0, 0.0, 1.0, 1.0));
            g["shapes"] = json!([format!("G{}", i + 1)]);
            g
        })
        .collect();
    let r = import(export(&["G0"], shapes, &[]));
    let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
    assert_eq!(rows, ["Layers nested too deeply"]);
    let bytes = ondin_core::io::save(&r.doc).unwrap();
    ondin_core::io::load(&bytes).expect("the document loads");
}

/// Shapes under a board turned a quarter, or mirrored, land where Penpot puts
/// them on the page: each child is divided by its parent's whole placement.
/// flip: divide by only the parent's translation in `shape` → fails at the
/// rect's corner under the turned board.
#[test]
fn children_of_a_turned_or_mirrored_board_keep_their_place() {
    for (name, m) in [
        (
            "turned",
            json!({ "a": 0, "b": 1, "c": -1, "d": 0, "e": 0, "f": 0 }),
        ),
        (
            "mirrored",
            json!({ "a": -1, "b": 0, "c": 0, "d": 1, "e": 0, "f": 0 }),
        ),
    ] {
        let mut board = shape("B", "frame", ROOT, (100.0, 100.0, 100.0, 100.0));
        board["transform"] = m.clone();
        board["shapes"] = json!(["R", "P"]);
        // Both stored in page space, the rect turned with its board.
        let mut rect = shape("R", "rect", "B", (110.0, 120.0, 20.0, 10.0));
        rect["transform"] = m.clone();
        let mut path = shape("P", "path", "B", (0.0, 0.0, 0.0, 0.0));
        path["selrect"] = json!({ "x": 150, "y": 160, "width": 10, "height": 10 });
        path["content"] = json!("M150,160L160,170");
        let r = import(export(&["B"], vec![board, rect, path], &[]));
        let doc = &r.doc;
        let world = |id| {
            let mut t = Affine::IDENTITY;
            let mut at = Some(id);
            while let Some(n) = at.and_then(|i: NodeId| doc.get(i)) {
                t = n.transform() * t;
                at = n.parent();
            }
            t
        };
        let near = |a: ondin_core::kurbo::Point, b: (f64, f64)| {
            (a.x - b.0).abs() < 1e-9 && (a.y - b.1).abs() < 1e-9
        };
        // Penpot's box turns about its centre (120, 125), as the board's does.
        let c = ondin_core::kurbo::Vec2::new(120.0, 125.0);
        let mm = match &m {
            v if v["a"] == 0 => Affine::new([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]),
            _ => Affine::new([-1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        };
        let expect = Affine::translate(c)
            * mm
            * Affine::translate(-c)
            * ondin_core::kurbo::Point::new(110.0, 120.0);
        let corner = world(one(doc, "R")) * ondin_core::kurbo::Point::ZERO;
        assert!(near(corner, (expect.x, expect.y)), "{name}: {corner:?}");
        let NodeKind::Path { path, .. } = doc.get(one(doc, "P")).unwrap().kind() else {
            panic!("a path");
        };
        let start = match path.elements()[0] {
            ondin_core::kurbo::PathEl::MoveTo(p) => p,
            _ => panic!("starts with a move"),
        };
        assert!(
            near(world(one(doc, "P")) * start, (150.0, 160.0)),
            "{name}: the path stays where the page has it"
        );
    }
}

/// An image layer stretches its picture onto its box, a picture fill keeps its
/// aspect when Penpot says so, a masked group's first shape is its mask, and a
/// board that shows what passes its edge does not clip. flip: give the image
/// layer `image_brush`'s own fit in `shape` → fails at the image layer's fit.
#[test]
fn pictures_masks_and_unclipped_boards_land() {
    let mut board = shape("B", "frame", ROOT, (0.0, 0.0, 400.0, 400.0));
    board["showContent"] = json!(true);
    board["shapes"] = json!(["I", "F", "M"]);
    let mut image = shape("I", "image", "B", (0.0, 0.0, 200.0, 100.0));
    image["metadata"] = json!({ "id": "m1", "width": 100, "height": 100 });
    let mut filled = shape("F", "rect", "B", (0.0, 0.0, 50.0, 50.0));
    filled["fills"] = json!([{ "fillImage": { "id": "m1", "keepAspectRatio": true } }]);
    let mut group = shape("M", "group", "B", (0.0, 0.0, 50.0, 50.0));
    group["maskedGroup"] = json!(true);
    group["shapes"] = json!(["MK", "MC"]);
    let mask = shape("MK", "rect", "M", (0.0, 0.0, 50.0, 50.0));
    let masked = shape("MC", "rect", "M", (0.0, 0.0, 80.0, 80.0));
    let mut files = export(&["B"], vec![board, image, filled, group, mask, masked], &[]);
    files.0.insert(
        format!("files/{FILE}/media/m1.json"),
        json!({ "id": "m1", "mediaId": "o1" })
            .to_string()
            .into_bytes(),
    );
    files
        .0
        .insert("objects/o1.png".into(), b"\x89PNG\r\n\x1a\nrest".to_vec());
    let r = import(files);
    let doc = &r.doc;
    assert!(r.report.is_empty(), "{:?}", r.report);
    let picture = |name| match solid(doc, one(doc, name)) {
        BrushSource::Literal(peniko::Brush::Image(b)) => (b.image.id.0.clone(), b.image.fit),
        other => panic!("{name}: {other:?}"),
    };
    assert_eq!(picture("I"), ("test:12".to_owned(), ImageFit::Crop));
    assert_eq!(picture("F"), ("test:12".to_owned(), ImageFit::Fill));
    assert_eq!(doc.images().count(), 1, "one picture, placed once");
    assert!(doc.get(one(doc, "MK")).unwrap().mask());
    assert!(!doc.get(one(doc, "MC")).unwrap().mask());
    assert!(!doc.get(one(doc, "B")).unwrap().clip());
}

/// A radial gradient is a circle in the box's fractions, so on a 200×100 box
/// it reaches the box's sides and its top and bottom alike. flip: build it as
/// a circle of the scaled points' distance in `gradient` → fails at the right
/// edge, which a circle of radius 50 never reaches.
#[test]
fn a_radial_gradient_is_an_ellipse_of_the_box() {
    let mut rect = shape("R", "rect", ROOT, (0.0, 0.0, 200.0, 100.0));
    rect["fills"] = json!([{ "fillColorGradient": {
        "type": "radial", "startX": 0.5, "startY": 0.5, "endX": 0.5, "endY": 1.0, "width": 1.0,
        "stops": [{ "offset": 0, "color": "#000000" }, { "offset": 1, "color": "#ffffff" }],
    }}]);
    let r = import(export(&["R"], vec![rect], &[]));
    let BrushSource::Literal(peniko::Brush::Gradient(g)) = solid(&r.doc, one(&r.doc, "R")) else {
        panic!("a gradient");
    };
    let peniko::GradientKind::Radial(radial) = g.gradient.kind else {
        panic!("radial");
    };
    // The gradient's own unit circle, wherever `transform` places it.
    let (c, radius) = (radial.end_center, f64::from(radial.end_radius));
    let at = |angle: f64| {
        g.transform
            * ondin_core::kurbo::Point::new(c.x + radius * angle.cos(), c.y + radius * angle.sin())
    };
    let near = |p: ondin_core::kurbo::Point, x: f64, y: f64| {
        (p.x - x).abs() < 1e-6 && (p.y - y).abs() < 1e-6
    };
    let edge = [0.0, 0.5, 1.0, 1.5].map(|q| at(q * std::f64::consts::PI));
    let hits = |x: f64, y: f64| edge.iter().any(|&p| near(p, x, y));
    assert!(hits(200.0, 50.0) && hits(0.0, 50.0), "{edge:?}");
    assert!(hits(100.0, 0.0) && hits(100.0, 100.0), "{edge:?}");
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
