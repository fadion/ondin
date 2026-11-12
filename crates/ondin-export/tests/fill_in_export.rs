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
