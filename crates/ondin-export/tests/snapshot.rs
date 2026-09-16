//! Snapshot projection: schema completeness, serde round-trip, determinism.

mod common;

use ondin_export::snapshot::{Geometry, PaintSummary, Snapshot, snapshot};

#[test]
fn snapshot_covers_tree_and_kinds() {
    let f = common::fixture();
    let snap = snapshot(&f.doc, &f.res);

    assert_eq!(
        snap.snapshot_version,
        ondin_export::snapshot::SNAPSHOT_VERSION
    );
    // root + artboard + rect + ellipse + line + group + nested + path + text = 9
    assert_eq!(snap.nodes.len(), 9);

    let kinds: Vec<&str> = snap.nodes.iter().map(|n| n.kind.as_str()).collect();
    for expected in [
        "root", "artboard", "group", "rect", "ellipse", "line", "path", "text",
    ] {
        assert!(kinds.contains(&expected), "missing kind {expected}");
    }
}

#[test]
fn rect_snapshot_has_world_data_and_paint() {
    let f = common::fixture();
    let snap = snapshot(&f.doc, &f.res);
    let rect = snap
        .nodes
        .iter()
        .find(|n| n.id == wire(f.rect))
        .expect("rect present");

    // Geometry projected.
    assert!(matches!(
        rect.geometry,
        Geometry::Rect { corner_radii, .. }
            if corner_radii == ondin_core::kurbo::RoundedRectRadii::from_single_radius(6.0)
    ));
    // Fill summarized as a solid hex color.
    assert_eq!(rect.fills.len(), 1);
    assert!(matches!(&rect.fills[0], PaintSummary::Solid { color, .. } if color == "#3c78dc"));
    assert_eq!(rect.strokes.len(), 1);
    // World transform = artboard(10,10)+rect(20,20) translation => e=30, f=30.
    assert_eq!(rect.world_transform[4], 30.0);
    assert_eq!(rect.world_transform[5], 30.0);
    // Membership: the rect belongs to the artboard.
    assert_eq!(rect.artboard, Some(wire(f.artboard)));
    // World bounds present.
    assert!(rect.world_bounds.is_some());
}

#[test]
fn snapshot_round_trips_through_json() {
    let f = common::fixture();
    let snap = snapshot(&f.doc, &f.res);
    let json = serde_json::to_string_pretty(&snap).unwrap();
    let back: Snapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(snap, back);
}

#[test]
fn snapshot_is_deterministic() {
    let f = common::fixture();
    let a = serde_json::to_string(&snapshot(&f.doc, &f.res)).unwrap();
    let b = serde_json::to_string(&snapshot(&f.doc, &f.res)).unwrap();
    assert_eq!(a, b);
}

fn wire(id: ondin_core::NodeId) -> String {
    id.to_wire()
}

