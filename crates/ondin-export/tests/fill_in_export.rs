//! A frame set to leave its fill out of exports (`Node::fill_hidden_in_export`):
//! every file drawn through `plan::render` draws its children and not its fills.

use ondin_core::kurbo::{Affine, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, ExportFormat, ExportScale, ExportSpec, Fill, IdSource, NodeId, NodeKind,
    Operation, Resolved, Transaction, keyed_by_position,
};

fn solid(id: NodeId, c: Color) -> Operation {
    Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        }]),
    }
}

/// A 16×16 white frame holding a black 4×4 dot at its corner, the flag at
/// `hidden`. Answers the document, its resolution and the frame.
fn fixture(hidden: bool) -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0xF11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, dot) = (ids.mint(), ids.mint());
    let ops = vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(16.0, 16.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: Some("icon".into()),
        },
        solid(frame, Color::WHITE),
        Operation::CreateNode {
            id: dot,
            parent: frame,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(4.0, 4.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        },
        solid(dot, Color::BLACK),
        Operation::SetFillHiddenInExport { id: frame, hidden },
    ];
    doc.apply(&Transaction(ops)).expect("the fixture");
    let res = Resolved::rebuild(&doc);
    (doc, res, frame)
}

fn export(doc: &Document, res: &Resolved, frame: NodeId, format: ExportFormat) -> Vec<u8> {
    let file = &ondin_export::plan(
        doc,
        res,
        &[frame],
        ondin_export::PlanOptions {
            unset: Some(ExportSpec::new(format, ExportScale::Times(1.0))),
            ..Default::default()
        },
    )[0];
    ondin_export::plan::render(doc, res, file).expect("a file")
}

/// The alpha at `(x, y)` of a PNG's pixels.
fn alpha_at(png: &[u8], x: u32, y: u32) -> u8 {
    let img = image::load_from_memory(png).expect("a PNG").to_rgba8();
    img.get_pixel(x, y).0[3]
}

/// A PNG of the frame keeps the dot and drops the white ground; the same frame
/// with the flag off keeps both. The canvas's document is not touched.
/// flip: `plan::render` skipping `for_export` → fails at "the ground is left out".
#[test]
fn a_png_leaves_the_frames_fill_out() {
    let (doc, res, frame) = fixture(true);
    let png = export(&doc, &res, frame, ExportFormat::Png);
    assert_eq!(alpha_at(&png, 12, 12), 0, "the ground is left out");
    assert_eq!(alpha_at(&png, 1, 1), 255, "the dot is drawn");
    assert_eq!(
        doc.get(frame).unwrap().paint().fills.len(),
        1,
        "the document keeps its fill"
    );

    let (doc, res, frame) = fixture(false);
    let png = export(&doc, &res, frame, ExportFormat::Png);
    assert_eq!(alpha_at(&png, 12, 12), 255, "the control: the ground drawn");
}

/// An SVG of the frame paints no white either: every writer draws the same copy.
#[test]
fn an_svg_leaves_the_frames_fill_out() {
    let white = |svg: &str| {
        let s = svg.to_ascii_lowercase();
        s.contains("#ffffff") || s.contains("#fff\"") || s.contains("white")
    };
    let (doc, res, frame) = fixture(false);
    let svg = String::from_utf8(export(&doc, &res, frame, ExportFormat::Svg)).unwrap();
    assert!(white(&svg), "the control: the ground is painted\n{svg}");
    let (doc, res, frame) = fixture(true);
    let svg = String::from_utf8(export(&doc, &res, frame, ExportFormat::Svg)).unwrap();
    assert!(!white(&svg), "the ground is left out\n{svg}");
}

/// A layer inside a flagged frame takes no ground from it under *Frame's own*:
/// the frame's fill is out of every export, its children's included.
/// flip: `frame_background` reading a flagged frame → fails at "no ground".
#[test]
fn a_flagged_frame_gives_its_children_no_ground() {
    let spec = ExportSpec {
        background: ondin_core::ExportBackground::Frame,
        ..ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0))
    };
    let (doc, _, frame) = fixture(false);
    let dot = doc.get(frame).unwrap().children()[0];
    assert_eq!(
        ondin_export::plan::background_color(&doc, dot, &spec),
        Some(Color::WHITE),
        "the control: the frame's white"
    );
    let (doc, _, frame) = fixture(true);
    let dot = doc.get(frame).unwrap().children()[0];
    assert_eq!(
        ondin_export::plan::background_color(&doc, dot, &spec),
        None,
        "no ground"
    );
}

/// A white frame holding a black dot in its corner, `side` square, at `x` on the
/// page and named `name`, its fill out of exports at `hidden`.
fn add_frame(
    doc: &mut Document,
    ids: &mut IdSource,
    at: (NodeId, f64),
    name: &str,
    side: f64,
    hidden: bool,
) -> NodeId {
    let (frame, dot) = (ids.mint(), ids.mint());
    let (parent, x) = at;
    let index = doc.get(parent).unwrap().children().len();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(side, side),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: Some(name.into()),
        },
        solid(frame, Color::WHITE),
        Operation::CreateNode {
            id: dot,
            parent: frame,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(side / 4.0, side / 4.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        },
        solid(dot, Color::BLACK),
        Operation::SetFillHiddenInExport { id: frame, hidden },
    ]))
    .expect("a frame");
    frame
}

