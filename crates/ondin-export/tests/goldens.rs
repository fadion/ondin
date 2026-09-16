//! Golden files (§11): one fixture document, its SVG and its JSON snapshot,
//! compared **byte for byte** against files checked into `tests/goldens/`.
//!
//! §11 has specified these since the design was written and none existed until
//! 2026-08-22 — every other export test asserts on substrings and in-process
//! measurements, which catch a value that is wrong and miss a value that merely
//! *changed*. That is the gap these close: a golden has no opinion about what
//! matters, so it notices the gradient that stopped being emitted, the transform
//! that moved up a level, the field that quietly left the snapshot schema.
//!
//! **SVG and JSON only.** The raster half needs its SIMD level pinned before a
//! byte comparison means anything, and although that pin now exists
//! (`ondin_render::cpu`'s `export_settings`, §15 D304) it is `Level::baseline()`
//! — a *per-architecture* answer, with aarch64 still unmeasured. A checked-in PNG
//! would therefore be a golden for x86 that fails on Apple silicon for reasons
//! having nothing to do with the change under review. It is one measurement away
//! and deliberately not taken here.
//!
//! ⚠️ **And the architecture is not the only axis — a raster golden is per
//! `vello_cpu` version too, measured at the 0.0.9 → 0.2.0 bump** (§15 D414).
//! Rendering the fixture below under both and comparing the raw buffers: 162
//! bytes over 78 pixels differ, worst channel delta 1, alpha unmoved, **and every
//! one of them belongs to the hexagon's mitered outside stroke** — confirmed by
//! hiding that stroke and getting byte-identical output. So a raster golden is
//! pinned to an architecture *and* to a dependency version. **That is a weaker
//! constraint than it sounds and does not block this**: a pinned dependency is a
//! decision in `Cargo.toml`, where a detected CPU is not. What it does mean is
//! that whoever bumps `vello_cpu` next will see a diff on every stroked shape,
//! and **must not read it as a regression** — regenerate, then check the residual
//! is confined to strokes and its worst delta is 1, which is what the two runs
//! above measured. A residual anywhere else is the finding.
//!
//! **When one of these fails, the diff is the review.** Regenerate with
//! `ONDIN_UPDATE_GOLDENS=1 cargo test -p ondin-export --test goldens`, then read
//! what moved before committing it. A regenerated golden that nobody read is
//! worse than no golden at all, because it converts a question into a record.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ondin_core::kurbo::{Affine, BezPath, Cap, Join, Point, RoundedRectRadii, Size};
use ondin_core::peniko::{Brush, Color, Gradient};
use ondin_core::{
    BoolOp, Document, Fill, IdSource, NodeKind, Operation, Resolved, Stroke, StrokeAlign,
    TextSizing, TextStyle, Transaction,
};
use ondin_export::snapshot::snapshot;
use ondin_export::svg::svg;

/// Every `NodeKind` the model has, one name per variant.
///
/// **These are this file's own spelling, not the snapshot's.** They coincide for
/// ten of the eleven; the snapshot bakes the operator into the name and writes
/// `boolean-subtract`. Deriving this list from the projection instead would make
/// the coverage check agree with whatever the projection currently does, which is
/// the one thing it must not do — the projection is under test.
///
/// Kept beside `kind_name`, whose `match` is exhaustive and is the actual
/// tripwire: **adding a variant to `NodeKind` breaks this file's compile**, which
/// is the point — a hand-written list alone would go on passing while the new
/// kind was never exported and never asserted, and a `NodeKind` sweep is
/// precisely the kind the compiler only half-finds. What the tripwire cannot do
/// is force the *fixture* to grow the new node; that is what the arm's neighbour
/// in this list is for, and why they sit together.
const EVERY_KIND: &[&str] = &[
    "root", "artboard", "group", "rect", "ellipse", "polygon", "star", "line", "path", "boolean",
    "text",
];

