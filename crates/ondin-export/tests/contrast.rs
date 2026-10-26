//! `contrast::check`: text measured against the backdrop actually drawn behind
//! it, at WCAG 2's thresholds.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Cell, Document, Fill, IdSource, Mode, NodeId, NodeKind, Operation, TextSizing,
    TextStyle, Transaction, VarKind, Variable, keyed_by_position,
};
use ondin_export::contrast::{Level, Report, check, luminance, ratio};

fn solid(r: u8, g: u8, b: u8) -> Vec<ondin_core::Keyed<Fill>> {
    keyed_by_position([Fill {
        brush: Brush::Solid(Color::from_rgba8(r, g, b, 255)).into(),
        visible: true,
    }])
}

/// A white frame holding, optionally, a dark panel, and over it a line of text in
/// `ink` at `size` px.
struct Doc {
    doc: Document,
    ids: IdSource,
    page: NodeId,
    frame: NodeId,
    text: NodeId,
}

fn doc(ink: (u8, u8, u8), size: f64, panel: bool) -> Doc {
    let mut ids = IdSource::new(0xC0);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let (frame, back, text) = (ids.mint(), ids.mint(), ids.mint());
    let mut ops = vec![
        Operation::CreateNode {
            id: frame,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: None,
            name: Some("Frame".into()),
        },
        Operation::SetFills {
            id: frame,
            fills: solid(255, 255, 255),
        },
    ];
    if panel {
        ops.push(Operation::CreateNode {
            id: back,
            parent: frame,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(400.0, 200.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: Some("Panel".into()),
        });
        ops.push(Operation::SetFills {
            id: back,
            fills: solid(20, 20, 40),
        });
    }
    ops.push(Operation::CreateNode {
        id: text,
        parent: frame,
        index: usize::from(panel),
        kind: NodeKind::Text {
            content: "Hello contrast".into(),
            style: Box::new(TextStyle {
                font_family: "Inter".into(),
                font_size: size,
                weight: 400,
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
        transform: Some(Affine::translate((20.0, 20.0))),
        name: Some("Label".into()),
    });
    ops.push(Operation::SetFills {
        id: text,
        fills: solid(ink.0, ink.1, ink.2),
    });
    doc.apply(&Transaction(ops)).unwrap();
    Doc {
        doc,
        ids,
        page,
        frame,
        text,
    }
}

fn only(r: &Report) -> &ondin_export::contrast::Finding {
    assert_eq!(r.findings.len(), 1, "{r:?}");
    &r.findings[0]
}

/// The ratio and luminance are WCAG's: black on white is 21 : 1, `#767676` on
/// white just over 4.5 : 1.
#[test]
fn the_ratio_is_wcag_s() {
    assert!((ratio([0.0; 3], [1.0; 3]) - 21.0).abs() < 1e-9);
    let grey = 118.0 / 255.0;
    let r = ratio([grey; 3], [1.0; 3]);
    assert!((4.5..4.6).contains(&r), "{r}");
    assert_eq!(luminance([1.0; 3]), 1.0);
}

/// Black type on a white frame passes at AA with room to spare.
#[test]
fn black_on_white_passes() {
    let d = doc((0, 0, 0), 16.0, false);
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    assert!(f.ratio > 15.0, "{f:?}");
    assert_eq!(f.needs, 4.5);
    assert!(f.passes());
    assert_eq!(f.trail, ["Frame", "Label"]);
}

/// `#999` on white is under 3 : 1, failing body text; `#767676` passes AA and
/// fails AAA.
#[test]
fn grey_fails_by_level() {
    let d = doc((153, 153, 153), 16.0, false);
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    assert!(!f.passes() && f.ratio < 3.0, "{f:?}");

    let d = doc((118, 118, 118), 16.0, false);
    let aa = only(&check(&d.doc, d.page, Level::Aa)).clone();
    let aaa = only(&check(&d.doc, d.page, Level::Aaa)).clone();
    assert!(aa.passes(), "{aa:?}");
    assert!(!aaa.passes() && aaa.needs == 7.0, "{aaa:?}");
}

/// Large type needs 3 : 1, and a layer's scale counts toward its size.
#[test]
fn large_text_needs_three_to_one() {
    let d = doc((140, 140, 140), 30.0, false);
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    assert_eq!(f.needs, 3.0, "{f:?}");
    let mut d = doc((140, 140, 140), 16.0, false);
    d.doc
        .apply(&Transaction(vec![Operation::SetTransform {
            id: d.text,
            transform: Affine::translate((20.0, 20.0)) * Affine::scale(2.0),
        }]))
        .unwrap();
    assert_eq!(only(&check(&d.doc, d.page, Level::Aa)).needs, 3.0);
}

/// White type over a dark panel passes, though the frame behind the panel is
/// white: the backdrop is what is drawn, not the frame's fill.
///
/// flip: `worst_ratio` measuring against the page ground instead of the
/// rendered backdrop → fails at `passes`.
#[test]
fn the_backdrop_is_what_is_drawn() {
    let d = doc((255, 255, 255), 16.0, true);
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    assert!(f.passes() && f.ratio > 10.0, "{f:?}");
}

/// The layer's opacity is part of its colour: black at 25% on white fails.
#[test]
fn opacity_counts() {
    let mut d = doc((0, 0, 0), 16.0, false);
    d.doc
        .apply(&Transaction(vec![Operation::SetOpacity {
            id: d.text,
            opacity: 0.25,
        }]))
        .unwrap();
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    assert!(!f.passes(), "{f:?}");
}

/// Every mode is checked and the worst kept: type bound to a colour that is
/// black in Light and near-white in Dark fails, measured in Dark.
#[test]
fn the_worst_mode_is_reported() {
    let mut d = doc((0, 0, 0), 16.0, false);
    let (light, dark, ink) = (d.ids.mint_mode(), d.ids.mint_mode(), d.ids.mint_var());
    let mode = |id, name: &str, index| Operation::AddMode {
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    d.doc
        .apply(&Transaction(vec![
            mode(light, "Light", 0),
            mode(dark, "Dark", 1),
            Operation::AddVariable {
                var: Variable {
                    id: ink,
                    name: "ink".into(),
                    kind: VarKind::Color,
                    cells: [
                        (light, Cell::Color(Color::from_rgba8(0, 0, 0, 255))),
                        (dark, Cell::Color(Color::from_rgba8(240, 240, 240, 255))),
                    ]
                    .into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ]))
        .unwrap();
    let tx =
        ondin_core::bind_brush_at_all(&d.doc, &[d.text], ondin_core::PaintTarget::Fill, 0, ink);
    d.doc.apply(&tx).unwrap();
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    assert!(!f.passes(), "{f:?}");
    assert_eq!(f.mode.as_deref(), Some("Dark"));
    let _ = d.frame;
}

/// A gradient fill cannot be one colour, so the layer is skipped and said so.
#[test]
fn a_gradient_fill_is_skipped() {
    let mut d = doc((0, 0, 0), 16.0, false);
    let gradient = Brush::Gradient(ondin_core::GradientBrush {
        gradient: ondin_core::peniko::Gradient::new_linear((0.0, 0.0), (100.0, 0.0))
            .with_stops([Color::BLACK, Color::WHITE].as_slice()),
        transform: Affine::IDENTITY,
        opacity: 1.0,
    });
    d.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: d.text,
            fills: keyed_by_position([Fill {
                brush: gradient.into(),
                visible: true,
            }]),
        }]))
        .unwrap();
    let r = check(&d.doc, d.page, Level::Aa);
    assert!(r.findings.is_empty(), "{r:?}");
    assert_eq!(r.skipped.len(), 1);
}
