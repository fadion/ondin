//! What an SVG export spec's own settings write (`svg::svg_spec`): the viewBox
//! from the frame or the artwork, at `0 0`; numbers to the spec's places; solid
//! paint as `currentColor` with its opacity kept; strokes as their filled ink.

use ondin_core::kurbo::{Affine, BezPath, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, ExportFormat, ExportScale, ExportSpec, Fill, IdSource, NodeId, NodeKind,
    Operation, Resolved, Stroke, SvgColour, SvgStrokes, SvgView, Transaction, keyed_by_position,
};
use ondin_export::svg::svg_spec;

const INK: Color = Color::from_rgba8(30, 40, 50, 255);
const ACCENT: Color = Color::from_rgba8(220, 30, 40, 102);

/// A 24×24 frame at (100, 40) holding an arrow stroked 2 wide in `INK` that
/// reaches 1.333… past the frame's right edge, and a 4×4 rect in `ACCENT` at 40%.
/// Answers the document, its resolution and the frame.
fn icon() -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0x1C0);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, arrow, dot) = (ids.mint(), ids.mint(), ids.mint());
    let mut path = BezPath::new();
    path.move_to((5.0, 12.0));
    path.line_to((25.333_333, 12.0));
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(24.0, 24.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((100.0, 40.0))),
            name: Some("Arrow".into()),
        },
        Operation::SetClip {
            id: frame,
            clip: false,
        },
        Operation::CreateNode {
            id: arrow,
            parent: frame,
            index: 0,
            kind: NodeKind::Path {
                path,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: ondin_core::symmetry::PathMirror::NONE,
            },
            transform: None,
            name: None,
        },
        Operation::SetStrokes {
            id: arrow,
            strokes: keyed_by_position([Stroke {
                brush: Brush::Solid(INK).into(),
                width: 2.0,
                ..Default::default()
            }]),
        },
        Operation::CreateNode {
            id: dot,
            parent: frame,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(4.0, 4.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((2.0, 2.0))),
            name: None,
        },
        Operation::SetFills {
            id: dot,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(ACCENT).into(),
                visible: true,
            }]),
        },
    ]))
    .expect("the fixture");
    let res = Resolved::rebuild(&doc);
    (doc, res, frame)
}

fn svg() -> ExportSpec {
    ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0))
}