/// Every `EffectKind` the model has, one name per variant — `EVERY_KIND`'s
/// shape for the **second** enum in the model with the same sweep hazard (§15
/// D658, `[S8.3-L6-07]`).
///
/// (Plain backticks throughout: nothing checks an intra-doc link in a
/// `crates/*/tests/` file — a separate crate root with no `deny` — so a `[link]`
/// here is decoration. §15 D319's convention, and §15 D622 for the third blind
/// target it turned out to have.)
///
/// 🚨 **The JSON golden's stated purpose named a case it could not answer.** This
/// file's header says a golden *"notices … the field that quietly left the
/// snapshot schema"*; measured against `tests/goldens/every-kind.json`, the
/// `effects` field and the whole of `EffectSummary` — `blur`, `spread`,
/// `brightness`, `contrast`, `saturation`, `hue` — appeared in **none** of its 51
/// keys, and neither did `pivot`. Both are `skip_serializing_if`-elided, so their
/// absence was correct *output* for a fixture that carried no `SetEffects` and no
/// pivot on any node. **Deleting the `effects` field from `NodeSnapshot` entirely
/// would have left both goldens byte-identical.**
///
/// `EffectKind` is covered elsewhere — `snapshot.rs`, `svg.rs`'s shadow and filter
/// tests, `svg_roundtrip.rs` — so nothing was uncovered outright. What was missing
/// is the **change-detector**, which is a different thing and is what a golden is
/// for.
///
/// Kept beside `effect_name`, whose `match` is exhaustive and is the actual
/// tripwire, exactly as `EVERY_KIND` sits beside `kind_name`: a fifth variant
/// breaks this file's compile.
const EVERY_EFFECT: &[&str] = &["drop-shadow", "inner-shadow", "layer-blur", "filters"];

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

