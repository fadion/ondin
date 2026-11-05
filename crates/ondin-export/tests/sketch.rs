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

/// The mask clips the picture above it, and paints nothing of its own: no
/// fill, no border.
/// flip: `flags` turning off the fills only → fails at the borders.
#[test]
fn a_mask_clips_and_does_not_paint() {
    let (_, z) = written();
    let clip = layers(&z)
        .into_iter()
        .map(|(_, l)| l)
        .find(|l| s(l, "name") == "Clip")
        .expect("the mask");
    assert_eq!(clip["hasClippingMask"], Value::Bool(true));
    for key in ["fills", "borders"] {
        let paints = clip["style"][key].as_array().unwrap();
        assert!(!paints.is_empty(), "the fixture: the mask has {key}");
        assert!(paints.iter().all(|f| f["isEnabled"] == false), "{key}");
    }
}

/// A frame's opacity and shadow are drawn once: by the group a nested frame
/// becomes, and by each instance of a main, never again by the paint layer
/// that leads it.
/// flip: `frame_paint` styling its rect with `style_of` → fails at the row's
/// paint layer.
#[test]
fn a_frames_paint_layer_carries_no_effects() {
    let (_, z) = written();
    let all = layers(&z);
    let opacity = |l: &Value| l["style"]["contextSettings"]["opacity"].as_f64();
    let shadows = |l: &Value| l["style"]["shadows"].as_array().map_or(0, Vec::len);
    let row = all
        .iter()
        .map(|(_, l)| l)
        .find(|l| s(l, "name") == "Row" && s(l, "_class") == "group")
        .expect("the row, as a group");
    assert_eq!((opacity(row), shadows(row)), (Some(0.5), 1), "on the group");
    let paint = &row["layers"][0];
    assert_eq!(s(paint, "_class"), "rectangle");
    assert_eq!(
        (opacity(paint), shadows(paint)),
        (Some(1.0), 0),
        "row's paint"
    );
    let master = all
        .iter()
        .map(|(_, l)| l)
        .find(|l| s(l, "_class") == "symbolMaster")
        .expect("the button's master");
    let paint = &master["layers"][0];
    assert_eq!(s(paint, "_class"), "rectangle");
    assert_eq!(shadows(paint), 0, "the master's paint");
    let instances: Vec<&Value> = all
        .iter()
        .map(|(_, l)| l)
        .filter(|l| s(l, "_class") == "symbolInstance")
        .collect();
    assert!(!instances.is_empty() && instances.iter().all(|i| shadows(i) == 1));
}

/// A copy whose label differs from its main's in its runs is written as plain
/// layers, since a string override carries no runs, and the report says so.
/// flip: `text_overrides` taking a `SetText` by its content alone → fails at
/// "plain".
#[test]
fn a_copy_whose_runs_differ_is_written_plain() {
    let f = fixture();
    let out = ondin_export::sketch::export(&f.doc, &f.res, "Fixture").unwrap();
    let z = unzip(&out.bytes);
    let flipped: Vec<String> = layers(&z)
        .into_iter()
        .filter(|(_, l)| s(l, "name") == "Flipped")
        .map(|(_, l)| s(&l, "_class").to_owned())
        .collect();
    // The group and the rect leading it with its paint.
    assert_eq!(flipped, ["group", "rectangle"], "plain");
    assert!(
        out.approximated
            .iter()
            .any(|a| a.starts_with("Instances that differ"))
    );
}

/// What Sketch cannot hold of the fixture, each once, in the order met.
/// flip: `text_overrides` taking a `SetText` by its content alone → fails at
/// the list, the plain-layers line gone.
#[test]
fn the_report_names_what_sketch_cannot_hold() {
    let f = fixture();
    let out = ondin_export::sketch::export(&f.doc, &f.res, "Fixture").unwrap();
    assert_eq!(
        out.approximated,
        [
            "Guides, left out",
            "Auto layout, kept at drawn positions",
            "Frames inside frames or groups, as groups",
            "Instances that differ from their main past text, as plain layers",
            "Layout grids, left out",
        ]
    );
}

