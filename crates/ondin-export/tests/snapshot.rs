//! Snapshot projection: schema completeness, serde round-trip, determinism.

mod common;

use ondin_core::keyed_by_position;
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
    assert!(
        matches!(&rect.fills[0].paint, PaintSummary::Solid { color, .. } if color == "#3c78dc")
    );
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

/// A text's line height, alignment and box, asserted as values on a default node
/// and on a centred, fixed-box one, since the fixture's defaults are what a
/// constant projection would answer.
/// flip: `paragraph` written as `ParagraphStyle::default()` → fails at the
/// non-default `paragraph.align`.
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
            paragraph,
            sizing,
            ..
        } => {
            // The fixture's is `Length::Em(1.2)`.
            assert_eq!(line_height, "120%");
            assert_eq!(paragraph.align, ondin_core::TextAlign::Start);
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
        ondin_export::snapshot::Geometry::Text {
            paragraph, sizing, ..
        } => {
            assert_eq!(
                paragraph.align,
                ondin_core::TextAlign::Center,
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
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(gradient.into()).into(),
                visible: true,
            }]),
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let snap = ondin_export::snapshot(&f.doc, &res);

    let rect = snap
        .nodes
        .iter()
        .find(|n| n.id == f.rect.to_wire())
        .expect("rect present");
    match &rect.fills[0].paint {
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
/// "these two fills are the same file" (§7.1, §15 D185). Its pixel size says
/// what aspect `fit` and `crop` keep.
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
                    width: 300,
                    height: 200,
                },
            },
            Operation::SetFills {
                id: f.rect,
                fills: keyed_by_position([Fill {
                    brush: ondin_core::Brush::Image(ondin_core::ImageBrush {
                        image: ImageRef {
                            fit: ImageFit::Tile,
                            ..ImageRef::new(id)
                        },
                        sampler: Default::default(),
                    })
                    .into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("give the rect a picture");
    f.res = Resolved::rebuild(&f.doc);

    let json = serde_json::to_string(&snapshot(&f.doc, &f.res)).expect("serialise");
    assert!(
        json.contains(
            r#""type":"Image","id":"hash-of-the-bytes","pixel_size":[300,200],"fit":"tile""#
        ),
        "the summary has to name the file, its size and how it sits: {json}"
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
                    fills: keyed_by_position([Fill {
                        brush: ondin_core::Brush::Image(ondin_core::ImageBrush {
                            image: ImageRef {
                                adjust,
                                ..ImageRef::new(id)
                            },
                            sampler: Default::default(),
                        })
                        .into(),
                        visible: true,
                    }]),
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
    assert!(
        !moved.contains("duotone"),
        "duotone's colours are absent while it draws nothing: {moved}"
    );

    let duo = picture(ImageAdjust {
        duotone: 0.5,
        ..ImageAdjust::NEUTRAL
    });
    assert!(
        duo.contains(r##""duotone":["#23215c","#f7b267"]"##),
        "a duotone names the two colours it maps onto: {duo}"
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
            effects: keyed_by_position([
                Effect::new(EffectKind::DropShadow(Shadow {
                    offset: Vec2::new(0.0, 4.0),
                    blur: 12.0,
                    spread: 1.5,
                    color: ondin_core::peniko::Color::from_rgba8(0, 0, 0, 102),
                    blend: ondin_core::BlendMode::Normal,
                })),
                // Hidden, so the filter that keeps it out is exercised alongside.
                Effect {
                    kind: EffectKind::LayerBlur { radius: 9.0 },
                    visible: false,
                    vars: Default::default(),
                },
            ]),
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
/// moved its pivot carries no key at all** — §15 D651.
///
/// 🚨 **`pivot` was the one field of the MCP surface with no test and no golden.**
/// Every other schema addition since version 4 has both halves covered —
/// `Image`/`fit`/`alpha`, `adjust`, `effects` — and `pivot` had neither: no reader
/// outside `snapshot.rs` workspace-wide, and `golden_document()` set no pivot on
/// any of its **fourteen** nodes, so `json_snapshot_matches_its_golden` was silent
/// about it. **Past tense from §15 D658**, which gave the fixture a
/// `Normalized(0.25, 0.75)` pivot on the ellipse; the golden now watches this
/// field, and this test is what says what the number means.
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

/// **A rotated layer's transforms and bounds are written at four places**
/// (§15 D972): a 30° rotation's `sin` and `cos` are where a
/// platform's maths library can differ in the last digit, and the golden is a
/// byte comparison on three of them.
///
/// **Flip-check, run**: `round4` returning `v` unchanged fails on the rect's
/// `local_transform`, whose `cos 30°` prints as `0.8660254037844387`.
#[test]
fn a_rotated_layers_numbers_are_written_at_four_places() {
    use ondin_core::{Operation, Resolved, Transaction};
    let mut f = common::fixture();
    let turned = ondin_core::kurbo::Affine::translate((20.0, 20.0))
        * ondin_core::kurbo::Affine::rotate(30f64.to_radians());
    f.doc
        .apply(&Transaction(vec![Operation::SetTransform {
            id: f.rect,
            transform: turned,
        }]))
        .expect("rotate the rect");
    f.res = Resolved::rebuild(&f.doc);
    let snap = snapshot(&f.doc, &f.res);
    let rect = snap.nodes.iter().find(|n| n.id == wire(f.rect)).unwrap();
    assert!(rect.local_transform[1] != 0.0, "the fixture is rotated");
    let places = |v: f64| {
        let s = serde_json::to_string(&v).unwrap();
        s.split_once('.').map_or(0, |(_, frac)| frac.len())
    };
    for node in &snap.nodes {
        let numbers = node
            .local_transform
            .iter()
            .chain(&node.world_transform)
            .chain(node.world_bounds.iter().flatten());
        for &v in numbers {
            assert!(places(v) <= 4, "{} writes {v}", node.id);
        }
    }
}

/// **The snapshot reports a bound field's drawn value, in the mode in force**
/// (D1037 premise (v)): the artboard set to mode B, the rect's fill, corners
/// and visibility and the text's content each bound — the rect blue, its
/// corners 12, the text *Beta*; then the rect hidden by its variable.
///
/// flips: `node_snapshot` reading the stored brush → fails *"B's blue"*;
/// reading `node.visible()` → fails *"hidden by its variable"*.
#[test]
fn a_bound_field_reports_its_drawn_value() {
    use ondin_core::{Cell, Mode, Operation, Transaction, VarKind, Variable};
    let mut f = common::fixture();
    let mut ids = ondin_core::IdSource::new(0xB1);
    let (a, b) = (ids.mint_mode(), ids.mint_mode());
    let [colour, radius, label, shown] = [(); 4].map(|_| ids.mint_var());
    let var =
        |id, name: &str, kind, cells: [(ondin_core::ModeId, Cell); 2]| Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind,
                cells: cells.into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
    let red = ondin_core::peniko::Color::from_rgba8(255, 0, 0, 255);
    let blue = ondin_core::peniko::Color::from_rgba8(0, 0, 255, 255);
    let mode = |id, name: &str, index| Operation::AddMode {
        collection: ondin_core::variable::CollectionId::FIRST,
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    f.doc
        .apply(&Transaction(vec![
            mode(a, "A", 0),
            mode(b, "B", 1),
            var(
                colour,
                "c",
                VarKind::Color,
                [(a, Cell::Color(red)), (b, Cell::Color(blue))],
            ),
            var(
                radius,
                "r",
                VarKind::Number,
                [(a, Cell::Number(4.0)), (b, Cell::Number(12.0))],
            ),
            var(
                label,
                "l",
                VarKind::String,
                [
                    (a, Cell::String("Alpha".into())),
                    (b, Cell::String("Beta".into())),
                ],
            ),
            var(
                shown,
                "s",
                VarKind::Boolean,
                [(a, Cell::Boolean(true)), (b, Cell::Boolean(false))],
            ),
            Operation::SetModes {
                id: f.artboard,
                modes: vec![b],
            },
        ]))
        .unwrap();
    let mut ops =
        ondin_core::bind_brush_at_all(&f.doc, &[f.rect], ondin_core::PaintTarget::Fill, 0, colour)
            .0;
    ops.extend(ondin_core::bind_corners_ops(
        &f.doc,
        f.rect,
        [Some(radius); 4],
    ));
    ops.extend(ondin_core::bind_text_ops(
        &f.doc,
        f.text,
        ondin_core::variable::text_vars_at(ondin_core::variable::TEXT_CONTENT, label),
    ));
    f.doc.apply(&Transaction(ops)).unwrap();
    let res = ondin_core::Resolved::rebuild(&f.doc);
    let snap = snapshot(&f.doc, &res);
    let node = |id| snap.nodes.iter().find(|n| n.id == wire(id)).unwrap();
    let rect = node(f.rect);
    assert!(
        matches!(&rect.fills[0].paint, PaintSummary::Solid { color, .. } if color == "#0000ff"),
        "B's blue: {:?}",
        rect.fills[0]
    );
    assert!(matches!(
        rect.geometry,
        Geometry::Rect { corner_radii, .. } if corner_radii.top_left == 12.0
    ));
    assert!(matches!(
        &node(f.text).geometry,
        Geometry::Text { content, .. } if content == "Beta"
    ));
    assert!(rect.visible, "the fixture: shown before its binding");

    let bind = ondin_core::bind_visible_ops(&f.doc, f.rect, shown);
    f.doc.apply(&Transaction(bind)).unwrap();
    let res = ondin_core::Resolved::rebuild(&f.doc);
    let snap = snapshot(&f.doc, &res);
    let rect = snap.nodes.iter().find(|n| n.id == wire(f.rect)).unwrap();
    assert!(!rect.visible, "hidden by its variable");
}

/// A chart frame carries its description — kind, variant, header, row count,
/// mapping and series — and every part under it its role; nothing else does.
///
/// flip: `node_snapshot` leaving `role` out → fails at "every part has its role".
#[test]
fn a_chart_reports_its_description_and_its_parts_their_roles() {
    use ondin_core::chart::{Chart, ChartKind, ColumnId, SeriesStyle, Stacking};
    use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
    use ondin_core::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};
    let mut ids = IdSource::new(0xC8);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let page = doc.first_page();
    let frame = ids.mint();
    let mut chart = Chart::sample(ChartKind::Column);
    chart.options.bars.stacking = Stacking::Stacked;
    chart.series.push(SeriesStyle {
        column: ColumnId(2),
        fill: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(0x11, 0x22, 0x33))
            .into(),
        visible: false,
        dash: Default::default(),
        point: Default::default(),
    });
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(480.0, 280.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: Some("Sales".into()),
        },
        Operation::SetChart {
            id: frame,
            chart: Some(Box::new(chart)),
        },
    ]))
    .unwrap();
    let res = Resolved::settled(&mut doc);
    let snap = snapshot(&doc, &res);
    let me = snap.nodes.iter().find(|n| n.id == wire(frame)).unwrap();
    let c = me.chart.as_ref().expect("the frame carries its chart");
    assert_eq!(me.role, None);
    assert_eq!(
        (c.kind.as_str(), c.variant.as_deref()),
        ("Column", Some("Stacked"))
    );
    let header: Vec<&str> = c.columns.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(header, ["Category", "Revenue", "Costs", "Profit"]);
    assert_eq!(
        c.columns.iter().map(|x| x.numeric).collect::<Vec<_>>(),
        [false, true, true, true]
    );
    assert_eq!(c.rows, 6);
    assert_eq!(c.mapping.category, Some(ColumnId(0)));
    assert_eq!(c.series.len(), 3);
    assert!(
        c.series[0].visible && c.series[0].fill.is_none(),
        "{:?}",
        c.series[0]
    );
    assert!(!c.series[1].visible);
    assert!(
        matches!(&c.series[1].fill, Some(PaintSummary::Solid { color, .. }) if color == "#112233"),
        "{:?}",
        c.series[1]
    );
    assert!(!c.linked);

    let parts = &doc.get(frame).unwrap().chart().unwrap().parts;
    assert!(parts.len() > 10, "the fixture settled: {}", parts.len());
    for (k, id) in parts {
        let n = snap.nodes.iter().find(|n| n.id == wire(*id)).unwrap();
        let role = serde_json::to_value(k.role).unwrap();
        assert_eq!(n.role.as_deref(), role.as_str(), "every part has its role");
        assert!(n.chart.is_none());
    }
    assert!(
        snap.nodes
            .iter()
            .any(|n| n.role.as_deref() == Some("Bar") && n.name == "Revenue · Jan"),
        "a column reads as a bar"
    );
    let back: Snapshot = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
    assert_eq!(snap, back);
}

/// A node's HTML element is its tag, and its address is written only while it
/// is a Link: a text set back to a heading keeps its address in the model, and
/// the snapshot leaves it out.
/// flip: `node_snapshot` writing `href` whatever the element → fails at "the
/// heading's address".
#[test]
fn an_element_is_its_tag_and_only_a_link_has_an_address() {
    use ondin_core::element::HtmlElement;
    use ondin_core::{Operation, Transaction};
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![
            Operation::SetElement {
                id: f.artboard,
                element: Some(HtmlElement::Nav),
            },
            Operation::SetElement {
                id: f.text,
                element: Some(HtmlElement::Link),
            },
            Operation::SetHref {
                id: f.text,
                href: "/go".into(),
            },
        ]))
        .unwrap();
    f.res = ondin_core::Resolved::rebuild(&f.doc);
    let node = |snap: &Snapshot, id| snap.nodes.iter().find(|n| n.id == wire(id)).cloned();
    let snap = snapshot(&f.doc, &f.res);
    let (board, text, rect) = (
        node(&snap, f.artboard).unwrap(),
        node(&snap, f.text).unwrap(),
        node(&snap, f.rect).unwrap(),
    );
    assert_eq!(
        (board.element.as_deref(), board.href.as_deref()),
        (Some("nav"), None)
    );
    assert_eq!(
        (text.element.as_deref(), text.href.as_deref()),
        (Some("a"), Some("/go")),
        "the link"
    );
    assert_eq!((rect.element, rect.href), (None, None), "no element");
    let json = serde_json::to_value(&snap).unwrap();
    let rect_json = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == wire(f.rect))
        .unwrap();
    assert!(
        rect_json.get("element").is_none() && rect_json.get("href").is_none(),
        "absent, not null: {rect_json}"
    );

    f.doc
        .apply(&Transaction(vec![Operation::SetElement {
            id: f.text,
            element: Some(HtmlElement::H1),
        }]))
        .unwrap();
    assert_eq!(
        f.doc.get(f.text).unwrap().href(),
        "/go",
        "the fixture keeps it"
    );
    f.res = ondin_core::Resolved::rebuild(&f.doc);
    let snap = snapshot(&f.doc, &f.res);
    let text = node(&snap, f.text).unwrap();
    assert_eq!(
        (text.element.as_deref(), text.href.as_deref()),
        (Some("h1"), None),
        "the heading's address"
    );
}

