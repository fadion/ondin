//! Sketch export (`ondin_export::sketch`): a file whose layers, symbols and
//! assets all name each other, in the structure Sketch insists on, and which
//! `ondin_core::sketch_in` reads back as the document it was written from.

mod rich;

use ondin_core::{IdSource, NodeId, NodeKind};
use rich::{Pictures, Zip, fixture, named, s, same_place, unzip};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Every layer of every page, depth first, with the class of the layer it sits in.
fn layers(z: &rich::Unzipped) -> Vec<(String, Value)> {
    fn walk(v: &Value, parent: &str, out: &mut Vec<(String, Value)>) {
        for l in v["layers"].as_array().into_iter().flatten() {
            out.push((parent.to_owned(), l.clone()));
            walk(l, s(l, "_class"), out);
        }
    }
    let mut out = Vec::new();
    for (name, v) in &z.json {
        if name.starts_with("pages/") {
            walk(v, "page", &mut out);
        }
    }
    out
}

fn written() -> (rich::Fixture, rich::Unzipped) {
    let f = fixture();
    let out = ondin_export::sketch::export(&f.doc, &f.res, "Fixture").expect("written");
    let z = unzip(&out.bytes);
    (f, z)
}

/// Every reference resolves: pages, symbols, overrides, swatches, text styles
/// and pictures; ids are Sketch's upper-case UUIDs, each used once; and an
/// artboard or a master sits only at the top of a page.
#[test]
fn every_reference_in_the_file_resolves() {
    let (_, z) = written();
    let document = &z.json["document.json"];
    for page in document["pages"].as_array().unwrap() {
        assert!(z.json.contains_key(&format!("{}.json", s(page, "_ref"))));
    }
    assert!(z.json.contains_key("meta.json") && z.json.contains_key("user.json"));
    let all = layers(&z);
    let mut ids = BTreeSet::new();
    let is_uuid = |id: &str| {
        id.len() == 36
            && id
                .chars()
                .all(|c| c == '-' || c.is_ascii_digit() || c.is_ascii_uppercase())
    };
    for (parent, l) in &all {
        let id = s(l, "do_objectID");
        assert!(is_uuid(id), "{id}");
        assert!(ids.insert(id.to_owned()), "{id} twice");
        if matches!(s(l, "_class"), "artboard" | "symbolMaster") {
            assert_eq!(parent, "page", "{} below the top", s(l, "name"));
        }
    }
    let masters: BTreeMap<&str, &Value> = all
        .iter()
        .filter(|(_, l)| s(l, "_class") == "symbolMaster")
        .map(|(_, l)| (s(l, "symbolID"), l))
        .collect();
    let swatches: BTreeSet<&str> = document["sharedSwatches"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| s(w, "do_objectID"))
        .collect();
    let text_styles: BTreeSet<&str> = document["layerTextStyles"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| s(t, "do_objectID"))
        .collect();
    let mut reached = (0, 0, 0, 0);
    for (_, l) in &all {
        if s(l, "_class") == "symbolInstance" {
            let master = masters[s(l, "symbolID")];
            let inside: BTreeSet<String> = {
                let mut v = Vec::new();
                fn ids(l: &Value, out: &mut Vec<String>) {
                    for c in l["layers"].as_array().into_iter().flatten() {
                        out.push(s(c, "do_objectID").to_owned());
                        ids(c, out);
                    }
                }
                ids(master, &mut v);
                v.into_iter().collect()
            };
            for o in l["overrideValues"].as_array().unwrap() {
                let name = s(o, "overrideName");
                let first = name.split(['/', '_']).next().unwrap();
                assert!(
                    inside.contains(first),
                    "{name} names no layer of its symbol"
                );
                reached.0 += 1;
            }
        }
        for fill in l["style"]["fills"].as_array().into_iter().flatten() {
            if let Some(w) = fill["color"].get("swatchID") {
                assert!(swatches.contains(w.as_str().unwrap()));
                reached.1 += 1;
            }
            if let Some(img) = fill.get("image") {
                assert!(z.raw.contains_key(s(img, "_ref")), "{img}");
                reached.2 += 1;
            }
        }
        if let Some(st) = l.get("sharedStyleID") {
            assert!(text_styles.contains(st.as_str().unwrap()));
            reached.3 += 1;
        }
    }
    assert_eq!(reached.0, 1, "the copy overrides its label");
    assert_eq!(reached.1, 1, "the swatch fill names its swatch");
    assert_eq!(reached.2, 1, "the photo names its picture");
    assert_eq!(reached.3, 1, "the title names its text style");
}

