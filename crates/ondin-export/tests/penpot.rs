//! Penpot export (`ondin_export::penpot`): a file whose shapes, assets and
//! components all name each other, and which `ondin_core::penpot_in` reads back
//! as the document it was written from.

mod rich;

use ondin_core::{IdSource, NodeId, NodeKind};
use rich::{Pictures, Zip, fixture, named, s, same_place, unzip};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

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
    assert_eq!(refs.3, 4, "each copy and its label name the main's layers");
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
        .find(|v| v.get("componentRoot").is_some() && s(v, "name") == "Button")
        .expect("the copy's root");
    let label = copies
        .iter()
        .find(|v| s(v, "type") == "text" && s(v, "parentId") == s(root, "id"))
        .unwrap();
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

/// What Penpot cannot hold of the fixture, each once, in the order met.
/// flip: `paints` dropping a hidden fill unreported → fails at the list.
#[test]
fn the_report_names_what_penpot_cannot_hold() {
    let f = fixture();
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    assert_eq!(
        out.approximated,
        [
            "Guides, left out",
            "Hidden fills, left out",
            "Layout grids, left out",
            "Slices, left out",
        ]
    );
}

/// A layer's export settings go out as Penpot's `exports`: the format as its
/// keyword, the multiplier, and the suffix Ondin would name the file with.
/// flip: `exports` writing `spec.suffix` for the suffix → fails at the 2× row.
#[test]
fn export_settings_go_out_as_penpots_exports() {
    let f = fixture();
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let z = unzip(&out.bytes);
    let with: Vec<&serde_json::Value> = z
        .json
        .values()
        .filter(|v| v.get("exports").is_some())
        .collect();
    assert_eq!(with.len(), 1, "only the screen carries a setting");
    assert_eq!(
        with[0]["exports"],
        serde_json::json!([{ "type": "png", "scale": 2.0, "suffix": "@2x" }])
    );
}

/// A row Penpot cannot hold exactly is still written, and named in the report.
/// flip: `exports` skipping the width check → fails at the report.
#[test]
fn an_export_by_width_goes_out_as_a_multiplier_and_is_reported() {
    let mut f = fixture();
    let screen = named(&f.doc, "Screen");
    // The screen is 480 wide: 1440 pixels across asks for 3×.
    let mut spec = ondin_core::ExportSpec::new(
        ondin_core::ExportFormat::Jpeg,
        ondin_core::ExportScale::Width(1440),
    );
    spec.prefix = "web/".into();
    f.doc
        .apply(&ondin_core::Transaction(vec![
            ondin_core::Operation::SetExports {
                id: screen,
                exports: ondin_core::keyed_by_position([spec]),
            },
        ]))
        .expect("set the screen's export");
    f.res = ondin_core::Resolved::rebuild(&f.doc);
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let z = unzip(&out.bytes);
    let row = z
        .json
        .values()
        .find_map(|v| v.get("exports"))
        .expect("the screen's exports");
    assert_eq!(row[0]["type"], "jpeg");
    assert!((row[0]["scale"].as_f64().unwrap() - 3.0).abs() < 1e-3);
    for line in [
        "Export sizes by width or height, as a multiplier",
        "Export prefixes, left out",
    ] {
        assert!(out.approximated.iter().any(|s| s == line), "{line}");
    }
}

/// Writing the same document twice gives the same bytes.
#[test]
fn the_same_document_writes_the_same_file() {
    let f = fixture();
    let a = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let b = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    assert!(a.bytes == b.bytes);
}

/// Penpot's reader takes the file back: the layers in their places, the swatch
/// still bound, the title in its style, the copy linked with its own label.
#[test]
fn penpot_import_reads_the_export_back() {
    let f = fixture();
    let out = ondin_export::penpot::export(&f.doc, &f.res, "Fixture").unwrap();
    let mut ids = IdSource::new(0x9E);
    let back = ondin_core::penpot_in::read(&mut Zip::of(out.bytes), &mut ids, &mut Pictures)
        .expect("read back");
    let doc = back.doc;
    for name in [
        "Screen", "Row", "Button", "Title", "Clip", "Glow", "Turned", "Flipped",
    ] {
        same_place(&f, &doc, name);
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
