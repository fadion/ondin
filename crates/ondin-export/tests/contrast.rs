//! `contrast::check`: text measured against the backdrop actually drawn behind
//! it, at WCAG 2's thresholds.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Cell, Document, Fill, IdSource, Mode, NodeId, NodeKind, Operation, TextSizing,
    TextStyle, Transaction, VarKind, Variable, keyed_by_position,
};
use ondin_export::contrast::{self, Level, Report, check, luminance, ratio};

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
                corner_smoothing: 0.0,
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
    assert_eq!(r.skipped[0].why, "gradient fill");
}

/// A layer drawn in two modes keeps a sample for each, in mode order, with the
/// ink and the backdrop it was found at; only a variable passing in both is
/// offered.
///
/// flip: `check_with` keeping only the worst mode's sample → fails at the
/// sample count.
#[test]
fn every_mode_keeps_its_sample() {
    let mut d = doc((0, 0, 0), 16.0, false);
    let (light, dark) = (d.ids.mint_mode(), d.ids.mint_mode());
    let (ink, both, light_only) = (d.ids.mint_var(), d.ids.mint_var(), d.ids.mint_var());
    let mode = |id, name: &str, index| Operation::AddMode {
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    let var = |id, name: &str, l: (u8, u8, u8), k: (u8, u8, u8)| Operation::AddVariable {
        var: Variable {
            id,
            name: name.into(),
            kind: VarKind::Color,
            cells: [
                (light, Cell::Color(Color::from_rgba8(l.0, l.1, l.2, 255))),
                (dark, Cell::Color(Color::from_rgba8(k.0, k.1, k.2, 255))),
            ]
            .into(),
            origin: None,
            forked: None,
            description: String::new(),
        },
    };
    d.doc
        .apply(&Transaction(vec![
            mode(light, "Light", 0),
            mode(dark, "Dark", 1),
            var(ink, "ink", (0, 0, 0), (240, 240, 240)),
            var(both, "text/primary", (10, 10, 10), (20, 20, 20)),
            var(light_only, "text/faint", (30, 30, 30), (250, 250, 250)),
        ]))
        .unwrap();
    let tx =
        ondin_core::bind_brush_at_all(&d.doc, &[d.text], ondin_core::PaintTarget::Fill, 0, ink);
    d.doc.apply(&tx).unwrap();
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    let names: Vec<_> = f.samples.iter().map(|s| s.name.as_deref()).collect();
    assert_eq!(names, [Some("Light"), Some("Dark")]);
    assert!(
        f.samples[0].score > 15.0 && f.samples[1].score < 1.5,
        "{f:?}"
    );
    let worst = f.worst().unwrap();
    assert_eq!(worst.name.as_deref(), Some("Dark"));
    assert_eq!(worst.ink.to_rgba8().to_u8_array(), [240, 240, 240, 255]);
    assert_eq!(
        worst.backdrop.to_rgba8().to_u8_array(),
        [255, 255, 255, 255]
    );

    let offered = contrast::passing_variables(&d.doc, &f, Level::Aa, worst.ink);
    assert_eq!(offered.iter().map(|(v, _)| *v).collect::<Vec<_>>(), [both]);
    // Both modes draw the text on white, so a darker raw colour passes both.
    assert!(contrast::nearest(&f, Level::Aa, worst.ink).is_some());
}

/// APCA scores Lc and holds body text to 75: `#767676` on white passes AA's
/// 4.5 : 1 and fails APCA.
#[test]
fn apca_scores_lc() {
    let d = doc((118, 118, 118), 16.0, false);
    let options = contrast::Options {
        level: Level::Apca,
        non_text: false,
    };
    let r = contrast::check_with(&d.doc, d.page, options, None);
    let f = only(&r);
    assert_eq!(f.needs, 75.0);
    assert!((60.0..75.0).contains(&f.ratio), "{f:?}");
}

/// The non-text check measures a small shape at 3 : 1 and leaves a large one,
/// a surface, alone.
#[test]
fn non_text_measures_marks_not_surfaces() {
    let mut d = doc((0, 0, 0), 16.0, false);
    let (icon, card) = (d.ids.mint(), d.ids.mint());
    let rect = |id, w, h, name: &str| Operation::CreateNode {
        id,
        parent: d.frame,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        // Side by side, so neither covers the other.
        transform: Some(Affine::translate((200.0 + w, 100.0))),
        name: Some(name.into()),
    };
    d.doc
        .apply(&Transaction(vec![
            rect(icon, 16.0, 16.0, "Icon"),
            Operation::SetFills {
                id: icon,
                fills: solid(200, 200, 200),
            },
            rect(card, 80.0, 60.0, "Card"),
            Operation::SetFills {
                id: card,
                fills: solid(200, 200, 200),
            },
        ]))
        .unwrap();
    let options = contrast::Options {
        level: Level::Aa,
        non_text: true,
    };
    let r = contrast::check_with(&d.doc, d.page, options, None);
    let icon_f = r.findings.iter().find(|f| f.id == icon).expect("measured");
    assert!(icon_f.non_text && icon_f.needs == 3.0 && !icon_f.passes());
    assert!(r.findings.iter().all(|f| f.id != card), "{r:?}");
    let plain = check(&d.doc, d.page, Level::Aa);
    assert!(
        plain.findings.iter().all(|f| f.id != icon),
        "off by default"
    );
}

/// `only` limits the measuring to the layers named.
#[test]
fn only_measures_the_layers_named() {
    let d = doc((0, 0, 0), 16.0, false);
    let r = contrast::check_with(&d.doc, d.page, Default::default(), Some(&[d.frame]));
    assert!(r.findings.is_empty(), "{r:?}");
    let r = contrast::check_with(&d.doc, d.page, Default::default(), Some(&[d.text]));
    assert_eq!(r.findings.len(), 1);
}

/// A text layer reading "Label" under `parent`, at `at`, in `size` px.
fn text_op(id: NodeId, parent: NodeId, index: usize, at: (f64, f64), size: f64) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Text {
            content: "Label".into(),
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
        transform: Some(Affine::translate(at)),
        name: Some("Label".into()),
    }
}