/// The main moves to a Symbols page as a master, and an instance of it stands
/// where it was; the copy is another instance, its label overridden.
#[test]
fn a_main_becomes_a_master_with_an_instance_in_its_place() {
    let (_, z) = written();
    let all = layers(&z);
    let master = all
        .iter()
        .find(|(_, l)| s(l, "_class") == "symbolMaster")
        .map(|(_, l)| l)
        .expect("a master");
    assert_eq!(s(master, "name"), "Button");
    // Its corners round, which a background colour cannot: its paint is a
    // rounded rect at its bottom.
    assert_eq!(master["hasBackgroundColor"], Value::Bool(false));
    let paint = &master["layers"][0];
    assert_eq!(s(paint, "_class"), "rectangle");
    assert_eq!(paint["points"][0]["cornerRadius"].as_f64(), Some(8.0));
    // The screen's one solid fill is the artboard's own background.
    let screen = all
        .iter()
        .find(|(_, l)| s(l, "name") == "Screen")
        .map(|(_, l)| l)
        .unwrap();
    assert_eq!(s(screen, "_class"), "artboard");
    assert_eq!(screen["hasBackgroundColor"], Value::Bool(true));
    let instances: Vec<&Value> = all
        .iter()
        .filter(|(_, l)| s(l, "_class") == "symbolInstance")
        .map(|(_, l)| l)
        .collect();
    assert_eq!(instances.len(), 2, "the main's place and the copy");
    let overrides: Vec<&str> = instances
        .iter()
        .flat_map(|i| i["overrideValues"].as_array().unwrap())
        .map(|o| s(o, "value"))
        .collect();
    assert_eq!(overrides, ["Go"]);
}

/// A frame below a page's top is a group whose first layer carries its paint
/// and clips it.
#[test]
fn a_nested_frame_is_a_group_led_by_its_paint() {
    let (_, z) = written();
    let row = layers(&z)
        .into_iter()
        .map(|(_, l)| l)
        .find(|l| s(l, "name") == "Row" && s(l, "_class") == "group")
        .expect("the row, as a group");
    let first = &row["layers"][0];
    assert_eq!(s(first, "_class"), "rectangle");
    assert_eq!(first["hasClippingMask"], Value::Bool(true));
}

/// The mask clips the picture above it, and paints nothing of its own.
#[test]
fn a_mask_clips_and_does_not_paint() {
    let (_, z) = written();
    let clip = layers(&z)
        .into_iter()
        .map(|(_, l)| l)
        .find(|l| s(l, "name") == "Clip")
        .expect("the mask");
    assert_eq!(clip["hasClippingMask"], Value::Bool(true));
    let fills = clip["style"]["fills"].as_array().unwrap();
    assert!(!fills.is_empty() && fills.iter().all(|f| f["isEnabled"] == false));
}

/// A rect turned 30° clockwise is its unturned frame about the same centre,
/// turned 30° counter-clockwise in Sketch's sense.
#[test]
fn a_turned_rect_keeps_its_frame() {
    let (_, z) = written();
    let turned = layers(&z)
        .into_iter()
        .map(|(_, l)| l)
        .find(|l| (l["rotation"].as_f64().unwrap_or(0.0) + 30.0).abs() < 1e-6)
        .expect("the turned rect");
    assert!((turned["frame"]["width"].as_f64().unwrap() - 60.0).abs() < 1e-9);
    assert!((turned["frame"]["height"].as_f64().unwrap() - 30.0).abs() < 1e-9);
}

/// Writing the same document twice gives the same bytes.
#[test]
fn the_same_document_writes_the_same_file() {
    let f = fixture();
    let a = ondin_export::sketch::export(&f.doc, &f.res, "Fixture").unwrap();
    let b = ondin_export::sketch::export(&f.doc, &f.res, "Fixture").unwrap();
    assert!(a.bytes == b.bytes);
}

/// Sketch's reader takes the file back: the layers in their places, the swatch
/// still bound, the title in its style, and the copy an instance of the
/// button with its own label.
#[test]
fn sketch_import_reads_the_export_back() {
    let f = fixture();
    let out = ondin_export::sketch::export(&f.doc, &f.res, "Fixture").unwrap();
    let mut ids = IdSource::new(0x9E);
    let back = ondin_core::sketch_in::read(&mut Zip::of(out.bytes), &mut ids, &mut Pictures)
        .expect("read back");
    let doc = back.doc;
    for name in ["Screen", "Title", "Clip", "Glow"] {
        same_place(&f, &doc, name);
    }
    let bound = doc
        .page_ids()
        .flat_map(|p| ondin_core::build::subtree_nodes(&doc, &[p]))
        .find_map(|id| {
            doc.get(id)?
                .paint()
                .fills
                .iter()
                .find_map(|fl| fl.brush.var())
        })
        .expect("a bound fill");
    assert_eq!(doc.variables().get(bound).unwrap().name, "Brand/Primary");
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
            .any(|&id| doc.get(id).unwrap().link().is_some())
    );
}
