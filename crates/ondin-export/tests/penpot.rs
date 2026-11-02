//! Penpot export (`ondin_export::penpot`): a file whose shapes, assets and
//! components all name each other, and which `ondin_core::penpot_in` reads back
//! as the document it was written from.

use ondin_core::container::{Display, Flex, LayoutItem};
use ondin_core::foreign::Archive;
use ondin_core::kurbo::{Affine, Point, RoundedRectRadii, Size};
use ondin_core::peniko::{Brush, Color, Gradient};
use ondin_core::style::{Style, StyleValue};
use ondin_core::svg_in::ImageSink;
use ondin_core::variable::{Cell, Mode, Variable};
use ondin_core::{
    BrushSource, CharAttr, CharSpans, Document, Fill, IdSource, ImageEntry, ImageFormat, ImageId,
    ImageSource, NodeId, NodeKind, Operation, Resolved, TextSizing, TextStyle, Transaction,
    VarKind, build::Placement, keyed_by_position,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The ids the fixture's assertions follow.
struct Fixture {
    doc: Document,
    res: Resolved,
    /// The colour variable the swatch's fill is bound to.
    brand: ondin_core::VarId,
}

fn solid(id: NodeId, rgb: [u8; 3]) -> Operation {
    Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgb8(rgb[0], rgb[1], rgb[2])).into(),
            visible: true,
        }]),
    }
}

fn rect(id: NodeId, parent: NodeId, index: usize, size: (f64, f64), at: Affine) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Rect {
            size: Size::new(size.0, size.1),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(at),
        name: None,
    }
}

fn text(content: &str, size: f64) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(TextStyle {
            font_family: "Inter".into(),
            font_size: size,
            ..TextStyle::default()
        }),
        spans: CharSpans::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

/// A 4×4 PNG, so a picture fill has bytes to carry.
fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 40, 90, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .expect("encode");
    out.into_inner()
}

