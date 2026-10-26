//! Golden files (§11): one fixture document, its SVG and its JSON snapshot,
//! compared byte for byte against files checked into `tests/goldens/`.
//!
//! Other export tests assert on substrings and measurements, which catch a wrong
//! value and miss a changed one; a golden notices the gradient that stopped being
//! emitted or the field that left the snapshot schema.
//!
//! SVG and JSON only: a raster golden is pinned to the architecture (the CPU
//! backend's SIMD level, aarch64 unmeasured) and to the `vello_cpu` version, whose
//! last bump moved only the mitered outside stroke by one channel step. A raster
//! golden would be regenerated after checking the residual is confined to strokes.
//!
//! When one fails, the diff is the review. Regenerate with
//! `ONDIN_UPDATE_GOLDENS=1 cargo test -p ondin-export --test goldens`, then read
//! what moved before committing it.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ondin_core::kurbo::{Affine, BezPath, Cap, Join, Point, RoundedRectRadii, Size};
use ondin_core::peniko::{Brush, Color, Gradient};
use ondin_core::{
    BoolOp, Document, Fill, IdSource, NodeKind, Operation, Resolved, Stroke, StrokeAlign,
    TextSizing, TextStyle, Transaction, keyed_by_position,
};
use ondin_export::snapshot::snapshot;
use ondin_export::svg::svg;

/// Every `NodeKind` the model has, one name per variant, read off `kind_name`'s
/// arms (`arm_names`) so the arm the compiler forces for a new variant also grows
/// what the fixture must hold. The names are this file's own, not the snapshot's
/// (which writes `boolean-subtract`): the projection is under test.
fn every_kind() -> BTreeSet<&'static str> {
    arm_names("kind_name")
}

/// The names a `*_name` function's `match` gives, read out of this file's own
/// source, since nothing at run time can enumerate a `match`'s arms. It takes every
/// `=> "…"` up to the function's first column-zero `}`; the scanner spells that
/// pattern with a backslash so it cannot read itself.
fn arm_names(function: &str) -> BTreeSet<&'static str> {
    const SRC: &str = include_str!("goldens.rs");
    let start = SRC
        .find(&format!("fn {function}("))
        .unwrap_or_else(|| panic!("`{function}` is in this file"));
    let body = &SRC[start..];
    // `"\n}"` and not `"\n}\n"`: under `core.autocrlf=true` lines end in `\r\n`.
    let end = body
        .find("\n}")
        .expect("and ends at a closing brace in column 0");
    body[..end]
        .split("=> \"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .collect()
}

/// Every `EffectKind` the model has, one name per variant, read off
/// `effect_name`'s arms as `every_kind` is off `kind_name`'s. The effect stack and
/// pivot are `skip_serializing_if`-elided, so the goldens only watch them while
/// the fixture carries them.
fn every_effect() -> BTreeSet<&'static str> {
    arm_names("effect_name")
}

fn effect_name(k: &ondin_core::EffectKind) -> &'static str {
    use ondin_core::EffectKind as E;
    match k {
        E::DropShadow(_) => "drop-shadow",
        E::InnerShadow(_) => "inner-shadow",
        E::LayerBlur { .. } => "layer-blur",
        E::Filters(_) => "filters",
    }
}

fn kind_name(k: &NodeKind) -> &'static str {
    match k {
        NodeKind::Root => "root",
        NodeKind::Artboard { .. } => "artboard",
        NodeKind::Group => "group",
        NodeKind::Rect { .. } => "rect",
        NodeKind::Ellipse { .. } => "ellipse",
        NodeKind::Polygon { .. } => "polygon",
        NodeKind::Star { .. } => "star",
        NodeKind::Line { .. } => "line",
        NodeKind::Path { .. } => "path",
        NodeKind::Boolean { .. } => "boolean",
        NodeKind::Text { .. } => "text",
    }
}

