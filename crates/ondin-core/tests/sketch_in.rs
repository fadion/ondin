//! Sketch import (`ondin_core::sketch_in`): pages, layers and their geometry,
//! paints, text, symbols with their overrides, swatches and shared styles, the
//! report of what has no counterpart, and a result that saves and loads.

use ondin_core::image::{ImageEntry, ImageFormat, ImageId, ImageSource};
use ondin_core::kurbo::{PathEl, Point};
use ondin_core::peniko::{self, Color};
use ondin_core::sketch_in::{Archive, Read, font_face, read};
use ondin_core::svg_in::ImageSink;
use ondin_core::{BrushSource, Document, IdSource, NodeId, NodeKind, TextSizing};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The entries of a `.sketch` zip, held in memory.
#[derive(Default)]
struct Files(BTreeMap<String, Vec<u8>>);

impl Archive for Files {
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        self.0.get(name).cloned()
    }
}

/// A sink that takes any picture; its id is the byte length.
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

fn color(r: f64, g: f64, b: f64) -> Value {
    json!({ "_class": "color", "red": r, "green": g, "blue": b, "alpha": 1 })
}

fn frame(x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({ "_class": "rect", "x": x, "y": y, "width": w, "height": h })
}

fn fill(c: Value) -> Value {
    json!({ "_class": "fill", "isEnabled": true, "fillType": 0, "color": c })
}

fn corner(x: f64, y: f64) -> Value {
    let p = format!("{{{x}, {y}}}");
    json!({ "_class": "curvePoint", "point": p, "curveFrom": p, "curveTo": p, "cornerRadius": 4 })
}

fn rectangle(id: &str, x: f64, y: f64, style: Value) -> Value {
    json!({
        "_class": "rectangle", "do_objectID": id, "name": "Rect", "isVisible": true,
        "frame": frame(x, y, 40.0, 20.0), "style": style, "isClosed": true,
        "points": [corner(0.0, 0.0), corner(1.0, 0.0), corner(1.0, 1.0), corner(0.0, 1.0)],
    })
}

fn text(id: &str, string: &str, runs: Value, behaviour: i64) -> Value {
    json!({
        "_class": "text", "do_objectID": id, "name": "Label", "isVisible": true,
        "frame": frame(0.0, 0.0, 120.0, 30.0), "textBehaviour": behaviour,
        "attributedString": { "string": string, "attributes": runs },
    })
}

fn run(at: usize, len: usize, font: &str, size: f64, c: Value) -> Value {
    json!({
        "location": at, "length": len,
        "attributes": {
            "MSAttributedStringFontAttribute": { "attributes": { "name": font, "size": size } },
            "MSAttributedStringColorAttribute": c,
            "paragraphStyle": { "alignment": 2 },
        }
    })
}

/// A file of `pages`, each `(id, name, layers)`, with `extra` merged into the
/// document and `images` as entries.
fn sketch(pages: &[(&str, &str, Value)], extra: Value, images: &[(&str, Vec<u8>)]) -> Files {
    let mut files = Files::default();
    let mut document = json!({
        "_class": "document",
        "pages": pages.iter().map(|(id, _, _)| json!({ "_ref": format!("pages/{id}") }))
            .collect::<Vec<_>>(),
    });
    if let (Some(d), Some(e)) = (document.as_object_mut(), extra.as_object()) {
        d.extend(e.clone());
    }
    files
        .0
        .insert("document.json".into(), document.to_string().into_bytes());
    for (id, name, layers) in pages {
        let page = json!({ "_class": "page", "do_objectID": id, "name": name, "layers": layers });
        files
            .0
            .insert(format!("pages/{id}.json"), page.to_string().into_bytes());
    }
    for (name, bytes) in images {
        files.0.insert((*name).into(), bytes.clone());
    }
    files
}

fn import(mut files: Files) -> Read {
    let mut ids = IdSource::new(0x5E);
    read(&mut files, &mut ids, &mut Pictures).expect("the file reads")
}

/// Every node under `page`, depth first, with its name.
fn named(doc: &Document, page: NodeId, name: &str) -> Vec<NodeId> {
    ondin_core::build::subtree_nodes(doc, &[page])
        .into_iter()
        .filter(|&id| doc.get(id).is_some_and(|n| n.name() == name))
        .collect()
}

fn one(doc: &Document, name: &str) -> NodeId {
    let found: Vec<NodeId> = doc.page_ids().flat_map(|p| named(doc, p, name)).collect();
    assert_eq!(found.len(), 1, "one layer named {name:?}: {found:?}");
    found[0]
}

fn solid(doc: &Document, id: NodeId) -> Color {
    match doc.get(id).unwrap().paint().fills[0].brush.literal() {
        Some(peniko::Brush::Solid(c)) => *c,
        other => panic!("not a solid fill: {other:?}"),
    }
}

/// Pages keep their names and order; an artboard is a clipping frame with its
/// background; a rectangle is a rect with its corners rounded, placed in the
/// artboard's space.
#[test]
fn pages_artboards_and_rectangles_come_in_as_frames_and_rects() {
    let board = json!({
        "_class": "artboard", "do_objectID": "A", "name": "Home", "isVisible": true,
        "frame": frame(100.0, 50.0, 300.0, 200.0), "hasBackgroundColor": true,
        "backgroundColor": color(0.0, 0.0, 1.0),
        "layers": [rectangle("R", 10.0, 20.0, json!({ "fills": [fill(color(1.0, 0.0, 0.0))] }))],
    });
    let r = import(sketch(
        &[("P1", "Cover", json!([board])), ("P2", "Web", json!([]))],
        json!({}),
        &[],
    ));
    let doc = &r.doc;
    let names: Vec<String> = doc
        .page_ids()
        .map(|p| doc.get(p).unwrap().name().to_owned())
        .collect();
    assert_eq!(names, ["Cover", "Web"]);
    let home = one(doc, "Home");
    let node = doc.get(home).unwrap();
    assert!(matches!(node.kind(), NodeKind::Artboard { size, .. } if size.width == 300.0));
    assert!(node.clip(), "an artboard clips what it holds");
    assert_eq!(node.transform().translation().x, 100.0);
    assert_eq!(solid(doc, home), Color::new([0.0, 0.0, 1.0, 1.0]));
    let rect = one(doc, "Rect");
    assert_eq!(doc.get(rect).unwrap().parent(), Some(home));
    assert!(matches!(
        doc.get(rect).unwrap().kind(),
        NodeKind::Rect { size, corner_radii, .. }
            if size.width == 40.0 && corner_radii.top_left == 4.0
    ));
    assert_eq!(doc.get(rect).unwrap().transform().translation().y, 20.0);
    assert_eq!(solid(doc, rect), Color::new([1.0, 0.0, 0.0, 1.0]));
    assert!(r.report.is_empty(), "{:?}", r.report);
}

