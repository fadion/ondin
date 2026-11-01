//! Figma import (`ondin_core::figma_in`): a link's file key, nodes placed in
//! their parent, geometry, export settings, text overrides, components linked
//! through instance ids with their overrides kept, styles, pictures, and a
//! result the loader accepts.

use ondin_core::figma_in::{Read, file_key, read};
use ondin_core::foreign::Archive;
use ondin_core::image::{ImageEntry, ImageFormat, ImageId, ImageSource};
use ondin_core::kurbo::{Affine, Point};
use ondin_core::peniko::{self, Color};
use ondin_core::svg_in::ImageSink;
use ondin_core::{
    BrushSource, Document, ExportFormat, ExportScale, ExportSpec, IdSource, NodeId, NodeKind,
    TextSizing,
};
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

/// A node at `(x, y)` in its parent, `w` by `h`.
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

/// A frame holds a rect and a group; the REST API places the group's child in
/// the group, not in the frame as the Plugin API does.
#[test]
fn nodes_are_placed_in_their_parent() {
    let rect = node(
        "1:3",
        "RECTANGLE",
        (10.0, 20.0, 40.0, 30.0),
        json!({
            "fills": [solid(rgb(1.0, 0.0, 0.0))], "rectangleCornerRadii": [4, 4, 0, 0],
        }),
    );
    let inner = node("1:5", "RECTANGLE", (10.0, 10.0, 10.0, 10.0), json!({}));
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
    let group = doc.get(one(doc, "1:4")).unwrap();
    assert_eq!(group.transform(), Affine::translate((50.0, 60.0)));
    let inner = doc.get(one(doc, "1:5")).unwrap();
    assert_eq!(inner.transform(), Affine::translate((10.0, 10.0)));
    let page = doc.first_page();
    assert_eq!(doc.get(page).unwrap().name(), "Flows");
}

/// A mask keeps its `maskType`: `VECTOR` (or none) an outline, `ALPHA` and
/// `LUMINANCE` the ink modes of the same names.
#[test]
fn a_mask_keeps_its_mask_type() {
    use ondin_core::MaskMode;
    let mask = |id: &str, kind: Value| {
        let mut extra = json!({ "isMask": true });
        if !kind.is_null() {
            extra["maskType"] = kind;
        }
        node(id, "RECTANGLE", (0.0, 0.0, 10.0, 10.0), extra)
    };
    let groups = [
        ("1:1", json!(null)),
        ("1:2", json!("VECTOR")),
        ("1:3", json!("ALPHA")),
        ("1:4", json!("LUMINANCE")),
    ]
    .into_iter()
    .map(|(id, kind)| {
        node(
            &format!("g{id}"),
            "GROUP",
            (0.0, 0.0, 10.0, 10.0),
            json!({ "children": [mask(id, kind)] }),
        )
    })
    .collect();
    let r = import(fetched(groups, json!({}), &[]));
    let mode = |name: &str| {
        let n = r.doc.get(one(&r.doc, name)).unwrap();
        assert!(n.mask(), "{name} is a mask");
        n.mask_mode()
    };
    assert_eq!(mode("1:1"), MaskMode::Shape);
    assert_eq!(mode("1:2"), MaskMode::Shape);
    assert_eq!(mode("1:3"), MaskMode::Alpha);
    assert_eq!(mode("1:4"), MaskMode::Luminance);
}