/// The document both goldens are taken from: one artboard holding one node of
/// every kind, painted so the writers have something to say about each. It is not
/// `common::fixture()`, whose substring assertions would have to move with it.
/// Everything is fixed — id seed, geometry, colours, and the bundled `Inter` — so
/// the output does not depend on the machine.
fn golden_document() -> (Document, Resolved) {
    let mut ids = IdSource::new(0x601D);
    let root = ids.mint();
    let mut doc = Document::new(root);

    let artboard = ids.mint();
    let rect = ids.mint();
    let ellipse = ids.mint();
    let polygon = ids.mint();
    let star = ids.mint();
    let line = ids.mint();
    let path = ids.mint();
    let group = ids.mint();
    let nested = ids.mint();
    let boolean = ids.mint();
    let operand_a = ids.mint();
    let operand_b = ids.mint();
    let text = ids.mint();

    let mut outline = BezPath::new();
    outline.move_to(Point::new(0.0, 0.0));
    outline.line_to(Point::new(40.0, 12.0));
    outline.line_to(Point::new(24.0, 44.0));
    outline.close_path();

    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: artboard,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 320.0),
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: Some("Board".into()),
        },
        Operation::SetFills {
            id: artboard,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(250, 250, 252, 255)).into(),
                visible: true,
            }]),
        },
        Operation::CreateNode {
            id: rect,
            parent: artboard,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(90.0, 50.0),
                corner_radii: RoundedRectRadii::new(8.0, 4.0, 0.0, 12.0),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((20.0, 20.0))),
            name: Some("Rounded".into()),
        },
        Operation::CreateNode {
            id: ellipse,
            parent: artboard,
            index: 1,
            kind: NodeKind::Ellipse {
                size: Size::new(70.0, 45.0),
            },
            transform: Some(Affine::translate((140.0, 20.0))),
            name: None,
        },
        Operation::CreateNode {
            id: polygon,
            parent: artboard,
            index: 2,
            kind: NodeKind::Polygon {
                size: Size::new(60.0, 60.0),
                sides: 6,
                corner_radius: 0.0,
            },
            transform: Some(Affine::translate((240.0, 20.0))),
            name: None,
        },
        Operation::CreateNode {
            id: star,
            parent: artboard,
            index: 3,
            kind: NodeKind::Star {
                size: Size::new(60.0, 60.0),
                points: 5,
                inner_ratio: 0.382,
                corner_radius: 0.0,
            },
            transform: Some(Affine::translate((310.0, 20.0))),
            name: None,
        },
        Operation::CreateNode {
            id: line,
            parent: artboard,
            index: 4,
            kind: NodeKind::Line {
                end: Point::new(120.0, 30.0),
            },
            transform: Some(Affine::translate((20.0, 100.0))),
            name: None,
        },
        Operation::CreateNode {
            id: path,
            parent: artboard,
            index: 5,
            kind: NodeKind::Path {
                path: outline,
                corner_radii: vec![6.0, 0.0, 3.0],
            },
            transform: Some(Affine::translate((160.0, 100.0))),
            name: Some("Triangle".into()),
        },
        // A group with a transform and a child, so group opacity has somewhere to
        // be wrong (on a `<g>` it fades the pair together).
        Operation::CreateNode {
            id: group,
            parent: artboard,
            index: 6,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((240.0, 100.0))),
            name: Some("Pair".into()),
        },
        Operation::CreateNode {
            id: nested,
            parent: group,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        // A boolean writes its result, never its operands.
        Operation::CreateNode {
            id: boolean,
            parent: artboard,
            index: 7,
            kind: NodeKind::Boolean {
                op: BoolOp::Subtract,
            },
            transform: Some(Affine::translate((20.0, 170.0))),
            name: Some("Notched".into()),
        },
        Operation::CreateNode {
            id: operand_a,
            parent: boolean,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 60.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: operand_b,
            parent: boolean,
            index: 1,
            kind: NodeKind::Ellipse {
                size: Size::new(50.0, 50.0),
            },
            transform: Some(Affine::translate((55.0, 20.0))),
            name: None,
        },
        Operation::CreateNode {
            id: text,
            parent: artboard,
            index: 8,
            kind: NodeKind::Text {
                content: "Golden & <escaped>".into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: 18.0,
                    weight: 500,
                    italic: false,
                    line_height: Some(ondin_core::Length::Em(1.25)),
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
            transform: Some(Affine::translate((160.0, 250.0))),
            name: None,
        },
    ]))
    .expect("build the golden fixture");

    doc.apply(&Transaction(vec![
        // A solid fill and a dashed, round-joined stroke.
        Operation::SetFills {
            id: rect,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(60, 120, 220, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetStrokes {
            id: rect,
            strokes: keyed_by_position([Stroke {
                brush: Brush::Solid(Color::from_rgba8(20, 30, 60, 255)).into(),
                width: 2.5,
                width_var: None,
                join: Join::Round,
                cap: Cap::Round,
                dashes: vec![6.0, 3.0],
                dash_offset: 1.0,
                sides: Default::default(),
                miter_limit: 4.0,
                dash_fit: false,
                align: StrokeAlign::Center,
                visible: true,
            }]),
        },
        // A gradient, so the `<defs>` block and the stop list are covered.
        Operation::SetFills {
            id: ellipse,
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(
                    Gradient::new_linear(Point::new(0.0, 0.0), Point::new(70.0, 45.0))
                        .with_stops([
                            (0.0_f32, Color::from_rgba8(255, 96, 0, 255)),
                            (0.5_f32, Color::from_rgba8(255, 200, 40, 255)),
                            (1.0_f32, Color::from_rgba8(0, 90, 200, 255)),
                        ])
                        .into(),
                )
                .into(),
                visible: true,
            }]),
        },
        Operation::SetFills {
            id: polygon,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(30, 160, 110, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetFills {
            id: star,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(220, 60, 90, 255)).into(),
                visible: true,
            }]),
        },
        // Centred, because `geometry::stroke_align_applies` is false for a `Line`;
        // the outside stroke that exercises `align_clip` is on the polygon below.
        Operation::SetStrokes {
            id: line,
            strokes: keyed_by_position([Stroke {
                brush: Brush::Solid(Color::from_rgba8(90, 90, 100, 255)).into(),
                width: 3.0,
                width_var: None,
                join: Join::Miter,
                cap: Cap::Butt,
                dashes: vec![],
                dash_offset: 0.0,
                sides: Default::default(),
                miter_limit: 4.0,
                dash_fit: false,
                align: StrokeAlign::Center,
                visible: true,
            }]),
        },
        // An outside stroke on a closed shape: a `<clipPath>` plus a doubled width.
        Operation::SetStrokes {
            id: polygon,
            strokes: keyed_by_position([Stroke {
                brush: Brush::Solid(Color::from_rgba8(10, 70, 50, 255)).into(),
                width: 3.0,
                width_var: None,
                join: Join::Miter,
                cap: Cap::Butt,
                dashes: vec![],
                dash_offset: 0.0,
                sides: Default::default(),
                miter_limit: 4.0,
                dash_fit: false,
                align: StrokeAlign::Outside,
                visible: true,
            }]),
        },
        Operation::SetFills {
            id: path,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(140, 100, 220, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetFills {
            id: nested,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(40, 40, 45, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetFills {
            id: operand_a,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(15, 130, 190, 255)).into(),
                visible: true,
            }]),
        },
        // On the boolean itself: an operand's fill is not inherited by the result,
        // and an unpainted boolean draws nothing.
        Operation::SetFills {
            id: boolean,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(15, 130, 190, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetOpacity {
            id: group,
            opacity: 0.6,
        },
        // All four effect kinds on one node, so `effects` reaches the JSON golden
        // and a `<filter>` the SVG one; presence in the output is the point, and
        // one node keeps the diff readable. Every number is a literal so no default
        // can move the golden.
        Operation::SetEffects {
            id: rect,
            effects: keyed_by_position([
                ondin_core::Effect::new(ondin_core::EffectKind::DropShadow(ondin_core::Shadow {
                    offset: ondin_core::kurbo::Vec2::new(3.0, 4.0),
                    blur: 6.0,
                    spread: 1.0,
                    color: Color::from_rgba8(0, 0, 0, 90),
                })),
                ondin_core::Effect::new(ondin_core::EffectKind::InnerShadow(ondin_core::Shadow {
                    offset: ondin_core::kurbo::Vec2::new(-2.0, -1.0),
                    blur: 4.0,
                    spread: 0.0,
                    color: Color::from_rgba8(20, 20, 40, 120),
                })),
                ondin_core::Effect::new(ondin_core::EffectKind::LayerBlur { radius: 2.0 }),
                ondin_core::Effect::new(ondin_core::EffectKind::Filters(ondin_core::Filters {
                    brightness: 1.1,
                    contrast: 0.9,
                    saturation: 1.2,
                    hue: 15.0,
                })),
            ]),
        },
        // A pivot on a different node, `Normalized` rather than `Local` because the
        // snapshot derives it against the node's box, which is where it can be wrong.
        Operation::SetPivot {
            id: ellipse,
            pivot: Some(ondin_core::Pivot::Normalized(ondin_core::kurbo::Vec2::new(
                0.25, 0.75,
            ))),
        },
    ]))
    .expect("paint the golden fixture");

    let res = Resolved::rebuild(&doc);
    (doc, res)
}