/// A shape path's segments run from a point's `curveFrom` to the next point's
/// `curveTo`, scaled to the frame; a straight one is a line.
#[test]
fn a_shape_path_draws_its_curve_points() {
    let path = json!({
        "_class": "shapePath", "do_objectID": "S", "name": "Wave", "isVisible": true,
        "frame": frame(0.0, 0.0, 200.0, 100.0), "isClosed": false,
        "style": { "borders": [{ "isEnabled": true, "fillType": 0, "color": color(0.0, 0.0, 0.0),
                                 "thickness": 2, "position": 1 }] },
        "points": [
            { "point": "{0, 0.5}", "curveFrom": "{0.25, 0}", "curveTo": "{0, 0.5}", "hasCurveFrom": true },
            { "point": "{0.5, 0.5}", "curveFrom": "{0.5, 0.5}", "curveTo": "{0.25, 1}", "hasCurveTo": true },
            { "point": "{1, 0.5}", "curveFrom": "{1, 0.5}", "curveTo": "{1, 0.5}" },
        ],
    });
    let r = import(sketch(&[("P", "Page", json!([path]))], json!({}), &[]));
    let id = one(&r.doc, "Wave");
    let node = r.doc.get(id).unwrap();
    let NodeKind::Path { path, .. } = node.kind() else {
        panic!("a path");
    };
    let els: Vec<PathEl> = path.elements().to_vec();
    assert_eq!(
        els,
        [
            PathEl::MoveTo(Point::new(0.0, 50.0)),
            PathEl::CurveTo(
                Point::new(50.0, 0.0),
                Point::new(50.0, 100.0),
                Point::new(100.0, 50.0)
            ),
            PathEl::LineTo(Point::new(200.0, 50.0)),
        ]
    );
    let stroke = &node.paint().strokes[0];
    assert_eq!(stroke.width, 2.0);
    assert_eq!(stroke.align, ondin_core::StrokeAlign::Inside);
}

/// A text layer's first run is its default, a later run's differences are spans
/// at the right bytes though Sketch counts UTF-16 units, and its box sizes as
/// Sketch's behaviour says.
#[test]
fn text_runs_become_a_default_and_spans() {
    let string = "😀 Hello";
    let t = text(
        "T",
        string,
        json!([
            run(0, 3, "Inter-Regular", 16.0, color(0.0, 0.0, 0.0)),
            run(3, 5, "Inter-SemiBoldItalic", 20.0, color(1.0, 0.0, 0.0)),
        ]),
        1,
    );
    let r = import(sketch(&[("P", "Page", json!([t]))], json!({}), &[]));
    let id = one(&r.doc, "Label");
    let NodeKind::Text {
        content,
        style,
        spans,
        sizing,
        paragraph,
        ..
    } = r.doc.get(id).unwrap().kind()
    else {
        panic!("a text node");
    };
    assert_eq!(content, string);
    assert_eq!((style.font_family.as_str(), style.weight), ("Inter", 400));
    assert_eq!(*sizing, TextSizing::AutoHeight(120.0));
    assert_eq!(paragraph.align, ondin_core::TextAlign::Center);
    let hello = string.find('H').unwrap();
    let weight: Vec<_> = spans
        .as_slice()
        .iter()
        .filter(|s| matches!(s.attr, ondin_core::CharAttr::Weight(600)))
        .map(|s| s.range())
        .collect();
    assert_eq!(
        weight,
        vec![hello..string.len()],
        "the second run, by bytes"
    );
    assert!(
        spans
            .as_slice()
            .iter()
            .any(|s| matches!(s.attr, ondin_core::CharAttr::Color(Some(_)))),
        "the red run keeps its colour"
    );
    assert_eq!(r.families.get("Inter"), Some(&2));
}

/// PostScript names split into a family, a weight and italic.
#[test]
fn a_postscript_name_reads_as_family_weight_and_italic() {
    assert_eq!(
        font_face("Inter-SemiBoldItalic"),
        ("Inter".into(), 600, true)
    );
    assert_eq!(font_face("Arial-BoldMT"), ("Arial".into(), 700, false));
    assert_eq!(
        font_face("TimesNewRomanPSMT"),
        ("Times New Roman".into(), 400, false)
    );
    assert_eq!(
        font_face("SFProDisplay-Light"),
        ("SF Pro Display".into(), 300, false)
    );
}