/// A vector is its fill geometry; one with nothing to fill is its network's
/// centre line, stroked and unfilled, its segments chained however Figma lists
/// them, when the file carries a `vectorNetwork` (a Plugin API field the REST
/// fetch is not documented to send); without one, its stroke geometry filled
/// with the stroke's paint.
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
    let v = |x: f64, y: f64| json!({ "position": { "x": x, "y": y }, "meta": 0 });
    let t = |x: f64, y: f64| json!({ "x": x, "y": y });
    let seg = |a: usize, t0: Value, t1: Value, b: usize| json!({ "start": a, "startTangent": t0, "endTangent": t1, "end": b });
    let networked = node(
        "2:3",
        "VECTOR",
        (0.0, 0.0, 30.0, 10.0),
        json!({
            "fills": [solid(rgb(1.0, 0.0, 0.0))],
            "strokes": [solid(rgb(0.0, 1.0, 0.0))], "strokeWeight": 2,
            "fillGeometry": [], "strokeGeometry": [{ "path": "M0 -1L10 -1L10 1L0 1Z" }],
            "vectorNetwork": {
                "vertices": [v(0.0, 0.0), v(10.0, 0.0), v(30.0, 10.0), v(20.0, 0.0)],
                "segments": [
                    seg(1, t(0.0, 0.0), t(0.0, 0.0), 3),
                    seg(3, t(5.0, 0.0), t(0.0, -5.0), 2),
                    seg(0, t(0.0, 0.0), t(0.0, 0.0), 1),
                ],
            },
        }),
    );
    let r = import(fetched(vec![closed, open, networked], json!({}), &[]));
    let doc = &r.doc;
    let networked = doc.get(one(doc, "2:3")).unwrap();
    let NodeKind::Path { path, .. } = networked.kind() else {
        panic!("a path");
    };
    assert_eq!(path.to_svg(), "M0,0 L10,0 L20,0 C25,0 30,5 30,10");
    assert!(networked.paint().fills.is_empty());
    assert_eq!(networked.paint().strokes.len(), 1);
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

/// A filled open "V" is split into its fill, unstroked, and above it its
/// outlined stroke, so no stroke runs along the closing edge Figma does not
/// draw; a closed triangle stays one stroked path. flip: always take the
/// `fillGeometry` path in `node` → fails at the V's kind.
#[test]
fn a_filled_open_path_is_not_stroked_along_its_closing_edge() {
    let stroked = |id: &str, stroke: &str| {
        node(
            id,
            "VECTOR",
            (0.0, 0.0, 10.0, 10.0),
            json!({
                "fills": [solid(rgb(1.0, 0.0, 0.0))],
                "strokes": [solid(rgb(0.0, 0.0, 0.0))], "strokeWeight": 2,
                "fillGeometry": [{ "path": "M0 0L5 10L10 0Z", "windingRule": "NONZERO" }],
                "strokeGeometry": [{ "path": stroke }],
            }),
        )
    };
    // One contour around the drawn line; a closed outline strokes into two.
    let v = stroked("V", "M-1 0L5 12L11 0L9 0L5 8L1 0Z");
    let tri = stroked("T", "M-1 -1L5 11L11 -1Z M1 1L5 9L9 1Z");
    let r = import(fetched(vec![v, tri], json!({}), &[]));
    let doc = &r.doc;
    let group = doc.get(one(doc, "V")).unwrap();
    assert!(
        matches!(group.kind(), NodeKind::Group),
        "{:?}",
        group.kind()
    );
    let (f, s) = (one(doc, "Fill"), one(doc, "Stroke"));
    assert_eq!(group.children(), [f, s]);
    let f = doc.get(f).unwrap();
    assert!(f.paint().strokes.is_empty(), "the fill draws no stroke");
    assert_eq!(
        fill(doc, f.id()),
        BrushSource::Literal(peniko::Brush::Solid(Color::new([1.0, 0.0, 0.0, 1.0])))
    );
    assert_eq!(
        fill(doc, s),
        BrushSource::Literal(peniko::Brush::Solid(Color::new([0.0, 0.0, 0.0, 1.0]))),
        "the stroke, outlined"
    );
    let t = doc.get(one(doc, "T")).unwrap();
    assert!(matches!(t.kind(), NodeKind::Path { .. }));
    assert_eq!(t.paint().strokes.len(), 1);
    let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
    assert_eq!(rows, ["Filled open paths"]);
}