/// A layer in an auto layout is measured where it sits: its backdrop is not the
/// sibling that would slide into its place were it hidden, even an identical one.
///
/// flip: `measure` building the backdrop with `SetVisible { visible: false }`
/// → fails at the first label's score.
#[test]
fn a_layer_in_an_auto_layout_keeps_its_place_while_measured() {
    let mut d = doc((0, 0, 0), 16.0, false);
    let (a, b) = (d.ids.mint(), d.ids.mint());
    d.doc
        .apply(&Transaction(vec![
            Operation::DeleteNode { id: d.text },
            text_op(a, d.frame, 0, (0.0, 0.0), 16.0),
            Operation::SetFills {
                id: a,
                fills: solid(0, 0, 0),
            },
            text_op(b, d.frame, 1, (0.0, 0.0), 16.0),
            Operation::SetFills {
                id: b,
                fills: solid(0, 0, 0),
            },
            Operation::SetDisplay {
                id: d.frame,
                display: Some(ondin_core::container::Display::Flex(
                    ondin_core::container::Flex {
                        column_gap: 8.0,
                        padding: [20.0; 4],
                        ..Default::default()
                    },
                )),
            },
        ]))
        .unwrap();
    let r = check(&d.doc, d.page, Level::Aa);
    for id in [a, b] {
        let f = r.findings.iter().find(|f| f.id == id).expect("measured");
        assert!(f.ratio > 15.0, "{f:?}");
    }
}

/// Ink the colour of its backdrop is measured, at 1 : 1, not left out as a
/// layer that draws nothing.
///
/// flip: `measure` reading coverage from the white ink against the backdrop
/// instead of against the black ink → fails at `only`.
#[test]
fn ink_the_colour_of_its_backdrop_scores_one() {
    let d = doc((255, 255, 255), 16.0, false);
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    assert!(f.ratio < 1.01 && !f.passes(), "{f:?}");
}

/// A faded group's opacity is counted once: white type over a black panel, both
/// in a group at 50% on white, draws white on mid grey (4 : 1), which passes for
/// large text.
///
/// flip: `measure` laying the ink over the backdrop (`under` always the
/// backdrop) → fails at `passes`.
#[test]
fn an_ancestor_s_opacity_is_counted_once() {
    let mut d = doc((255, 255, 255), 30.0, false);
    let (group, panel, label) = (d.ids.mint(), d.ids.mint(), d.ids.mint());
    d.doc
        .apply(&Transaction(vec![
            Operation::DeleteNode { id: d.text },
            Operation::CreateNode {
                id: group,
                parent: d.frame,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: Some("Group".into()),
            },
            Operation::CreateNode {
                id: panel,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(400.0, 200.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Panel".into()),
            },
            Operation::SetFills {
                id: panel,
                fills: solid(0, 0, 0),
            },
            text_op(label, group, 1, (20.0, 20.0), 30.0),
            Operation::SetFills {
                id: label,
                fills: solid(255, 255, 255),
            },
            Operation::SetOpacity {
                id: group,
                opacity: 0.5,
            },
        ]))
        .unwrap();
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    assert_eq!(f.needs, 3.0);
    assert!(f.passes() && (3.5..4.5).contains(&f.ratio), "{f:?}");
}

/// A mark is scored by the colour it draws at each pixel: a white box with a
/// dark border on white passes on its border, which its white fill never
/// changes.
///
/// flip: `measure` giving every pixel the first colour → fails at `passes`.
#[test]
fn a_bordered_mark_is_scored_by_the_colour_it_draws() {
    let mut d = doc((0, 0, 0), 16.0, false);
    let check_box = d.ids.mint();
    d.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: check_box,
                parent: d.frame,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(16.0, 16.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((300.0, 120.0))),
                name: Some("Box".into()),
            },
            Operation::SetFills {
                id: check_box,
                fills: solid(255, 255, 255),
            },
            Operation::SetStrokes {
                id: check_box,
                strokes: keyed_by_position([ondin_core::Stroke {
                    brush: Brush::Solid(Color::from_rgba8(0x40, 0x40, 0x40, 255)).into(),
                    width: 1.5,
                    align: ondin_core::StrokeAlign::Inside,
                    ..Default::default()
                }]),
            },
        ]))
        .unwrap();
    let options = contrast::Options {
        level: Level::Aa,
        non_text: true,
    };
    let r = contrast::check_with(&d.doc, d.page, options, Some(&[check_box]));
    let f = only(&r);
    assert!(f.passes() && f.ratio > 8.0, "{f:?}");
}

