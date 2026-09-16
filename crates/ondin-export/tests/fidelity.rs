//! The SVG writer says what it could not say exactly (§15 D780, `[S8.1-L7-05]`).
//!
//! **Every assertion here checks a claim against the markup**, never against a
//! second copy of the rule. `svg::report_node` reads the five features off the
//! node, where the arms that *write* them are scattered through `gradient_def`,
//! `shadow_graph`, `collect_defs` and `write_element` — so the report is a second
//! statement of each rule, which is `[A1-L7-07]`'s shape and this project's
//! most-recorded failure. What keeps the two in step is that a test saying
//! *"it reported a sweep"* also says *"and there is a `<radialGradient>` in the
//! file"*: a writer that stops approximating, or starts, fails here rather than
//! lying quietly.
//!
//! (Plain backticks throughout per §15 D319 — nothing in `crates/*/tests/` is read
//! by `cargo doc`.)

mod common;

use ondin_core::kurbo::{Affine, BezPath, Point, Size, Vec2};
use ondin_core::peniko::{Color, ColorStop, Gradient, GradientKind, SweepGradientPosition};
use ondin_core::{
    Brush, Effect, EffectKind, Fill, GeometryPatch, GradientBrush, ImageBrush, ImageEntry,
    ImageFormat, ImageId, ImageRef, ImageSource, NodeKind, Operation, Resolved, Shadow,
    Transaction,
};
use ondin_export::svg::{svg_of, svg_of_reported};

/// A sweep gradient — the one gradient kind SVG 1.1 cannot express.
fn sweep() -> Brush {
    Brush::Gradient(GradientBrush {
        gradient: Gradient {
            kind: GradientKind::Sweep(SweepGradientPosition {
                center: Point::new(50.0, 50.0),
                start_angle: 0.0,
                end_angle: std::f32::consts::TAU,
            }),
            stops: [
                ColorStop {
                    offset: 0.0,
                    color: Color::BLACK.into(),
                },
                ColorStop {
                    offset: 1.0,
                    color: Color::WHITE.into(),
                },
            ]
            .as_slice()
            .into(),
            ..Default::default()
        },
        transform: Affine::IDENTITY,
        opacity: 1.0,
    })
}

/// A rail for type to sit on, and the text node that sits on it.
fn rail() -> BezPath {
    let mut p = BezPath::new();
    p.move_to(Point::new(0.0, 0.0));
    p.line_to(Point::new(120.0, 0.0));
    p
}

/// The fixture's rect wearing every feature this writer approximates, plus a
/// **hidden** sibling wearing one — which is the control for the visibility rule.
fn everything_lossy() -> common::Fixture {
    let mut f = common::fixture();
    let pic = ImageId("pic".into());
    let hidden = ondin_core::IdSource::new(0x0F1D).mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::AddImage {
                id: pic.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(vec![1, 2, 3].into()),
                    format: ImageFormat::Png,
                    width: 8,
                    height: 8,
                },
            },
            // A sweep and a picture on one node: two features, two lists.
            Operation::SetFills {
                id: f.rect,
                fills: vec![
                    Fill {
                        brush: sweep(),
                        visible: true,
                    },
                    Fill {
                        brush: Brush::Image(ImageBrush {
                            image: ImageRef::new(pic),
                            sampler: Default::default(),
                        }),
                        visible: true,
                    },
                ],
            },
            // A spread, which is what `feMorphology` is for.
            Operation::SetEffects {
                id: f.rect,
                effects: vec![Effect::new(EffectKind::DropShadow(Shadow {
                    offset: Vec2::new(2.0, 2.0),
                    blur: 3.0,
                    spread: 4.0,
                    color: Color::BLACK,
                }))],
            },
            // Flipped type on a rail: one approximation and one round-trip loss.
            // **Two patches and not one**, which is the model's own separation —
            // the flip outlives the rail, so *Detach from path* can send
            // `TextPath(None)` without saying anything about the flip (§15 D406).
            Operation::SetGeometry {
                id: f.text,
                geometry: GeometryPatch::TextPath(Some(rail())),
            },
            Operation::SetGeometry {
                id: f.text,
                geometry: GeometryPatch::TextPathFlip(true),
            },
            // The control: the same sweep, on a layer the file does not contain.
            Operation::CreateNode {
                id: hidden,
                parent: f.artboard,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: hidden,
                fills: vec![Fill {
                    brush: sweep(),
                    visible: true,
                }],
            },
            Operation::SetVisible {
                id: hidden,
                visible: false,
            },
        ]))
        .expect("build the lossy fixture");
    f.res = Resolved::rebuild(&f.doc);
    f
}

fn has(list: &[String], needle: &str) -> bool {
    list.iter().any(|s| s.contains(needle))
}