const UPDATE_ENV: &str = "ONDIN_UPDATE_GOLDENS";

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("goldens")
        .join(name)
}

fn regenerate_hint() -> String {
    format!("{UPDATE_ENV}=1 cargo test -p ondin-export --test goldens")
}

/// Compare `actual` against the golden `name`, or rewrite it when
/// `ONDIN_UPDATE_GOLDENS=1`.
///
/// The failure message carries the first differing lines from both sides and the
/// line and byte counts. Written with `\n` and read untranslated; `.gitattributes`
/// marks this directory `-text` so `core.autocrlf=true` cannot fail the test.
fn check_golden(name: &str, actual: &str) {
    let path = golden_path(name);

    if std::env::var(UPDATE_ENV).is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().expect("goldens dir")).expect("create goldens dir");
        std::fs::write(&path, actual).expect("write golden");
        eprintln!("wrote {}", path.display());
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read the golden {}: {e}\n\
             If it has never been generated, run:\n  {}",
            path.display(),
            regenerate_hint()
        )
    });
    if expected == actual {
        return;
    }

    let want: Vec<&str> = expected.lines().collect();
    let got: Vec<&str> = actual.lines().collect();
    let at = want
        .iter()
        .zip(got.iter())
        .position(|(a, b)| a != b)
        .unwrap_or(want.len().min(got.len()));

    let window = |lines: &[&str]| -> String {
        let from = at.saturating_sub(2);
        let to = (at + 3).min(lines.len());
        lines[from..to]
            .iter()
            .enumerate()
            .map(|(i, l)| format!("    {:>4} | {l}\n", from + i + 1))
            .collect()
    };

    panic!(
        "{name} no longer matches its golden.\n\
         \n  golden: {} lines, {} bytes\
         \n  actual: {} lines, {} bytes\
         \n  first difference at line {}\
         \n\n  golden:\n{}\n  actual:\n{}\n\
         If the change is intended, regenerate with\n  {}\n\
         and read the diff before committing it — that diff is the review.\n",
        want.len(),
        expected.len(),
        got.len(),
        actual.len(),
        at + 1,
        window(&want),
        window(&got),
        regenerate_hint()
    );
}