/// The document both goldens are taken from: one artboard holding **one node of
/// every kind**, painted so the writers have something to say about each.
///
/// Deliberately not `common::fixture()`, which is missing three kinds and is the
/// subject of two dozen substring assertions that would have to move with it. A
/// golden's fixture wants to be free to grow.
///
/// Everything here is fixed: the id seed, the geometry, the colours, and a font
/// (`Inter`) that ships in the binary — so nothing in the output depends on the
/// machine, the clock, or what fonts happen to be installed.
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
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(250, 250, 252, 255)),
                visible: true,
            }],
        },
        Operation::CreateNode {
            id: rect,
            parent: artboard,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(90.0, 50.0),
                corner_radii: RoundedRectRadii::new(8.0, 4.0, 0.0, 12.0),
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
        // A group with a real transform and a child, so group opacity has
        // somewhere to be wrong: on a `<g>` it fades the pair together, on the
        // child it fades each separately.
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
            },
            transform: None,
            name: None,
        },
        // A boolean writes its *result*, never its operands — the one kind whose
        // markup is not a transcription of the node.
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
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(60, 120, 220, 255)),
                visible: true,
            }],
        },
        Operation::SetStrokes {
            id: rect,
            strokes: vec![Stroke {
                brush: Brush::Solid(Color::from_rgba8(20, 30, 60, 255)),
                width: 2.5,
                join: Join::Round,
                cap: Cap::Round,
                dashes: vec![6.0, 3.0],
                dash_offset: 1.0,
                sides: Default::default(),
                miter_limit: 4.0,
                dash_fit: false,
                align: StrokeAlign::Center,
                visible: true,
            }],
        },
        // A gradient, so the `<defs>` block and the stop list are covered.
        Operation::SetFills {
            id: ellipse,
            fills: vec![Fill {
                brush: Brush::Gradient(
                    Gradient::new_linear(Point::new(0.0, 0.0), Point::new(70.0, 45.0))
                        .with_stops([
                            (0.0_f32, Color::from_rgba8(255, 96, 0, 255)),
                            (0.5_f32, Color::from_rgba8(255, 200, 40, 255)),
                            (1.0_f32, Color::from_rgba8(0, 90, 200, 255)),
                        ])
                        .into(),
                ),
                visible: true,
            }],
        },
        Operation::SetFills {
            id: polygon,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(30, 160, 110, 255)),
                visible: true,
            }],
        },
        Operation::SetFills {
            id: star,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(220, 60, 90, 255)),
                visible: true,
            }],
        },
        // Centred, because `geometry::stroke_align_applies` is false for a `Line`
        // — it has no interior, so "outside" is not a side. The outside stroke
        // that exercises `align_clip` is on the polygon below; putting it here
        // was the first draft and the golden showed it doing nothing.
        Operation::SetStrokes {
            id: line,
            strokes: vec![Stroke {
                brush: Brush::Solid(Color::from_rgba8(90, 90, 100, 255)),
                width: 3.0,
                join: Join::Miter,
                cap: Cap::Butt,
                dashes: vec![],
                dash_offset: 0.0,
                sides: Default::default(),
                miter_limit: 4.0,
                dash_fit: false,
                align: StrokeAlign::Center,
                visible: true,
            }],
        },
        // An *outside* stroke on a closed shape — not a `stroke` attribute but a
        // `<clipPath>` plus a doubled width (§7.1, §15 D97).
        Operation::SetStrokes {
            id: polygon,
            strokes: vec![Stroke {
                brush: Brush::Solid(Color::from_rgba8(10, 70, 50, 255)),
                width: 3.0,
                join: Join::Miter,
                cap: Cap::Butt,
                dashes: vec![],
                dash_offset: 0.0,
                sides: Default::default(),
                miter_limit: 4.0,
                dash_fit: false,
                align: StrokeAlign::Outside,
                visible: true,
            }],
        },
        Operation::SetFills {
            id: path,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(140, 100, 220, 255)),
                visible: true,
            }],
        },
        Operation::SetFills {
            id: nested,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(40, 40, 45, 255)),
                visible: true,
            }],
        },
        Operation::SetFills {
            id: operand_a,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(15, 130, 190, 255)),
                visible: true,
            }],
        },
        // **On the boolean itself, not on its operands.** The container carries
        // the paint; an operand's fill is not inherited by the result. The first
        // draft painted only `operand_a` and the golden came back with the
        // notched path at `fill="none"` — a boolean that draws nothing, which is
        // a golden of the wrong document.
        Operation::SetFills {
            id: boolean,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(15, 130, 190, 255)),
                visible: true,
            }],
        },
        Operation::SetOpacity {
            id: group,
            opacity: 0.6,
        },
        // **All four effect kinds on one node** (§15 D658, `[S8.3-L6-07]`), so
        // `effects` and every field of `EffectSummary` reach the JSON golden and
        // a `<filter>` reaches the SVG one. Neither had a single occurrence
        // before, which made the goldens silent about a whole schema branch.
        //
        // On the rect rather than spread about, because the point here is
        // *presence in the output* rather than a rendering claim — the rendering
        // ones live in `svg.rs` and `gpu_effects.rs` — and one node carrying four
        // keeps the diff readable. Every number is a literal for the same reason
        // everything else in this fixture is: the golden must not move because a
        // default did.
        Operation::SetEffects {
            id: rect,
            effects: vec![
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
            ],
        },
        // **A pivot, on a different node** — the other field with no occurrence
        // in either golden (§15 D658). `Normalized` rather than `Local`, because
        // the snapshot *derives* this one against the node's box and the fraction
        // is the arm where that derivation can be wrong; a local point would
        // round-trip through a constant projection.
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
/// The failure message carries the first differing line from **both** sides and
/// the line and byte counts, because "the bytes differ" on a 300-line file is a
/// message that sends the reader to a diff tool rather than telling them
/// anything. Written with `\n` and read without translation; `.gitattributes`
/// marks this directory `-text` so a checkout on a machine with
/// `core.autocrlf=true` cannot turn a line ending into a test failure.
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