/// By default the viewBox is the frame's own box at `0 0`, whatever the arrow's
/// reach; *Fit the artwork* takes the ink instead, still at `0 0`.
/// flip: `svg::frame_view` answering `None` for a frame → fails at "the frame's box".
#[test]
fn the_view_box_is_the_frames_unless_the_artwork_is_asked_for() {
    let (doc, res, frame) = icon();
    let out = svg_spec(&doc, &res, frame, &svg()).svg;
    assert!(
        out.contains(r#"width="24" height="24" viewBox="0 0 24 24""#),
        "the frame's box: {out}"
    );
    let fit = ExportSpec {
        view: SvgView::Artwork,
        ..svg()
    };
    let out = svg_spec(&doc, &res, frame, &fit).svg;
    assert!(out.contains(r#"viewBox="0 0 "#), "at the origin: {out}");
    assert!(
        !out.contains(r#"viewBox="0 0 24 24""#),
        "the ink reaches past the frame: {out}"
    );
}

/// *Pad out to a square* grows a wide artwork's viewBox to a square about its
/// centre.
/// flip: `svg_spec` ignoring `pad_square` → fails at "a square".
#[test]
fn a_padded_svg_is_square() {
    let (doc, res, frame) = icon();
    let spec = ExportSpec {
        view: SvgView::Artwork,
        pad_square: true,
        ..svg()
    };
    let out = svg_spec(&doc, &res, frame, &spec).svg;
    let tag = out.lines().next().expect("the root");
    let (w, h) = (attr(tag, "width"), attr(tag, "height"));
    assert_eq!(w, h, "a square: {tag}");
}

/// The value of `name="…"` in `tag`.
fn attr<'a>(tag: &'a str, name: &str) -> &'a str {
    let at = tag.find(&format!(" {name}=\"")).expect("the attribute") + name.len() + 3;
    &tag[at..at + tag[at..].find('"').expect("closed")]
}

/// Numbers are written to the spec's places: 25.333333 at two places is 25.33,
/// at none 25. The plain writer keeps four.
#[test]
fn numbers_round_to_the_specs_places() {
    let (doc, res, frame) = icon();
    let two = svg_spec(&doc, &res, frame, &svg()).svg;
    assert!(two.contains("25.33,12"), "two places: {two}");
    assert!(!two.contains("25.333"), "no third place: {two}");
    let none = ExportSpec {
        precision: 0,
        ..svg()
    };
    let none = svg_spec(&doc, &res, frame, &none).svg;
    assert!(none.contains("25,12"), "whole numbers: {none}");
    let plain = ondin_export::svg::svg_of(&doc, &res, &[frame]);
    assert!(
        plain.contains("25.3333"),
        "the plain writer keeps four: {plain}"
    );
}

/// At no places, unitless numbers keep four places: the 40% fill and a 30° turn's
/// matrix, whose translation is no length to round.
/// flip: `alpha_attr` through `fmt` → fails at "the opacity".
#[test]
fn no_places_keeps_whole_numbers_and_unitless_numbers_keep_four() {
    let (mut doc, _, frame) = icon();
    let dot = doc.get(frame).unwrap().children()[1];
    doc.apply(&Transaction(vec![Operation::SetTransform {
        id: dot,
        transform: Affine::translate((19.7, 0.0)) * Affine::rotate(30f64.to_radians()),
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let spec = ExportSpec {
        precision: 0,
        ..svg()
    };
    let out = svg_spec(&doc, &res, frame, &spec).svg;
    assert!(out.contains(r#"fill-opacity="0.4""#), "the opacity: {out}");
    assert!(
        out.contains("matrix(0.866 0.5 -0.5 0.866 19.7 0)"),
        "the turn: {out}"
    );
    assert!(!out.contains(r#"="""#), "no empty attribute: {out}");
}

/// `currentColor` replaces both solids, the rect's 40% kept as `fill-opacity`,
/// and the export counts the two colours it merged.
/// flip: `solid_attr` ignoring `Style::current_color` → fails at "no hex colour left".
#[test]
fn current_color_replaces_solids_and_keeps_their_opacity() {
    let (doc, res, frame) = icon();
    let spec = ExportSpec {
        colour: SvgColour::CurrentColor,
        ..svg()
    };
    let out = svg_spec(&doc, &res, frame, &spec);
    assert!(
        !out.svg.contains("#1e2832"),
        "no hex colour left: {}",
        out.svg
    );
    assert!(
        !out.svg.contains("#dc1e28"),
        "no hex colour left: {}",
        out.svg
    );
    assert!(
        out.svg.contains(r#"stroke="currentColor""#),
        "the stroke: {}",
        out.svg
    );
    assert!(
        out.svg
            .contains(r#"fill="currentColor" fill-opacity="0.4""#),
        "the rect, its alpha kept: {}",
        out.svg
    );
    assert_eq!(out.merged, 2, "two colours became one");
    assert_eq!(svg_spec(&doc, &res, frame, &svg()).merged, 0);
}

/// *Outline* writes the arrow's stroke as a filled path in its colour, and no
/// `stroke` attribute is left.
/// flip: `emit_strokes` ignoring `Style::outline_strokes` → fails at "no stroke left".
#[test]
fn outlined_strokes_are_filled_ink() {
    let (doc, res, frame) = icon();
    let spec = ExportSpec {
        strokes: SvgStrokes::Outline,
        ..svg()
    };
    let out = svg_spec(&doc, &res, frame, &spec).svg;
    assert!(!out.contains("stroke"), "no stroke left: {out}");
    assert!(
        out.contains(r##"fill="#1e2832""##),
        "the ink, filled: {out}"
    );
}

/// An *Icons* set with *Name* (*Arrow left*, *Check*) and *Size* (*16*, *24*),
/// its four variants created in z-order against the properties' order, and a fifth
/// that repeats *Check, 16*. Answers the document, its resolution and the set.
fn icon_set() -> (Document, Resolved, NodeId) {
    use ondin_core::variant::{VariantProp, VariantSet};
    let mut ids = IdSource::new(0x5E7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let set = ids.mint();
    let frame = |id, parent, index, side: f64, name: &str| Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(side, side),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: Some(name.into()),
    };
    let mut ops = vec![
        frame(set, root, 0, 200.0, "Icons"),
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![
                    VariantProp::new("Name", vec!["Arrow left".into(), "Check".into()]),
                    VariantProp::new("Size", vec!["16".into(), "24".into()]),
                ],
                ..Default::default()
            }),
        },
    ];
    let values = [
        ["Check", "24"],
        ["Arrow left", "24"],
        ["Check", "16"],
        ["Arrow left", "16"],
        ["Check", "16"],
    ];
    for (i, [name, size]) in values.into_iter().enumerate() {
        let v = ids.mint();
        ops.push(frame(v, set, i, size.parse().unwrap(), "variant"));
        ops.push(Operation::SetComponent {
            id: v,
            component: true,
        });
        ops.push(Operation::SetVariant {
            id: v,
            values: vec![name.into(), size.into()],
        });
    }
    doc.apply(&Transaction(ops)).expect("the fixture");
    let res = Resolved::rebuild(&doc);
    (doc, res, set)
}

/// *Each variant as its own file* plans one file per variant in a folder named
/// for the set, named by its values in the properties' order, the clash
/// de-collided and the prefix in front; off, the set is one picture.
/// flip: `plan::variant_files` ignoring `each_variant` → fails at "one picture".
#[test]
fn a_set_exports_a_file_per_variant_into_its_folder() {
    use ondin_export::plan::{PlanOptions, plan};
    let (doc, res, set) = icon_set();
    let opts = |spec: ExportSpec| PlanOptions {
        unset: Some(spec),
        folders_from_names: true,
        ..Default::default()
    };
    let one = plan(&doc, &res, &[set], opts(svg()));
    let paths: Vec<&str> = one.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["Icons.svg"], "one picture");

    let each = ExportSpec {
        each_variant: true,
        prefix: "assets/".into(),
        ..svg()
    };
    let files = plan(&doc, &res, &[set], opts(each));
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "assets/icons/arrow-left-16.svg",
            "assets/icons/arrow-left-24.svg",
            "assets/icons/check-16.svg",
            "assets/icons/check-16 (2).svg",
            "assets/icons/check-24.svg",
        ]
    );
    assert!(
        files.iter().all(|f| f.node != set),
        "each file is a variant"
    );
}