/// A symbol master is a component; an instance is a linked copy placed where the
/// instance was, with its text override written on the copy and no placeholder
/// left behind. flip: skip `overrides` in `place` → fails at the label.
#[test]
fn a_symbol_instance_is_a_linked_copy_with_its_overrides() {
    let master = json!({
        "_class": "symbolMaster", "do_objectID": "M", "name": "Button", "isVisible": true,
        "symbolID": "SYM", "frame": frame(0.0, 0.0, 120.0, 30.0),
        "layers": [text("L", "Label", json!([run(0, 5, "Inter-Regular", 14.0, color(0.0, 0.0, 0.0))]), 0)],
    });
    let instance = json!({
        "_class": "symbolInstance", "do_objectID": "I", "name": "Buy", "isVisible": true,
        "symbolID": "SYM", "frame": frame(300.0, 40.0, 120.0, 30.0),
        "overrideValues": [{ "overrideName": "L_stringValue", "value": "Buy now" }],
    });
    let r = import(sketch(
        &[("P", "Page", json!([master, instance]))],
        json!({}),
        &[],
    ));
    let doc = &r.doc;
    let main = one(doc, "Button");
    assert!(doc.get(main).unwrap().component());
    let buy = one(doc, "Buy");
    let node = doc.get(buy).unwrap();
    assert_eq!(node.link(), Some(main));
    assert_eq!(node.transform().translation().x, 300.0);
    let copy = node.children()[0];
    let NodeKind::Text { content, .. } = doc.get(copy).unwrap().kind() else {
        panic!("the label's copy");
    };
    assert_eq!(content, "Buy now");
    let page = doc.first_page();
    assert_eq!(doc.get(page).unwrap().children().len(), 2, "no placeholder");
}

/// A symbol whose master holds another symbol's instance comes in carrying the
/// nested instance. Flipping the placement order in `place_instances` does not
/// fail this: the commit's propagation also carries the inner instance into an
/// outer copy placed first, so the order is a second defence.
#[test]
fn a_nested_symbol_is_placed_before_the_symbol_holding_it() {
    let icon = json!({
        "_class": "symbolMaster", "do_objectID": "MI", "name": "Icon", "symbolID": "ICON",
        "frame": frame(0.0, 0.0, 10.0, 10.0),
        "layers": [rectangle("RI", 0.0, 0.0, json!({}))],
    });
    let button = json!({
        "_class": "symbolMaster", "do_objectID": "MB", "name": "Button", "symbolID": "BTN",
        "frame": frame(100.0, 0.0, 50.0, 20.0),
        "layers": [{ "_class": "symbolInstance", "do_objectID": "II", "name": "Inner icon",
                     "symbolID": "ICON", "frame": frame(2.0, 2.0, 10.0, 10.0) }],
    });
    // The outer instance first, so the order in the file is the wrong one.
    let outer = json!({
        "_class": "symbolInstance", "do_objectID": "IB", "name": "Outer", "symbolID": "BTN",
        "frame": frame(300.0, 0.0, 50.0, 20.0),
    });
    let r = import(sketch(
        &[("P", "Page", json!([outer, button, icon]))],
        json!({}),
        &[],
    ));
    let doc = &r.doc;
    let outer = one(doc, "Outer");
    let nested = doc.get(outer).unwrap().children()[0];
    assert!(
        matches!(doc.get(nested).unwrap().kind(), NodeKind::Artboard { .. }),
        "the copy holds the icon instance, not an empty group"
    );
    assert!(r.report.is_empty(), "{:?}", r.report);
}

/// Several instances placed together in one parent, between other layers, keep
/// the stacking their placeholders had, each with its own override.
/// flip: index each copy at its placeholder's position without counting the
/// copies inserted before it in `copy_of` → fails at the order.
#[test]
fn instances_sharing_a_parent_keep_their_places() {
    let master = json!({
        "_class": "symbolMaster", "do_objectID": "M", "name": "Chip", "symbolID": "S",
        "frame": frame(0.0, 0.0, 40.0, 20.0),
        "layers": [text("L", "Chip", json!([run(0, 4, "Inter-Regular", 12.0, color(0.0, 0.0, 0.0))]), 0)],
    });
    let layer = |id: &str, instance: bool| {
        if instance {
            json!({
                "_class": "symbolInstance", "do_objectID": id, "name": id, "symbolID": "S",
                "frame": frame(0.0, 0.0, 40.0, 20.0),
                "overrideValues": [{ "overrideName": "L_stringValue", "value": format!("Label {id}") }],
            })
        } else {
            let mut r = rectangle(id, 0.0, 0.0, json!({}));
            r["name"] = json!(id);
            r
        }
    };
    let order = |instances: bool| {
        let layers: Vec<Value> = ["A", "X", "Y", "B", "Z"]
            .iter()
            .map(|id| layer(id, instances && !matches!(*id, "A" | "B")))
            .collect();
        let group = json!({
            "_class": "group", "do_objectID": "G", "name": "Row",
            "frame": frame(500.0, 0.0, 200.0, 20.0), "layers": layers,
        });
        let r = import(sketch(
            &[("P", "Page", json!([master.clone(), group]))],
            json!({}),
            &[],
        ));
        let row = one(&r.doc, "Row");
        let names: Vec<String> = r
            .doc
            .get(row)
            .unwrap()
            .children()
            .iter()
            .map(|&c| r.doc.get(c).unwrap().name().to_owned())
            .collect();
        (names, r)
    };
    let (plain, _) = order(false);
    let (placed, r) = order(true);
    assert_eq!(placed, plain);
    for id in ["X", "Y", "Z"] {
        let copy = r.doc.get(one(&r.doc, id)).unwrap().children()[0];
        let NodeKind::Text { content, .. } = r.doc.get(copy).unwrap().kind() else {
            panic!("the label's copy");
        };
        assert_eq!(*content, format!("Label {id}"));
    }
}