/// The layout reads report a flex frame's gap as its variable draws it, an
/// item's grow and a pinned layer's insets, each in the shape its write tool
/// takes, and stay absent on a layer with none.
/// flip: `display` read from `Node::display` → fails at the drawn gap.
#[test]
fn layout_reads_report_display_item_and_insets() {
    use ondin_core::container::{Display, Flex, LayoutItem};
    use ondin_core::variable::{Cell, CollectionId, Mode, VarKind, Variable};
    use ondin_core::{IdSource, Insets, LengthPct, Operation, Transaction};

    let mut f = common::fixture();
    let mut ids = IdSource::new(0x7777);
    let (mode, gap) = (ids.mint_mode(), ids.mint_var());
    f.doc
        .apply(&Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: Mode {
                    id: mode,
                    name: "A".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: gap,
                    name: "gap".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(12.0))].into_iter().collect(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetDisplay {
                id: f.artboard,
                display: Some(Display::Flex(Flex {
                    row_gap: 3.0,
                    ..Flex::default()
                })),
            },
            Operation::SetLayoutItem {
                id: f.rect,
                item: LayoutItem {
                    grow: 1.0,
                    ..LayoutItem::default()
                },
            },
            Operation::SetInsets {
                id: f.text,
                insets: Insets {
                    top: Some(LengthPct::Px(8.0)),
                    ..Insets::default()
                },
            },
        ]))
        .expect("lay the frame out");
    let bind = ondin_core::bind_spacing_ops(
        &f.doc,
        f.artboard,
        [Some(gap), None, None, None, None, None],
    );
    f.doc
        .apply(&Transaction(bind))
        .expect("bind the column gap");
    f.res = ondin_core::Resolved::rebuild(&f.doc);

    let snap = snapshot(&f.doc, &f.res);
    let at = |id: ondin_core::NodeId| snap.nodes.iter().find(|n| n.id == wire(id)).unwrap();
    match &at(f.artboard).display {
        Some(Display::Flex(flex)) => {
            assert_eq!(flex.column_gap, 12.0, "the bound gap as drawn");
            assert_eq!(flex.row_gap, 3.0, "the literal gap kept");
        }
        other => panic!("the frame's flex layout, got {other:?}"),
    }
    assert_eq!(at(f.rect).item.map(|i| i.grow), Some(1.0));
    assert_eq!(
        at(f.text).insets.and_then(|i| i.top),
        Some(LengthPct::Px(8.0))
    );
    let ellipse = snap.nodes.iter().find(|n| n.kind == "ellipse").unwrap();
    assert!(
        ellipse.display.is_none() && ellipse.item.is_none() && ellipse.insets.is_none(),
        "absent on a layer with no layout"
    );
    let back: Snapshot = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
    assert_eq!(back, snap, "the layout fields round-trip");
}