/// The three text-style fields an agent needs, asserted as **values**.
///
/// An agent asked to "centre that heading" needs to know what the alignment and
/// box already are; the earlier snapshot omitted all three.
///
/// ⚠️ **This test could not fail until §15 D480** (`[S8.3-L6-03]`). Two of its
/// three assertions checked the projection's output against the projection's own
/// **codomain** — `line_height` is one of `"auto"` / `…%` / `…px` because the
/// `match` producing it has exactly those three arms, and `align` is one of six
/// strings because its `match` is exhaustive over `TextAlign` — and the third,
/// `sizing`, was discarded with `let _`. Replacing the whole `align` arm with
/// `"start".to_string()` and the whole `sizing` arm with `TextSizingSummary::Auto`
/// left `cargo test --workspace` green. **An MCP client reading a constant
/// `"start"` for every text node is the exact failure this test was written
/// against.** It is *"asserted a consequence the wrong implementation also
/// produces"* in its purest form, the consequence being that the value
/// type-checks.
///
/// ⚠️ **The default-valued fixture is the other half, and asserting its values
/// alone would not have closed it.** `common::fixture`'s text is
/// `TextAlign::Start` and `TextSizing::Auto` — the two values a constant
/// projection would return anyway — so a second, **non-default** node is what
/// separates a projection from a constant. It is applied here rather than in the
/// shared fixture on purpose: that fixture feeds the byte-compared JSON golden
/// (§15 D304), and churning the golden to close a test-quality gap spends a lot
/// to prove a little.
///
/// ⚠️ **Flip-check, run: `align`'s `match` replaced by `"start".to_string()` and
/// `sizing`'s by `TextSizingSummary::Auto`**, which is the finding's own diff.
/// Fails on the non-default `align` assertion — the predicted site — and on the
/// `sizing` assertion behind it. The default-valued half stays green under that
/// flip, which is exactly why it cannot be the whole test.
#[test]
fn text_snapshot_carries_the_full_style() {
    use ondin_core::{GeometryPatch, Operation, Resolved, TextSizing, Transaction};

    let mut f = common::fixture();
    let snap = ondin_export::snapshot(&f.doc, &f.res);
    let text = snap
        .nodes
        .iter()
        .find(|n| n.kind == "text")
        .expect("fixture has text");
    let id = text.id.clone();
    match &text.geometry {
        ondin_export::snapshot::Geometry::Text {
            line_height,
            align,
            sizing,
            ..
        } => {
            // A string, because the model's three line-height modes are
            // genuinely different things and a bare number could only report one
            // of them. The fixture's is `Length::Em(1.2)`.
            assert_eq!(line_height, "120%");
            assert_eq!(align, "start");
            assert_eq!(sizing, &ondin_export::snapshot::TextSizingSummary::Auto);
        }
        other => panic!("expected text geometry, got {other:?}"),
    }

    // The same node given a non-default alignment and a fixed box, so a constant
    // in place of either projection is visible.
    let node = ondin_core::NodeId::from_wire(&id)
        .expect("the snapshot's text id is the wire form of a real one");
    let paragraph = ondin_core::ParagraphStyle {
        align: ondin_core::TextAlign::Center,
        ..Default::default()
    };
    f.doc
        .apply(&Transaction(vec![
            Operation::SetParagraphStyle {
                id: node,
                paragraph,
                spans: None,
            },
            Operation::SetGeometry {
                id: node,
                geometry: GeometryPatch::TextSizing(TextSizing::Fixed(
                    ondin_core::kurbo::Size::new(300.0, 80.0),
                )),
            },
        ]))
        .expect("centre it and give it a box");
    f.res = Resolved::rebuild(&f.doc);

    let snap = ondin_export::snapshot(&f.doc, &f.res);
    let text = snap
        .nodes
        .iter()
        .find(|n| n.kind == "text")
        .expect("still there");
    match &text.geometry {
        ondin_export::snapshot::Geometry::Text { align, sizing, .. } => {
            assert_eq!(
                align, "center",
                "a centred paragraph must report centred — a constant `start` \
                 tells an agent the heading is already left-aligned"
            );
            assert_eq!(
                sizing,
                &ondin_export::snapshot::TextSizingSummary::Fixed {
                    width: 300.0,
                    height: 80.0
                },
                "and a fixed box must report its size rather than `Auto`"
            );
        }
        other => panic!("expected text geometry, got {other:?}"),
    }
}