/// A symbol master without a background shown in its instances comes in
/// unfilled, so an instance does not draw the white Sketch shows only behind
/// the master; an artboard without a background is still white.
/// flip: fill a master as an artboard in `layer` → fails at the instance.
#[test]
fn a_masters_background_shows_only_when_its_instances_take_it() {
    let master = json!({
        "_class": "symbolMaster", "do_objectID": "M", "name": "Cover", "symbolID": "S",
        "frame": frame(0.0, 0.0, 40.0, 40.0), "hasBackgroundColor": false,
        "backgroundColor": color(1.0, 1.0, 1.0), "includeBackgroundColorInInstance": false,
        "layers": [],
    });
    let board = json!({
        "_class": "artboard", "do_objectID": "A", "name": "Screen",
        "frame": frame(100.0, 0.0, 200.0, 200.0),
        "layers": [{ "_class": "symbolInstance", "do_objectID": "I", "name": "Art",
                     "symbolID": "S", "frame": frame(0.0, 0.0, 40.0, 40.0) }],
    });
    let r = import(sketch(
        &[("P", "Page", json!([master, board]))],
        json!({}),
        &[],
    ));
    let fills = |name: &str| r.doc.get(one(&r.doc, name)).unwrap().paint().fills.len();
    assert_eq!(fills("Art"), 0, "the instance");
    assert_eq!(fills("Screen"), 1, "the artboard");
}

/// A master's layers are pinned as Sketch pins them, and an instance's copy
/// carries the pins, so resizing it stretches them: both edges pinned is two px
/// insets, nothing pinned scales by percentages, a pinned width alone centres.
/// flip: skip `pin` in `children` → fails at the stretched layer.
#[test]
fn a_masters_layers_take_their_pins_as_insets() {
    use ondin_core::container::{AutoMargins, Insets, LengthPct::*};
    let layer = |id: &str, name: &str, x: f64, c: i64| {
        let mut r = rectangle(id, x, 0.0, json!({}));
        r["name"] = json!(name);
        r["resizingConstraint"] = json!(c);
        r
    };
    let master = json!({
        "_class": "symbolMaster", "do_objectID": "M", "name": "Bar", "symbolID": "S",
        "frame": frame(0.0, 0.0, 100.0, 20.0),
        // Left and right and top pinned; nothing; width alone.
        "layers": [layer("A", "Stretched", 30.0, 63 & !1 & !4 & !32),
                   layer("B", "Scaled", 30.0, 63), layer("C", "Centred", 30.0, 63 & !2 & !16)],
    });
    let instance = json!({ "_class": "symbolInstance", "do_objectID": "I", "name": "Wide",
        "symbolID": "S", "frame": frame(200.0, 0.0, 300.0, 20.0) });
    let r = import(sketch(
        &[("P", "Page", json!([master, instance]))],
        json!({}),
        &[],
    ));
    let copy = |name: &str| {
        let wide = one(&r.doc, "Wide");
        let found: Vec<NodeId> = ondin_core::build::subtree_nodes(&r.doc, &[wide])
            .into_iter()
            .filter(|&n| r.doc.get(n).is_some_and(|n| n.name() == name))
            .collect();
        *r.doc.get(found[0]).unwrap().insets()
    };
    let stretched = copy("Stretched");
    assert_eq!(
        (stretched.left, stretched.right),
        (Some(Px(30.0)), Some(Px(30.0)))
    );
    assert_eq!((stretched.top, stretched.bottom), (Some(Px(0.0)), None));
    let scaled = copy("Scaled");
    assert_eq!(
        (scaled.left, scaled.right),
        (Some(Percent(30.0)), Some(Percent(30.0)))
    );
    assert_eq!(
        copy("Centred"),
        Insets {
            left: Some(Px(0.0)),
            right: Some(Px(0.0)),
            top: Some(Px(0.0)),
            bottom: Some(Px(0.0)),
            margin_auto: AutoMargins {
                top: true,
                right: true,
                bottom: true,
                left: true
            },
        }
    );
}

/// A mask's own fill is drawn under what it clips, as Sketch draws it: the mask
/// paints nothing in Ondin, so an unmasking copy just above it carries the fill.
/// flip: skip the copy in `children` → fails at the layer count.
#[test]
fn a_masks_fill_is_drawn_under_what_it_clips() {
    let mut mask = rectangle(
        "K",
        0.0,
        0.0,
        json!({ "fills": [fill(color(0.8, 0.8, 0.8))] }),
    );
    mask["hasClippingMask"] = json!(true);
    mask["name"] = json!("Mask");
    let group = json!({
        "_class": "group", "do_objectID": "G", "name": "Cover",
        "frame": frame(0.0, 0.0, 40.0, 20.0), "layers": [mask, rectangle("R", 0.0, 0.0, json!({}))],
    });
    let r = import(sketch(&[("P", "Page", json!([group]))], json!({}), &[]));
    let page = r.doc.first_page();
    let masks = named(&r.doc, page, "Mask");
    assert_eq!(masks.len(), 2, "the mask and its paint");
    let (clip, ink) = (r.doc.get(masks[0]).unwrap(), r.doc.get(masks[1]).unwrap());
    assert!(clip.mask() && !ink.mask());
    assert_eq!(ink.paint().fills.len(), 1);
    let cover = r.doc.get(one(&r.doc, "Cover")).unwrap().children().to_vec();
    assert_eq!(
        cover,
        [masks[0], masks[1], one(&r.doc, "Rect")],
        "under the clipped layer"
    );
}

/// An alpha mask (`clippingMaskMode` 1) masks by its paint's alpha and gets no
/// copy of its paint above it, since its paint is the mask. flip: skip the
/// `SetMaskMode` in `create` → fails at the mode.
#[test]
fn an_alpha_mask_keeps_its_mode_and_draws_no_paint() {
    let mut mask = rectangle(
        "K",
        0.0,
        0.0,
        json!({ "fills": [fill(color(1.0, 1.0, 1.0))] }),
    );
    mask["hasClippingMask"] = json!(true);
    mask["clippingMaskMode"] = json!(1);
    mask["name"] = json!("Mask");
    let group = json!({
        "_class": "group", "do_objectID": "G", "name": "Fade",
        "frame": frame(0.0, 0.0, 40.0, 20.0), "layers": [mask, rectangle("R", 0.0, 0.0, json!({}))],
    });
    let r = import(sketch(&[("P", "Page", json!([group]))], json!({}), &[]));
    let mask = r.doc.get(one(&r.doc, "Mask")).unwrap();
    assert!(mask.mask());
    assert_eq!(mask.mask_mode(), ondin_core::node::MaskMode::Alpha);
    assert!(r.report.is_empty(), "{:?}", r.report);
}