/// A layer reports what it is bound to — variables by `bind_variable`'s field,
/// styles by slot, the modes it sets — its part in a component, and each fill's
/// and stroke's place in its whole list past a hidden one.
/// flip: `component_summary` without its instance-root test → fails at the
/// copy's role.
#[test]
fn bindings_styles_modes_and_component_links_are_reported() {
    use ondin_core::build::BoundField;
    use ondin_core::kurbo::Affine;
    use ondin_core::style::{Style, StyleValue};
    use ondin_core::variable::{Cell, CollectionId, Mode, VarKind, Variable};
    use ondin_core::{Brush, Fill, IdSource, Operation, Transaction};

    let mut f = common::fixture();
    let mut ids = IdSource::new(0x8888);
    let (mode, half, style) = (ids.mint_mode(), ids.mint_var(), ids.mint_style());
    let (copy_frame, copy_rect) = (ids.mint(), ids.mint());
    let solid = |r| Fill {
        brush: Brush::Solid(ondin_core::peniko::Color::from_rgba8(r, 0, 0, 255)).into(),
        visible: true,
    };
    let rect = f.doc.get(f.rect).unwrap();
    let rect_kind = rect.kind().clone();
    let mut fills = rect.paint().fills.clone();
    fills.insert(
        0,
        ondin_core::Keyed {
            id: ondin_core::ItemId(ids.mint()),
            value: Fill {
                visible: false,
                ..solid(9)
            },
        },
    );
    let page = f.doc.first_page();
    f.doc
        .apply(&Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: Mode {
                    id: mode,
                    name: "A".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: half,
                    name: "half".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(50.0))].into_iter().collect(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::AddStyle {
                style: Style {
                    id: style,
                    name: "Ink".into(),
                    value: StyleValue::Fills(keyed_by_position([solid(200)])),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetFills { id: f.rect, fills },
            Operation::SetOpacityVar {
                id: f.rect,
                var: Some(half),
            },
            Operation::SetModes {
                id: f.artboard,
                modes: vec![mode],
            },
            Operation::SetComponent {
                id: f.artboard,
                component: true,
            },
            Operation::CreateNode {
                id: copy_frame,
                parent: page,
                index: 1,
                kind: f.doc.get(f.artboard).unwrap().kind().clone(),
                transform: Some(Affine::translate((600.0, 0.0))),
                name: None,
            },
            Operation::CreateNode {
                id: copy_rect,
                parent: copy_frame,
                index: 0,
                kind: rect_kind,
                transform: None,
                name: None,
            },
            Operation::SetLink {
                id: copy_frame,
                link: Some(f.artboard),
            },
            Operation::SetLink {
                id: copy_rect,
                link: Some(f.rect),
            },
        ]))
        .expect("bind, style and copy");
    f.doc
        .apply(&Transaction(vec![Operation::SetFillStyle {
            id: f.text,
            style: Some(style),
        }]))
        .expect("style a fill");
    f.res = ondin_core::Resolved::rebuild(&f.doc);

    let snap = snapshot(&f.doc, &f.res);
    let at = |id: ondin_core::NodeId| snap.nodes.iter().find(|n| n.id == wire(id)).unwrap();
    let rect = at(f.rect);
    assert!(
        rect.bindings
            .iter()
            .any(|b| b.field == BoundField::Opacity && b.variable == half),
        "the opacity's variable, got {:?}",
        rect.bindings
    );
    assert_eq!(rect.opacity, 0.5, "and its drawn value");
    assert_eq!(
        rect.fills.iter().map(|f| f.index).collect::<Vec<_>>(),
        vec![1],
        "the shown fill sits above a hidden one"
    );
    assert_eq!(rect.strokes[0].index, 0);
    assert_eq!(at(f.artboard).modes, vec![mode.0.to_wire()]);

    let role = |id| at(id).component.as_ref().map(|c| c.role.as_str());
    assert_eq!(role(f.artboard), Some("main"));
    assert_eq!(role(copy_frame), Some("instance"));
    assert_eq!(role(copy_rect), Some("copy"));
    assert_eq!(role(f.text), None, "the main's own child is no copy");
    let instance = at(copy_frame).component.as_ref().unwrap();
    assert_eq!(instance.main.as_deref(), Some(wire(f.artboard).as_str()));
    let copy = at(copy_rect).component.as_ref().unwrap();
    assert_eq!(copy.source.as_deref(), Some(wire(f.rect).as_str()));
    assert_eq!(copy.main, None, "only an instance's root names its main");

    assert_eq!(at(f.text).styles.map(|s| s.fills), Some(Some(style)));
    let back: Snapshot = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
    assert_eq!(back, snap, "the new fields round-trip");
}