/// An ellipse's arc: a full turn (Figma's `f32` 2π) is the whole ellipse, a
/// ring when it has an inner radius; a quarter is 90°; an arc crossing 0° and
/// one stored backwards keep their span. flip: wrap the sweep by
/// `rem_euclid(360)` in `arc` → fails at the ring, a hair-wide wedge.
#[test]
fn an_arc_reads_its_sweep_and_ring() {
    use std::f64::consts::{FRAC_PI_2, PI};
    let full = 6.2831854820251465;
    let cases = [
        ((0.0, full, 0.0), (0.0, 360.0, 0.0)),
        ((0.0, full, 0.6), (0.0, 360.0, 0.6)),
        ((0.0, FRAC_PI_2, 0.0), (0.0, 90.0, 0.0)),
        ((-FRAC_PI_2, FRAC_PI_2, 0.5), (270.0, 180.0, 0.5)),
        ((PI, FRAC_PI_2, 0.0), (90.0, 90.0, 0.0)),
    ];
    let ellipses = cases
        .iter()
        .enumerate()
        .map(|(i, ((s, e, r), _))| {
            node(
                &format!("e{i}"),
                "ELLIPSE",
                (0.0, 0.0, 10.0, 10.0),
                json!({ "arcData": { "startingAngle": s, "endingAngle": e, "innerRadius": r } }),
            )
        })
        .collect();
    let r = import(fetched(ellipses, json!({}), &[]));
    for (i, (_, (start, sweep, ratio))) in cases.iter().enumerate() {
        let NodeKind::Ellipse { arc, .. } =
            r.doc.get(one(&r.doc, &format!("e{i}"))).unwrap().kind()
        else {
            panic!("an ellipse");
        };
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(
            near(arc.start, *start) && near(arc.sweep, *sweep) && near(arc.ratio, *ratio),
            "case {i}: {arc:?}"
        );
    }
}

/// Gradient handles are fractions of the box: a linear one runs between its
/// first two, a radial one is the unit circle carried onto the ellipse whose
/// axes end at the second and third. flip: scale the handles by the width on
/// both axes in `gradient` → fails at the linear's end.
#[test]
fn gradient_handles_map_onto_the_box() {
    let paint = |kind: &str, handles: Value| {
        json!({ "type": kind, "gradientHandlePositions": handles, "gradientStops": [
            { "position": 0, "color": rgb(0.0, 0.0, 0.0) },
            { "position": 1, "color": rgb(1.0, 1.0, 1.0) },
        ]})
    };
    let h = |x: f64, y: f64| json!({ "x": x, "y": y });
    let linear = node(
        "L",
        "RECTANGLE",
        (0.0, 0.0, 200.0, 100.0),
        json!({ "fills": [paint("GRADIENT_LINEAR", json!([h(0.0, 0.5), h(1.0, 0.5), h(0.0, 1.0)]))] }),
    );
    let radial = node(
        "R",
        "RECTANGLE",
        (0.0, 0.0, 200.0, 100.0),
        json!({ "fills": [paint("GRADIENT_RADIAL", json!([h(0.5, 0.5), h(1.0, 0.5), h(0.5, 1.0)]))] }),
    );
    let r = import(fetched(vec![linear, radial], json!({}), &[]));
    let gradient = |name: &str| match fill(&r.doc, one(&r.doc, name)) {
        BrushSource::Literal(peniko::Brush::Gradient(g)) => g,
        other => panic!("{other:?}"),
    };
    let g = gradient("L");
    let peniko::GradientKind::Linear(line) = g.gradient.kind else {
        panic!("linear");
    };
    assert_eq!(
        (g.transform * line.start, g.transform * line.end),
        (Point::new(0.0, 50.0), Point::new(200.0, 50.0))
    );
    let g = gradient("R");
    assert!(matches!(g.gradient.kind, peniko::GradientKind::Radial(_)));
    assert_eq!(g.transform * Point::new(0.0, 0.0), Point::new(100.0, 50.0));
    assert_eq!(g.transform * Point::new(1.0, 0.0), Point::new(200.0, 50.0));
    assert_eq!(g.transform * Point::new(0.0, 1.0), Point::new(100.0, 100.0));
}