/// Each reported approximation, corroborated by the markup that caused it.
///
/// **The pairing is the whole test.** Asserting the list alone would pass for a
/// report hard-coded to say all five; asserting the markup alone is what the rest
/// of this suite already does. Together they say the report is *about* this file.
#[test]
fn every_reported_approximation_is_visible_in_the_markup() {
    let f = everything_lossy();
    let out = svg_of_reported(&f.doc, &f.res, &[f.artboard]);

    assert!(
        has(&out.approximated, "sweep gradient"),
        "a sweep gradient is approximated: {:?}",
        out.approximated
    );
    assert!(
        out.svg.contains("<radialGradient"),
        "…and the thing it was approximated *as* is in the file"
    );

    assert!(
        has(&out.approximated, "shadow's spread"),
        "a spread is approximated: {:?}",
        out.approximated
    );
    assert!(
        out.svg.contains("feMorphology"),
        "…and the square kernel that makes it one is in the file"
    );

    assert!(
        has(&out.approximated, "type on a path"),
        "type on a rail is approximated: {:?}",
        out.approximated
    );
    assert!(
        out.svg.contains("<textPath"),
        "…and the element whose glyph placement is the viewer's is in the file"
    );
}

/// The other list: written exactly, and the *document* will not come back.
///
/// ⚠️ **These two are the ones `[S8.1-L7-05]` filed as approximations and this
/// writer's own comments argue are not.** A reversed rail is the identical
/// drawing — `text::PathWarp::flip` *is* traversing the rail backwards — and the
/// `<pattern>` was measured in a browser by the finding itself. Calling either
/// "approximated" would tell a user their drawing is wrong when it is not.
#[test]
fn a_lossless_write_with_a_lossy_re_import_is_reported_separately() {
    let f = everything_lossy();
    let out = svg_of_reported(&f.doc, &f.res, &[f.artboard]);

    assert!(
        has(&out.round_trip, "image fill"),
        "an image fill is a round-trip loss: {:?}",
        out.round_trip
    );
    assert!(out.svg.contains("<pattern"));
    assert!(
        !has(&out.approximated, "image fill"),
        "…and NOT an approximation — the picture lands where the document puts it"
    );

    assert!(
        has(&out.round_trip, "flipped type"),
        "a flipped rail is a round-trip loss: {:?}",
        out.round_trip
    );
    assert!(
        !has(&out.approximated, "flipped type"),
        "…and NOT an approximation — a reversed rail is the same drawing"
    );
}

/// A hidden layer is not in the file, so it is not in the report.
///
/// 🚨 **This is the assertion that rules out building the report in
/// `collect_defs`**, which is where it belongs by every other measure: that pass
/// already walks every origin, already carries a `&mut`, and already sees all five
/// features. It has no visibility guard — it recurses into hidden children on
/// purpose, because an unreferenced def is harmless — so a report built there
/// announces a sweep gradient on a layer the file does not contain.
///
/// **Flipped**: moving `report_node` above `emit_node`'s visibility guard — which
/// is the same mistake spelled in the place it is most tempting — fails here and
/// **nowhere else**, at `["a sweep gradient, written as a radial one"] / []`. Note
/// that `an_invisible_shadow_is_not_reported` stays green under it: that test's
/// node is visible and only its *effect* is not, so the two pin different rules
/// rather than one twice.
#[test]
fn a_hidden_layer_is_not_reported() {
    let f = everything_lossy();
    let out = svg_of_reported(&f.doc, &f.res, &[f.artboard]);
    // The visible rect carries a sweep too, so the count is what discriminates:
    // one mention, deduplicated, whatever the hidden sibling wears.
    assert_eq!(
        out.approximated
            .iter()
            .filter(|s| s.contains("sweep gradient"))
            .count(),
        1,
        "deduplicated like the reader's lists: {:?}",
        out.approximated
    );
    // **The fixture is asserted to be in the state this test is about**, or the
    // half below is about nothing: if the hidden node were missing, or visible, or
    // carried no sweep, "nothing is reported" would be green for the wrong reason.
    // Shown by *un*-hiding it and watching the report appear.
    let mut lit = everything_lossy();
    let hidden = lit
        .doc
        .get(lit.artboard)
        .expect("the artboard")
        .children()
        .iter()
        .copied()
        .find(|c| !lit.doc.get(*c).expect("a child").visible())
        .expect("the fixture has exactly one hidden child");
    lit.doc
        .apply(&Transaction(vec![
            Operation::SetFills {
                id: lit.rect,
                fills: vec![],
            },
            Operation::SetEffects {
                id: lit.rect,
                effects: vec![],
            },
            Operation::SetGeometry {
                id: lit.text,
                geometry: GeometryPatch::TextPath(None),
            },
            Operation::SetVisible {
                id: hidden,
                visible: true,
            },
        ]))
        .expect("strip the visible node and light the hidden one");
    lit.res = Resolved::rebuild(&lit.doc);
    assert!(
        has(
            &svg_of_reported(&lit.doc, &lit.res, &[lit.artboard]).approximated,
            "sweep gradient"
        ),
        "the hidden sibling really does carry a sweep — the premise of the half below"
    );

    // And with the visible fills taken away, nothing at all is reported — the
    // hidden sibling still has its sweep.
    let mut f2 = f;
    f2.doc
        .apply(&Transaction(vec![
            Operation::SetFills {
                id: f2.rect,
                fills: vec![],
            },
            Operation::SetEffects {
                id: f2.rect,
                effects: vec![],
            },
            Operation::SetGeometry {
                id: f2.text,
                geometry: GeometryPatch::TextPath(None),
            },
        ]))
        .expect("strip the visible node's features");
    f2.res = Resolved::rebuild(&f2.doc);
    let out = svg_of_reported(&f2.doc, &f2.res, &[f2.artboard]);
    assert!(
        out.approximated.is_empty() && out.round_trip.is_empty(),
        "the only lossy layer left is hidden: {:?} / {:?}",
        out.approximated,
        out.round_trip
    );
}