/// The fixture still holds one node of every kind; a golden that lost a kind
/// would go on matching itself.
#[test]
fn the_fixture_holds_one_node_of_every_kind() {
    let (doc, _) = golden_document();

    // Walked from the root, not read off the snapshot, which is under test.
    fn walk(doc: &Document, id: ondin_core::NodeId, out: &mut BTreeSet<&'static str>) {
        let Some(node) = doc.get(id) else { return };
        out.insert(kind_name(node.kind()));
        for child in node.children() {
            walk(doc, *child, out);
        }
    }
    let mut present = BTreeSet::new();
    walk(&doc, doc.first_page(), &mut present);
    let expected = every_kind();
    // At least, not exactly: an exact count would be hand-kept.
    assert!(
        expected.len() >= 11,
        "the fixture: the scan read `kind_name`'s arms, got {expected:?}"
    );

    let missing: Vec<&&str> = expected.difference(&present).collect();
    assert!(
        missing.is_empty(),
        "the golden fixture no longer covers every NodeKind — missing {missing:?}. \
         Add a node of each to `golden_document`, then regenerate: {}",
        regenerate_hint()
    );
    assert_eq!(
        present, expected,
        "`kind_name` and the fixture disagree about what kinds exist"
    );
}

/// The fixture holds one node of every effect kind and a pivot, the fields the
/// goldens only watch while the fixture carries them. The expected set is read off
/// the model's `effect_name` arms, not the snapshot, which is under test.
///
/// flip: a guarded arm `E::Filters(_) if false => "halftone"` in `effect_name` →
/// fails at "the golden fixture no longer covers every EffectKind".
///
/// Not covered by either golden: `<mask>`, `<radialGradient>`, `<image>`,
/// `<textPath>` and `<use>`. The SVG filter emits the blur and colour matrix
/// first and casts both shadows off that result (§6.4), which is why the stack's
/// list order is not the markup order. A pivot changes no SVG, since the writer
/// never sees it.
#[test]
fn the_fixture_holds_one_node_of_every_effect_kind() {
    let (doc, _) = golden_document();

    fn walk(doc: &Document, id: ondin_core::NodeId, out: &mut BTreeSet<&'static str>) {
        let Some(node) = doc.get(id) else { return };
        for e in node.effects() {
            out.insert(effect_name(&e.kind));
        }
        for child in node.children() {
            walk(doc, *child, out);
        }
    }
    let mut present = BTreeSet::new();
    walk(&doc, doc.first_page(), &mut present);
    let expected = every_effect();
    // At least, not exactly: an exact count would be hand-kept.
    assert!(
        expected.len() >= 4,
        "the fixture: the scan read `effect_name`'s arms, got {expected:?}"
    );

    assert_eq!(
        present,
        expected,
        "the golden fixture no longer covers every EffectKind. Add one of each to \
         `golden_document`, then regenerate: {}",
        regenerate_hint()
    );

    // A pivot too: the same claim, that the fixture reaches the watched states.
    fn any_pivot(doc: &Document, id: ondin_core::NodeId) -> bool {
        let Some(node) = doc.get(id) else {
            return false;
        };
        node.pivot().is_some() || node.children().iter().any(|c| any_pivot(doc, *c))
    }
    assert!(
        any_pivot(&doc, doc.first_page()),
        "no node carries a pivot, so `NodeSnapshot::pivot` is unwatched: {}",
        regenerate_hint()
    );
}