/// A page with a flex row, a component and an instance whose label is
/// overridden, a fill bound to a colour variable, a text style, a mask over a
/// picture, a turned rect, a radial gradient and a hidden, locked layer.
fn fixture() -> Fixture {
    let mut ids = IdSource::new(0x7E7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (mode, brand, heading) = (ids.mint_mode(), ids.mint_var(), ids.mint_style());
    let [
        screen,
        row,
        a,
        b,
        swatch,
        button,
        label,
        title,
        masked,
        mask,
        photo,
        turned,
        glow,
        ghost,
    ] = [(); 14].map(|_| ids.mint());
    let picture = ImageId("test:photo".into());
    let mut bold = CharSpans::default();
    let title_style = TextStyle {
        font_family: "Inter".into(),
        font_size: 24.0,
        ..TextStyle::default()
    };
    // The first run keeps the style's type, so a reader binds the style by it.
    bold.set(
        0..5,
        CharAttr::Color(Some(Color::from_rgb8(200, 0, 0))),
        &title_style,
    );
    bold.set(6..11, CharAttr::Weight(700), &title_style);
    doc.apply(&Transaction(vec![
        Operation::AddMode {
            mode: Mode {
                id: mode,
                name: "Default".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddVariable {
            var: Variable {
                id: brand,
                name: "Brand/Primary".into(),
                kind: VarKind::Color,
                cells: [(mode, Cell::Color(Color::from_rgb8(30, 90, 200)))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
        Operation::AddStyle {
            style: Style {
                id: heading,
                name: "Type/Heading".into(),
                value: StyleValue::Text(Box::new(ondin_core::style::type_part(&title_style))),
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
        Operation::AddImage {
            id: picture.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(png().into()),
                format: ImageFormat::Png,
                width: 4,
                height: 4,
            },
        },
        Operation::CreateNode {
            id: screen,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(480.0, 360.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((40.0, 30.0))),
            name: Some("Screen".into()),
        },
        solid(screen, [250, 250, 250]),
        // A flex row of two rects, the second growing.
        Operation::CreateNode {
            id: row,
            parent: screen,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(300.0, 60.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((20.0, 20.0))),
            name: Some("Row".into()),
        },
        Operation::SetDisplay {
            id: row,
            display: Some(Display::Flex(Flex {
                column_gap: 12.0,
                padding: [8.0, 10.0, 8.0, 10.0],
                ..Flex::default()
            })),
        },
        rect(a, row, 0, (40.0, 40.0), Affine::IDENTITY),
        solid(a, [10, 150, 120]),
        rect(b, row, 1, (40.0, 40.0), Affine::IDENTITY),
        solid(b, [240, 160, 20]),
        Operation::SetLayoutItem {
            id: b,
            item: LayoutItem {
                grow: 1.0,
                ..LayoutItem::default()
            },
        },
        // A swatch bound to the brand colour.
        rect(
            swatch,
            screen,
            1,
            (50.0, 50.0),
            Affine::translate((20.0, 100.0)),
        ),
        Operation::SetFills {
            id: swatch,
            fills: keyed_by_position([Fill {
                brush: BrushSource::Bound {
                    var: brand,
                    alpha: 1.0,
                },
                visible: true,
            }]),
        },
        // A main component: a frame holding a label.
        Operation::CreateNode {
            id: button,
            parent: screen,
            index: 2,
            kind: NodeKind::Artboard {
                size: Size::new(120.0, 40.0),
                corner_radii: RoundedRectRadii::from_single_radius(8.0),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((100.0, 100.0))),
            name: Some("Button".into()),
        },
        solid(button, [30, 30, 40]),
        Operation::CreateNode {
            id: label,
            parent: button,
            index: 0,
            kind: text("Press", 14.0),
            transform: Some(Affine::translate((12.0, 10.0))),
            name: Some("Label".into()),
        },
        solid(label, [255, 255, 255]),
        Operation::SetComponent {
            id: button,
            component: true,
        },
        // A title in the heading style, with a bold run and a red one.
        Operation::CreateNode {
            id: title,
            parent: screen,
            index: 3,
            kind: NodeKind::Text {
                content: "Hello world\nSecond line".into(),
                style: Box::new(title_style.clone()),
                spans: bold,
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate((20.0, 180.0))),
            name: Some("Title".into()),
        },
        solid(title, [20, 20, 30]),
        Operation::SetTypeStyle {
            id: title,
            style: Some(heading),
        },
        // A circle masking a picture.
        Operation::CreateNode {
            id: masked,
            parent: screen,
            index: 4,
            kind: NodeKind::Group,
            transform: None,
            name: Some("Avatar".into()),
        },
        Operation::CreateNode {
            id: mask,
            parent: masked,
            index: 0,
            kind: NodeKind::Ellipse {
                size: Size::new(60.0, 60.0),
                arc: Default::default(),
            },
            transform: Some(Affine::translate((300.0, 100.0))),
            name: Some("Clip".into()),
        },
        solid(mask, [0, 0, 0]),
        Operation::SetMask {
            id: mask,
            mask: true,
        },
        rect(
            photo,
            masked,
            1,
            (80.0, 80.0),
            Affine::translate((290.0, 90.0)),
        ),
        Operation::SetFills {
            id: photo,
            fills: keyed_by_position([Fill {
                brush: ondin_core::image_brush(picture).into(),
                visible: true,
            }]),
        },
        // A rect turned a quarter of the way round, and a radial glow.
        rect(
            turned,
            screen,
            5,
            (60.0, 30.0),
            Affine::translate((380.0, 220.0)) * Affine::rotate(30f64.to_radians()),
        ),
        solid(turned, [120, 60, 200]),
        Operation::CreateNode {
            id: glow,
            parent: screen,
            index: 6,
            kind: NodeKind::Ellipse {
                size: Size::new(100.0, 60.0),
                arc: Default::default(),
            },
            transform: Some(Affine::translate((40.0, 260.0))),
            name: Some("Glow".into()),
        },
        Operation::SetFills {
            id: glow,
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(
                    Gradient::new_radial(Point::new(50.0, 30.0), 30.0)
                        .with_stops([
                            (0.0_f32, Color::from_rgb8(255, 220, 0)),
                            (1.0_f32, Color::from_rgb8(255, 80, 0)),
                        ])
                        .into(),
                )
                .into(),
                visible: true,
            }]),
        },
        rect(
            ghost,
            screen,
            7,
            (20.0, 20.0),
            Affine::translate((440.0, 20.0)),
        ),
        solid(ghost, [0, 0, 0]),
        Operation::SetVisible {
            id: ghost,
            visible: false,
        },
        Operation::SetLocked {
            id: ghost,
            locked: true,
        },
    ]))
    .expect("build the fixture");

    // An instance of the button, its label overridden.
    let (tx, made) = ondin_core::insert_subtrees(
        &doc,
        &mut ids,
        &[Placement {
            nodes: doc.capture_subtree(button).expect("the main"),
            parent: screen,
            index: None,
        }],
        Default::default(),
    );
    doc.apply(&tx).expect("an instance");
    let instance = made[0];
    let copy_label = doc.get(instance).unwrap().children()[0];
    doc.apply(&Transaction(vec![
        Operation::SetTransform {
            id: instance,
            transform: Affine::translate((100.0, 260.0)),
        },
        Operation::SetText {
            id: copy_label,
            content: "Go".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        },
    ]))
    .expect("override the copy");
    let res = Resolved::rebuild(&doc);
    Fixture { doc, res, brand }
}

/// The written file's entries, JSON parsed.
struct Unzipped {
    json: BTreeMap<String, Value>,
    raw: BTreeMap<String, Vec<u8>>,
}

fn unzip(bytes: &[u8]) -> Unzipped {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("a zip");
    let (mut json, mut raw) = (BTreeMap::new(), BTreeMap::new());
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        let name = f.name().to_owned();
        if name.ends_with(".json") {
            json.insert(name.clone(), serde_json::from_slice(&v).expect("JSON"));
        }
        raw.insert(name, v);
    }
    Unzipped { json, raw }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Every shape names a parent that lists it and a frame that is a board, and
/// every asset a shape names is in the file: what Penpot's import checks.
#[test]
fn every_reference_in_the_file_resolves() {
    let f = fixture();
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").expect("written");
    let z = unzip(&out.bytes);
    let manifest = &z.json["manifest.json"];
    let file = s(&manifest["files"][0], "id").to_owned();
    assert_eq!(s(manifest, "type"), "penpot/export-files");
    assert!(z.json.contains_key(&format!("files/{file}.json")));

    let mut shapes: BTreeMap<String, Value> = BTreeMap::new();
    for (name, v) in &z.json {
        if name.starts_with(&format!("files/{file}/pages/")) && name.matches('/').count() == 4 {
            shapes.insert(s(v, "id").to_owned(), v.clone());
        }
    }
    let colours: BTreeSet<String> = z
        .json
        .keys()
        .filter(|n| n.contains("/colors/"))
        .map(|n| {
            n.rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(".json")
                .to_owned()
        })
        .collect();
    let typographies: BTreeSet<String> = z
        .json
        .keys()
        .filter(|n| n.contains("/typographies/"))
        .map(|n| {
            n.rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(".json")
                .to_owned()
        })
        .collect();
    let mut refs = (0, 0, 0, 0);
    for (id, shape) in &shapes {
        let parent = &shapes[s(shape, "parentId")];
        let listed = parent["shapes"]
            .as_array()
            .is_some_and(|l| l.iter().any(|c| c.as_str() == Some(id)));
        assert!(
            listed || id == s(shape, "parentId"),
            "{id} not in its parent"
        );
        assert_eq!(s(&shapes[s(shape, "frameId")], "type"), "frame", "{id}");
        for child in shape["shapes"].as_array().into_iter().flatten() {
            let child = child.as_str().unwrap();
            assert_eq!(s(&shapes[child], "parentId"), id, "{child}'s parent");
        }
        for fill in shape["fills"].as_array().into_iter().flatten() {
            if let Some(c) = fill.get("fillColorRefId") {
                assert!(colours.contains(c.as_str().unwrap()));
                refs.0 += 1;
            }
            if let Some(img) = fill.get("fillImage") {
                let media = format!("files/{file}/media/{}.json", s(img, "id"));
                let object = s(&z.json[&media], "mediaId").to_owned();
                let meta = &z.json[&format!("objects/{object}.json")];
                let bytes = &z.raw[&format!("objects/{object}.png")];
                assert_eq!(meta["size"].as_u64(), Some(bytes.len() as u64));
                refs.1 += 1;
            }
        }
        if let Some(t) = shape["content"].get("children") {
            for p in t[0]["children"].as_array().unwrap() {
                for leaf in p["children"].as_array().unwrap() {
                    if let Some(r) = leaf.get("typographyRefId") {
                        assert!(typographies.contains(r.as_str().unwrap()));
                        refs.2 += 1;
                    }
                }
            }
        }
        if let Some(r) = shape.get("shapeRef") {
            assert!(
                shapes.contains_key(r.as_str().unwrap()),
                "{id}'s main layer"
            );
            refs.3 += 1;
        }
        if shape.get("mainInstance").is_some() {
            let c = format!("files/{file}/components/{}.json", s(shape, "componentId"));
            assert_eq!(s(&z.json[&c], "mainInstanceId"), id);
        }
    }
    // The fixture reached each kind of reference.
    assert_eq!(refs.0, 1, "the swatch names its library colour");
    assert_eq!(refs.1, 1, "the photo names its picture");
    assert!(refs.2 >= 2, "the title's runs name their typography");
    assert_eq!(refs.3, 2, "the copy and its label name the main's layers");
}

/// The masked group holds the mask first, then the picture it clips.
#[test]
fn a_mask_and_its_run_become_a_masked_group() {
    let f = fixture();
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let z = unzip(&out.bytes);
    let clip = z
        .json
        .values()
        .find(|v| s(v, "name") == "Clip" && s(v, "type") == "circle")
        .expect("the mask");
    let group = z
        .json
        .values()
        .find(|v| v.get("maskedGroup") == Some(&Value::Bool(true)))
        .expect("a masked group");
    let kids = group["shapes"].as_array().unwrap();
    assert_eq!(kids[0].as_str(), Some(s(clip, "id")));
    assert_eq!(kids.len(), 2);
}

/// The copy names the layers it copies and lists its overridden text as touched.
#[test]
fn a_copy_names_its_main_and_what_it_overrides() {
    let f = fixture();
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let z = unzip(&out.bytes);
    let copies: Vec<&Value> = z
        .json
        .values()
        .filter(|v| v.get("shapeRef").is_some())
        .collect();
    let root = copies
        .iter()
        .find(|v| v.get("componentRoot").is_some())
        .expect("the copy's root");
    assert_eq!(s(root, "name"), "Button");
    let label = copies.iter().find(|v| s(v, "type") == "text").unwrap();
    let touched: Vec<&str> = label["touched"]
        .as_array()
        .expect("the label's overrides")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(touched.contains(&"content-group"), "{touched:?}");
    assert!(
        root.get("touched")
            .is_none_or(|t| { !t.as_array().unwrap().iter().any(|g| g == "content-group") })
    );
}

/// The row is Penpot's flex board at its own width; its growing item fills the
/// row, the other keeps its width, and both stretch across it.
/// flip: a frame hugging under an `auto` width → fails at the row's sizing.
#[test]
fn a_flex_row_keeps_its_sizes() {
    let f = fixture();
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let z = unzip(&out.bytes);
    let shape = |name: &str, kind: &str| {
        z.json
            .values()
            .find(|v| s(v, "name") == name && s(v, "type") == kind)
            .unwrap_or_else(|| panic!("{name}"))
            .clone()
    };
    let row = shape("Row", "frame");
    assert_eq!(s(&row, "layout"), "flex");
    assert_eq!(row["layoutGap"]["columnGap"].as_f64(), Some(12.0));
    assert_eq!(s(&row, "layoutItemHSizing"), "fix");
    let items: Vec<&Value> = z
        .json
        .values()
        .filter(|v| s(v, "parentId") == s(&row, "id"))
        .collect();
    let sizing: BTreeSet<(&str, &str)> = items
        .iter()
        .map(|v| (s(v, "layoutItemHSizing"), s(v, "layoutItemVSizing")))
        .collect();
    assert_eq!(sizing, BTreeSet::from([("fix", "fill"), ("fill", "fill")]));
}

/// A rect turned 30° is its unturned box about the same centre, turned.
#[test]
fn a_turned_rect_keeps_its_box_and_turns_about_its_centre() {
    let f = fixture();
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let z = unzip(&out.bytes);
    let turned = z
        .json
        .values()
        .find(|v| (v["rotation"].as_f64().unwrap_or(0.0) - 30.0).abs() < 1e-6)
        .expect("the turned rect");
    assert!((turned["width"].as_f64().unwrap() - 60.0).abs() < 1e-9);
    assert!((turned["height"].as_f64().unwrap() - 30.0).abs() < 1e-9);
    let t = &turned["transform"];
    let (a, b) = (t["a"].as_f64().unwrap(), t["b"].as_f64().unwrap());
    assert!(
        (a - 30f64.to_radians().cos()).abs() < 1e-9 && (b - 30f64.to_radians().sin()).abs() < 1e-9
    );
}

/// Writing the same document twice gives the same bytes.
#[test]
fn the_same_document_writes_the_same_file() {
    let f = fixture();
    let a = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let b = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    assert!(a.bytes == b.bytes);
}

struct Zip(zip::ZipArchive<std::io::Cursor<Vec<u8>>>);

impl Archive for Zip {
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut f = self.0.by_name(name).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(v)
    }
    fn names(&mut self) -> Vec<String> {
        self.0.file_names().map(str::to_owned).collect()
    }
}

struct Pictures;

impl ImageSink for Pictures {
    fn place(&mut self, bytes: Vec<u8>, format: ImageFormat) -> Option<(ImageId, ImageEntry)> {
        Some((
            ImageId(format!("back:{}", bytes.len())),
            ImageEntry {
                source: ImageSource::Embedded(bytes.into()),
                format,
                width: 4,
                height: 4,
            },
        ))
    }
}

fn named(doc: &Document, name: &str) -> NodeId {
    let found: Vec<NodeId> = doc
        .page_ids()
        .flat_map(|p| ondin_core::build::subtree_nodes(doc, &[p]))
        .filter(|&id| doc.get(id).is_some_and(|n| n.name() == name))
        .collect();
    assert!(!found.is_empty(), "a layer named {name:?}");
    found[0]
}

/// Penpot's reader takes the file back: the layers in their places, the swatch
/// still bound, the title in its style, the copy linked with its own label.
#[test]
fn penpot_import_reads_the_export_back() {
    let f = fixture();
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let mut zip = Zip(zip::ZipArchive::new(std::io::Cursor::new(out.bytes)).unwrap());
    let mut ids = IdSource::new(0x9E);
    let back = ondin_core::penpot_in::read(&mut zip, &mut ids, &mut Pictures).expect("read back");
    let doc = back.doc;
    let res = Resolved::rebuild(&doc);
    for name in ["Screen", "Row", "Button", "Title", "Clip", "Glow"] {
        let (a, b) = (named(&f.doc, name), named(&doc, name));
        let (wa, wb) = (f.res.world_bounds(a).unwrap(), res.world_bounds(b).unwrap());
        for (x, y) in [
            (wa.x0, wb.x0),
            (wa.y0, wb.y0),
            (wa.x1, wb.x1),
            (wa.y1, wb.y1),
        ] {
            assert!((x - y).abs() < 0.5, "{name}: {wa:?} came back as {wb:?}");
        }
    }
    let swatch = doc
        .page_ids()
        .flat_map(|p| ondin_core::build::subtree_nodes(&doc, &[p]))
        .find(|&id| {
            doc.get(id)
                .is_some_and(|n| n.paint().fills.iter().any(|fl| fl.brush.is_bound()))
        })
        .expect("a bound fill");
    let var = doc.get(swatch).unwrap().paint().fills[0]
        .brush
        .var()
        .unwrap();
    assert_eq!(doc.variables().get(var).unwrap().name, "Brand/Primary");
    let _ = f.brand;
    assert!(
        doc.get(named(&doc, "Title"))
            .unwrap()
            .styles()
            .text
            .is_some()
    );
    let labels: Vec<NodeId> = doc
        .page_ids()
        .flat_map(|p| ondin_core::build::subtree_nodes(&doc, &[p]))
        .filter(|&id| doc.get(id).is_some_and(|n| n.name() == "Label"))
        .collect();
    let contents: BTreeSet<String> = labels
        .iter()
        .filter_map(|&id| match doc.get(id).unwrap().kind() {
            NodeKind::Text { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        contents,
        BTreeSet::from(["Go".to_owned(), "Press".to_owned()])
    );
    assert!(
        labels
            .iter()
            .any(|&id| doc.get(id).unwrap().link().is_some()),
        "the copy's label is linked to the main's"
    );
}