/// A nested instance swapped to another component keeps its slot in the
/// outer instance but shows its own component: its layers link to that
/// component's, and an edit to the component it replaced does not reach it.
/// flip: drop the swap test in `components` → fails at the swapped root.
#[test]
fn a_swapped_nested_instance_shows_its_own_component() {
    let icon = |id: &str, child: &str, kind: &str| {
        node(
            id,
            "COMPONENT",
            (0.0, 0.0, 10.0, 10.0),
            json!({ "children": [node(child, kind, (0.0, 0.0, 10.0, 10.0), json!({}))] }),
        )
    };
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
    let swapped = node(
        "5:1",
        "INSTANCE",
        (200.0, 0.0, 40.0, 20.0),
        json!({ "componentId": "4:1", "children": [
            node("I5:1;4:3", "INSTANCE", (2.0, 2.0, 10.0, 10.0), json!({ "componentId": "7:1", "children": [
                node("I5:1;4:3;7:2", "ELLIPSE", (0.0, 0.0, 10.0, 10.0), json!({})),
            ]})),
        ]}),
    );
    let r = import(fetched(
        vec![
            icon("3:1", "3:2", "RECTANGLE"),
            icon("7:1", "7:2", "ELLIPSE"),
            button,
            swapped,
        ],
        json!({}),
        &[],
    ));
    let mut doc = r.doc;
    let at = |doc: &Document, name: &str| doc.get(one(doc, name)).unwrap().clone();
    let root = at(&doc, "I5:1;4:3");
    assert_eq!(root.link(), Some(one(&doc, "4:3")), "keeps its slot");
    assert_eq!(root.swap(), Some(one(&doc, "7:1")), "shows Icon B");
    assert_eq!(
        at(&doc, "I5:1;4:3;7:2").link(),
        Some(one(&doc, "7:2")),
        "its ellipse copies Icon B's"
    );
    assert_eq!(at(&doc, "5:1").link(), Some(one(&doc, "4:1")));
    // A layer added to Icon A reaches Button's icon but not the swapped one.
    let mut ids = IdSource::new(0xF2);
    let added = ids.mint();
    let mut tx = ondin_core::Transaction(vec![ondin_core::Operation::CreateNode {
        id: added,
        parent: one(&doc, "3:1"),
        index: 1,
        kind: NodeKind::Group,
        transform: None,
        name: Some("added".into()),
    }]);
    ondin_core::propagate::owed(&doc, &mut tx, &mut ids);
    doc.apply(&tx).expect("the edit applies");
    assert_eq!(
        at(&doc, "4:3").children().len(),
        2,
        "Icon A's instance takes it"
    );
    assert_eq!(
        at(&doc, "I5:1;4:3").children(),
        [one(&doc, "I5:1;4:3;7:2")],
        "the swapped instance does not"
    );
}

/// A text run's own fill keeps its opacity, and an invisible run fill is not
/// drawn. flip: take the run's first fill's colour without its opacity in
/// `text` → fails at the half-clear run.
#[test]
fn a_runs_fill_keeps_its_opacity() {
    let red = |opacity: f64, visible: bool| {
        json!({ "fills": [{ "type": "SOLID", "color": rgb(1.0, 0.0, 0.0),
                            "opacity": opacity, "visible": visible }] })
    };
    let text = node(
        "3:1",
        "TEXT",
        (0.0, 0.0, 200.0, 40.0),
        json!({
            "characters": "ab cd ef",
            "fills": [solid(rgb(0.0, 0.0, 0.0))],
            "style": { "fontFamily": "Inter", "fontSize": 16 },
            "characterStyleOverrides": [0, 0, 0, 1, 1, 0, 2, 2],
            "styleOverrideTable": { "1": red(0.5, true), "2": red(1.0, false) },
        }),
    );
    let r = import(fetched(vec![text], json!({}), &[]));
    let NodeKind::Text { spans, .. } = r.doc.get(one(&r.doc, "3:1")).unwrap().kind() else {
        panic!("a text");
    };
    let colours: Vec<_> = spans
        .as_slice()
        .iter()
        .filter_map(|s| match &s.attr {
            ondin_core::CharAttr::Color(Some(c)) => Some((s.range(), c.to_rgba8().a)),
            _ => None,
        })
        .collect();
    assert_eq!(
        colours,
        [(3..5, 128)],
        "only the half-clear run is coloured"
    );
}