/// Every PNG inside an icon container, decoded, keyed by its side.
fn pngs_in(bytes: &[u8]) -> Vec<(u32, image::RgbaImage)> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    (0..bytes.len().saturating_sub(SIG.len()))
        .filter(|&i| &bytes[i..i + SIG.len()] == SIG)
        .filter_map(|i| image::load_from_memory(&bytes[i..]).ok())
        .map(|img| (img.width(), img.to_rgba8()))
        .collect()
}

fn render_with(doc: &Document, node: NodeId, spec: ExportSpec) -> Vec<u8> {
    let res = Resolved::rebuild(doc);
    let file = &ondin_export::plan(
        doc,
        &res,
        &[node],
        ondin_export::PlanOptions {
            unset: Some(spec),
            ..Default::default()
        },
    )[0];
    ondin_export::plan::render(doc, &res, file).expect("a file")
}

/// An icon size drawn from its own `‹layer›-‹size›` frame leaves that frame's
/// fill out too, in ICO and ICNS alike; the flag-off control keeps it.
/// flip: `plan::render` stripping `origins_of` rather than `drawn_roots` → fails at "16 from its frame".
#[test]
fn an_icon_size_drawn_from_its_own_frame_leaves_that_frames_fill_out() {
    for format in [ExportFormat::Ico, ExportFormat::Icns] {
        for hidden in [true, false] {
            let mut ids = IdSource::new(0xF12);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let icon = add_frame(&mut doc, &mut ids, (root, 0.0), "icon", 64.0, hidden);
            let small = add_frame(&mut doc, &mut ids, (root, 100.0), "icon-16", 16.0, hidden);
            let spec = ExportSpec {
                sizes: vec![16, 32],
                size_frames: true,
                ..ExportSpec::new(format, ExportScale::Times(1.0))
            };
            let res = Resolved::rebuild(&doc);
            let roots = ondin_export::plan::drawn_roots(&doc, &res, icon, &spec);
            assert_eq!(roots, vec![icon, small], "the fixture: a size frame found");
            let bytes = render_with(&doc, icon, spec);
            let pngs = pngs_in(&bytes);
            let ground = |side: u32| {
                let (_, img) = pngs.iter().find(|(s, _)| *s == side).expect("the size");
                img.get_pixel(side * 3 / 4, side * 3 / 4).0[3]
            };
            let want = if hidden { 0 } else { 255 };
            assert_eq!(
                ground(16),
                want,
                "16 from its frame, {format:?} hidden={hidden}"
            );
            assert_eq!(ground(32), want, "32 scaled, {format:?} hidden={hidden}");
        }
    }
}

/// A slice over a flagged frame and a flagged frame inside an exported plain one
/// draw no ground either: the strip reaches everything the file draws.
/// flip: `for_export` not descending into children → fails at "nested".
#[test]
fn a_slice_and_a_nested_frame_leave_the_fill_out() {
    for hidden in [true, false] {
        let mut ids = IdSource::new(0xF13);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let outer = add_frame(&mut doc, &mut ids, (root, 0.0), "outer", 64.0, false);
        doc.apply(&Transaction(vec![Operation::SetFills {
            id: outer,
            fills: Vec::new(),
        }]))
        .unwrap();
        let inner = add_frame(&mut doc, &mut ids, (outer, 0.0), "inner", 32.0, hidden);
        let png = |doc: &Document, node| {
            let bytes = render_with(
                doc,
                node,
                ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0)),
            );
            image::load_from_memory(&bytes).unwrap().to_rgba8()
        };
        let want = if hidden { 0 } else { 255 };
        assert_eq!(
            png(&doc, outer).get_pixel(24, 24).0[3],
            want,
            "nested, hidden={hidden}"
        );

        let slice = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: slice,
            parent: root,
            index: 1,
            kind: NodeKind::Slice {
                size: Size::new(32.0, 32.0),
            },
            transform: Some(Affine::IDENTITY),
            name: Some("slice".into()),
        }]))
        .unwrap();
        assert!(doc.get(inner).is_some());
        assert_eq!(
            png(&doc, slice).get_pixel(24, 24).0[3],
            want,
            "a slice, hidden={hidden}"
        );
    }
}

/// A PDF of a flagged frame paints no white: the flag-off control's content
/// stream holds a fill the flagged one lacks.
/// flip: `plan::render` skipping `for_export` → fails at "the PDF differs".
#[test]
fn a_pdf_leaves_the_frames_fill_out() {
    let (on, res_on, frame_on) = fixture(true);
    let (off, res_off, frame_off) = fixture(false);
    let a = export(&on, &res_on, frame_on, ExportFormat::Pdf);
    let b = export(&off, &res_off, frame_off, ExportFormat::Pdf);
    assert!(a.starts_with(b"%PDF"), "a PDF");
    assert_ne!(a, b, "the PDF differs");
}