/// A text override inside a nested copy that its master writes as plain layers
/// names the text by its own id there; every path resolves segment by segment,
/// through symbol instances only, to a text layer.
/// flip: `override_path` prefixing the nearest copy whether or not it is
/// written as an instance → fails at the resolution.
#[test]
fn an_override_through_a_plain_nested_copy_resolves() {
    use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
    use ondin_core::{
        Document, Fill, Operation, Resolved, TextSizing, TextStyle, Transaction, build::Placement,
        keyed_by_position, peniko::Color,
    };
    let mut ids = IdSource::new(0x5E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [n, t, m] = [(); 3].map(|_| ids.mint());
    let frame = |id, w, x| Operation::CreateNode {
        id,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(w, 60.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((x, 0.0))),
        name: None,
    };
    doc.apply(&Transaction(vec![
        frame(n, 100.0, 0.0),
        Operation::CreateNode {
            id: t,
            parent: n,
            index: 0,
            kind: NodeKind::Text {
                content: "Hi".into(),
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
            transform: None,
            name: None,
        },
        Operation::SetComponent {
            id: n,
            component: true,
        },
        frame(m, 200.0, 200.0),
    ]))
    .unwrap();
    let copy_into = |doc: &mut Document, ids: &mut IdSource, main, parent| {
        let (tx, made) = ondin_core::insert_subtrees(
            doc,
            ids,
            &[Placement {
                nodes: doc.capture_subtree(main).unwrap(),
                parent,
                index: None,
            }],
            Default::default(),
        );
        doc.apply(&tx).unwrap();
        made[0]
    };
    // M holds a copy of N with a fill of its own, which M's master writes plain.
    let r = copy_into(&mut doc, &mut ids, n, m);
    doc.apply(&Transaction(vec![
        Operation::SetFills {
            id: r,
            fills: keyed_by_position([Fill {
                brush: ondin_core::peniko::Brush::Solid(Color::from_rgb8(9, 9, 9)).into(),
                visible: true,
            }]),
        },
        Operation::SetComponent {
            id: m,
            component: true,
        },
    ]))
    .unwrap();
    let i = copy_into(&mut doc, &mut ids, m, root);
    let text = doc
        .get(doc.get(i).unwrap().children()[0])
        .unwrap()
        .children()[0];
    doc.apply(&Transaction(vec![Operation::SetText {
        id: text,
        content: "Yo".into(),
        spans: Default::default(),
        para_spans: Default::default(),
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let out = ondin_export::sketch::export(&doc, &res, "Nested").unwrap();
    let z = unzip(&out.bytes);
    let all = layers(&z);
    let masters: BTreeMap<&str, &Value> = all
        .iter()
        .filter(|(_, l)| s(l, "_class") == "symbolMaster")
        .map(|(_, l)| (s(l, "symbolID"), l))
        .collect();
    fn find<'a>(l: &'a Value, id: &str) -> Option<&'a Value> {
        l["layers"].as_array()?.iter().find_map(|c| {
            (s(c, "do_objectID") == id)
                .then_some(c)
                .or_else(|| find(c, id))
        })
    }
    let mut reached = 0;
    for (_, inst) in all
        .iter()
        .filter(|(_, l)| s(l, "_class") == "symbolInstance")
    {
        for o in inst["overrideValues"].as_array().unwrap() {
            let name = s(o, "overrideName");
            let path = name
                .strip_suffix("_stringValue")
                .expect("a string override");
            let mut master = masters[s(inst, "symbolID")];
            let segments: Vec<&str> = path.split('/').collect();
            for (k, seg) in segments.iter().enumerate() {
                let layer = find(master, seg).unwrap_or_else(|| panic!("{name}: {seg}"));
                if k + 1 == segments.len() {
                    assert_eq!(s(layer, "_class"), "text", "{name}");
                } else {
                    assert_eq!(s(layer, "_class"), "symbolInstance", "{name}: {seg}");
                    master = masters[s(layer, "symbolID")];
                }
            }
            reached += 1;
        }
    }
    assert_eq!(reached, 1, "the copy of M overrides its text");
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
/// flip: `place` writing the rotation unnegated → fails at the turned rect's
/// corners, whose bounds alone agree.
#[test]
fn sketch_import_reads_the_export_back() {
    let f = fixture();
    let out = ondin_export::sketch::export(&f.doc, &f.res, "Fixture").unwrap();
    let mut ids = IdSource::new(0x9E);
    let back = ondin_core::sketch_in::read(&mut Zip::of(out.bytes), &mut ids, &mut Pictures)
        .expect("read back");
    let doc = back.doc;
    for name in ["Screen", "Title", "Clip", "Glow", "Turned"] {
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