/// A kerning Sketch writes as a widened float lands in the text's default as
/// the canonical length a span holds, so no span repeats it over the whole
/// text and the text stays bound to its shared style. flip: build the default
/// literally in `text_style`, without `canonical_style` → fails at the spans.
#[test]
fn a_widened_kerning_is_the_default_not_a_span() {
    let mut r0 = run(0, 5, "Inter-Regular", 14.0, color(0.0, 0.0, 0.0));
    r0["attributes"]["kerning"] = json!(0.23999999463558197);
    let mut label = text("T", "Hello", json!([r0.clone()]), 0);
    label["sharedStyleID"] = json!("TS");
    let styles = json!({ "layerTextStyles": { "objects": [{
        "do_objectID": "TS", "name": "Body",
        "value": { "textStyle": { "encodedAttributes": r0["attributes"] } },
    }]}});
    let r = import(sketch(&[("P", "Page", json!([label]))], styles, &[]));
    let node = r.doc.get(one(&r.doc, "Label")).unwrap();
    let NodeKind::Text { style, spans, .. } = node.kind() else {
        panic!("a text");
    };
    assert_eq!(style.letter_spacing, ondin_core::Length::Px(0.24));
    assert!(spans.as_slice().is_empty(), "{:?}", spans.as_slice());
    assert!(node.styles().text.is_some(), "bound to its shared style");
}

/// Two symbols holding each other's instances are left as empty groups and
/// reported, and the read returns. flip: place every pending at once in
/// `place_instances`, waiting or not → the read fails at `import`, its commit
/// refusing a master copied into itself.
#[test]
fn symbols_that_hold_each_other_are_left_empty() {
    let master = |id: &str, holds: &str| {
        json!({
            "_class": "symbolMaster", "do_objectID": format!("M{id}"), "name": id,
            "symbolID": id, "frame": frame(0.0, 0.0, 10.0, 10.0),
            "layers": [{ "_class": "symbolInstance", "do_objectID": format!("I{id}"),
                         "name": format!("{holds} in {id}"), "symbolID": holds,
                         "frame": frame(0.0, 0.0, 10.0, 10.0) }],
        })
    };
    let top = json!({ "_class": "symbolInstance", "do_objectID": "T", "name": "Top",
                      "symbolID": "A", "frame": frame(50.0, 0.0, 10.0, 10.0) });
    let r = import(sketch(
        &[(
            "P",
            "Page",
            json!([master("A", "B"), master("B", "A"), top]),
        )],
        json!({}),
        &[],
    ));
    let rows: Vec<(&str, &str)> = r
        .report
        .iter()
        .map(|u| (u.what, u.count.as_str()))
        .collect();
    assert_eq!(rows, [("Symbols that contain themselves", "3 instances")]);
    for name in ["B in A", "A in B", "Top"] {
        let g = r.doc.get(one(&r.doc, name)).unwrap();
        assert!(matches!(g.kind(), NodeKind::Group) && g.children().is_empty());
    }
}

/// An override reaching through a nested instance (`ID2/TC_stringValue`) lands
/// on the copy under that instance's copy in the outer instance, not on its
/// sibling's, the nested master's own text or another copy. flip: take only the
/// path's last step in `copy_at` → fails at `Top`, the first sibling changed.
#[test]
fn an_override_through_a_nested_instance_lands_on_its_copy() {
    let label = |id: &str| {
        text(
            id,
            "Deep",
            json!([run(0, 4, "Inter-Regular", 12.0, color(0.0, 0.0, 0.0))]),
            0,
        )
    };
    let c = json!({
        "_class": "symbolMaster", "do_objectID": "MC", "name": "C", "symbolID": "C",
        "frame": frame(0.0, 0.0, 50.0, 20.0), "layers": [label("TC")],
    });
    let d = json!({
        "_class": "symbolMaster", "do_objectID": "MD", "name": "D", "symbolID": "D",
        "frame": frame(100.0, 0.0, 50.0, 20.0),
        "layers": [{ "_class": "symbolInstance", "do_objectID": "ID1", "name": "Inner 1",
                     "symbolID": "C", "frame": frame(0.0, 0.0, 50.0, 20.0) },
                   { "_class": "symbolInstance", "do_objectID": "ID2", "name": "Inner 2",
                     "symbolID": "C", "frame": frame(0.0, 30.0, 50.0, 20.0) }],
    });
    let top = json!({ "_class": "symbolInstance", "do_objectID": "TOP", "name": "Top",
                      "symbolID": "D", "frame": frame(300.0, 0.0, 50.0, 20.0),
                      "overrideValues": [{ "overrideName": "ID2/TC_stringValue", "value": "changed" }] });
    let other = json!({ "_class": "symbolInstance", "do_objectID": "OTHER", "name": "Other",
                        "symbolID": "C", "frame": frame(400.0, 0.0, 50.0, 20.0) });
    let r = import(sketch(
        &[("P", "Page", json!([c, d, top, other]))],
        json!({}),
        &[],
    ));
    let doc = &r.doc;
    let page = doc.first_page();
    let words = |id: NodeId| match doc.get(id).unwrap().kind() {
        NodeKind::Text { content, .. } => content.clone(),
        _ => panic!("a text"),
    };
    let under = |name: &str| -> Vec<String> {
        let root = one(doc, name);
        ondin_core::build::subtree_nodes(doc, &[root])
            .into_iter()
            .filter(|&n| matches!(doc.get(n).unwrap().kind(), NodeKind::Text { .. }))
            .map(words)
            .collect()
    };
    assert_eq!(under("Top"), ["Deep", "changed"]);
    assert_eq!(under("C"), ["Deep"], "the master's own text");
    assert_eq!(
        under("D"),
        ["Deep", "Deep"],
        "the nested copies in D's master"
    );
    assert_eq!(under("Other"), ["Deep"]);
    assert_eq!(named(doc, page, "Label").len(), 6);
}