/// The fixture is only worth comparing if it still holds one of everything.
///
/// This runs before the two comparisons are worth reading: a golden that lost a
/// node kind goes on matching itself forever, and the failure it should have
/// produced never happens.
#[test]
fn the_fixture_holds_one_node_of_every_kind() {
    let (doc, _) = golden_document();

    // Walked from the root rather than read off the snapshot: the snapshot is one
    // of the things under test, and a kind it had stopped projecting would make
    // this assertion agree with the bug.
    fn walk(doc: &Document, id: ondin_core::NodeId, out: &mut BTreeSet<&'static str>) {
        let Some(node) = doc.get(id) else { return };
        out.insert(kind_name(node.kind()));
        for child in node.children() {
            walk(doc, *child, out);
        }
    }
    let mut present = BTreeSet::new();
    walk(&doc, doc.root(), &mut present);
    let expected: BTreeSet<&str> = EVERY_KIND.iter().copied().collect();

    let missing: Vec<&&str> = expected.difference(&present).collect();
    assert!(
        missing.is_empty(),
        "the golden fixture no longer covers every NodeKind — missing {missing:?}. \
         Add a node of each to `golden_document`, then regenerate: {}",
        regenerate_hint()
    );
    assert_eq!(
        present, expected,
        "`EVERY_KIND` and the fixture disagree about what kinds exist"
    );
}

/// And it holds one of every **effect** kind, which is the second enum with the
/// same sweep hazard — §15 D658, `[S8.3-L6-07]`.
///
/// 🚨 **Both goldens were silent about a whole schema branch.** `effects` and
/// every field of `EffectSummary` appeared in none of `every-kind.json`'s keys,
/// and neither did `pivot`; the SVG golden had no `<filter>` for the same reason.
/// Both fields are `skip_serializing_if`-elided, so that was correct output for a
/// fixture carrying neither — which means **deleting `NodeSnapshot::effects`
/// would have left both goldens byte-identical**, against a header that claims
/// this file notices *"the field that quietly left the snapshot schema"*.
///
/// The tripwire is `effect_name`'s exhaustive `match`, not the assertion below:
/// a fifth `EffectKind` breaks this file's compile, and the assertion is what then
/// forces the *fixture* to grow it. Exactly `EVERY_KIND`'s arrangement, and it is
/// written out twice rather than shared because the two enums have nothing in
/// common but the hazard.
///
/// ⚠️ **Read off the model, not off the snapshot**, for the reason the kind sweep
/// gives one function up: the projection is under test, and an effect it had
/// stopped projecting would make this assertion agree with the bug.
///
/// ⚠️ **What this still does not cover is a `<mask>`**, which both goldens also
/// lack, along with `<radialGradient>`, `<image>`, `<textPath>` and `<use>` on the
/// SVG side. One masked node in `golden_document` would close most of that on both
/// sides at once; it is left because a mask restructures the markup around the run
/// it covers and that diff wants reading on its own.
///
/// **The regeneration diff, read** — this file's header asks for that and *"a
/// regenerated golden that nobody read is worse than no golden at all"*. SVG: one
/// `<filter id="fx-601d_3">` in `<defs>` and a `<g filter=…>` around the rect,
/// 28 lines. Every number checks out by hand — `stdDeviation` is half the typed
/// blur at all three of 6/4/2, `spread: 1` is `feMorphology dilate radius="1"`,
/// the offsets are verbatim, `flood-opacity` is `90/255 = 0.3529` and
/// `120/255 = 0.4706`, and the filter matrix's `0.05` column is contrast 0.9's
/// intercept, `(1 − 0.9) / 2`.
///
/// ⚠️ **The second regeneration was one line and it is the `<filter>`'s own
/// region** (§15 D742, `[S10.2-L1-01]`): `x="8.75" y="9.75" width="118.5"
/// height="78.5"` became `x="0.5139" y="1.5139" width="134.9721"
/// height="94.9721"`, which is 8.2361 outward on all four sides — the fixture's
/// inner shadow at `reach(2) + |spread 0| + |offset (−2, −1)|`. **Nothing else in
/// the file moved**, and nothing about the picture did either: on this node, a
/// rounded rect with a non-zero offset, the padded and unpadded regions sample
/// identically in a browser. That is worth knowing rather than reassuring — the
/// shapes the bug is *visible* on are the ones with no corners to spare, and this
/// golden holds one of the others, so **this diff is not evidence the repair
/// works and the two render tests are.** JSON: the four effect objects and
/// `"pivot": [17.5, 33.75]` on the ellipse, which is `70 × 0.25` and `45 × 0.75`
/// against a 70 × 45 box — the derivation, not the stored fraction, which is the
/// whole reason a `Normalized` pivot is the one worth putting here.
///
/// ⚠️ **The pivot produced no SVG change at all, and that is correct**:
/// `Operation::SetPivot`'s own doc says the render boundary, `Resolved`,
/// hit-testing and the SVG writer never see it. A diff on the SVG side would have
/// been the finding.
///
/// ⚠️ **One thing in the diff that looks like a leak of list order and is the
/// design.** The stack is written `[DropShadow, InnerShadow, LayerBlur, Filters]`
/// and the filter emits the blur and the colour matrix **first**, casting both
/// shadows off the result (`fxa0` derives from `fx3`). §6.4 decides that: *"`run`
/// is two stages, not one fold, and the SVG writer is arranged the same way for
/// the same reason. `Filters` and `LayerBlur` transform the layer's own appearance
/// in list order; every shadow is then cast from **that** result"* — the one-fold
/// spelling is rejected there because an inner shadow after a drop shadow would
/// take its silhouette from the layer *plus* the shadow. §5.3a's `stack_escape`
/// computes the escape per stage on the same basis (§15 D473).
///
/// 🚨 **This paragraph said it was "§5.3a's question and not this file's" until
/// `arch-scribe` read the citation** — a resolving reference pointing at the
/// opposite of what it says, which is the shape session 16 wrote down and this is
/// the next instance of. **Ask what the cited section decides, not whether it is
/// the right neighbourhood.**
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
    walk(&doc, doc.root(), &mut present);
    let expected: BTreeSet<&str> = EVERY_EFFECT.iter().copied().collect();

    assert_eq!(
        present,
        expected,
        "the golden fixture no longer covers every EffectKind. Add one of each to \
         `golden_document`, then regenerate: {}",
        regenerate_hint()
    );

    // And a pivot, the other field that had no occurrence in either golden. Not a
    // separate test: it is the same claim — that the fixture reaches the states
    // the goldens are supposed to be watching.
    fn any_pivot(doc: &Document, id: ondin_core::NodeId) -> bool {
        let Some(node) = doc.get(id) else {
            return false;
        };
        node.pivot().is_some() || node.children().iter().any(|c| any_pivot(doc, *c))
    }
    assert!(
        any_pivot(&doc, doc.root()),
        "no node carries a pivot, so `NodeSnapshot::pivot` is unwatched: {}",
        regenerate_hint()
    );
}