/// An image paint's framing: a `STRETCH` with an `imageTransform` shows the
/// crop that transform names, a `TILE` repeats at its `scalingFactor`, and a
/// turned crop is drawn filling its shape and reported. flip: map `STRETCH`
/// to `Fill` in `framing` → fails at the crop's fit.
#[test]
fn an_image_paints_crop_and_tile_scale_come_over() {
    let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
    let rect = |id: &str, paint: Value| {
        let mut p = json!({ "type": "IMAGE", "imageRef": "abc" });
        if let (Some(p), Some(e)) = (p.as_object_mut(), paint.as_object()) {
            p.extend(e.clone());
        }
        node(
            id,
            "RECTANGLE",
            (0.0, 0.0, 10.0, 10.0),
            json!({ "fills": [p] }),
        )
    };
    let cropped = rect(
        "C",
        json!({ "scaleMode": "STRETCH", "imageTransform": [[0.5, 0, 0.25], [0, 0.5, 0.1]] }),
    );
    let tiled = rect("T", json!({ "scaleMode": "TILE", "scalingFactor": 0.25 }));
    let turned = rect(
        "R",
        json!({ "scaleMode": "STRETCH", "imageTransform": [[0, 1, 0], [-1, 0, 1]] }),
    );
    let r = import(fetched(
        vec![cropped, tiled, turned],
        json!({}),
        &[("abc", png)],
    ));
    let image = |name: &str| match fill(&r.doc, one(&r.doc, name)) {
        BrushSource::Literal(peniko::Brush::Image(b)) => b.image,
        other => panic!("{other:?}"),
    };
    let c = image("C");
    assert_eq!(c.fit, ondin_core::ImageFit::Crop);
    assert_eq!(c.crop, ondin_core::kurbo::Rect::new(0.25, 0.1, 0.75, 0.6));
    let t = image("T");
    assert_eq!((t.fit, t.tile_scale), (ondin_core::ImageFit::Tile, 0.25));
    assert_eq!(image("R").fit, ondin_core::ImageFit::Fill);
    let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
    assert_eq!(rows, ["Turned or skewed picture crops"]);
}

/// Export settings come over with their format, scale and suffix; a suffix that
/// only repeats the scale's marker is left for Ondin to derive.
#[test]
fn export_settings_come_over() {
    let setting = |format: &str, kind: &str, value: f64, suffix: &str| {
        json!({ "format": format, "suffix": suffix,
                "constraint": { "type": kind, "value": value } })
    };
    let rect = node(
        "3:1",
        "RECTANGLE",
        (0.0, 0.0, 10.0, 10.0),
        json!({ "exportSettings": [
            setting("PNG", "SCALE", 2.0, "@2x"),
            setting("JPG", "WIDTH", 512.0, "-wide"),
            setting("SVG", "SCALE", 1.0, ""),
        ]}),
    );
    let r = import(fetched(vec![rect], json!({}), &[]));
    let doc = &r.doc;
    let specs: Vec<_> = doc
        .get(one(doc, "3:1"))
        .unwrap()
        .exports()
        .iter()
        .map(|k| &k.value)
        .collect();
    let png = ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0));
    let mut jpg = ExportSpec::new(ExportFormat::Jpeg, ExportScale::Width(512));
    jpg.suffix = "-wide".into();
    let svg = ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0));
    assert_eq!(specs, [&png, &jpg, &svg]);
    assert!(r.report.is_empty(), "{:?}", r.report);
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

/// A background blur lands as one at Figma's radius, hidden as Figma hid it,
/// and is not reported.
#[test]
fn a_background_blur_lands_as_one() {
    let glass = node(
        "10:1",
        "RECTANGLE",
        (0.0, 0.0, 20.0, 10.0),
        json!({
            "effects": [{ "type": "BACKGROUND_BLUR", "radius": 16, "visible": false }],
        }),
    );
    let r = import(fetched(vec![glass], json!({}), &[]));
    assert!(r.report.is_empty(), "{:?}", r.report);
    let fx = r.doc.get(one(&r.doc, "10:1")).unwrap().effects().to_vec();
    assert!(
        matches!(
            fx[..],
            [ref e] if !e.visible
                && matches!(e.kind, ondin_core::EffectKind::BackgroundBlur { radius } if radius == 16.0)
        ),
        "{fx:?}"
    );
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