/// A swatch is a colour variable a fill naming it is bound to; a shared style is
/// an Ondin style its users are bound to while they match it.
#[test]
fn swatches_and_shared_styles_become_variables_and_styles() {
    let red = color(1.0, 0.0, 0.0);
    let mut bound = red.clone();
    bound["swatchID"] = json!("SW");
    let style = json!({ "fills": [fill(color(0.0, 1.0, 0.0))] });
    let extra = json!({
        "sharedSwatches": { "objects": [{ "do_objectID": "SW", "name": "Brand/Red", "value": red }] },
        "layerStyles": { "objects": [{ "do_objectID": "LS", "name": "Card", "value": style }] },
    });
    let mut matching = rectangle("A", 0.0, 0.0, style.clone());
    matching["name"] = json!("Matching");
    matching["sharedStyleID"] = json!("LS");
    let mut drifted = rectangle(
        "B",
        0.0,
        0.0,
        json!({ "fills": [fill(color(0.0, 0.0, 1.0))] }),
    );
    drifted["name"] = json!("Drifted");
    drifted["sharedStyleID"] = json!("LS");
    let mut swatched = rectangle("C", 0.0, 0.0, json!({ "fills": [fill(bound)] }));
    swatched["name"] = json!("Swatched");
    let r = import(sketch(
        &[("P", "Page", json!([matching, drifted, swatched]))],
        extra,
        &[],
    ));
    let doc = &r.doc;
    let var = doc.variables().iter().next().expect("the swatch");
    assert_eq!(var.name, "Brand/Red");
    let s = doc.get(one(doc, "Swatched")).unwrap().paint().fills[0]
        .brush
        .clone();
    assert_eq!(
        s,
        BrushSource::Bound {
            var: var.id,
            alpha: 1.0
        }
    );
    assert_eq!(doc.styles().iter().count(), 1);
    assert!(
        doc.get(one(doc, "Matching"))
            .unwrap()
            .styles()
            .fills
            .is_some()
    );
    assert!(
        doc.get(one(doc, "Drifted"))
            .unwrap()
            .styles()
            .fills
            .is_none(),
        "a layer out of step with its style is left unbound, as drawn"
    );
}

/// A picture is read once into the table and drawn as an image fill.
#[test]
fn a_bitmap_is_a_rect_showing_its_picture() {
    let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
    let bitmap = json!({
        "_class": "bitmap", "do_objectID": "B", "name": "Photo",
        "frame": frame(0.0, 0.0, 50.0, 40.0), "image": { "_ref": "images/abc.png" },
    });
    let r = import(sketch(
        &[("P", "Page", json!([bitmap]))],
        json!({}),
        &[("images/abc.png", png.clone())],
    ));
    assert_eq!(r.doc.image_count(), 1);
    let fill = &r.doc.get(one(&r.doc, "Photo")).unwrap().paint().fills[0];
    assert!(matches!(
        fill.brush.literal(),
        Some(peniko::Brush::Image(_))
    ));
}

/// What has no counterpart is reported, one row a kind, and the rest still lands.
#[test]
fn what_does_not_map_is_reported() {
    let mut linked = rectangle("R", 0.0, 0.0, json!({}));
    linked["flow"] = json!({ "destinationArtboardID": "X" });
    let group = json!({
        "_class": "group", "do_objectID": "G", "name": "Row",
        "frame": frame(0.0, 0.0, 10.0, 10.0),
        "groupLayout": { "_class": "MSImmutableInferredGroupLayout" },
        "layers": [linked],
    });
    let r = import(sketch(&[("P", "Page", json!([group]))], json!({}), &[]));
    let rows: Vec<(&str, &str)> = r.report.iter().map(|u| (u.what, u.how)).collect();
    assert_eq!(
        rows,
        [
            ("Prototype links", "Left out"),
            ("Smart layout", "Kept at their drawn positions"),
        ]
    );
    one(&r.doc, "Rect");
}

/// A background blur (blur type 3) lands as one, at its radius, and reports
/// nothing.
#[test]
fn a_background_blur_lands_as_one() {
    let blurred = rectangle(
        "R",
        0.0,
        0.0,
        json!({
            "blur": { "isEnabled": true, "type": 3, "radius": 10 },
        }),
    );
    let r = import(sketch(&[("P", "Page", json!([blurred]))], json!({}), &[]));
    assert!(r.report.is_empty(), "{:?}", r.report);
    let node = r.doc.get(one(&r.doc, "Rect")).unwrap();
    assert!(
        matches!(
            node.effects()[..],
            [ref e] if matches!(e.kind, ondin_core::EffectKind::BackgroundBlur { radius } if radius == 10.0)
        ),
        "{:?}",
        node.effects()
    );
}

/// A shape group of several shapes is a boolean of them carrying the group's
/// style.
#[test]
fn a_shape_group_of_several_shapes_is_a_boolean() {
    let mut second = rectangle("R2", 20.0, 0.0, json!({}));
    second["booleanOperation"] = json!(1);
    let group = json!({
        "_class": "shapeGroup", "do_objectID": "SG", "name": "Cutout",
        "frame": frame(0.0, 0.0, 60.0, 20.0),
        "style": { "fills": [fill(color(0.0, 0.0, 0.0))] },
        "layers": [rectangle("R1", 0.0, 0.0, json!({})), second],
    });
    let r = import(sketch(&[("P", "Page", json!([group]))], json!({}), &[]));
    let id = one(&r.doc, "Cutout");
    let node = r.doc.get(id).unwrap();
    assert!(matches!(
        node.kind(),
        NodeKind::Boolean {
            op: ondin_core::BoolOp::Subtract
        }
    ));
    assert_eq!(node.children().len(), 2);
    assert_eq!(node.paint().fills.len(), 1);
}