/// A layer shown in one mode only is checked in that mode.
///
/// flip: `check_with` choosing the subjects once, in the document as saved →
/// fails at `only`.
#[test]
fn a_layer_shown_in_one_mode_only_is_checked_there() {
    let mut d = doc((0, 0, 0), 16.0, false);
    let (light, dark, shown) = (d.ids.mint_mode(), d.ids.mint_mode(), d.ids.mint_var());
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
                    id: shown,
                    name: "dark-only".into(),
                    kind: VarKind::Boolean,
                    cells: [(light, Cell::Boolean(false)), (dark, Cell::Boolean(true))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetVisibleVar {
                id: d.text,
                var: Some(shown),
            },
        ]))
        .unwrap();
    let f = only(&check(&d.doc, d.page, Level::Aa)).clone();
    let names: Vec<_> = f.samples.iter().map(|s| s.name.as_deref()).collect();
    assert_eq!(names, [Some("Dark")]);
}

/// A mask's paint is never drawn, so a text mask is not measured.
///
/// Pins the result, not `subjects`' mask filter: with the filter gone the mask
/// is still left out, since its ink covers nothing for `measure` to read.
#[test]
fn a_mask_is_not_measured() {
    let mut d = doc((150, 150, 150), 16.0, true);
    d.doc
        .apply(&Transaction(vec![Operation::SetMask {
            id: d.text,
            mask: true,
        }]))
        .unwrap();
    let r = check(&d.doc, d.page, Level::Aa);
    assert!(
        r.findings.iter().all(|f| f.id != d.text),
        "no finding: {r:?}"
    );
}

/// The size and weight edges of every level's thresholds.
#[test]
fn the_size_thresholds() {
    let table = [
        // (px, weight, AA, AAA, APCA)
        (18.65, 700, 4.5, 7.0, 60.0),
        (18.66, 700, 3.0, 4.5, 60.0),
        (18.66, 400, 4.5, 7.0, 75.0),
        (23.99, 400, 4.5, 7.0, 75.0),
        (24.0, 400, 3.0, 4.5, 60.0),
        (24.0, 700, 3.0, 4.5, 45.0),
        (16.0, 700, 4.5, 7.0, 60.0),
        (15.99, 700, 4.5, 7.0, 75.0),
        (36.0, 400, 3.0, 4.5, 45.0),
    ];
    for (px, weight, aa, aaa, apca) in table {
        assert_eq!(Level::Aa.needs(px, weight), aa, "AA {px} {weight}");
        assert_eq!(Level::Aaa.needs(px, weight), aaa, "AAA {px} {weight}");
        assert_eq!(Level::Apca.needs(px, weight), apca, "APCA {px} {weight}");
    }
}

/// The strictest run's threshold is the layer's: large type with one small run
/// needs body text's 4.5 : 1.
///
/// flip: `check_one` folding the runs' thresholds with `min` → fails at the
/// threshold.
#[test]
fn the_strictest_run_sets_the_threshold() {
    let mut d = doc((0, 0, 0), 30.0, false);
    let style = TextStyle {
        font_family: "Inter".into(),
        font_size: 30.0,
        weight: 400,
        ..Default::default()
    };
    let mut spans = ondin_core::CharSpans::default();
    spans.set(0..5, ondin_core::CharAttr::Size(12.0), &style);
    d.doc
        .apply(&Transaction(vec![Operation::SetTextSpans {
            id: d.text,
            spans,
        }]))
        .unwrap();
    assert_eq!(only(&check(&d.doc, d.page, Level::Aa)).needs, 4.5);
}