/// A layer its insets stretch reports its authored transform and size beside the
/// used ones; a layer drawn as authored reports none.
/// flip: `specified` comparing the used geometry with itself → fails at the
/// authored width.
#[test]
fn a_stretched_layer_reports_its_specified_values() {
    use ondin_core::{Insets, LengthPct, Operation, Transaction};
    use ondin_export::snapshot::Specified;
    let mut f = common::fixture();
    let stored = match f.doc.get(f.rect).unwrap().kind() {
        ondin_core::NodeKind::Rect { size, .. } => size.width,
        other => panic!("the fixture's rect is a {other:?}"),
    };
    let frame = match f.doc.get(f.artboard).unwrap().kind() {
        ondin_core::NodeKind::Artboard { size, .. } => size.width,
        other => panic!("the fixture's artboard is a {other:?}"),
    };
    assert_ne!(stored, frame - 20.0, "the stretch must change the width");
    f.doc
        .apply(&Transaction(vec![Operation::SetInsets {
            id: f.rect,
            insets: Insets {
                left: Some(LengthPct::Px(10.0)),
                right: Some(LengthPct::Px(10.0)),
                ..Default::default()
            },
        }]))
        .expect("pin both edges");
    f.res = ondin_core::Resolved::rebuild(&f.doc);

    let snap = snapshot(&f.doc, &f.res);
    let at = |id: ondin_core::NodeId| snap.nodes.iter().find(|n| n.id == id.to_wire()).unwrap();
    let rect = at(f.rect);
    assert!(
        matches!(rect.geometry, Geometry::Rect { width, .. } if width == frame - 20.0),
        "used: {:?}",
        rect.geometry
    );
    let Some(Specified {
        geometry: Some(Geometry::Rect { width, .. }),
        ..
    }) = &rect.specified
    else {
        panic!("the authored width: {:?}", rect.specified);
    };
    assert_eq!(*width, stored);
    assert_eq!(at(f.text).specified, None, "drawn as authored");
}