/// An ordinary drawing reports nothing, which is the assertion that stops the two
/// lists becoming decoration nobody reads.
#[test]
fn a_document_the_writer_can_say_exactly_reports_nothing() {
    let f = common::fixture();
    let out = svg_of_reported(&f.doc, &f.res, &[f.artboard]);
    assert!(
        out.approximated.is_empty(),
        "nothing approximated: {:?}",
        out.approximated
    );
    assert!(
        out.round_trip.is_empty(),
        "nothing lost to a round trip: {:?}",
        out.round_trip
    );
}

/// A shadow **without** a spread emits no `feMorphology`, so it is not reported.
///
/// ⚠️ **The first draft of `report_node` failed this**, and it is the cheapest of
/// the three ways that function can warn about markup that is not in the file (an
/// invisible effect and an absent filter region are the others). A fidelity report
/// that cries wolf is worse than none: it is read once and then ignored.
#[test]
fn a_shadow_with_no_spread_is_not_reported() {
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: f.rect,
            effects: vec![Effect::new(EffectKind::DropShadow(Shadow {
                offset: Vec2::new(2.0, 2.0),
                blur: 3.0,
                spread: 0.0,
                color: Color::BLACK,
            }))],
        }]))
        .expect("a plain drop shadow");
    f.res = Resolved::rebuild(&f.doc);
    let out = svg_of_reported(&f.doc, &f.res, &[f.artboard]);
    assert!(
        !out.svg.contains("feMorphology"),
        "no spread, no morphology — the premise this test rests on"
    );
    assert!(
        !has(&out.approximated, "shadow's spread"),
        "and so nothing to report: {:?}",
        out.approximated
    );
}

/// An **invisible** effect is skipped by `effect_def`'s own loop, so its spread is
/// not in the file either.
#[test]
fn an_invisible_shadow_is_not_reported() {
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: f.rect,
            effects: vec![Effect {
                kind: EffectKind::DropShadow(Shadow {
                    offset: Vec2::new(2.0, 2.0),
                    blur: 3.0,
                    spread: 4.0,
                    color: Color::BLACK,
                }),
                visible: false,
            }],
        }]))
        .expect("a switched-off drop shadow");
    f.res = Resolved::rebuild(&f.doc);
    let out = svg_of_reported(&f.doc, &f.res, &[f.artboard]);
    assert!(!out.svg.contains("feMorphology"));
    assert!(
        !has(&out.approximated, "shadow's spread"),
        "{:?}",
        out.approximated
    );
}

/// The shim is the same bytes as the reported form.
///
/// **Worth one test** because `svg_of` is what thirty-odd existing assertions and
/// every production caller but two go through: if the two ever diverged, the
/// suite would be checking markup nobody ships.
#[test]
fn the_shim_returns_the_reported_forms_markup() {
    let f = everything_lossy();
    assert_eq!(
        svg_of(&f.doc, &f.res, &[f.artboard]),
        svg_of_reported(&f.doc, &f.res, &[f.artboard]).svg
    );
}

/// A text node that is **not** on a rail reports nothing, which is the arm that
/// makes the `on_path` test above about the rail rather than about text.
///
/// The shared fixture's text node has no rail, so this asserts against it as it
/// stands — there is nothing to set up, and setting something up would be a way
/// to get the premise wrong.
#[test]
fn ordinary_text_is_not_reported() {
    let f = common::fixture();
    let out = svg_of_reported(&f.doc, &f.res, &[f.artboard]);
    assert!(out.svg.contains("<text"), "there is text in the file");
    assert!(
        !has(&out.approximated, "type on a path"),
        "{:?}",
        out.approximated
    );
}