/// SVG output, byte for byte.
#[test]
fn svg_matches_its_golden() {
    let (doc, res) = golden_document();
    check_golden("every-kind.svg", &svg(&doc, &res, None));
}

/// The JSON snapshot, byte for byte — and produced exactly the way
/// `ondin export --json` produces it, so the golden is of the *command's* output
/// and not of a spelling only this test uses.
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

/// The property the two goldens above rest on: exporting the same document twice
/// in one process gives the same bytes.
///
/// Cheap, and it separates two failures that look identical from the outside — a
/// golden that no longer matches because the writer *changed* from one that no
/// longer matches because the writer is not deterministic. Without this, the
/// first suspicion on a red golden is the wrong one.
#[test]
fn exporting_the_same_document_twice_gives_the_same_bytes() {
    let (doc, res) = golden_document();
    assert_eq!(
        svg(&doc, &res, None),
        svg(&doc, &res, None),
        "SVG is stable"
    );

    let a = serde_json::to_vec_pretty(&snapshot(&doc, &res)).unwrap();
    let b = serde_json::to_vec_pretty(&snapshot(&doc, &res)).unwrap();
    assert_eq!(a, b, "the snapshot is stable");

    // And a second *resolve* of the same document, which is where an iteration
    // order could leak in that one `Resolved` cannot show.
    let again = Resolved::rebuild(&doc);
    assert_eq!(
        svg(&doc, &res, None),
        svg(&doc, &again, None),
        "SVG does not depend on which rebuild produced the layout"
    );
}