#[test]
fn gradients_are_described_not_just_flagged() {
    use ondin_core::Brush;
    use ondin_core::kurbo::Point;
    use ondin_core::peniko::{Color, Gradient};
    use ondin_core::{Fill, Operation, Resolved, Transaction};
    use ondin_export::snapshot::PaintSummary;

    let mut f = common::fixture();
    let gradient = Gradient::new_linear(Point::new(0.0, 0.0), Point::new(10.0, 0.0)).with_stops([
        (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
        (1.0_f32, Color::from_rgba8(0, 255, 0, 255)),
    ]);
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: f.rect,
            fills: vec![Fill {
                brush: Brush::Gradient(gradient.into()),
                visible: true,
            }],
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let snap = ondin_export::snapshot(&f.doc, &res);

    let rect = snap
        .nodes
        .iter()
        .find(|n| n.id == f.rect.to_wire())
        .expect("rect present");
    match &rect.fills[0] {
        PaintSummary::Gradient {
            kind,
            opacity,
            stops,
        } => {
            assert_eq!(kind, "linear");
            assert_eq!(stops.len(), 2);
            assert_eq!(stops[0].color, "#ff0000");
            assert_eq!(stops[1].color, "#00ff00");
            // The ramp's own multiplier, new in snapshot version 10 (§15 D767).
            // Destructured by name rather than with `..`, so the next field added
            // to this summary is a compile error here rather than a value that
            // silently never reaches an agent.
            assert_eq!(
                *opacity, 1.0,
                "an untouched gradient's ramp is fully opaque"
            );
        }
        other => panic!("expected a described gradient, got {other:?}"),
    }
}

#[test]
fn missing_fonts_are_reported_as_warnings() {
    use ondin_core::{NodeKind, Operation, Resolved, TextSizing, Transaction};
    use ondin_export::snapshot::Warning;

    let mut f = common::fixture();
    let NodeKind::Text { style, .. } = f.doc.get(f.text).unwrap().kind().clone() else {
        panic!("fixture text node");
    };
    let mut missing = style;
    missing.font_family = "Definitely Not Installed 9000".into();
    f.doc
        .apply(&Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            style: *missing,
            spans: None,
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let snap = ondin_export::snapshot(&f.doc, &res);

    assert!(
        snap.warnings.iter().any(|w| matches!(
            w,
            Warning::MissingFont { node, family }
                if *node == f.text.to_wire() && family.contains("9000")
        )),
        "expected a missing-font warning, got {:?}",
        snap.warnings
    );
    let _ = TextSizing::Auto;
}

#[test]
fn a_document_with_available_fonts_warns_about_nothing() {
    let f = common::fixture();
    let snap = ondin_export::snapshot(&f.doc, &f.res);
    assert!(
        snap.warnings.is_empty(),
        "bundled Inter should resolve: {:?}",
        snap.warnings
    );
}

/// **The snapshot says *which* picture, not just that there is one.** A bare
/// `Image` variant cannot answer the first question anyone asks about a
/// photograph in a document, and the id is its content hash, so it doubles as
/// "these two fills are the same file" (§7.1, §15 D185).
#[test]
fn an_image_fill_is_summarised_by_its_id_and_its_framing_mode() {
    use ondin_core::{
        Fill, ImageEntry, ImageFit, ImageFormat, ImageId, ImageRef, ImageSource, Operation,
        Resolved, Transaction,
    };
    let mut f = common::fixture();
    let id = ImageId("hash-of-the-bytes".into());
    f.doc
        .apply(&Transaction(vec![
            Operation::AddImage {
                id: id.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(vec![1, 2, 3].into()),
                    format: ImageFormat::Png,
                    width: 200,
                    height: 200,
                },
            },
            Operation::SetFills {
                id: f.rect,
                fills: vec![Fill {
                    brush: ondin_core::Brush::Image(ondin_core::ImageBrush {
                        image: ImageRef {
                            fit: ImageFit::Tile,
                            ..ImageRef::new(id)
                        },
                        sampler: Default::default(),
                    }),
                    visible: true,
                }],
            },
        ]))
        .expect("give the rect a picture");
    f.res = Resolved::rebuild(&f.doc);

    let json = serde_json::to_string(&snapshot(&f.doc, &f.res)).expect("serialise");
    assert!(
        json.contains(r#""type":"Image","id":"hash-of-the-bytes","fit":"tile""#),
        "the summary has to name the file and how it sits: {json}"
    );
}

/// **An adjusted picture says which sliders moved, and an untouched one says
/// nothing at all.**
///
/// Both halves are the claim. An agent asked "why is this photograph so dark"
/// needs the exposure to be in the summary at all; and seven zeroes on every
/// picture in a document would be seven fields to read past on the overwhelming
/// majority, which carry none — so the untouched case has to stay byte-identical
/// to what version 4 wrote.
#[test]
fn an_adjusted_picture_summarises_only_the_sliders_that_moved() {
    use ondin_core::{
        Fill, ImageAdjust, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource, Operation,
        Resolved, Transaction,
    };
    let picture = |adjust: ImageAdjust| {
        let mut f = common::fixture();
        let id = ImageId("hash-of-the-bytes".into());
        f.doc
            .apply(&Transaction(vec![
                Operation::AddImage {
                    id: id.clone(),
                    entry: ImageEntry {
                        source: ImageSource::Embedded(vec![1, 2, 3].into()),
                        format: ImageFormat::Png,
                        width: 200,
                        height: 200,
                    },
                },
                Operation::SetFills {
                    id: f.rect,
                    fills: vec![Fill {
                        brush: ondin_core::Brush::Image(ondin_core::ImageBrush {
                            image: ImageRef {
                                adjust,
                                ..ImageRef::new(id)
                            },
                            sampler: Default::default(),
                        }),
                        visible: true,
                    }],
                },
            ]))
            .expect("give the rect a picture");
        f.res = Resolved::rebuild(&f.doc);
        serde_json::to_string(&snapshot(&f.doc, &f.res)).expect("serialise")
    };

    let untouched = picture(ImageAdjust::NEUTRAL);
    assert!(
        !untouched.contains("adjust"),
        "an untouched picture must carry no adjustment key: {untouched}"
    );

    let moved = picture(ImageAdjust {
        exposure: -0.5,
        saturation: 0.25,
        ..ImageAdjust::NEUTRAL
    });
    assert!(
        moved.contains(r#""adjust":{"exposure":-0.5,"saturation":0.25}"#),
        "the two moved sliders and only those, in the model's own units: {moved}"
    );
}

/// **The projection must not describe a shadowed layer as an unshadowed one.**
///
/// A reader of this file cannot tell "no effects" from "effects this writer does
/// not know about", so the absence of the key has to mean the first — which makes
/// its *presence* the thing worth pinning, and the numbers with it.
///
/// The authored radius, not the gaussian deviation: a caller setting `blur` back
/// would otherwise halve it every round trip. And the opacity split out of the
/// colour, exactly as a solid fill's is, so a reader meets one convention.
#[test]
fn the_snapshot_carries_a_layers_effects_in_the_units_the_panel_shows() {
    use ondin_core::kurbo::Vec2;
    use ondin_core::{Effect, EffectKind, Operation, Resolved, Shadow, Transaction};

    let mut f = common::fixture();
    let plain = serde_json::to_string(&snapshot(&f.doc, &f.res)).unwrap();
    assert!(
        !plain.contains("effects"),
        "a document with no effect carries no key: {plain}"
    );

    f.doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: f.rect,
            effects: vec![
                Effect::new(EffectKind::DropShadow(Shadow {
                    offset: Vec2::new(0.0, 4.0),
                    blur: 12.0,
                    spread: 1.5,
                    color: ondin_core::peniko::Color::from_rgba8(0, 0, 0, 102),
                })),
                // Hidden, so the filter that keeps it out is exercised alongside.
                Effect {
                    kind: EffectKind::LayerBlur { radius: 9.0 },
                    visible: false,
                },
            ],
        }]))
        .unwrap();
    f.res = Resolved::rebuild(&f.doc);
    let out = serde_json::to_string(&snapshot(&f.doc, &f.res)).unwrap();

    assert!(
        out.contains(r#""kind":"DropShadow""#),
        "named, not numbered"
    );
    assert!(
        out.contains(r#""blur":12.0"#),
        "the authored radius, not the deviation of 6: {out}"
    );
    assert!(out.contains(r#""spread":1.5"#));
    assert!(
        out.contains(r#""opacity":0.4"#),
        "the colour's alpha, split out like a fill's: {out}"
    );
    assert!(
        !out.contains("LayerBlur"),
        "and the hidden entry is filtered out, as a hidden fill is: {out}"
    );
}

/// **A pivot reaches the snapshot as a resolved point, and a node that has not
/// moved its pivot carries no key at all** — §15 D651, `[S8.2-L6-06]`.
///
/// 🚨 **`pivot` was the one field of the MCP surface with no test and no golden.**
/// Every other schema addition since version 4 has both halves covered —
/// `Image`/`fit`/`alpha`, `adjust`, `effects` — and `pivot` had neither: no reader
/// outside `snapshot.rs` workspace-wide, and `golden_document()` set no pivot on
/// any of its **fourteen** nodes, so `json_snapshot_matches_its_golden` was silent
/// about it. (⚠️ **The finding says "eleven" and this comment did too until
/// `arch-scribe` counted**: `golden_document` issues thirteen `CreateNode`s and
/// `every-kind.json` holds fourteen node snapshots, the document root included. No
/// spelling of it is eleven, and the argument does not depend on the figure —
/// which is exactly why it went unchecked.) **Past tense from §15 D658**, which
/// gave the fixture a `Normalized(0.25, 0.75)` pivot on the ellipse; the golden
/// now watches this field, and this test is what says what the number means.
///
/// **It is also the only field in `node_snapshot` that *derives* rather than
/// projects.** The others read the node; this one measures
/// `ondin_core::local_box` and resolves the model's fraction-or-point enum
/// against it through `geometry::pivot_point`, which is what the field's own doc
/// promises (*"reported as the resolved point rather than as the model's
/// fraction-or-point enum"*). So the two `Pivot` variants have to be asserted
/// separately: a `Local` point passes through and a `Normalized` fraction does
/// not, and only the second can be wrong.
///
/// ⚠️ **The fourth case pins a rough edge rather than a bug.** `local_box` is
/// `None` for a node it cannot measure — an empty `Group` falls through to a child
/// union that is `None` — and the writer's `unwrap_or_default()` makes that
/// `Rect::ZERO`, so a *fractional* pivot on an empty group resolves against a zero
/// box and is reported as `[0, 0]` whatever the fraction says. It is recorded here
/// because it is the one input that makes the derivation observable, not because
/// it is reachable: nothing in the UI sets a pivot on an empty group.
///
/// ⚠️ **Flip run, twice, and the predicted site was right both times** — the
/// `Normalized` assertion. `pivot: node.pivot().map(|_| [0.0, 0.0])` fails there
/// at `[0, 0]` against `[80, 40]`, and leaves the *absent* assertion green, which
/// is why that one is asserted at all. Replacing `geometry::pivot_point`'s
/// `Normalized` arm with `local.center()` — the plausible wrong version, since
/// `None` already means the centre — fails the same assertion at `[40, 20]` and
/// leaves `Local` green.
#[test]
fn a_pivot_is_reported_as_a_point_and_only_when_it_has_been_moved() {
    use ondin_core::kurbo::{Point, Vec2};
    use ondin_core::{IdSource, NodeKind, Operation, Pivot, Resolved, Transaction};

    let pivot_of = |snap: &Snapshot, id: ondin_core::NodeId| -> Option<[f64; 2]> {
        snap.nodes
            .iter()
            .find(|n| n.id == wire(id))
            .unwrap_or_else(|| panic!("{} is in the snapshot", wire(id)))
            .pivot
    };

    // The snapshot of the fixture's 80x40 rect with `pivot` set to `p`.
    let rect_pivot = |p: Option<Pivot>| -> Option<[f64; 2]> {
        let mut f = common::fixture();
        f.doc
            .apply(&Transaction(vec![Operation::SetPivot {
                id: f.rect,
                pivot: p,
            }]))
            .expect("a pivot on a rect");
        f.res = Resolved::rebuild(&f.doc);
        pivot_of(&snapshot(&f.doc, &f.res), f.rect)
    };

    // The fixture: a rect that has never had its pivot moved says nothing, and
    // the key is absent from the JSON rather than present and null.
    let f = common::fixture();
    let untouched = snapshot(&f.doc, &f.res);
    assert_eq!(
        pivot_of(&untouched, f.rect),
        None,
        "a node on its own centre carries no pivot"
    );
    let json = serde_json::to_string(&untouched).expect("serialise");
    assert!(
        !json.contains("\"pivot\""),
        "and the key is absent rather than null — every document in the world \
         would carry it otherwise: {json}"
    );

    // A fraction is resolved against the node's own box. `(1, 1)` is its
    // bottom-right, and the rect is 80 x 40.
    assert_eq!(
        rect_pivot(Some(Pivot::Normalized(Vec2::new(1.0, 1.0)))),
        Some([80.0, 40.0]),
        "a normalized pivot is reported as the point it resolves to"
    );
    // A point is already in local space and passes through.
    assert_eq!(
        rect_pivot(Some(Pivot::Local(Point::new(7.0, 9.0)))),
        Some([7.0, 9.0]),
        "a local pivot is reported as itself"
    );

    // The rough edge: an empty group has no measurable box, so a fraction
    // resolves against `Rect::ZERO`.
    let mut f = common::fixture();
    let mut ids = IdSource::new(0x9147);
    let empty = ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: empty,
                parent: f.artboard,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::SetPivot {
                id: empty,
                pivot: Some(Pivot::Normalized(Vec2::new(1.0, 1.0))),
            },
        ]))
        .expect("an empty group with a fractional pivot");
    f.res = Resolved::rebuild(&f.doc);
    assert_eq!(
        pivot_of(&snapshot(&f.doc, &f.res), empty),
        Some([0.0, 0.0]),
        "a fraction of a box that cannot be measured is a fraction of zero — \
         recorded, not endorsed"
    );
}