/// A shape group holding a text is a group whose layers keep their order, each
/// shape wearing the group's paint.
#[test]
fn a_shape_group_holding_a_text_keeps_its_layers_in_order() {
    let label = text(
        "T",
        "Hi",
        json!([run(0, 2, "Inter-Regular", 12.0, color(0.0, 0.0, 0.0))]),
        0,
    );
    let group = json!({
        "_class": "shapeGroup", "do_objectID": "SG", "name": "Badge",
        "frame": frame(0.0, 0.0, 60.0, 20.0),
        "style": { "fills": [fill(color(0.0, 1.0, 0.0))] },
        "layers": [rectangle("R1", 0.0, 0.0, json!({})), label, rectangle("R2", 20.0, 0.0, json!({}))],
    });
    let r = import(sketch(&[("P", "Page", json!([group]))], json!({}), &[]));
    let doc = &r.doc;
    let badge = doc.get(one(doc, "Badge")).unwrap();
    assert!(matches!(badge.kind(), NodeKind::Group));
    let kinds: Vec<&str> = badge
        .children()
        .iter()
        .map(|&c| match doc.get(c).unwrap().kind() {
            NodeKind::Rect { .. } => "rect",
            NodeKind::Text { .. } => "text",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["rect", "text", "rect"]);
    let last = badge.children()[2];
    assert_eq!(solid(doc, last), Color::new([0.0, 1.0, 0.0, 1.0]));
}

/// What the reader builds is a document the loader accepts.
#[test]
fn an_imported_document_saves_and_loads() {
    let master = json!({
        "_class": "symbolMaster", "do_objectID": "M", "name": "Chip", "symbolID": "S",
        "frame": frame(0.0, 0.0, 20.0, 20.0), "layers": [rectangle("R", 0.0, 0.0, json!({}))],
    });
    let inst = json!({
        "_class": "symbolInstance", "do_objectID": "I", "name": "Chip 1", "symbolID": "S",
        "frame": frame(40.0, 0.0, 30.0, 30.0),
    });
    let r = import(sketch(
        &[("P", "Page", json!([master, inst]))],
        json!({}),
        &[],
    ));
    let bytes = ondin_core::io::save(&r.doc).unwrap();
    ondin_core::io::load(&bytes).expect("the document loads");
    let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
    assert_eq!(rows, ["Resized symbols"]);
}

/// A file without `document.json` is refused, not read as empty.
#[test]
fn a_zip_that_is_not_a_sketch_file_is_refused() {
    let mut ids = IdSource::new(1);
    assert!(read(&mut Files::default(), &mut ids, &mut Pictures).is_err());
}

/// Two shared styles sharing a name, and two swatches, are numbered apart
/// rather than failing the build, and each renaming is reported as about the
/// styles or the variables, so an import leaving those out drops its row.
/// flip: `Names::unique` answering the name as given → the read fails.
#[test]
fn a_name_used_twice_is_numbered_apart() {
    let style = json!({ "fills": [fill(color(0.0, 1.0, 0.0))] });
    let red = color(1.0, 0.0, 0.0);
    let extra = json!({
        "sharedSwatches": { "objects": [
            { "do_objectID": "W1", "name": "Red", "value": red },
            { "do_objectID": "W2", "name": "Red", "value": red },
        ] },
        "layerStyles": { "objects": [
            { "do_objectID": "L1", "name": "Shadow/None", "value": style },
            { "do_objectID": "L2", "name": "Shadow/None", "value": style },
            { "do_objectID": "L3", "name": "", "value": style },
        ] },
    });
    let r = import(sketch(&[("P", "Page", json!([]))], extra, &[]));
    let mut styles: Vec<&str> = r.doc.styles().iter().map(|s| s.name.as_str()).collect();
    styles.sort();
    assert_eq!(styles, ["Shadow/None", "Shadow/None 2", "Style"]);
    let mut vars: Vec<&str> = r.doc.variables().iter().map(|v| v.name.as_str()).collect();
    vars.sort();
    assert_eq!(vars, ["Red", "Red 2"]);
    let rows: Vec<(&str, String, ondin_core::sketch_in::Part)> = r
        .report
        .iter()
        .map(|u| (u.what, u.count.clone(), u.part))
        .collect();
    use ondin_core::sketch_in::Part;
    assert_eq!(
        rows,
        [
            ("Colour names used twice", "1".to_owned(), Part::Variables),
            ("Style names used twice", "1".to_owned(), Part::Styles),
        ]
    );
}

/// A layer left out (a hotspot) takes no index, so the siblings after it still
/// land, in order.
/// flip: `children` passing the layer's position in the file → the read fails.
#[test]
fn a_layer_left_out_leaves_no_gap() {
    let hotspot = json!({
        "_class": "MSImmutableHotspotLayer", "do_objectID": "H", "name": "Hot",
        "frame": frame(0.0, 0.0, 10.0, 10.0),
    });
    let mut a = rectangle("A", 0.0, 0.0, json!({}));
    a["name"] = json!("A");
    let mut b = rectangle("B", 0.0, 0.0, json!({}));
    b["name"] = json!("B");
    let r = import(sketch(
        &[("P", "Page", json!([hotspot, a, b]))],
        json!({}),
        &[],
    ));
    let page = r.doc.page_ids().next().unwrap();
    let names: Vec<&str> = r
        .doc
        .get(page)
        .unwrap()
        .children()
        .iter()
        .map(|c| r.doc.get(*c).unwrap().name())
        .collect();
    assert_eq!(names, ["A", "B"]);
}

/// An older file's shape keeps its points in a nested `path`, and still draws
/// its outline; its archived text is reported, left empty.
/// flip: `shape_kind` reading only the layer's own `points` → fails at "the outline".
#[test]
fn an_older_files_shapes_and_text() {
    let shape = json!({
        "_class": "shapePath", "do_objectID": "S", "name": "Tri",
        "frame": frame(0.0, 0.0, 10.0, 10.0),
        "path": { "_class": "path", "isClosed": true,
            "points": [corner(0.0, 0.0), corner(1.0, 0.0), corner(0.5, 1.0)] },
    });
    let mut old_text = text("T", "", json!([]), 0);
    old_text["attributedString"] =
        json!({ "archivedAttributedString": { "_archive": "YnBsaXN0" } });
    let r = import(sketch(
        &[("P", "Page", json!([shape, old_text]))],
        json!({}),
        &[],
    ));
    match r.doc.get(one(&r.doc, "Tri")).unwrap().kind() {
        NodeKind::Path { path, .. } => {
            assert_eq!(
                path.elements().len(),
                5,
                "the outline: move, three sides, close"
            )
        }
        other => panic!("not a path: {other:?}"),
    }
    let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
    assert_eq!(rows, ["Texts in Sketch's older format"]);
}

/// "Play" in Avenir Black 14, blue, kerned 1.5, as Sketch before 48 archived it.
const ARCHIVED_PLAY: &str = "YnBsaXN0MDBVJG51bGxcQXZlbmlyLUJsYWNrgAEjQCwAAAAAAACAAl8QE05TRm9udE5hbWVBdHRyaWJ1dGWAA18QE05TRm9udFNpemVBdHRyaWJ1dGWABFdOUy5rZXlzWk5TLm9iamVjdHOiBgiiAgTSCQoLDIAFXxAaTlNGb250RGVzY3JpcHRvckF0dHJpYnV0ZXPRDw6ABkcwIDAgMSAxgAdcTlNDb21wb25lbnRz0RQTgAgjP/gAAAAAAACACV8QH01TQXR0cmlidXRlZFN0cmluZ0ZvbnRBdHRyaWJ1dGWACldOU0NvbG9ygAtWTlNLZXJugAxXTlMua2V5c1pOUy5vYmplY3RzoxocHqMRFhjSHyAhIoANVFBsYXmADlhOU1N0cmluZ1xOU0F0dHJpYnV0ZXPSJygmJIAPrxAQAAEDBQcNEBIVFxkbHSMlKVgkb2JqZWN0c1QkdG9wVHJvb3TRLirSLC0rLwAIAA4AGwAdACYAKAA+AEAAVgBYAGAAawBuAHEAdgB4AJUAmACaAKIApACxALQAtgC/AMEA4wDlAO0A7wD2APgBAAELAQ8BEwEYARoBHwEhASoBNwE8AT4BUQFaAV8BZAFnAAAAAAAAAgEAAAAAAAAAMQAAAAAAAAAwAAAAAAAAAWw=";

/// A text Sketch archived comes in with its words, font, size and colour, and
/// nothing is reported. flip: skip the archive in `text` → fails at the content.
#[test]
fn an_archived_text_comes_in_with_its_words_and_style() {
    let mut old = text("T", "", json!([]), 0);
    old["attributedString"] = json!({ "archivedAttributedString": { "_archive": ARCHIVED_PLAY } });
    let r = import(sketch(&[("P", "Page", json!([old]))], json!({}), &[]));
    let id = one(&r.doc, "Label");
    let NodeKind::Text { content, style, .. } = r.doc.get(id).unwrap().kind() else {
        panic!("a text");
    };
    assert_eq!(content, "Play");
    assert_eq!(style.font_family, "Avenir");
    assert_eq!(style.font_size, 14.0);
    assert_eq!(
        style.letter_spacing,
        ondin_core::typography::Length::Px(1.5)
    );
    assert_eq!(solid(&r.doc, id), Color::from_rgb8(0, 0, 255));
    assert!(r.report.is_empty(), "{:?}", r.report);
}

/// A layer's export presets become its export settings: format, scale or
/// size, and Sketch's name as a suffix or prefix, the bare `@2x` left to Ondin.
#[test]
fn export_presets_become_export_settings() {
    use ondin_core::export::{ExportFormat, ExportScale};
    let mut r = rectangle("A", 0.0, 0.0, json!({}));
    r["exportOptions"] = json!({ "exportFormats": [
        { "fileFormat": "png", "scale": 2, "name": "@2x", "namingScheme": 0, "visibleScaleType": 0 },
        { "fileFormat": "jpg", "scale": 1, "absoluteSize": 512, "name": "-big", "namingScheme": 0, "visibleScaleType": 1 },
        { "fileFormat": "svg", "scale": 1, "name": "icons/", "namingScheme": 1, "visibleScaleType": 0 },
        { "fileFormat": "tiff", "scale": 1, "name": "", "namingScheme": 0, "visibleScaleType": 0 },
    ] });
    let read = import(sketch(&[("P", "Page", json!([r]))], json!({}), &[]));
    let node = read.doc.get(one(&read.doc, "Rect")).unwrap();
    let got: Vec<_> = node
        .exports()
        .iter()
        .map(|k| {
            (
                k.value.format,
                k.value.scale,
                k.value.prefix.as_str(),
                k.value.suffix.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (ExportFormat::Png, ExportScale::Times(2.0), "", ""),
            (ExportFormat::Jpeg, ExportScale::Width(512), "", "-big"),
            (ExportFormat::Svg, ExportScale::Times(1.0), "icons/", ""),
        ]
    );
    let rows: Vec<&str> = read.report.iter().map(|u| u.what).collect();
    assert_eq!(rows, ["Export formats Ondin lacks"]);
}