/// A text's runs and per-paragraph overrides read back in characters, as
/// `style_text` and `style_paragraphs` take them: a coloured run past a
/// two-byte character, a list item's marker and level, the block style, the
/// glyphs' own colour, and a run's missing family as a warning.
/// flip: `text_runs` counting bytes instead of characters → fails at the run's
/// range.
#[test]
fn a_texts_runs_paragraphs_and_block_read_back() {
    use ondin_core::{
        BlockStyle, CharAttr, CharSpans, ListMarker, Operation, ParaAttr, ParaSpans,
        ParagraphStyle, TextStyle, Transaction, peniko::Color,
    };
    use ondin_export::snapshot::{TextRun, Warning};
    let mut f = common::fixture();
    // `é` is two bytes, so "Two" is bytes 6..9 and characters 5..8.
    let content = "Caf\u{e9}\nTwo";
    let style = TextStyle {
        color: Some(Color::from_rgba8(0x11, 0x22, 0x33, 0xff)),
        ..TextStyle::default()
    };
    let red = Color::from_rgb8(0xff, 0, 0);
    let mut spans = CharSpans::default();
    spans.set(6..9, CharAttr::Color(Some(red)), &style);
    spans.set(6..9, CharAttr::Family("Nowhere Sans".into()), &style);
    let paragraph = ParagraphStyle::default();
    let mut para_spans = ParaSpans::default();
    para_spans.set(
        6..9,
        ParaAttr::Marker(Some(ListMarker::Decimal)),
        &paragraph,
    );
    para_spans.set(6..9, ParaAttr::Level(1), &paragraph);
    let block = BlockStyle {
        max_lines: 3,
        ..BlockStyle::default()
    };
    f.doc
        .apply(&Transaction(vec![
            Operation::SetText {
                id: f.text,
                content: content.into(),
                spans: CharSpans::default(),
                para_spans,
            },
            Operation::SetTextStyle {
                id: f.text,
                style: style.clone(),
                spans: Some(spans),
            },
            Operation::SetBlockStyle { id: f.text, block },
        ]))
        .expect("style the text");
    f.res = ondin_core::Resolved::rebuild(&f.doc);

    let snap = snapshot(&f.doc, &f.res);
    let id = f.text.to_wire();
    let text = snap.nodes.iter().find(|n| n.id == id).unwrap();
    let Geometry::Text {
        color,
        block: read_block,
        runs,
        paragraphs,
        ..
    } = &text.geometry
    else {
        panic!("expected text geometry, got {:?}", text.geometry);
    };
    assert_eq!(color.as_deref(), Some("#112233ff"));
    assert_eq!(read_block.max_lines, 3);
    assert_eq!(runs.len(), 1, "one range, both attributes: {runs:?}");
    assert_eq!((runs[0].start, runs[0].end), (5, 8));
    assert!(runs[0].attrs.contains(&CharAttr::Color(Some(red))));
    assert_eq!(
        paragraphs,
        &vec![TextRun {
            start: 5,
            end: 8,
            attrs: vec![
                ParaAttr::Marker(Some(ListMarker::Decimal)),
                ParaAttr::Level(1)
            ],
        }]
    );
    assert!(
        snap.warnings.contains(&Warning::MissingFont {
            node: id.clone(),
            family: "Nowhere Sans".into(),
        }),
        "{:?}",
        snap.warnings
    );
}