/// SVG output, byte for byte.
#[test]
fn svg_matches_its_golden() {
    let (doc, res) = golden_document();
    check_golden("every-kind.svg", &svg(&doc, &res, doc.first_page(), None));
}

/// The JSON snapshot, byte for byte, produced the way `ondin export --json` does.
#[test]
fn json_snapshot_matches_its_golden() {
    let (doc, res) = golden_document();
    let snap = snapshot(&doc, &res);
    let json = serde_json::to_vec_pretty(&snap).expect("serialize snapshot");
    check_golden(
        "every-kind.json",
        &String::from_utf8(json).expect("snapshot is UTF-8"),
    );
}

/// Exporting the same document twice gives the same bytes, which separates a
/// changed writer from a non-deterministic one when a golden goes red.
#[test]
fn exporting_the_same_document_twice_gives_the_same_bytes() {
    let (doc, res) = golden_document();
    assert_eq!(
        svg(&doc, &res, doc.first_page(), None),
        svg(&doc, &res, doc.first_page(), None),
        "SVG is stable"
    );

    let a = serde_json::to_vec_pretty(&snapshot(&doc, &res)).unwrap();
    let b = serde_json::to_vec_pretty(&snapshot(&doc, &res)).unwrap();
    assert_eq!(a, b, "the snapshot is stable");

    // A second resolve, where an iteration order could leak in.
    let again = Resolved::rebuild(&doc);
    assert_eq!(
        svg(&doc, &res, doc.first_page(), None),
        svg(&doc, &again, doc.first_page(), None),
        "SVG does not depend on which rebuild produced the layout"
    );
}

/// Every kind and effect converts to a PDF in both text modes, and the same
/// document gives the same bytes, so a re-export does not churn a checked-in file.
#[test]
fn every_kind_converts_to_a_stable_pdf() {
    use ondin_core::ExportText;
    use ondin_export::pdf::pdf_of;
    let (doc, res) = golden_document();
    let page = [doc.first_page()];
    for text in [ExportText::Live, ExportText::Outlines] {
        let a = pdf_of(&doc, &res, &page, text).expect("the converter takes our SVG");
        assert!(a.pdf.starts_with(b"%PDF-"), "{text:?}");
        let b = pdf_of(&doc, &res, &page, text).expect("again");
        assert!(
            a.pdf == b.pdf,
            "{text:?} PDF bytes differ between two exports"
        );
    }
}
