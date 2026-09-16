//! Shared fixture for export tests: a document exercising most node kinds and
//! both fill and stroke, plus its resolved layer.
//!
//! **Not every kind** — this header said so until 2026-08-22 and had never been
//! true of `Polygon`, `Star` or `Boolean`. The complete one is
//! `goldens::golden_document`, which is checked against `NodeKind` by an
//! exhaustive `match` so it cannot fall behind the model the way this one did.
//! This fixture stays as it is on purpose: two dozen substring assertions are
//! written against its exact shape, and a golden's fixture wants to be free to
//! grow where a shared one does not.

// **The one `dead_code` allow in this workspace that is load-bearing**, and it is
// worth saying which kind it is (§15 D732, `[A8-L3-06]`). A `tests/common` module
// is compiled **once per integration test file** that declares it, and each file
// uses a different subset — so `fixture_with_stroke_align` is genuinely unused in
// `snapshot` and genuinely used in `svg_roundtrip`. Measured by removing this
// line: four warnings across two targets, every one of them a false report.
//
// ⚠️ **The two crate-level ones were not this**, and both are gone: removing
// `#![allow(dead_code)]` from `ondin-export/src/lib.rs` and `ondin-mcp/src/lib.rs`
// produced **zero** warnings under every spelling. They disabled a lint over two
// whole crates and were hiding nothing.
#![allow(dead_code)]

use ondin_core::kurbo::{Affine, BezPath, Point, RoundedRectRadii, Size};
use ondin_core::peniko::{Brush, Color};
use ondin_core::{
    Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Stroke, StrokeAlign,
    TextSizing, TextStyle, Transaction,
};

pub struct Fixture {
    pub doc: Document,
    pub res: Resolved,
    pub artboard: NodeId,
    pub rect: NodeId,
    pub text: NodeId,
}

pub fn fixture() -> Fixture {
    let mut ids = IdSource::new(0x5A5A);
    let root = ids.mint();
    let mut doc = Document::new(root);

    let artboard = ids.mint();
    let rect = ids.mint();
    let ellipse = ids.mint();
    let line = ids.mint();
    let group = ids.mint();
    let nested = ids.mint();
    let path = ids.mint();
    let text = ids.mint();

    let mut p = BezPath::new();
    p.move_to(Point::new(0.0, 0.0));
    p.line_to(Point::new(20.0, 30.0));
    p.line_to(Point::new(0.0, 30.0));
    p.close_path();

    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: artboard,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: Some("Board".into()),
        },
        Operation::SetFills {
            id: artboard,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(245, 245, 245, 255)),
                visible: true,
            }],
        },
        Operation::CreateNode {
            id: rect,
            parent: artboard,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 40.0),
                corner_radii: RoundedRectRadii::from_single_radius(6.0),
            },
            transform: Some(Affine::translate((20.0, 20.0))),
            name: None,
        },
        Operation::CreateNode {
            id: ellipse,
            parent: artboard,
            index: 1,
            kind: NodeKind::Ellipse {
                size: Size::new(50.0, 50.0),
            },
            transform: Some(Affine::translate((120.0, 20.0))),
            name: None,
        },
        Operation::CreateNode {
            id: line,
            parent: artboard,
            index: 2,
            kind: NodeKind::Line {
                end: Point::new(60.0, 0.0),
            },
            transform: Some(Affine::translate((20.0, 90.0))),
            name: None,
        },
        Operation::CreateNode {
            id: group,
            parent: artboard,
            index: 3,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((200.0, 100.0))),
            name: None,
        },
        Operation::CreateNode {
            id: nested,
            parent: group,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(30.0, 30.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: path,
            parent: artboard,
            index: 4,
            kind: NodeKind::Path {
                path: p,
                corner_radii: Vec::new(),
            },
            transform: Some(Affine::translate((120.0, 120.0))),
            name: None,
        },
        Operation::CreateNode {
            id: text,
            parent: artboard,
            index: 5,
            kind: NodeKind::Text {
                content: "Hi <there>".into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: 16.0,
                    weight: 500,
                    italic: false,
                    line_height: Some(ondin_core::Length::Em(1.2)),
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate((20.0, 200.0))),
            name: None,
        },
    ]))
    .expect("build fixture");

    // Give the rect a solid fill and a stroke.
    doc.apply(&Transaction(vec![
        Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(60, 120, 220, 255)),
                visible: true,
            }],
        },
        Operation::SetStrokes {
            id: rect,
            strokes: vec![Stroke {
                brush: Brush::Solid(Color::BLACK),
                width: 2.0,
                join: ondin_core::kurbo::Join::Round,
                cap: ondin_core::kurbo::Cap::Round,
                dashes: vec![],
                dash_offset: 0.0,
                sides: Default::default(),
                miter_limit: 4.0,
                dash_fit: false,
                align: StrokeAlign::Center,
                visible: true,
            }],
        },
    ]))
    .expect("paint rect");

    let res = Resolved::rebuild(&doc);
    Fixture {
        doc,
        res,
        artboard,
        rect,
        text,
    }
}

/// `fixture` with the rect's stroke aligned to one side and widened to 3, so
/// the doubled width the aligned construction emits (6) is unambiguous.
pub fn fixture_with_stroke_align(align: StrokeAlign) -> Fixture {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetStrokes {
            id: f.rect,
            strokes: vec![Stroke {
                brush: Brush::Solid(Color::BLACK),
                width: 3.0,
                join: ondin_core::kurbo::Join::Round,
                cap: ondin_core::kurbo::Cap::Round,
                dashes: vec![],
                dash_offset: 0.0,
                sides: Default::default(),
                miter_limit: 4.0,
                dash_fit: false,
                align,
                visible: true,
            }],
        }]))
        .expect("align stroke");
    f.res = Resolved::rebuild(&f.doc);
    f
}
