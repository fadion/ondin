//! SVG export: completeness (every node kind appears) and well-formedness.

mod common;

use ondin_core::kurbo::Vec2;
use ondin_core::{Effect, EffectKind, Filters, Operation, Resolved, Shadow, Transaction};
use ondin_export::svg::{svg, svg_of};

/// **A set of layers exports as those layers and their neighbours stay out**, and
/// each one lands where it sits in the world rather than where it sits in its
/// parent (`context-menus.md` §3's *Copy as SVG*).
///
/// Two claims and they fail in opposite directions:
///
/// - **Scope.** The fixture's rect and text are two of six children of one
///   artboard, so an export that walked the parent would bring the ellipse, the
///   line and the group along — and would also emit the artboard's own background
///   `<rect>`, which is why the rect count is asserted rather than its presence.
/// - **Placement.** The rect sits at `translate(20, 20)` inside a board at
///   `translate(10, 10)`, so its world position is `(30, 30)`. Emitting it under
///   the identity — the obvious spelling, since the subtree is being lifted out of
///   its parent — puts it at `(20, 20)` while the viewBox is still written from
///   world bounds, so the copy arrives in the other application with everything
///   shifted by the frame's own offset and the relationship between the two
///   layers silently wrong.
///
/// ⚠️ Flipped against exactly that identity spelling, where the scope assertions
/// all pass and only the transform fails.
#[test]
fn a_set_of_layers_exports_without_its_neighbours_and_keeps_world_positions() {
    let f = common::fixture();
    let out = svg_of(&f.doc, &f.res, &[f.rect, f.text]);

    assert!(out.starts_with("<svg"));
    assert!(out.ends_with("</svg>\n"));
    assert_eq!(
        out.matches("<rect").count(),
        1,
        "one rect and no artboard background: {out}"
    );
    assert_eq!(out.matches("<text").count(), 1);
    for absent in ["<ellipse", "<line", "clipPath"] {
        assert!(
            !out.contains(absent),
            "{absent} is a neighbour, not part of the selection: {out}"
        );
    }
    assert!(
        out.contains("matrix(1 0 0 1 30 30)"),
        "the rect keeps its world position — 20 inside a board at 10: {out}"
    );
}

#[test]
fn emits_every_node_kind() {
    let f = common::fixture();
    let out = svg(&f.doc, &f.res, None);

    // Each shape kind produced its semantic element.
    assert!(out.contains("<svg"));
    assert!(out.contains("<rect"), "rect/artboard background");
    assert!(out.contains("<ellipse"));
    assert!(out.contains("<line"));
    assert!(out.contains("<path"));
    assert!(out.contains("<text"));
    assert!(out.ends_with("</svg>\n"));
}

#[test]
fn fill_and_stroke_are_emitted() {
    let f = common::fixture();
    let out = svg(&f.doc, &f.res, None);
    // The rect's blue fill and black stroke.
    assert!(out.contains(r##"fill="#3c78dc""##), "solid fill as hex");
    assert!(out.contains(r##"stroke="#000000""##));
    assert!(out.contains(r#"stroke-width="2""#));
    assert!(out.contains(r#"stroke-linejoin="round""#));
}

#[test]
fn text_content_is_xml_escaped() {
    let f = common::fixture();
    let out = svg(&f.doc, &f.res, None);
    assert!(out.contains("Hi &lt;there&gt;"), "angle brackets escaped");
    assert!(!out.contains("Hi <there>"), "raw brackets must not leak");
}

/// **A scoped export lands where its own viewBox says it does** (§15 D258).
///
/// This test asserted that a `<rect>` and an `<ellipse` were *present* and never
/// read a number, and it was green for as long as the path had been wrong: the
/// origin was positioned by its **own** world transform where `emit_node`
/// composes `outer * node.transform()`, so the local was applied twice. The
/// fixture's board sits at `translate(10, 10)`, the viewBox was written from
/// `world_bounds` as `10 10 400 300`, and the group came out at `translate(20,
/// 20)` — a tenth of the artboard off the right and bottom, a blank strip at the
/// top left, and every child of it dragged along.
///
/// So the assertion is the **pair**, not either half: the viewBox comes from one
/// query and the transform from another, and what makes the picture right is that
/// the two agree. Checking the transform alone would pass against a viewBox
/// written from the wrong corner.
///
/// ⚠️ Flipped by putting `res.world_transform(origin)` back, which is the spelling
/// that was there.
#[test]
fn single_artboard_export_lands_inside_its_own_view_box() {
    let f = common::fixture();
    let board = svg(&f.doc, &f.res, Some(f.artboard));
    assert!(board.starts_with("<svg"));
    assert!(board.ends_with("</svg>\n"));
    // The artboard's contents are present — the fixture check the numeric
    // assertions below would otherwise be made about nothing.
    assert!(board.contains("<rect"));
    assert!(board.contains("<ellipse"));

    assert!(
        board.contains(r#"viewBox="10 10 400 300""#),
        "the viewBox is the board's world bounds: {}",
        board.lines().next().unwrap_or_default()
    );
    let group = board
        .lines()
        .find(|l| l.trim_start().starts_with("<g "))
        .expect("the board is wrapped in a group carrying its transform");
    assert!(
        group.contains("matrix(1 0 0 1 10 10)"),
        "the board's own translate, once — not twice: {group}"
    );
}

#[test]
fn hidden_nodes_are_omitted() {
    use ondin_core::{Operation, Resolved, Transaction};
    let mut f = common::fixture();
    // Hide the text node and re-resolve.
    f.doc
        .apply(&Transaction(vec![Operation::SetVisible {
            id: f.text,
            visible: false,
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = svg(&f.doc, &res, None);
    assert!(!out.contains("<text"), "hidden text must not be emitted");
}

#[test]
fn structure_is_nested_not_flattened() {
    // Groups and artboards become real `<g>` elements so the output stays
    // editable, and so group opacity composites the subtree as a unit.
    let f = common::fixture();
    let out = svg(&f.doc, &f.res, None);
    assert!(out.contains("<g"), "containers must produce groups");
    assert!(out.contains("</g>"));
    // Shapes sit inside the artboard group rather than carrying its transform.
    let g_at = out.find("<g").expect("a group");
    let shape_at = out.find("<ellipse").expect("the ellipse");
    assert!(g_at < shape_at, "shapes should be nested inside the group");
}

#[test]
fn artboards_emit_a_clip_path() {
    let f = common::fixture();
    let out = svg(&f.doc, &f.res, None);
    assert!(out.contains("<clipPath"), "frames clip their contents");
    assert!(out.contains("clip-path=\"url(#"), "and reference it");
}

/// A frame inside a frame (§5.3, §15 D62) comes out as a nested group with a clip
/// of its own, and its contents are written in *its* coordinates rather than the
/// outer frame's. Nothing in the writer is about depth, so this is a check that
/// nothing in it assumed frames were only ever root-level either.
#[test]
fn a_nested_frame_clips_inside_its_parent() {
    use ondin_core::Brush;
    use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{Fill, NodeKind, Operation, Resolved, Transaction};

    let mut f = common::fixture();
    let mut ids = ondin_core::IdSource::new(0xBEEF);
    let card = ids.mint();
    let inner = ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: card,
                parent: f.artboard,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(120.0, 90.0),
                },
                transform: Some(Affine::translate((40.0, 50.0))),
                name: Some("Card".into()),
            },
            Operation::SetFills {
                id: card,
                fills: vec![Fill {
                    brush: Brush::Solid(Color::from_rgba8(9, 9, 9, 255)),
                    visible: true,
                }],
            },
            Operation::CreateNode {
                id: inner,
                parent: card,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::translate((5.0, 5.0))),
                name: None,
            },
        ]))
        .expect("a frame nests inside a frame");
    f.res = Resolved::rebuild(&f.doc);

    let out = svg(&f.doc, &f.res, None);
    // Two frames, so two clips, and the inner one is referenced inside the outer
    // group rather than beside it.
    assert_eq!(
        out.matches("<clipPath").count(),
        2,
        "each frame needs its own clip:\n{out}"
    );
    let outer_clip = out.find("clip-path=\"url(#").expect("the outer clip");
    let inner_clip = out[outer_clip + 1..]
        .find("clip-path=\"url(#")
        .expect("the inner clip");
    assert!(
        inner_clip > 0,
        "the nested clip should come after the outer"
    );
    // The rect keeps the card's local coordinates — 5,5 — rather than being
    // rewritten into the outer frame's space (which would put it at 45,55).
    assert!(
        out.contains("matrix(1 0 0 1 5 5)"),
        "the nested frame's child should stay in its own coordinates:\n{out}"
    );
}

#[test]
fn gradients_become_real_defs_not_a_flat_grey() {
    use ondin_core::Brush;
    use ondin_core::kurbo::Point;
    use ondin_core::peniko::{Color, Gradient};
    use ondin_core::{Fill, Operation, Resolved, Transaction};

    let mut f = common::fixture();
    let gradient = Gradient::new_linear(Point::new(0.0, 0.0), Point::new(60.0, 40.0)).with_stops([
        (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
        (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
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
    let out = svg(&f.doc, &res, None);

    assert!(out.contains("<defs>"), "gradient defs block");
    assert!(out.contains("<linearGradient"), "a real gradient element");
    assert!(out.contains("stop-color=\"#ff0000\""), "the first stop");
    assert!(out.contains("stop-color=\"#0000ff\""), "the last stop");
    assert!(
        out.contains("fill=\"url(#grad-"),
        "the shape must reference the def, not a flattened colour"
    );
    assert!(
        !out.contains("fill=\"#808080\""),
        "the old mid-grey approximation must be gone"
    );
}

#[test]
fn group_opacity_is_carried_by_the_group_not_each_child() {
    use ondin_core::{NodeKind, Operation, Resolved, Transaction};

    let mut f = common::fixture();
    // Find the group in the fixture and make it semi-transparent.
    let group = {
        let mut found = None;
        let mut stack = vec![f.doc.root()];
        while let Some(id) = stack.pop() {
            let node = f.doc.get(id).unwrap();
            if matches!(node.kind(), NodeKind::Group) {
                found = Some(id);
                break;
            }
            stack.extend(node.children().iter().copied());
        }
        found.expect("fixture has a group")
    };
    f.doc
        .apply(&Transaction(vec![Operation::SetOpacity {
            id: group,
            opacity: 0.4,
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = svg(&f.doc, &res, None);

    assert!(
        out.contains("<g") && out.contains("opacity=\"0.4\""),
        "the group element should carry the opacity"
    );
    let g_line = out
        .lines()
        .find(|l| l.contains("opacity=\"0.4\""))
        .expect("a line with the opacity");
    assert!(
        g_line.trim_start().starts_with("<g"),
        "opacity belongs on the group, not a shape: {g_line}"
    );
}

/// SVG has no stroke alignment, so an aligned stroke has to be *built*: the
/// stroke is drawn at double width and clipped to the side it belongs on. Both
/// halves have to be present or the export silently loses the effect.
#[test]
fn an_aligned_stroke_exports_as_a_clipped_double_width_stroke() {
    for (align, expect_evenodd) in [
        (ondin_core::StrokeAlign::Inside, false),
        (ondin_core::StrokeAlign::Outside, true),
    ] {
        let f = common::fixture_with_stroke_align(align);
        let out = svg(&f.doc, &f.res, None);
        assert!(
            out.contains("<clipPath id=\"sa-"),
            "{align:?}: no clip path"
        );
        assert!(
            out.contains("clip-path=\"url(#sa-"),
            "{align:?}: clip not referenced"
        );
        // The fixture's stroke is 3 wide, so the drawn stroke must be 6.
        assert!(
            out.contains(r#"stroke-width="6""#),
            "{align:?}: stroke was not doubled"
        );
        assert_eq!(
            out.contains(r#"clip-rule="evenodd""#),
            expect_evenodd,
            "{align:?}: wrong clip rule — outside clips to everything but the shape"
        );
    }
}

/// A centred stroke is the overwhelming majority and must keep the plain
/// semantic markup — no clip paths, no duplicated elements.
#[test]
fn a_centred_stroke_exports_as_one_plain_element() {
    let out = svg(&common::fixture().doc, &common::fixture().res, None);
    assert!(!out.contains("sa-"), "centred strokes need no clip path");
}

/// `<rect rx>` can say one radius, so unequal corners have to leave the `rect`
/// element behind entirely. Emitting the `rx` anyway (or dropping it) would
/// export a shape the canvas never drew.
#[test]
fn unequal_corners_are_exported_as_a_path_not_a_rounded_rect() {
    use ondin_core::kurbo::RoundedRectRadii;
    use ondin_core::{GeometryPatch, Operation, Resolved, Transaction};

    let mut f = common::fixture();
    let uniform = svg(&f.doc, &f.res, None);
    assert!(
        uniform.contains(r#"rx="6""#),
        "the fixture's rect starts with four equal corners:\n{uniform}"
    );

    f.doc
        .apply(&Transaction(vec![Operation::SetGeometry {
            id: f.rect,
            geometry: GeometryPatch::CornerRadii(RoundedRectRadii::new(6.0, 0.0, 6.0, 0.0)),
        }]))
        .unwrap();
    f.res = Resolved::rebuild(&f.doc);

    let mixed = svg(&f.doc, &f.res, None);
    // The ellipse keeps its own `rx`; what must be gone is a *rect* claiming a
    // single radius for four corners that no longer share one.
    for line in mixed.lines().filter(|l| l.contains("<rect")) {
        assert!(!line.contains("rx="), "a rect kept its rx:\n{line}");
    }
    // Order-independent: `[S8.1-L6-04]` put a `data-name` in front of the
    // transform, and the claim is about the *element* and its position.
    assert!(
        mixed.lines().any(|l| l.trim_start().starts_with("<path")
            && l.contains(r#"transform="matrix(1 0 0 1 20 20)""#)),
        "the rect became a path at its own position:\n{mixed}"
    );
}

/// A node can hold several fills and several strokes, and the canvas paints all of
/// them — so the export has to as well, or the file and the screen disagree about
/// the artwork. One element per paint inside a group carrying the transform.
///
/// The z-order is the one the scene walk uses: **all fills, then all strokes, in
/// list order**, so the last element of each list is the one on top.
#[test]
fn stacked_paints_export_one_element_each_in_paint_order() {
    use ondin_core::Brush;
    use ondin_core::peniko::Color;
    use ondin_core::{Fill, Operation, Resolved, Stroke, StrokeAlign, Transaction};

    let solid = |r: u8| Brush::Solid(Color::from_rgba8(r, 0, 0, 255));
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![
            Operation::SetFills {
                id: f.rect,
                fills: vec![
                    Fill {
                        brush: solid(0x11),
                        visible: true,
                    },
                    Fill {
                        brush: solid(0x22),
                        visible: true,
                    },
                ],
            },
            Operation::SetStrokes {
                id: f.rect,
                strokes: vec![
                    Stroke {
                        brush: solid(0x33),
                        width: 1.0,
                        join: ondin_core::kurbo::Join::Miter,
                        cap: ondin_core::kurbo::Cap::Butt,
                        dashes: vec![],
                        dash_offset: 0.0,
                        sides: Default::default(),
                        miter_limit: 4.0,
                        dash_fit: false,
                        align: StrokeAlign::Center,
                        visible: true,
                    },
                    Stroke {
                        brush: solid(0x44),
                        width: 1.0,
                        join: ondin_core::kurbo::Join::Miter,
                        cap: ondin_core::kurbo::Cap::Butt,
                        dashes: vec![],
                        dash_offset: 0.0,
                        sides: Default::default(),
                        miter_limit: 4.0,
                        dash_fit: false,
                        align: StrokeAlign::Center,
                        visible: true,
                    },
                ],
            },
        ]))
        .expect("stack paints");
    f.res = Resolved::rebuild(&f.doc);
    let out = svg(&f.doc, &f.res, None);

    // Each paint reached the file...
    for hex in ["#110000", "#220000", "#330000", "#440000"] {
        assert!(out.contains(hex), "{hex} missing from:\n{out}");
    }
    // ...in fills-then-strokes order, each on its own element.
    let at = |needle: &str| out.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(
        at(r##"fill="#110000""##) < at(r##"fill="#220000""##),
        "fills out of order"
    );
    assert!(
        at(r##"fill="#220000""##) < at(r##"stroke="#330000""##),
        "a stroke before a fill"
    );
    assert!(
        at(r##"stroke="#330000""##) < at(r##"stroke="#440000""##),
        "strokes out of order"
    );
    // A stroke element must not fill as well, or it would paint over the fills
    // stacked beneath it.
    for line in out.lines().filter(|l| l.contains(r##"stroke="#330000""##)) {
        assert!(
            line.contains(r#"fill="none""#),
            "stroke element also fills:\n{line}"
        );
    }
    // Gradient slots are indexed now that there can be more than one of each, so
    // an id has to name *which* paint.
    let g = svg(&common::fixture().doc, &common::fixture().res, None);
    if g.contains("grad-") {
        assert!(
            g.contains("-fill-") || g.contains("-stroke-") || g.contains("-bg"),
            "a gradient slot must name its index:\n{g}"
        );
    }
}

/// The single-fill, single-centred-stroke shape is the overwhelming majority and
/// has to keep the plain one-element markup: no wrapper group, no duplication.
#[test]
fn an_ordinary_shape_still_exports_as_one_element() {
    let f = common::fixture();
    let out = svg(&f.doc, &f.res, None);
    // The fixture's rect: fill and stroke on the same element, with its own
    // transform rather than one hoisted onto a wrapper.
    let rect = out
        .lines()
        .find(|l| l.contains("<rect") && l.contains(r##"fill="#3c78dc""##))
        .expect("the fixture's blue rect");
    assert!(rect.contains("stroke="), "stroke left the element:\n{rect}");
    assert!(
        rect.contains("transform="),
        "transform left the element:\n{rect}"
    );
}

/// A per-side stroke is not the shape's outline, so it cannot ride the `<rect>`
/// element: it has to come out as one `<path>` per edge, at that edge's own
/// width.
///
/// The assertion that matters is the *count* and the widths. An export that
/// emitted the whole outline once would still contain a stroke and still look
/// plausible in a diff, and would be drawing three edges the canvas does not.
#[test]
fn a_per_side_stroke_is_exported_as_one_path_per_edge() {
    use ondin_core::{Operation, Resolved, Stroke, StrokeSides, Transaction};

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetStrokes {
            id: f.rect,
            strokes: vec![Stroke {
                brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::BLACK),
                width: 9.0,
                sides: StrokeSides::Custom {
                    top: 1.0,
                    right: 0.0,
                    bottom: 4.0,
                    left: 2.0,
                },
                ..Default::default()
            }],
        }]))
        .expect("per-side stroke");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);

    // Three sides carry paint; the one set to 0 does not.
    for w in ["1", "2", "4"] {
        assert!(
            out.contains(&format!(r#"stroke-width="{w}""#)),
            "side of width {w} missing from:\n{out}"
        );
    }
    assert!(
        !out.contains(r#"stroke-width="9""#),
        "the nominal width is not a side and must not be drawn:\n{out}"
    );
    assert!(
        !out.contains(r#"stroke-width="0""#),
        "a side of 0 is no stroke at all:\n{out}"
    );
    // One `<path>` per painted side, and the rect's own element carries no stroke.
    let sides = out.matches(r#"<path d=""#).count();
    assert!(
        sides >= 3,
        "expected three side paths, found {sides}:\n{out}"
    );
}

/// SVG can express neither a zero-length dot nor "fit to corners", so both have
/// to arrive in the file already turned into numbers — otherwise the export draws
/// a solid line where the canvas drew dots.
#[test]
fn a_dotted_stroke_exports_as_numbers_svg_can_draw() {
    use ondin_core::kurbo::Cap;
    use ondin_core::{Operation, Resolved, Stroke, Transaction};

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetStrokes {
            id: f.rect,
            strokes: vec![Stroke {
                brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::BLACK),
                width: 2.0,
                // A dot: zero length, 6 apart centre to centre.
                dashes: vec![0.0, 6.0],
                cap: Cap::Butt,
                ..Default::default()
            }],
        }]))
        .expect("dotted stroke");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);

    assert!(
        out.contains(r#"stroke-dasharray="2,4""#),
        "a butt cap cannot draw a zero-length dash, so it must be given a square \
         dot with the period preserved:\n{out}"
    );
    assert!(
        !out.contains(r#"stroke-dasharray="0,6""#),
        "the model's own zero-length pattern would draw nothing:\n{out}"
    );
}

/// The mitre limit is emitted only when it is not SVG's own default of 4 — the
/// same number kurbo uses. Writing it unconditionally would rewrite the stroke
/// attributes of every document ever exported to say what they already meant.
#[test]
fn the_mitre_limit_reaches_the_file_only_when_it_is_not_the_default() {
    use ondin_core::{Operation, Resolved, Stroke, Transaction};

    let plain = ondin_export::svg::svg(&common::fixture().doc, &common::fixture().res, None);
    assert!(!plain.contains("stroke-miterlimit"), "{plain}");

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetStrokes {
            id: f.rect,
            strokes: vec![Stroke {
                brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::BLACK),
                width: 2.0,
                miter_limit: 1.5,
                ..Default::default()
            }],
        }]))
        .expect("mitre limit");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    assert!(out.contains(r#"stroke-miterlimit="1.5""#), "{out}");
}

/// A decoration's colour, thickness and line style reach the file, where the
/// writer used to emit `text-decoration="underline"` and drop all three.
///
/// **The attribute order is asserted, not incidental.** `text-decoration` is a
/// shorthand: its omitted parts reset to their initial values, so a shorthand
/// written *after* the longhands silently undoes them, and the mistake is
/// invisible in the source and in the output alike.
#[test]
fn a_decorations_colour_thickness_and_style_are_exported() {
    use ondin_core::peniko::Color;
    use ondin_core::{Decoration, Length, LineStyle, Operation, Resolved, TextStyle, Transaction};

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            spans: None,
            style: TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                underline: Some(Decoration {
                    thickness: Some(Length::Px(3.0)),
                    style: LineStyle::Wavy,
                    color: Some(Color::from_rgba8(255, 0, 0, 255)),
                    ..Decoration::default()
                }),
                ..Default::default()
            },
        }]))
        .expect("decorate the text");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);

    assert!(
        out.contains(r##"text-decoration-color="#ff0000""##),
        "the colour the picker sets:\n{out}"
    );
    assert!(
        // `px`, not a bare `3`: this is CSS's property, not SVG's, and a unitless
        // length there is invalid and drops the whole declaration.
        out.contains(r#"text-decoration-thickness="3px""#),
        "the thickness, with a unit:\n{out}"
    );
    assert!(
        out.contains(r#"text-decoration-style="wavy""#),
        "the line style verbatim:\n{out}"
    );
    let line = out.find("text-decoration=").expect("the line list");
    let colour = out.find("text-decoration-color").unwrap();
    assert!(
        line < colour,
        "the shorthand must precede the longhands or it resets them:\n{out}"
    );

    // ⚠️ **A negative thickness is floored here too** — `[S6.3-L1-02]`, §15 D546.
    // `core::text::decoration_ink` ends its resolve with `.max(0.0)`, so the
    // canvas drew nothing for a negative; this writer had no counterpart and
    // emitted `-5px` verbatim, so the file and the screen disagreed about whether
    // there was a line — and re-importing gave a third answer, because `svg_in`
    // reads no decoration thickness at all and it came back as "the font
    // decides".
    //
    // ⚠️ **This assertion bites, and the comment here said it would not.**
    // `canonical_decoration` clamps at the model since D546, and the first draft
    // of this test claimed the fixture therefore stored `0.0` already and was
    // only pinning that the writer *agrees*. The flip says otherwise: removing
    // the writer's `.max(0.0)` fails this test. **The model clamp is reached
    // from `TextStyle::set`, and the op layer does not go through it** —
    // `op_set_text_style` is `mem::replace(s, Box::new(style.clone()))`, and
    // `CreateNode` is the same. The *panel* is safe because
    // `panels::typography::char_attrs_tx` calls `set` on a local style before
    // building its op; this fixture builds `Operation::SetTextStyle` by hand and
    // walks straight past. So the negative really is in the document and the
    // writer really is the thing that floors it.
    //
    // *Recorded rather than tidied: the claim was written from reading and the
    // flip was what corrected it.*
    f.doc
        .apply(&Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            spans: None,
            style: TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                underline: Some(Decoration {
                    thickness: Some(Length::Px(-5.0)),
                    ..Decoration::default()
                }),
                ..Default::default()
            },
        }]))
        .expect("a negative thickness is storable, it is just not drawable");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    assert!(
        !out.contains("thickness=\"-"),
        "no negative thickness may reach the file — the canvas draws none:\n{out}"
    );
    assert!(
        out.contains(r#"text-decoration-thickness="0px""#),
        "and it is written as an explicit 0, which is \"no line\" — dropping the \
         attribute would mean \"the font decides\" and put the line back:\n{out}"
    );
}

/// An em thickness resolves against the font size, the way `letter-spacing`
/// already does — SVG has no em for the value to stay in.
#[test]
fn an_em_decoration_thickness_is_resolved_to_user_units() {
    use ondin_core::{Decoration, Length, Operation, Resolved, TextStyle, Transaction};

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            spans: None,
            style: TextStyle {
                font_family: "Inter".into(),
                font_size: 40.0,
                underline: Some(Decoration {
                    thickness: Some(Length::Em(0.05)),
                    ..Decoration::default()
                }),
                ..Default::default()
            },
        }]))
        .expect("decorate the text");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    assert!(
        out.contains(r#"text-decoration-thickness="2px""#),
        "5% of 40:\n{out}"
    );
    assert!(
        !out.contains("text-decoration-style"),
        "Solid is CSS's initial value too, so writing it is noise:\n{out}"
    );
    assert!(
        !out.contains("text-decoration-color"),
        "an unset colour must stay inherited from the text:\n{out}"
    );
}

/// **An underline's offset reaches the file, negated, and only when the run has
/// no strikethrough.**
///
/// `text-underline-offset` is underline-only and has no strikethrough
/// counterpart, so a run carrying both cannot say two offsets — the one property
/// of the five that stays lost, and only for that shape.
///
/// **The negation is the assertion worth having**, because getting it backwards
/// exports an underline the same distance on the *wrong side* of the baseline and
/// nothing errors: ours is parley's, positive up from the baseline; CSS's is
/// positive "outward from the text", i.e. down.
#[test]
fn an_underlines_offset_is_exported_negated_unless_a_strikethrough_shares_the_run() {
    use ondin_core::{Decoration, Length, Operation, Resolved, TextStyle, Transaction};

    let decorated = |underline: Option<Decoration>, strikethrough: Option<Decoration>| {
        let mut f = common::fixture();
        f.doc
            .apply(&Transaction(vec![Operation::SetTextStyle {
                id: f.text,
                spans: None,
                style: TextStyle {
                    font_family: "Inter".into(),
                    font_size: 20.0,
                    underline,
                    strikethrough,
                    ..Default::default()
                },
            }]))
            .expect("decorate the text");
        f.res = Resolved::rebuild(&f.doc);
        ondin_export::svg::svg(&f.doc, &f.res, None)
    };
    let raised = Some(Decoration {
        offset: Some(Length::Px(2.0)),
        ..Decoration::default()
    });

    let out = decorated(raised, None);
    assert!(
        out.contains(r#"text-underline-offset="-2px""#),
        "a stored +2 is 2px *above* the baseline, which CSS spells -2px:\n{out}"
    );
    // An em offset resolves against the font size, as the thickness does.
    let out = decorated(
        Some(Decoration {
            offset: Some(Length::Em(0.1)),
            ..Decoration::default()
        }),
        None,
    );
    assert!(
        out.contains(r#"text-underline-offset="-2px""#),
        "10% of 20:\n{out}"
    );
    // And the case the limit was recorded for: two decorations, one property.
    let out = decorated(raised, Some(Decoration::default()));
    assert!(
        !out.contains("text-underline-offset"),
        "a run with both cannot say two offsets:\n{out}"
    );
    // Nothing written for a decoration that leaves the offset to the font.
    let out = decorated(Some(Decoration::default()), None);
    assert!(
        !out.contains("text-underline-offset"),
        "an unset offset must stay the font's own:\n{out}"
    );
}

/// **Two decorations that disagree can say neither.** CSS gives an element one
/// `text-decoration-color`, this writer emits one `<text>` per node, and picking
/// the underline's would export a strikethrough in a colour the document never
/// says. Dropping the property leaves both inheriting the text's own ink, which
/// is what the file did before any of these three were written.
#[test]
fn decorations_that_disagree_export_no_property_rather_than_a_winner() {
    use ondin_core::peniko::Color;
    use ondin_core::{Decoration, Length, LineStyle, Operation, Resolved, TextStyle, Transaction};

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            spans: None,
            style: TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                underline: Some(Decoration {
                    thickness: Some(Length::Px(3.0)),
                    style: LineStyle::Wavy,
                    color: Some(Color::from_rgba8(255, 0, 0, 255)),
                    ..Decoration::default()
                }),
                strikethrough: Some(Decoration {
                    thickness: Some(Length::Px(1.0)),
                    style: LineStyle::Dotted,
                    color: Some(Color::from_rgba8(0, 0, 255, 255)),
                    ..Decoration::default()
                }),
                ..Default::default()
            },
        }]))
        .expect("decorate the text twice");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);

    assert!(
        out.contains(r#"text-decoration="underline line-through""#),
        "both lines are still named — that part is a list:\n{out}"
    );
    for dropped in [
        "text-decoration-color",
        "text-decoration-thickness",
        "text-decoration-style",
    ] {
        assert!(
            !out.contains(dropped),
            "{dropped} cannot describe two decorations at once:\n{out}"
        );
    }
}

/// And when they agree, the shared value is written once — the case that proves
/// the test above is asserting on disagreement rather than on there being two.
#[test]
fn two_decorations_that_agree_export_the_shared_value() {
    use ondin_core::peniko::Color;
    use ondin_core::{Decoration, LineStyle, Operation, Resolved, TextStyle, Transaction};

    let both = Some(Decoration {
        style: LineStyle::Dashed,
        color: Some(Color::from_rgba8(0, 128, 0, 255)),
        ..Decoration::default()
    });
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            spans: None,
            style: TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                underline: both,
                strikethrough: both,
                ..Default::default()
            },
        }]))
        .expect("decorate the text twice");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    assert!(
        out.contains(r##"text-decoration-color="#008000""##),
        "{out}"
    );
    assert!(out.contains(r#"text-decoration-style="dashed""#), "{out}");
}

/// A translucent decoration colour has nowhere but the hex to put its alpha:
/// unlike `fill`, `text-decoration-color` has no `-opacity` companion.
#[test]
fn a_translucent_decoration_colour_carries_its_alpha_in_the_hex() {
    use ondin_core::peniko::Color;
    use ondin_core::{Decoration, Operation, Resolved, TextStyle, Transaction};

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            spans: None,
            style: TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                underline: Some(Decoration {
                    color: Some(Color::from_rgba8(255, 0, 0, 128)),
                    ..Decoration::default()
                }),
                ..Default::default()
            },
        }]))
        .expect("decorate the text");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    assert!(
        out.contains(r##"text-decoration-color="#ff000080""##),
        "eight-digit hex, since there is no text-decoration-color-opacity:\n{out}"
    );
}

/// **A frame's border is emitted after its contents and outside its clip group.**
///
/// The markup shape of the canvas's own arrangement: two groups when there is both a
/// clip and a stroke — an outer one with the transform and opacity, an inner one with
/// only the `clip-path` — and the `<rect>` carrying the border between the inner
/// group's close and the outer's. Inside the clip group it would be clipped by the
/// very box it draws on, which is an export that silently disagrees with the canvas.
#[test]
fn a_frames_border_is_written_outside_the_group_that_clips_it() {
    use ondin_core::{Operation, Resolved, Stroke, Transaction};
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![
            Operation::SetClip {
                id: f.artboard,
                clip: true,
            },
            Operation::SetStrokes {
                id: f.artboard,
                strokes: vec![Stroke {
                    brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                        255, 0, 0, 255,
                    )),
                    width: 3.0,
                    ..Default::default()
                }],
            },
        ]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);

    let border = out
        .find(r##"stroke="#ff0000""##)
        .unwrap_or_else(|| panic!("the frame's border is in the file:\n{out}"));
    let clip_open = out
        .find(r#"<g clip-path="url(#clip-"#)
        .expect("the clip group");
    let clip_close = out[clip_open..]
        .find("</g>")
        .map(|i| clip_open + i)
        .expect("it closes");
    assert!(
        border > clip_close,
        "the border must be written after the clip group closes, or the frame clips \
         its own stroke away:\n{out}"
    );
    // The outer group is the one carrying the transform, so the border sits in the
    // frame's own user space.
    //
    // ⚠️ **Read as "the element with the transform has no clip on it" rather than
    // as one exact string**, which is what it was until `[S8.1-L6-04]` gave every
    // node's element a `data-name` in front of its transform. The claim was never
    // about attribute *order*; spelling it as a literal made a test of the split
    // fail for an attribute that has nothing to do with clipping.
    let outer = out
        .lines()
        .find(|l| l.contains(r#"transform="matrix(1 0 0 1 10 10)""#))
        .unwrap_or_else(|| panic!("the frame's own group is in the file:\n{out}"));
    assert!(
        !outer.contains("clip-path="),
        "the transform moved onto the clipping group:\n{outer}"
    );
    // And the border is unfilled: the background already painted, and a filled
    // element here would cover the children.
    let after = &out[border.saturating_sub(200)..border];
    assert!(
        after.contains(r#"fill="none""#),
        "the border element must not fill:\n{after}"
    );
}

/// A frame with no stroke is written exactly as it always was — one group, doing both
/// jobs. The exact-string assertions elsewhere in this suite are the other half of it;
/// this one says *why* nothing moved.
#[test]
fn a_frame_without_a_border_is_still_one_group() {
    use ondin_core::{Operation, Resolved, Transaction};
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetClip {
            id: f.artboard,
            clip: true,
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);
    // Order-independent for the reason its sibling above gives.
    let outer = out
        .lines()
        .find(|l| l.contains(r#"transform="matrix(1 0 0 1 10 10)""#))
        .unwrap_or_else(|| panic!("the frame's own group is in the file:\n{out}"));
    assert!(
        outer.contains(r#"clip-path="url(#clip-"#),
        "transform and clip on one element:\n{outer}"
    );
}

/// **A text node's stroke reaches the file**, on the `<text>` element itself so the
/// output stays re-shapeable — and for outlined type, inside a `clipPath` built from
/// the glyph outlines, which is the same construction a shape's aligned stroke gets.
///
/// Text took fills and dropped strokes entirely before this, in the walk and here.
#[test]
fn text_carries_its_stroke_and_an_outside_one_gets_a_glyph_clip() {
    use ondin_core::{Operation, Resolved, Stroke, StrokeAlign, Transaction};
    let red =
        ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(255, 0, 0, 255));

    // Centred: one plain element with a stroke on it.
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetStrokes {
            id: f.text,
            strokes: vec![Stroke {
                brush: red.clone(),
                width: 2.0,
                ..Default::default()
            }],
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);
    let text_line = out
        .lines()
        .find(|l| l.contains("<text"))
        .unwrap_or_else(|| panic!("a text element:\n{out}"));
    assert!(
        text_line.contains(r##"stroke="#ff0000""##) && text_line.contains(r#"stroke-width="2""#),
        "the stroke rides the text element:\n{text_line}"
    );

    // Outside: doubled width inside a clip built from the glyphs.
    f.doc
        .apply(&Transaction(vec![Operation::SetStrokes {
            id: f.text,
            strokes: vec![Stroke {
                brush: red,
                width: 2.0,
                align: StrokeAlign::Outside,
                ..Default::default()
            }],
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);
    assert!(
        out.contains(r#"clip-rule="evenodd""#),
        "outlined type needs the complement clip:\n{out}"
    );
    assert!(
        out.contains(r#"stroke-width="4""#),
        "and the doubled width the clip halves back:\n{out}"
    );
    // The clip path is the glyph outlines, not the node's box: a box would clip
    // nothing and the stroke would come out centred on every letter.
    let clip = out
        .lines()
        .find(|l| l.contains("<clipPath") && l.contains("evenodd"))
        .expect("the clipPath line");
    // Quadratics, because Inter is `glyf` — a cubic-only test would pass on a CFF
    // face and fail on the bundled one, which is the only one this suite has.
    assert!(
        clip.matches('Q').count() + clip.matches('C').count() > 4,
        "glyph outlines are curves, so the clip is not a rectangle:\n{clip}"
    );
    // The complement ring is in there too — the outer box the even-odd rule turns
    // into "everything but the letters".
    assert!(clip.matches('Z').count() > 2, "ring plus contours:\n{clip}");
}

/// **A fitted dash on a multi-subpath path is one `<path>` per subpath**, each with
/// its own `stroke-dasharray` — an element can only say one pattern, and a fit scale
/// is a property of one closed run. Fitting the whole path gave both subpaths one
/// pattern and neither of them its own corners.
#[test]
fn a_fitted_dash_on_two_subpaths_is_two_paths_with_two_patterns() {
    use ondin_core::kurbo::{BezPath, Rect, Shape};
    use ondin_core::{IdSource, NodeKind, Operation, Resolved, Stroke, Transaction};

    let mut ids = IdSource::new(0xDA54);
    let root = ids.mint();
    let mut doc = ondin_core::Document::new(root);
    let node = ids.mint();
    let mut path = Rect::new(0.0, 0.0, 30.0, 20.0).to_path(0.01);
    path.extend(Rect::new(100.0, 0.0, 115.0, 5.0).to_path(0.01).iter());
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: node,
        parent: root,
        index: 0,
        kind: NodeKind::Path {
            path,
            corner_radii: Vec::new(),
        },
        transform: None,
        name: None,
    }]))
    .unwrap();
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: node,
        strokes: vec![Stroke {
            width: 1.0,
            dashes: vec![5.0, 2.0],
            dash_fit: true,
            ..Default::default()
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let out = ondin_export::svg::svg(&doc, &res, None);

    let arrays: Vec<&str> = out
        .match_indices("stroke-dasharray=\"")
        .map(|(i, m)| {
            let rest = &out[i + m.len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();
    assert_eq!(
        arrays.len(),
        2,
        "one element per subpath, each with its own pattern:\n{out}"
    );
    assert_ne!(
        arrays[0], arrays[1],
        "the two subpaths have perimeters 100 and 40 against a period of 7, so one \
         shared pattern is the approximation this replaced:\n{out}"
    );
    // Each fits its own subpath a whole number of times.
    for (array, length) in arrays.iter().zip([100.0, 40.0]) {
        let period: f64 = array
            .split(',')
            .map(|n| n.parse::<f64>().expect(array))
            .sum();
        let n: f64 = length / period;
        assert!(
            (n - n.round()).abs() < 1e-3,
            "{array} has period {period}, which fits {n} times into {length}"
        );
    }
    // The unrounded path is still what carries the ink, and there is no fill on it.
    assert_eq!(out.matches("<path").count(), 2, "{out}");

    // Unfitted, the same stroke is one element with one pattern.
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: node,
        strokes: vec![Stroke {
            width: 1.0,
            dashes: vec![5.0, 2.0],
            dash_fit: false,
            ..Default::default()
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let out = ondin_export::svg::svg(&doc, &res, None);
    assert_eq!(
        out.matches("stroke-dasharray=\"").count(),
        1,
        "no split when there is no fit to disagree about:\n{out}"
    );
    let _ = BezPath::new();
}

/// **An unpainted text node exports black, as the canvas draws it.** It exported
/// `fill="none"` — present in the file, counted in the viewBox, invisible. The
/// condition is the one §15 D145 sharpened: no fill *and no stroke*, so outlined type
/// keeps its hollow letters.
#[test]
fn an_unpainted_text_node_exports_the_black_the_canvas_draws() {
    use ondin_core::{Operation, Resolved, Stroke, Transaction};
    let f = common::fixture();
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    let line = out
        .lines()
        .find(|l| l.contains("<text"))
        .unwrap_or_else(|| panic!("a text element:\n{out}"));
    assert!(
        line.contains(r##"fill="#000000""##),
        "the fixture's text has no fills, so it must export black:\n{line}"
    );

    // With a stroke on it, there is ink, so no stand-in fill.
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetStrokes {
            id: f.text,
            strokes: vec![Stroke {
                brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                    255, 0, 0, 255,
                )),
                width: 2.0,
                ..Default::default()
            }],
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);
    let line = out
        .lines()
        .find(|l| l.contains("<text"))
        .unwrap_or_else(|| panic!("a text element:\n{out}"));
    assert!(
        line.contains(r#"fill="none""#),
        "outlined type must stay hollow:\n{line}"
    );
}

/// **A wrapped node exports as one `<text>` per line.** The whole of §15 D81's
/// first half: this writer emitted a single unwrapped `<text>` with the entire
/// content in it, so a paragraph that wrapped on the canvas exported as one long
/// line running out of its own frame. Each line now sits at its own baseline, which
/// is the shaped one rather than the old `y = font_size` guess.
#[test]
fn a_wrapped_text_node_exports_one_text_element_per_line() {
    use ondin_core::kurbo::Size;
    use ondin_core::{GeometryPatch, Operation, Resolved, TextSizing, Transaction};

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![
            Operation::SetText {
                id: f.text,
                content: "wrap me over several lines please".into(),
                spans: Default::default(),
                para_spans: Default::default(),
            },
            Operation::SetGeometry {
                id: f.text,
                geometry: GeometryPatch::TextSizing(TextSizing::Fixed(Size::new(70.0, 200.0))),
            },
        ]))
        .expect("a narrow fixed box wraps");
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);

    let texts: Vec<&str> = out.lines().filter(|l| l.contains("<text")).collect();
    assert!(
        texts.len() > 1,
        "70pt at 16pt type has to wrap into several <text> elements:\n{out}"
    );
    // Descending baselines, which is the thing one element could not express.
    let ys: Vec<f64> = texts
        .iter()
        .map(|l| {
            let at = l.find(r#" y=""#).expect("a y attribute") + 4;
            let rest = &l[at..];
            rest[..rest.find('"').expect("a closing quote")]
                .parse()
                .expect("a number")
        })
        .collect();
    assert!(
        ys.windows(2).all(|w| w[1] > w[0]),
        "each line's baseline must sit below the last: {ys:?}"
    );
    // And every character is still in the file, exactly once.
    let words = ["wrap", "several", "please"];
    for w in words {
        assert_eq!(
            out.matches(w).count(),
            1,
            "{w} must appear exactly once across the lines:\n{out}"
        );
    }
}

/// **A list marker reaches the file, and as a `<text>` of its own** (§15 D169).
///
/// The marker is ink the canvas draws out of `TextLayout::runs`, and this writer does
/// not read that field at all — it walks `text::export_lines`, so a marker emitted
/// only into the cached layout would draw on screen and be missing from every export.
/// That is the second-path bug this test exists for.
///
/// A `<text>` rather than a `<tspan>` because tspans **flow**: sharing the line's
/// element would put the marker against the first word instead of out in the gutter,
/// which is the one thing a marker must not do.
#[test]
fn a_list_marker_exports_as_its_own_text_element() {
    use ondin_core::{
        ListMarker, Operation, ParaAttr, ParaSpans, ParagraphStyle, Resolved, Transaction,
    };

    let mut f = common::fixture();
    let paragraph = ParagraphStyle {
        marker: Some(ListMarker::Decimal),
        indent_start: ondin_core::Length::Px(30.0),
        ..Default::default()
    };
    // Five items with the middle one opting out, so the numbering restart is in the
    // file too and not only in core's own tests.
    //
    // **Byte 8 because that is where "three" *starts*.** A paragraph reads the value
    // in force at its first byte (§5.4), so a span covering any later byte of it
    // changes nothing — this test was first written against byte 6, which is inside
    // "two", and it duly numbered all five.
    let mut para_spans = ParaSpans::default();
    para_spans.set(8..9, ParaAttr::Marker(None), &paragraph);
    // **The style op goes first, and that is load-bearing** — the same order
    // `para_attrs_tx` builds and for the same reason (§15 D164). Every op that
    // installs a span list normalizes it against the defaults *in force at the time*,
    // and a `Marker(None)` span is equal to a default of `None`: with `SetText` first
    // the span was dropped as redundant before the marker default existed, and this
    // test duly numbered all five items.
    f.doc
        .apply(&Transaction(vec![
            Operation::SetParagraphStyle {
                id: f.text,
                paragraph,
                spans: None,
            },
            Operation::SetText {
                id: f.text,
                content: "one\ntwo\nthree\nfour\nfive".into(),
                spans: Default::default(),
                para_spans,
            },
        ]))
        .expect("a list applies");
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);

    // `1.` `2.` on the first two items, then `1.` again after the gap — the third
    // paragraph is not an item, so the run restarts.
    for marker in ["1.", "2."] {
        assert_eq!(
            out.matches(&format!(">{marker}<")).count(),
            2,
            "{marker} should appear twice — once per numbered run:\n{out}"
        );
    }
    assert!(
        !out.contains(">3.<"),
        "the run restarted, so nothing is numbered 3:\n{out}"
    );
    // And each marker is a `<text>` of its own, left of the 30pt text column.
    let xs: Vec<f64> = out
        .lines()
        .filter(|l| l.contains(">1.<") || l.contains(">2.<"))
        .map(|l| {
            assert!(l.contains("<text "), "a marker is its own element: {l}");
            let at = l.find(r#" x=""#).expect("an x attribute") + 4;
            let rest = &l[at..];
            rest[..rest.find('"').expect("a closing quote")]
                .parse()
                .expect("a number")
        })
        .collect();
    assert_eq!(xs.len(), 4, "four markers, four elements: {xs:?}");
    assert!(
        xs.iter().all(|x| *x > 0.0 && *x < 30.0),
        "every marker sits in the gutter between 0 and the text column: {xs:?}"
    );
}

/// **A nested item exports on its own gutter, marker and text together.**
///
/// The nesting arithmetic is one function in core (`ParagraphStyle::start_edge`), but
/// the *marker* reaches SVG through `export_lines` and the *text* through parley's own
/// line x — two paths that meet only in the file. This is where a nested marker left in
/// the outer gutter would show up, and the numbering rides along: the level-1 item is
/// `1.` while the outer run counts through it.
#[test]
fn a_nested_item_exports_on_its_own_gutter() {
    use ondin_core::{
        ListMarker, Operation, ParaAttr, ParaSpans, ParagraphStyle, Resolved, Transaction,
    };

    let mut f = common::fixture();
    let paragraph = ParagraphStyle {
        marker: Some(ListMarker::Decimal),
        indent_start: ondin_core::Length::Px(30.0),
        ..Default::default()
    };
    // Byte 4 is where "two" starts — a paragraph reads its value at its own first
    // byte, and the style op goes first so the span is not normalized away (see
    // `a_list_marker_exports_as_its_own_text_element`, which learnt both the hard way).
    let mut para_spans = ParaSpans::default();
    para_spans.set(4..5, ParaAttr::Level(1), &paragraph);
    f.doc
        .apply(&Transaction(vec![
            Operation::SetParagraphStyle {
                id: f.text,
                paragraph,
                spans: None,
            },
            Operation::SetText {
                id: f.text,
                content: "one\ntwo\nthree".into(),
                spans: Default::default(),
                para_spans,
            },
        ]))
        .expect("a nested list applies");
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);

    // The outer run counts through the nested one, so `1.` appears twice — once for
    // item "one" and once for the sublist — and "three" is `2.`.
    assert_eq!(
        out.matches(">1.<").count(),
        2,
        "two lists, each opening at 1:\n{out}"
    );
    assert_eq!(
        out.matches(">2.<").count(),
        1,
        "and the outer list resumes at 2 rather than restarting:\n{out}"
    );

    // Every `<text>` element's x, in file order. The fixture has one text node, and a
    // marker is emitted immediately before the line it belongs to, so this is
    // `[marker, line]` three times over.
    //
    // **The text cannot be matched on, which is why this reads positionally**: a run
    // carries its own hard break, so `<tspan>two\n</tspan>` puts a literal newline in
    // the file and the element's own `x` is on the output line above it.
    let xs: Vec<f64> = out
        .lines()
        .filter(|l| l.contains("<text "))
        .map(|l| {
            let at = l.find(r#" x=""#).expect("an x attribute") + 4;
            let rest = &l[at..];
            rest[..rest.find('"').expect("a closing quote")]
                .parse()
                .expect("a number")
        })
        .collect();
    assert_eq!(xs.len(), 6, "three markers and three lines: {xs:?}\n{out}");
    assert_eq!(
        (xs[1], xs[3], xs[5]),
        (30.0, 60.0, 30.0),
        "the nested item's text is two gutters in and its neighbours are one: {xs:?}"
    );
    // And each marker in its own item's gutter — the assertion a multiply applied to
    // the lines alone fails, leaving the nested marker back at the outer column.
    assert!(
        xs[0] > 0.0 && xs[0] < 30.0 && xs[4] > 0.0 && xs[4] < 30.0,
        "the outer markers hang left of the 30pt column: {xs:?}"
    );
    assert!(
        xs[2] > 30.0 && xs[2] < 60.0,
        "and the nested marker left of the 60pt one: {xs:?}"
    );
}

/// **A character span exports as its own `<tspan>`, carrying its own face.**
/// The other half of D81: every attribute came off the node's *defaults*, so a
/// word set at 40pt exported at the node's 16 and an underlined word exported
/// undecorated — silently, in a file that looked complete.
#[test]
fn a_styled_span_exports_as_its_own_tspan() {
    use ondin_core::{CharAttr, CharSpans, Operation, Resolved, TextStyle, Transaction};

    let mut f = common::fixture();
    let defaults = TextStyle {
        font_family: "Inter".into(),
        font_size: 16.0,
        ..Default::default()
    };
    let mut spans = CharSpans::default();
    spans.set(0..2, CharAttr::Size(40.0), &defaults);
    f.doc
        .apply(&Transaction(vec![
            Operation::SetText {
                id: f.text,
                content: "abcdef".into(),
                spans: Default::default(),
                para_spans: Default::default(),
            },
            Operation::SetTextSpans { id: f.text, spans },
        ]))
        .expect("style the first two characters");
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);

    assert!(
        out.contains(r#"<tspan font-family="Inter" font-size="40""#),
        "the span's own 40pt has to reach its tspan:\n{out}"
    );
    assert!(
        out.contains(r#"font-size="16""#),
        "and the rest keeps the node's 16:\n{out}"
    );
    // Two runs, one element: the split is inside a single <text>.
    let line = out
        .lines()
        .find(|l| l.contains("<text"))
        .unwrap_or_else(|| panic!("a text element:\n{out}"));
    assert_eq!(
        line.matches("<tspan").count(),
        2,
        "one tspan per run, both on one line:\n{line}"
    );
}

/// **A coloured run carries its own `fill` on its `<tspan>`**, and an uncoloured one
/// carries none — absence inherits the `<text>` element's, which is exactly what
/// `TextStyle::color`'s `None` means (§15 D154). Alpha rides `fill-opacity`, because
/// `fill` has one where `text-decoration-color` does not.
#[test]
fn a_coloured_run_carries_its_own_fill_and_an_uncoloured_one_inherits() {
    use ondin_core::peniko::Color;
    use ondin_core::{CharAttr, CharSpans, Operation, Resolved, TextStyle, Transaction};

    let mut f = common::fixture();
    let defaults = TextStyle {
        font_family: "Inter".into(),
        font_size: 16.0,
        ..Default::default()
    };
    let mut spans = CharSpans::default();
    spans.set(
        0..2,
        CharAttr::Color(Some(Color::from_rgba8(0x91, 0x84, 0xd9, 128))),
        &defaults,
    );
    f.doc
        .apply(&Transaction(vec![
            Operation::SetText {
                id: f.text,
                content: "abcdef".into(),
                spans: Default::default(),
                para_spans: Default::default(),
            },
            Operation::SetTextSpans { id: f.text, spans },
        ]))
        .expect("colour the first two characters");
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);

    assert!(
        out.contains(r##"fill="#9184d9""##),
        "the run's own colour has to reach its tspan:\n{out}"
    );
    assert!(
        out.contains(r#"fill-opacity="0.502""#),
        "and its alpha rides fill-opacity — 128/255 is 0.502, not 0.5, which is what \
         8-bit alpha means:\n{out}"
    );
    let line = out
        .lines()
        .find(|l| l.contains("<text"))
        .unwrap_or_else(|| panic!("a text element:\n{out}"));
    assert_eq!(
        line.matches("fill=").count(),
        2,
        "one fill on the <text> and one on the coloured tspan — the uncoloured run \
         must inherit rather than repeat it:\n{line}"
    );
}

/// **A per-paragraph indent reaches the file, end to end.** The measure a
/// paragraph is broken to is set through parley's per-line controls rather than
/// applied afterwards, so it lands in the line's `inline_min_coord` — and D81 put
/// the alignment in each line's own `x`, which is the one attribute that would
/// silently drop it. The whole point of stage 1 landing before the panel is that a
/// hand-authored file draws correctly, and this is the assertion that it does.
#[test]
fn a_per_paragraph_indent_reaches_each_lines_x() {
    use ondin_core::kurbo::Size;
    use ondin_core::{
        GeometryPatch, Length, Operation, ParaAttr, ParaSpans, ParagraphStyle, Resolved,
        TextSizing, Transaction,
    };

    let content = "one\ntwo";
    let second = content.find('\n').expect("two paragraphs") + 1;
    let paragraph = ParagraphStyle::default();
    let mut para_spans = ParaSpans::default();
    para_spans.set(
        second..content.len(),
        ParaAttr::IndentStart(Length::Px(40.0)),
        &paragraph,
    );

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![
            Operation::SetText {
                id: f.text,
                content: content.into(),
                spans: Default::default(),
                para_spans,
            },
            Operation::SetGeometry {
                id: f.text,
                geometry: GeometryPatch::TextSizing(TextSizing::Fixed(Size::new(200.0, 100.0))),
            },
        ]))
        .expect("two paragraphs in a fixed box");
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &res, None);

    let xs: Vec<f64> = out
        .lines()
        .filter(|l| l.contains("<text"))
        .map(|l| {
            let at = l.find(r#" x=""#).expect("an x attribute") + 4;
            let rest = &l[at..];
            rest[..rest.find('"').expect("a closing quote")]
                .parse()
                .expect("a number")
        })
        .collect();
    assert_eq!(xs.len(), 2, "one <text> per line:\n{out}");
    assert!(
        (xs[1] - xs[0] - 40.0).abs() < 0.01,
        "the second paragraph's line should start 40 further in: {xs:?}\n{out}"
    );
}

// --- images (§5.5a, §7.1) -------------------------------------------------

/// The fixture's rect (80×40) filled with a 200×200 picture in `fit`, and its
/// solid fill replaced so the only thing painting the interior is the image.
///
/// The bytes are not a real PNG and do not need to be: nothing in this writer
/// decodes them, which is the point of exporting the *encoded original* — they
/// go out base64'd exactly as the table holds them (`AQID` is `[1, 2, 3]`).
fn image_fixture(fit: ondin_core::ImageFit) -> common::Fixture {
    image_fixture_adjusted(fit, ondin_core::ImageAdjust::NEUTRAL)
}

/// `image_fixture` carrying adjustments as well as a framing.
fn image_fixture_adjusted(
    fit: ondin_core::ImageFit,
    adjust: ondin_core::ImageAdjust,
) -> common::Fixture {
    use ondin_core::{
        Fill, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource, Operation, Resolved,
        Transaction,
    };
    let mut f = common::fixture();
    let id = ImageId("pic".into());
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
                            fit,
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
    f
}

/// The six numbers of the first `patternTransform="matrix(…)"` in `out`.
fn pattern_matrix(out: &str) -> [f64; 6] {
    let at = out
        .find("patternTransform=\"matrix(")
        .expect("a patternTransform")
        + 25;
    let rest = &out[at..];
    let inside = &rest[..rest.find(')').expect("a closing paren")];
    let n: Vec<f64> = inside
        .split(' ')
        .map(|v| v.parse().expect("a number"))
        .collect();
    n.try_into().expect("six coefficients")
}

/// **An image fill exports as a `<pattern>` naming the encoded original**, where
/// it used to export as `fill="none"` — a picture in the file, in the viewBox and
/// invisible. That is a broken feature rather than a missing one, which is why
/// the image Export scope stayed open until all three writers drew a picture
/// (§7.1, §15 D185).
#[test]
fn an_image_fill_exports_as_a_pattern_over_the_encoded_original() {
    let f = image_fixture(ondin_core::ImageFit::Fill);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    let slot = format!("img-{}-fill-0", f.rect.to_wire().replace(':', "_"));

    assert!(out.contains(&format!(r#"<pattern id="{slot}""#)), "{out}");
    assert!(
        out.contains(r#"href="data:image/png;base64,AQID""#),
        "the stored bytes, base64'd, not a re-encode:\n{out}"
    );
    // ⚠️ **No ` none` fallback after the reference, and that was tried** (§15
    // D477): `[A5-L6-01]` reads the round-trip's black rectangle as the inherited
    // fill showing through an unresolvable `url(…)`, which the fallback would fix.
    // Measured in Chrome, `fill="url(#nope)"` and `fill="url(#nope) none"` paint
    // the same **0** black pixels, and this app's own reader answers *unpainted*
    // for either. `paint_attr`'s comment carries the measurement.
    assert!(
        out.contains(&format!(r##"fill="url(#{slot})""##)),
        "the shape has to name the pattern:\n{out}"
    );
    // The tile is the source's own pixel box, because that is the space the
    // framing transform maps *from*.
    assert!(
        out.contains(r#"width="200" height="200""#),
        "the pattern and its image are the picture's intrinsic size:\n{out}"
    );
}

/// **The mode lives in the `patternTransform`, and it is the shape's own box it
/// is computed against.** A 200×200 picture covering an 80×40 rect scales to 0.4
/// and hangs 20 units off each side — the same arithmetic `ImageRef::framing`
/// hands the canvas, so the export cannot frame a picture the renderer does not.
#[test]
fn the_framing_mode_is_carried_by_the_pattern_transform() {
    let f = image_fixture(ondin_core::ImageFit::Fill);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    let m = pattern_matrix(&out);
    assert_eq!(m, [0.4, 0.0, 0.0, 0.4, 0.0, -20.0], "cover:\n{out}");
}

/// **A contained picture is cut to its own rectangle**, which is the one thing
/// framing costs a layer for (§5.5a). SVG has no non-repeating pattern and peniko
/// has no transparent extend, so both writers have to cut the letterbox or the
/// band beside the picture fills with picture.
#[test]
fn a_letterboxing_fit_clips_the_band_beside_the_picture() {
    let f = image_fixture(ondin_core::ImageFit::Fit);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    let wire = f.rect.to_wire().replace(':', "_");

    assert_eq!(
        pattern_matrix(&out),
        [0.2, 0.0, 0.0, 0.2, 20.0, 0.0],
        "contain:\n{out}"
    );
    assert!(
        out.contains(&format!(
            r#"<clipPath id="ic-{wire}-fill-0"><rect x="20" y="0" width="40" height="40"/></clipPath>"#
        )),
        "the clip is the picture's rectangle inside the frame:\n{out}"
    );
    assert!(
        out.contains(&format!(r##"<g clip-path="url(#ic-{wire}-fill-0)">"##)),
        "and something has to be inside it:\n{out}"
    );
}

/// **A reference the table cannot answer for exports as the placeholder**, never
/// as a dangling `url(#…)`: SVG treats a reference to a paint server that is not
/// in the file as an error, where a grey box with a cross is a picture. Written
/// first as "exports as no paint", which is what both writers did until §15 D179
/// was answered — the agreement that matters is with the canvas, and the canvas
/// now draws something.
#[test]
fn an_image_with_no_table_entry_exports_as_the_placeholder() {
    use ondin_core::{Fill, ImageId, Operation, Resolved, Transaction};
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: f.rect,
            fills: vec![Fill {
                brush: ondin_core::image_brush(ImageId("missing".into())),
                visible: true,
            }],
        }]))
        .expect("a fill pointing at nothing");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);

    assert!(!out.contains("<pattern"), "nothing to define:\n{out}");
    assert!(!out.contains("url(#img-"), "no dangling reference:\n{out}");

    // The same three pieces the canvas paints, off the same `missing_placeholder`
    // — the ground on the shape's own element, then the rim and the cross inside
    // a clip of that shape, so a rounded rect stays rounded.
    let m = ondin_core::missing_placeholder(ondin_core::kurbo::Rect::new(0.0, 0.0, 80.0, 40.0))
        .expect("a placeholder for a real box");
    let [r, g, b] = [
        m.ground.to_rgba8().r,
        m.ground.to_rgba8().g,
        m.ground.to_rgba8().b,
    ];
    assert!(
        out.contains(&format!(r##"fill="#{r:02x}{g:02x}{b:02x}""##)),
        "the ground:\n{out}"
    );
    assert!(
        out.contains(&format!(
            r#"<clipPath id="mi-{}-fill-0">"#,
            f.rect.to_wire().replace(':', "_")
        )),
        "the shape as a clip:\n{out}"
    );
    assert!(
        out.contains("M0,0 L80,40 M80,0 L0,40"),
        "the cross, corner to corner of the shape's box:\n{out}"
    );
}

/// **Text is framed by its layout box, not by the ink.** The scene walk frames an
/// image-filled text node against `layout.bounds()`, so the words read as cut out
/// of one picture; this writer reaches for glyph outlines everywhere else, and
/// taking them here would frame the picture to the ink instead. Invisible unless
/// you already know the photograph.
#[test]
fn an_image_filled_text_node_is_framed_by_its_layout_box() {
    use ondin_core::{
        Fill, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource, Operation, Resolved,
        Transaction,
    };
    let mut f = common::fixture();
    let id = ImageId("pic".into());
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
                id: f.text,
                fills: vec![Fill {
                    brush: ondin_core::image_brush(id.clone()),
                    visible: true,
                }],
            },
        ]))
        .expect("fill the text with a picture");
    f.res = Resolved::rebuild(&f.doc);

    use ondin_core::kurbo::Shape as _;
    let layout = f.res.text_layout(f.text).expect("a shaped layout");
    let ink = ondin_core::text::outline(layout).bounding_box();
    let box_ = layout.bounds();
    // The fixture has to be in the state this is about: glyph ink is smaller than
    // the line box it sits in, or the two framings would agree and prove nothing.
    assert!(
        (ink.height() - box_.height()).abs() > 1.0,
        "ink {ink:?} and layout box {box_:?} are the same shape"
    );

    let r = ImageRef::new(id);
    let want = r.framing(box_, 200, 200).transform.as_coeffs();
    let wrong = r.framing(ink, 200, 200).transform.as_coeffs();
    let got = pattern_matrix(&ondin_export::svg::svg(&f.doc, &f.res, None));
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() < 0.01,
            "coefficient {i}: got {got:?}, want {want:?} (the ink's would be {wrong:?})"
        );
    }
}

/// **A picture sitting perfectly still writes no transform at all.** The case is
/// an ordinary one — an 800×600 photograph placed in an 800×600 frame — and the
/// export said `patternTransform="matrix(1 0 0 1 0 0)"`, an identity spelled out
/// longhand in the one file where every other number was exact. Reported from a
/// real export.
///
/// The rect here is the fixture's, whose corner radius is what makes this worth a
/// test rather than a glance: its outline's bounding box has a top of `-8.88e-16`,
/// so the affine is *not* `Affine::IDENTITY` and only the printed form is.
#[test]
fn a_picture_at_its_frames_own_size_writes_no_pattern_transform() {
    use ondin_core::{
        Fill, ImageEntry, ImageFormat, ImageId, ImageSource, Operation, Resolved, Transaction,
    };
    let mut f = common::fixture();
    let id = ImageId("exact".into());
    f.doc
        .apply(&Transaction(vec![
            Operation::AddImage {
                id: id.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(vec![1, 2, 3].into()),
                    format: ImageFormat::Png,
                    // The fixture's rect, exactly.
                    width: 80,
                    height: 40,
                },
            },
            Operation::SetFills {
                id: f.rect,
                fills: vec![Fill {
                    brush: ondin_core::image_brush(id),
                    visible: true,
                }],
            },
        ]))
        .expect("a picture the size of its frame");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);

    assert!(
        out.contains("<pattern"),
        "the picture is still there:\n{out}"
    );
    assert!(
        !out.contains("patternTransform"),
        "an identity written out longhand:\n{out}"
    );
}

/// **A frame's ground is a fill like any other, and it exported a hole.**
///
/// The walk sends a frame's fills through the same `fill_or_placeholder` a shape's
/// go through, so a frame whose picture had gone drew the placeholder on canvas
/// and wore the amber glyph in the panel — while this writer sent the ground
/// through `paint_attr` alone and wrote `fill="none"`. Found by reading the two
/// against each other rather than by a failing test, which is why there is now a
/// test (§15 D179).
///
/// ⚠️ **The clip id is `mi-{id}-fill-0` and was `mi-{id}-bg`** (§15 D400). That
/// string is not decoration: it is the slot name `emit_node` and `collect_defs`
/// have to agree on, and it changed because a frame's ground stopped being "the
/// background" and became fill 0. Asserting the literal is what would catch the
/// two sides drifting apart — a `<pattern>` registered under one name and
/// referenced under another writes a shape filled with nothing.
#[test]
fn a_frames_missing_background_exports_the_placeholder_too() {
    use ondin_core::{Fill, ImageId, Operation, Resolved, Transaction};
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: f.artboard,
            fills: vec![Fill {
                brush: ondin_core::image_brush(ImageId("gone".into())),
                visible: true,
            }],
        }]))
        .expect("a frame background pointing at nothing");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);

    assert!(
        out.contains(&format!(
            r#"<clipPath id="mi-{}-fill-0">"#,
            f.artboard.to_wire().replace(':', "_")
        )),
        "the frame's own box as a clip:\n{out}"
    );
    // The fixture's frame is 400×300, so the cross runs its diagonals.
    assert!(
        out.contains("M0,0 L400,300 M400,0 L0,300"),
        "the cross across the frame:\n{out}"
    );
}

// --- image adjustments (§5.5a) ---------------------------------------------

/// The numbers of the first attribute of this name in `out`, space-separated.
fn numbers(out: &str, attr: &str) -> Vec<f32> {
    let at = out
        .find(attr)
        .unwrap_or_else(|| panic!("no {attr} in:\n{out}"))
        + attr.len();
    let rest = &out[at..];
    rest[..rest.find('"').expect("a closing quote")]
        .split_whitespace()
        .map(|v| v.parse().expect("a number"))
        .collect()
}

/// **An untouched picture exports exactly the markup it did before adjustments
/// existed** — no filter, no reference to one.
///
/// The overwhelming majority of pictures are untouched, so this is what the
/// feature costs every document that does not use it. A version that emitted an
/// identity filter would be correct on screen and would put a `<filter>` and 53
/// numbers into every export with a photograph in it.
#[test]
fn an_unadjusted_picture_exports_no_filter_at_all() {
    let f = image_fixture(ondin_core::ImageFit::Fill);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    assert!(!out.contains("<filter"), "{out}");
    assert!(!out.contains("filter=\"url("), "{out}");
}

/// **An adjusted picture exports a `<filter>` the `<image>` names**, with the one
/// attribute whose absence would be a silent, total mismatch.
///
/// `color-interpolation-filters="sRGB"` is not decoration: a filter's default
/// working space is *linear* RGB, so without it a viewer linearises the picture,
/// runs the matrix, and converts back — a visibly different exposure and a
/// visibly different saturation from the canvas, on every adjusted image in the
/// file, with nothing in the markup looking wrong.
#[test]
fn an_adjusted_picture_exports_a_filter_the_image_names() {
    let f = image_fixture_adjusted(
        ondin_core::ImageFit::Fill,
        ondin_core::ImageAdjust {
            exposure: 0.4,
            shadows: 0.6,
            ..ondin_core::ImageAdjust::NEUTRAL
        },
    );
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    let name = format!("adj-{}-fill-0", f.rect.to_wire().replace(':', "_"));

    assert!(out.contains(&format!(r#"<filter id="{name}""#)), "{out}");
    assert!(
        out.contains(r#"color-interpolation-filters="sRGB""#),
        "without this a viewer works in linear RGB and draws a different picture:\n{out}"
    );
    // The region is the element's own box. SVG's default is 10% larger on each
    // side, and inside a `<pattern>` that lets one tile's filter output bleed into
    // its neighbours — invisible on a `Fill`, which shows one tile, and a grid of
    // seams on a `Tile`.
    assert!(
        out.contains(r#"x="0" y="0" width="1" height="1""#),
        "the filter region has to be pinned to the element's box:\n{out}"
    );
    assert!(
        out.contains(&format!(r##"filter="url(#{name})""##)),
        "the <image> inside the pattern has to name it:\n{out}"
    );
    // On the picture, not on the pattern: a filter on the pattern would colour
    // whatever the pattern ends up filling rather than the photograph.
    let img_at = out.find("<image ").expect("an image element");
    let filter_at = out.find("filter=\"url(").expect("a filter reference");
    assert!(
        filter_at > img_at,
        "the filter is hung on the wrong element:\n{out}"
    );
    // Three channels, and deliberately no alpha: a shadow lift running on alpha
    // would turn a soft edge opaque.
    for ch in ["R", "G", "B"] {
        assert!(
            out.contains(&format!("<feFunc{ch} type=\"table\"")),
            "{out}"
        );
    }
    assert!(!out.contains("feFuncA"), "{out}");
}

/// **Each stage is emitted only when it does something**, so an exposure nudge
/// does not carry a 33-number tone curve it does not bend, and a highlight
/// recovery does not carry an identity matrix.
#[test]
fn a_filter_carries_only_the_stages_that_are_not_the_identity() {
    let f = image_fixture_adjusted(
        ondin_core::ImageFit::Fill,
        ondin_core::ImageAdjust {
            saturation: -1.0,
            ..ondin_core::ImageAdjust::NEUTRAL
        },
    );
    let matrix_only = ondin_export::svg::svg(&f.doc, &f.res, None);
    assert!(matrix_only.contains("feColorMatrix"), "{matrix_only}");
    assert!(
        !matrix_only.contains("feComponentTransfer"),
        "saturation bends no tone curve:\n{matrix_only}"
    );

    let f = image_fixture_adjusted(
        ondin_core::ImageFit::Fill,
        ondin_core::ImageAdjust {
            highlights: -0.5,
            ..ondin_core::ImageAdjust::NEUTRAL
        },
    );
    let curve_only = ondin_export::svg::svg(&f.doc, &f.res, None);
    assert!(curve_only.contains("feComponentTransfer"), "{curve_only}");
    assert!(
        !curve_only.contains("feColorMatrix"),
        "highlights are not a colour matrix:\n{curve_only}"
    );
}

/// **The numbers in the file are the numbers the canvas paints with**, read back
/// out of the markup and run through SVG's own definition of the two primitives.
///
/// This is the assertion the whole design exists to make possible, and it is not
/// tautological: it catches the two mistakes that are invisible in both the code
/// and the picture. A 4×5 matrix written **column-major** is still twenty
/// plausible numbers and produces a completely different colour; a table written
/// backwards is still thirty-three monotonic ones. Neither shows up in a test
/// that only counts them.
///
/// Deliberately an adjustment that is **not symmetric in the channels** — a
/// temperature and a tint together — because a grey-preserving matrix transposes
/// to something very close to itself, and a saturation-only probe would pass
/// against the transpose.
#[test]
fn the_filters_numbers_paint_the_picture_the_canvas_paints() {
    let adjust = ondin_core::ImageAdjust {
        exposure: 0.3,
        temperature: 0.6,
        tint: -0.4,
        saturation: 0.5,
        contrast: 0.2,
        highlights: -0.35,
        shadows: 0.25,
    };
    let f = image_fixture_adjusted(ondin_core::ImageFit::Fill, adjust);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);

    let m = numbers(&out, r#"values=""#);
    assert_eq!(m.len(), 20, "a 4×5 colour matrix:\n{out}");
    let table = numbers(&out, r#"tableValues=""#);
    assert_eq!(table.len(), ondin_core::TONE_STEPS, "a tone curve:\n{out}");

    // A viewer, spelled out: row-major matrix over straight RGBA, clamped, then
    // the table on each channel by linear interpolation.
    let viewer = |rgb: [f32; 3]| -> [f32; 3] {
        let [r, g, b] = rgb;
        let after = [0usize, 1, 2].map(|row| {
            (m[row * 5] * r + m[row * 5 + 1] * g + m[row * 5 + 2] * b + m[row * 5 + 4])
                .clamp(0.0, 1.0)
        });
        after.map(|v| ondin_core::transfer(&table, v))
    };
    let pipeline = adjust.pipeline().expect("seven moved sliders");
    for probe in [
        [0.1f32, 0.2, 0.9],
        [0.8, 0.4, 0.1],
        [0.5, 0.5, 0.5],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    ] {
        let (ours, theirs) = (pipeline.apply(probe), viewer(probe));
        for c in 0..3 {
            assert!(
                (ours[c] - theirs[c]).abs() < 2e-3,
                "the file and the canvas disagree about {probe:?}: \
                 canvas {ours:?}, file {theirs:?}"
            );
        }
    }
    // The alpha row is the identity and nothing writes into it, so the filter
    // cannot change a picture's transparency.
    assert_eq!(&m[15..20], &[0.0, 0.0, 0.0, 1.0, 0.0]);
}

/// **A frame's background and a stroke are adjusted too**, which is the half of
/// this rule that §15 D179 had to be told twice.
///
/// The missing-image placeholder shipped covering a shape's fills and missing an
/// artboard's *background*, because the two reach this writer by different
/// routes. §15 D179's closing lesson — "one drawing" is a claim about every *call
/// site* of it, and the two writers each have their own list of them — predicts
/// that adjustments have the same shape of problem. They do not, and this is why:
/// the filter is collected in
/// `collect_defs`'s one `consider` closure, which fills, strokes and the
/// background all pass through — so it is structurally one call site rather than
/// three kept in step. Asserted anyway, because "structurally" is the kind of
/// claim that stops being true quietly.
#[test]
fn an_adjustment_reaches_a_stroke_and_a_frames_background_as_well_as_a_fill() {
    use ondin_core::{
        Fill, ImageAdjust, ImageBrush, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource,
        Operation, Resolved, Stroke, Transaction,
    };
    let mut f = common::fixture();
    let id = ImageId("pic".into());
    let brush = || {
        ondin_core::Brush::Image(ImageBrush {
            image: ImageRef {
                adjust: ImageAdjust {
                    saturation: -1.0,
                    ..ImageAdjust::NEUTRAL
                },
                ..ImageRef::new(id.clone())
            },
            sampler: Default::default(),
        })
    };
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
                id: f.artboard,
                fills: vec![Fill {
                    brush: brush(),
                    visible: true,
                }],
            },
            Operation::SetStrokes {
                id: f.rect,
                strokes: vec![Stroke {
                    brush: brush(),
                    width: 6.0,
                    ..Default::default()
                }],
            },
        ]))
        .expect("a picture behind the frame and around the rect");
    f.res = Resolved::rebuild(&f.doc);
    let out = ondin_export::svg::svg(&f.doc, &f.res, None);

    // `fill-0`, not `bg`: a frame's ground is fill 0 of its own list now (§15 D400),
    // and this string is the name `collect_defs` registers the filter under and
    // `emit_node` references it by. The pair has to agree or the picture exports
    // unadjusted.
    for slot in [
        format!("adj-{}-fill-0", f.artboard.to_wire().replace(':', "_")),
        format!("adj-{}-stroke-0", f.rect.to_wire().replace(':', "_")),
    ] {
        assert!(
            out.contains(&format!(r#"<filter id="{slot}""#)),
            "no filter for {slot}:\n{out}"
        );
        assert!(
            out.contains(&format!(r##"filter="url(#{slot})""##)),
            "{slot} is defined and never named:\n{out}"
        );
    }
}

/// **A file that says nothing about skip-ink now gets the picture the canvas
/// drew**, which is the exact reverse of what this test asserted when it was
/// written, and the reversal is the point of keeping it.
///
/// CSS's initial value is `auto` and browsers implement that as breaking an
/// underline around every descender it crosses. Until 2026-08-25 ours did not
/// break, so the writer said `none` out loud (§15 D281) — the one property here
/// whose *absence* was a statement. `text::skip_ink` now takes the crossing ink
/// out of every underline band before the renderer fills it (§15 D356), so the
/// initial value is the value we want and writing anything would be the noise
/// `text-decoration-style`'s `solid` is.
///
/// **This test is why that flip could not be half-made.** Its whole job, said in
/// the roadmap entry that filed it, was to go red the day the renderer learned to
/// skip and the writer had not been told — so the assertions below are the same
/// three arms, inverted, and the fixture checks that go with them matter more than
/// before: an absence is also what a misspelled attribute name looks like, so each
/// arm proves the writer reached the decoration path at all.
///
/// ⚠️ **It pins the writer's agreement with the renderer, not the renderer.** No
/// SVG assertion can see a gap — the band is still a `text-decoration` property
/// and not a path. `ondin_core::text::tests::an_underlined_descender_breaks_the_band`
/// is the other half of the pair.
///
/// ⚠️ Flipped by restoring the `push_str`, which fails at the first assertion. The
/// *interesting* flip is narrower: making the write unconditional instead — the old
/// code guarded it on `underline.is_some()` — still fails the first arm, so this
/// test alone never proved that guard did anything, and the strikethrough arm below
/// is what did.
///
/// ⚠️ **The silence is now conditional, and this test is the `auto` half of a pair**
/// (§15 D357). Every arm here leaves `Decoration::skip_ink` at its default, which is
/// what makes them the ordinary case; the run that switches it off writes the
/// attribute again, and `an_underline_that_switched_skipping_off_says_so` below is
/// where that is asserted. Neither test is enough alone — this one going green for a
/// writer that had lost the attribute entirely is exactly what that one catches.
#[test]
fn an_exported_underline_leaves_skip_ink_to_its_initial_value() {
    use ondin_core::{Decoration, Operation, Resolved, TextStyle, Transaction};

    let decorated = |underline: Option<Decoration>, strikethrough: Option<Decoration>| {
        let mut f = common::fixture();
        f.doc
            .apply(&Transaction(vec![Operation::SetTextStyle {
                id: f.text,
                spans: None,
                style: TextStyle {
                    font_family: "Inter".into(),
                    font_size: 20.0,
                    underline,
                    strikethrough,
                    ..Default::default()
                },
            }]))
            .expect("decorate the text");
        f.res = Resolved::rebuild(&f.doc);
        ondin_export::svg::svg(&f.doc, &f.res, None)
    };
    let plain = Some(Decoration::default());

    let out = decorated(plain, None);
    assert!(
        out.contains(r#"text-decoration="underline""#),
        "the underline arm must actually decorate something:\n{out}"
    );
    assert!(
        !out.contains("text-decoration-skip-ink"),
        "the canvas skips now, so `auto` is what we want and silence is how CSS \
         spells it:\n{out}"
    );
    // A run carrying both: the underline half is skipped and the strikethrough
    // half is not, which is one behaviour CSS already has a name for and still
    // needs nothing written.
    let out = decorated(plain, plain);
    assert!(
        out.contains(r#"text-decoration="underline line-through""#),
        "the both arm must carry both:\n{out}"
    );
    assert!(
        !out.contains("text-decoration-skip-ink"),
        "a run with both says nothing about skipping either:\n{out}"
    );
    // And a strikethrough alone: skipping never applied to `line-through`, so this
    // arm is the one that was already silent and still is.
    let out = decorated(None, plain);
    assert!(
        out.contains(r#"text-decoration="line-through""#),
        "the strikethrough arm must actually decorate something:\n{out}"
    );
    assert!(
        !out.contains("text-decoration-skip-ink"),
        "line-through has no descenders to skip:\n{out}"
    );
}

/// **A run that switched skipping off says so, and it is the underline's word
/// alone** (§15 D357).
///
/// The other half of the pair above. `text-decoration-skip-ink="none"` is back —
/// the attribute D281 wrote unconditionally and D356 deleted — for exactly the runs
/// whose underline asked for a continuous bar, which is the one case where our
/// drawing and CSS's initial value disagree. It is also the only value of the two
/// that exports *exactly*: `none` is a whole band either way, where two `auto`
/// implementations gap in nearly-but-not-quite the same places.
///
/// **The strikethrough arm is the one that earns its place.** Skip-ink applies to
/// underlines and overlines and never to a `line-through` (css-text-decor-4), so a
/// strikethrough carrying the flag has to write nothing at all — the writer reads
/// `style.underline` and not the `agreed` value over both decorations, which is the
/// spelling the three properties above it would suggest and would be wrong here.
///
/// ⚠️ Flipped by dropping the `!` from the guard: fails here on the first arm's
/// skip-ink assertion — and takes the `auto` test above down with it, which is worth
/// knowing, because it means that pair of tests brackets the guard from both sides
/// and neither would survive the attribute being written for the wrong half of the
/// setting.
///
/// ⚠️ Flipped a second way, by asking `agreed(&each, |d| d.skip_ink) == Some(false)`
/// — the spelling the three element-scoped properties above would suggest: fails on
/// the **strikethrough** arm, which is the one this test has that the other does not.
/// The `auto` test stays green through that flip. So the arm justified two paragraphs
/// up is the arm that catches it, and it is the only one that does.
#[test]
fn an_underline_that_switched_skipping_off_says_so() {
    use ondin_core::{Decoration, Operation, Resolved, TextStyle, Transaction};

    let decorated = |underline: Option<Decoration>, strikethrough: Option<Decoration>| {
        let mut f = common::fixture();
        f.doc
            .apply(&Transaction(vec![Operation::SetTextStyle {
                id: f.text,
                spans: None,
                style: TextStyle {
                    font_family: "Inter".into(),
                    font_size: 20.0,
                    underline,
                    strikethrough,
                    ..Default::default()
                },
            }]))
            .expect("decorate the text");
        f.res = Resolved::rebuild(&f.doc);
        ondin_export::svg::svg(&f.doc, &f.res, None)
    };
    let unbroken = Some(Decoration {
        skip_ink: false,
        ..Decoration::default()
    });

    let out = decorated(unbroken, None);
    assert!(
        out.contains(r#"text-decoration="underline""#),
        "the underline arm must actually decorate something:\n{out}"
    );
    assert!(
        out.contains(r#"text-decoration-skip-ink="none""#),
        "the underline asked for a continuous bar, which CSS spells `none`:\n{out}"
    );
    // The control: the same fixture with the flag left alone writes nothing, so the
    // assertion above is about the flag and not about the writer having grown a
    // second unconditional attribute.
    let out = decorated(Some(Decoration::default()), None);
    assert!(
        !out.contains("text-decoration-skip-ink"),
        "the default is CSS's initial value and stays unwritten:\n{out}"
    );
    // A strikethrough carrying the flag: nothing to say, because skipping never
    // applied to a `line-through` in the renderer either.
    let out = decorated(None, unbroken);
    assert!(
        out.contains(r#"text-decoration="line-through""#),
        "the strikethrough arm must actually decorate something:\n{out}"
    );
    assert!(
        !out.contains("text-decoration-skip-ink"),
        "a line-through was never skipped, so switching it off is not news:\n{out}"
    );
    // And a run carrying both still says it, because the property is the
    // underline's: the strikethrough has no opinion to be outvoted by.
    let out = decorated(unbroken, Some(Decoration::default()));
    assert!(
        out.contains(r#"text-decoration-skip-ink="none""#),
        "the underline's word carries even beside a strikethrough:\n{out}"
    );
}

/// A mask becomes a `<clipPath>` and a `<g clip-path>` over the siblings above
/// it, and emits **no element of its own**.
///
/// The second half is the one a reader would not think to check and the one that
/// makes the export disagree with the canvas if it is wrong: a mask that also
/// wrote its own shape would paint over the very artwork it is meant to reveal,
/// and only in the exported file.
#[test]
fn a_mask_wraps_the_run_above_it_and_writes_no_shape_of_its_own() {
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{NodeKind, Operation, Resolved, Transaction};
    let mut f = common::fixture();
    let mut ids = ondin_core::IdSource::new(0x4D5C);
    let mask = ids.mint();
    // Below everything already on the board, so the whole rest of the fixture is
    // the run it governs.
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: mask,
            parent: f.artboard,
            index: 0,
            kind: NodeKind::Ellipse {
                size: Size::new(64.0, 48.0),
            },
            transform: Some(Affine::translate((7.0, 9.0))),
            name: Some("Masker".into()),
        }]))
        .unwrap();

    // 64×48 → `rx="32" ry="24"`, which nothing else in the fixture writes. The
    // fixture's *other* ellipse is 50×50, so "is there an `<ellipse>`" would be
    // answered by the wrong node in both directions.
    let own = r#"rx="32" ry="24""#;
    let plain = ondin_export::svg::svg(&f.doc, &Resolved::rebuild(&f.doc), None);
    assert!(
        plain.contains(own),
        "fixture: the layer draws itself until it is a mask:\n{plain}"
    );

    f.doc
        .apply(&Transaction(vec![Operation::SetMask {
            id: mask,
            mask: true,
        }]))
        .unwrap();
    let out = ondin_export::svg::svg(&f.doc, &Resolved::rebuild(&f.doc), None);

    let wire = mask.to_wire().replace(':', "_");
    assert!(
        out.contains(&format!(r#"<clipPath id="mask-{wire}">"#)),
        "the mask's outline is written as a clipPath:\n{out}"
    );
    let group = out
        .find(&format!(r#"<g clip-path="url(#mask-{wire})">"#))
        .unwrap_or_else(|| panic!("and referenced by a group over the run:\n{out}"));
    // The fixture's rect sits above the mask, so it has to be inside that group.
    let rect = out
        .find(r#"rx="6""#)
        .unwrap_or_else(|| panic!("the fixture's rounded rect is in the file:\n{out}"));
    assert!(
        rect > group,
        "the layers above the mask are written inside its group:\n{out}"
    );
    assert!(
        !out.contains(own),
        "and the mask itself draws nothing — it is a clip, not artwork:\n{out}"
    );
}

/// A mask inside a **clipping, stroked** frame nests three groups deep and every
/// one of them closes.
///
/// This is the only place in the writer where three `<g>`s stack — the frame's
/// outer group carrying transform and opacity, its clip group, and the mask's
/// over the run — and getting the order wrong produces markup that is still
/// *plausible* and no longer valid. Counted rather than read, because the failure
/// is a missing or misplaced `</g>` and eyeballing a nest is exactly how one gets
/// past review.
#[test]
fn a_mask_inside_a_clipping_stroked_frame_nests_and_closes() {
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{NodeKind, Operation, Resolved, Stroke, Transaction};
    let mut f = common::fixture();
    let mut ids = ondin_core::IdSource::new(0x3E57);
    let mask = ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: mask,
                parent: f.artboard,
                index: 0,
                kind: NodeKind::Ellipse {
                    size: Size::new(64.0, 48.0),
                },
                transform: Some(Affine::translate((7.0, 9.0))),
                name: None,
            },
            // Both, which is what forces the frame into two groups (§15 D144).
            Operation::SetClip {
                id: f.artboard,
                clip: true,
            },
            Operation::SetStrokes {
                id: f.artboard,
                strokes: vec![Stroke {
                    brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                        255, 0, 0, 255,
                    )),
                    width: 3.0,
                    ..Default::default()
                }],
            },
            Operation::SetMask {
                id: mask,
                mask: true,
            },
        ]))
        .unwrap();
    let out = ondin_export::svg::svg(&f.doc, &Resolved::rebuild(&f.doc), None);

    // Every `<g>` and `</g>` in this writer is on a line of its own.
    let mut depth = 0i32;
    let mut mask_depth = None;
    for line in out.lines() {
        let t = line.trim_start();
        if t.starts_with("</g>") {
            depth -= 1;
            assert!(depth >= 0, "a group closed that was never opened:\n{out}");
        } else if t.starts_with("<g") {
            if t.contains("url(#mask-") {
                mask_depth = Some(depth);
            }
            depth += 1;
        }
    }
    assert_eq!(depth, 0, "every group opened is closed:\n{out}");
    assert_eq!(
        mask_depth,
        Some(2),
        "the frame's outer group, then its clip group, then the mask's:\n{out}"
    );
}

/// **An alpha mask exports as a `<mask>` carrying the layer's own markup**, and
/// `mask-type: alpha` is the assertion that matters.
///
/// SVG's `<mask>` defaults to **luminance**. Without that property every alpha
/// mask we write would silently become the one mode this app deliberately does
/// not have (§15 D288) — and would *look* right whenever the mask happened to be
/// white, which is exactly the class of bug that ships. The element also has to
/// contain the mask's real drawing rather than its outline, since what masks is
/// the ink.
#[test]
fn an_alpha_mask_exports_as_a_mask_element_typed_alpha() {
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{MaskMode, NodeKind, Operation, Resolved, Transaction};
    let mut f = common::fixture();
    let mut ids = ondin_core::IdSource::new(0xA1FA);
    let mask = ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: mask,
                parent: f.artboard,
                index: 0,
                kind: NodeKind::Ellipse {
                    size: Size::new(64.0, 48.0),
                },
                transform: Some(Affine::translate((7.0, 9.0))),
                name: None,
            },
            Operation::SetMask {
                id: mask,
                mask: true,
            },
            Operation::SetMaskMode {
                id: mask,
                mode: MaskMode::Alpha,
            },
        ]))
        .unwrap();
    let out = ondin_export::svg::svg(&f.doc, &Resolved::rebuild(&f.doc), None);

    let wire = mask.to_wire().replace(':', "_");
    assert!(
        out.contains(&format!(
            r#"<mask id="mask-{wire}" maskUnits="userSpaceOnUse" x="0.6" y="4.2" width="76.8" height="57.6" style="mask-type:alpha">"#
        )),
        "a <mask>, typed alpha — the default is luminance, which we do not have — \
         carrying its own region, whose default is worse (§15 D458). The ellipse is \
         64×48 at (7, 9); the region is that box inflated a tenth:\n{out}"
    );
    assert!(
        !out.contains(&format!(r#"<clipPath id="mask-{wire}">"#)),
        "and not a clipPath, which is the other mode's element:\n{out}"
    );
    let mask_open = out
        .find(&format!(r#"<mask id="mask-{wire}""#))
        .expect("the mask element");
    let mask_close = out[mask_open..]
        .find("</mask>")
        .map(|i| mask_open + i)
        .expect("it closes");
    assert!(
        out[mask_open..mask_close].contains(r#"rx="32" ry="24""#),
        "the mask element holds the layer's own drawing, not its outline:\n{out}"
    );
    assert!(
        out.contains(&format!(r#"<g mask="url(#mask-{wire})">"#)),
        "and the run above it is wrapped in a group referencing it:\n{out}"
    );
}

/// **An exclusion exports `fill-rule="evenodd"`, and without it the file is a
/// union** (§15 D239).
///
/// ⚠️ **Written because removing the attribute broke nothing.** The rule was plumbed
/// through the model, the canvas, the raster path and the hit test, each with a test
/// that bites — and the SVG writer's line was flipped out and the whole suite stayed
/// green. An `Exclude`'s outline is its operands concatenated, so a browser reading
/// the exported file non-zero fills the overlaps solid: the shape on the page and the
/// shape in the file would be *different pictures*, which is the one divergence §3
/// exists to prevent, and nothing anywhere would have said so.
///
/// Two circles rather than a ring, because this is about one attribute in the markup
/// and the smallest fixture that has an overlap to get wrong is two.
#[test]
fn an_exclusion_exports_the_even_odd_rule() {
    use ondin_core::kurbo::{Affine, Point, Size};
    use ondin_core::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};

    let mut ids = IdSource::new(0x0E0D);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (a, b) = (ids.mint(), ids.mint());
    let mut ops = Vec::new();
    for (id, x) in [(a, 0.0), (b, 60.0)] {
        ops.push(Operation::CreateNode {
            id,
            parent: root,
            index: if id == a { 0 } else { 1 },
            kind: NodeKind::Ellipse {
                size: Size::new(100.0, 100.0),
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: None,
        });
    }
    doc.apply(&Transaction(ops)).expect("two circles");
    let (tx, node) =
        ondin_core::build::boolean(&doc, &mut ids, &[a, b], ondin_core::BoolOp::Exclude, None)
            .expect("an exclusion");
    doc.apply(&tx).expect("apply");
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: node,
        fills: vec![ondin_core::Fill {
            brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(0, 0, 0)),
            visible: true,
        }],
    }]))
    .expect("a fill to write the rule beside");

    let res = Resolved::rebuild(&doc);
    let out = svg(&doc, &res, None);
    assert!(
        out.contains(r#"fill-rule="evenodd""#),
        "an exclusion read non-zero is a union — the file must say which:\n{out}"
    );
    // And the default still writes nothing, so ordinary artwork keeps its bytes.
    let plain_doc = {
        let mut d = Document::new(root);
        let c = ids.mint();
        d.apply(&Transaction(vec![Operation::CreateNode {
            id: c,
            parent: root,
            index: 0,
            kind: NodeKind::Ellipse {
                size: Size::new(10.0, 10.0),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
        d
    };
    let plain_res = Resolved::rebuild(&plain_doc);
    assert!(
        !svg(&plain_doc, &plain_res, None).contains("fill-rule"),
        "and a shape at the default adds no attribute"
    );

    // ⚠️ **`[S8.1-L2-02]`: the same exclusion, on the *other* writing arm.**
    // `emit_shape` writes a shape as one element only while it has at most one
    // fill, no alignment clip and no letterbox; past any of those it goes through
    // `emit_stacked`, which built its fill attribute without the rule. So whether
    // an exclusion exported as an exclusion depended on how many paints it
    // happened to have — and giving one an inside or outside stroke, an ordinary
    // thing to do, made the file read as a **union** while the canvas went on
    // drawing it correctly.
    //
    // The assertions above exercise the single-element arm alone: one fill, no
    // stroke. Two ways onto the other arm are checked here because they are
    // different conditions in `emit_shape`'s `plain` — a second fill, and an
    // aligned stroke — and a fix inside one branch would satisfy only one.
    //
    // Flip-check, run: dropping `with_fill_rule` from `emit_stacked`'s loop
    // fails at *"a second fill must not change what the outline means"*. ⚠️ **And
    // the run was repeated with that first assertion disabled**, because a test
    // that aborts on its first case says nothing about its second: the stroke
    // case then fails on its own at *"nor must an aligned stroke"*. Both
    // conditions really do reach `emit_stacked`, which is the thing a single
    // red run here could not have told anyone.
    let stacked = |extra: Transaction| {
        let mut d = doc.clone();
        d.apply(&extra).expect("the stacked arm's condition");
        let r = Resolved::rebuild(&d);
        svg(&d, &r, None)
    };
    let black = ondin_core::Fill {
        brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(0, 0, 0)),
        visible: true,
    };
    let two_fills = stacked(Transaction(vec![Operation::SetFills {
        id: node,
        fills: vec![black.clone(), black],
    }]));
    assert!(
        two_fills.contains(r#"fill-rule="evenodd""#),
        "a second fill must not change what the outline means:\n{two_fills}"
    );
    let with_stroke = stacked(Transaction(vec![Operation::SetStrokes {
        id: node,
        strokes: vec![ondin_core::Stroke {
            brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(0, 0, 0)),
            width: 4.0,
            align: ondin_core::StrokeAlign::Inside,
            ..Default::default()
        }],
    }]));
    assert!(
        with_stroke.contains(r#"fill-rule="evenodd""#),
        "nor must an aligned stroke:\n{with_stroke}"
    );
    let _ = Point::ZERO;
}

/// **A boolean the arithmetic gave up on exports as the placeholder, not as
/// nothing** (§15 D298) — the second half of §5.5a's rule that a layer which
/// cannot be drawn is never a silent blank, and the reason the drawing lives in
/// `missing_placeholder` rather than in either writer.
///
/// This is the end of the whole chain in one assertion: `boolean::failures` moved
/// while `Resolved` was evaluating, `boolean_failed` recorded it, `boolean_placeholder`
/// turned it into the operands' box, and the writer put the same three pieces in
/// markup that `scene::paint_shape` puts on the canvas. Nothing else in the suite
/// crosses all four crates' worth of it.
///
/// The fixture is the forty-circle ring the module docs record, built out of
/// `Ellipse` nodes — and it asserts it *is* the failing one before asserting
/// anything about the output, because a ring that quietly stopped tripping the
/// upstream defect would leave this test green and about nothing.
#[test]
fn a_boolean_the_arithmetic_gave_up_on_exports_as_the_placeholder() {
    use ondin_core::kurbo::{Affine, Point, Size};
    use ondin_core::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};

    let mut ids = IdSource::new(0x0B00);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let b = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: b,
        parent: root,
        index: 0,
        kind: NodeKind::Boolean {
            op: ondin_core::BoolOp::Exclude,
        },
        transform: None,
        name: None,
    }];
    for i in 0..40 {
        let a = f64::from(i) / 40.0 * std::f64::consts::TAU;
        let c = Point::new(200.0 + 120.0 * a.cos(), 200.0 + 120.0 * a.sin());
        ops.push(Operation::CreateNode {
            id: ids.mint(),
            parent: b,
            index: i as usize,
            kind: NodeKind::Ellipse {
                size: Size::new(180.0, 180.0),
            },
            transform: Some(Affine::translate((c.x - 90.0, c.y - 90.0))),
            name: None,
        });
    }
    doc.apply(&Transaction(ops)).expect("build the ring");

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    // **Injected since flo_curves 0.8.1 fixed the defect this ring used to trip**
    // (`boolean::poison_next`, §15 D239). The assertion below is unchanged: it is
    // what says the fixture reached the failed state before any markup is read.
    ondin_core::boolean::poison_next(1);
    let res = Resolved::rebuild(&doc);
    std::panic::set_hook(hook);
    assert!(
        res.boolean_failed(b),
        "the ring has to be in the failed state, or this test is about nothing"
    );

    let out = svg(&doc, &res, None);
    let area = ondin_core::boolean_placeholder(&doc, &res, b).expect("a box to draw in");
    assert!(
        (area.x0 + 10.0).abs() < 1.0 && (area.x1 - 410.0).abs() < 1.0,
        "the operands' union, which is the only extent left: {area:?}"
    );
    let m = ondin_core::missing_placeholder(area).expect("a placeholder for a real box");
    let rgba = m.ground.to_rgba8();
    assert!(
        out.contains(&format!(
            r##"fill="#{:02x}{:02x}{:02x}""##,
            rgba.r, rgba.g, rgba.b
        )),
        "the ground, and it is the *same* grey a missing picture gets:\n{out}"
    );
    assert!(
        out.contains(&format!(
            r#"<clipPath id="mb-{}">"#,
            b.to_wire().replace(':', "_")
        )),
        "the box as a clip, so the rim and the cross stop at its edge:\n{out}"
    );
    // The rim and the cross: two stroked paths in the placeholder's ink. Counted
    // rather than matched against coordinates, because the box comes off flattened
    // ellipses and pinning its digits here would make this a test of kurbo's
    // tolerance. The ground above is what pins the colour; this pins that both
    // pieces of ink are drawn, which is the half a "wrote the clip and nothing in
    // it" mistake would lose.
    let ink = m.ink.to_rgba8();
    let ink = format!(r##"stroke="#{:02x}{:02x}{:02x}""##, ink.r, ink.g, ink.b);
    assert_eq!(
        out.matches(&ink).count(),
        2,
        "the rim and the cross, both in the placeholder's ink:\n{out}"
    );

    // The control: the ordinary drawing is *not* also emitted. The placeholder
    // replaces the node's paint rather than sitting over it, and a boolean with no
    // outline would otherwise write an element with no `d` at all.
    let (doc, res) = {
        let mut d = doc;
        d.apply(&Transaction(vec![Operation::SetGeometry {
            id: b,
            geometry: ondin_core::GeometryPatch::BoolOp(ondin_core::BoolOp::Union),
        }]))
        .expect("switch it to a union");
        let r = Resolved::rebuild(&d);
        (d, r)
    };
    assert!(!res.boolean_failed(b), "the same ring unions cleanly");
    let out = svg(&doc, &res, None);
    assert!(
        !out.contains(r#"<clipPath id="mb-"#),
        "a boolean that worked draws its shape and no placeholder:\n{out}"
    );
}

/// Put `effects` on the fixture's rect and export it alone.
fn with_effects(effects: Vec<Effect>) -> String {
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: f.rect,
            effects,
        }]))
        .unwrap();
    f.res = Resolved::rebuild(&f.doc);
    svg_of(&f.doc, &f.res, &[f.rect])
}

fn shadow(blur: f64, dy: f64) -> Shadow {
    Shadow {
        offset: Vec2::new(0.0, dy),
        blur,
        spread: 0.0,
        ..Shadow::default()
    }
}

/// **The viewBox has to grow to hold the shadow**, which is the visible half of
/// why `world_bounds` and `ink_bounds` are two questions (§5.3a).
///
/// The fixture's rect is 80×40 at world (30, 30) **and carries a 2px centred
/// stroke**, so its editing box is already 29…71 rather than 30…70. A shadow 4
/// down with a 12 blur has a reach of `3 × 6 = 18` and an offset of 4, so it
/// escapes **14 above and 22 below** — the asymmetry `EffectKind::escape` exists
/// to produce, and the thing most likely to be written as one number.
///
/// ⚠️ Flipped against `extent` reading `world_bounds`, where the filter is still
/// emitted, still referenced and still correct — and the file crops it at the
/// layer's own edge. Every assertion but the last two passes in that version,
/// which is why the numbers are here rather than a `contains("<filter")`.
///
/// (The stroke is also why the numbers are written as offsets from the measured
/// box rather than as literals: this test failed first time because the box was
/// assumed to be the geometry, and a test that agrees with a wrong assumption is
/// how a fixture stops describing the thing it is named after.)
#[test]
fn an_exported_shadow_is_inside_the_view_box_rather_than_cropped_at_the_layer() {
    let out = with_effects(vec![Effect::new(EffectKind::DropShadow(shadow(12.0, 4.0)))]);
    assert!(
        out.contains("<filter id=\"fx-"),
        "a filter is emitted:\n{out}"
    );
    assert!(
        out.contains("<g filter=\"url(#fx-"),
        "and something references it:\n{out}"
    );
    let view = out
        .split("viewBox=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("a viewBox");
    let n: Vec<f64> = view.split(' ').map(|v| v.parse().unwrap()).collect();
    // The unshadowed box, measured rather than assumed.
    let plain = with_effects(vec![]);
    let pv = plain
        .split("viewBox=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("a viewBox");
    let p: Vec<f64> = pv.split(' ').map(|v| v.parse().unwrap()).collect();
    assert_eq!(p[1] - n[1], 14.0, "reach 18 less the 4 of offset: {view}");
    assert_eq!(
        (n[1] + n[3]) - (p[1] + p[3]),
        22.0,
        "reach 18 plus the 4 of offset: {view}"
    );
    assert_eq!(p[0] - n[0], 18.0, "and the reach alone sideways: {view}");
}

/// ⚠️ **Every shadow is cast from the layer, not from the effect above it.**
///
/// The one-fold version — each effect reading whatever the previous produced — is
/// the natural way to write this and puts `in="fx0"` on the inner shadow's alpha
/// extraction, so it takes its silhouette from the layer *plus the drop shadow*
/// and draws a second inner edge along the outside of the shadow. The markup is
/// well-formed either way and both files open; only the picture differs.
///
/// Asserted on `SourceGraphic` appearing twice as a filter input, which is the
/// shape of the claim rather than a proxy for it.
#[test]
fn two_shadows_are_both_cast_from_the_layer_itself() {
    let out = with_effects(vec![
        Effect::new(EffectKind::DropShadow(shadow(12.0, 4.0))),
        Effect::new(EffectKind::InnerShadow(shadow(6.0, 2.0))),
    ]);
    // The alpha extractions specifically — counting every `in="SourceGraphic"`
    // in the file also catches the merge node that stacks the layer between the
    // two shadows, which is a different claim and answers 3.
    let casts = out
        .lines()
        .filter(|l| l.contains("<feColorMatrix") && l.contains("in=\"SourceGraphic\""))
        .count();
    assert_eq!(casts, 2, "each shadow's silhouette is the layer's:\n{out}");
    // And the composite order is drop, layer, inner — bottom to top.
    let merge = out
        .split("<feMerge>")
        .nth(1)
        .and_then(|s| s.split("</feMerge>").next())
        .expect("a merge");
    let order: Vec<&str> = merge
        .lines()
        .filter_map(|l| l.split("in=\"").nth(1)?.split('"').next())
        .collect();
    assert_eq!(order.len(), 3, "three layers merged: {merge}");
    assert_eq!(order[1], "SourceGraphic", "the layer sits between them");
}

/// **An inner shadow's filter region has to be bigger than the layer, or the
/// inversion has nothing to invert** — the export half of `[S10.2-L1-01]`,
/// §15 D742.
///
/// Outside the filter region SVG defines every primitive's result as transparent
/// black, so a region set to the layer's own ink bounds leaves
/// `feComponentTransfer tableValues="1 0"` nothing outside the shape to invert
/// from. On a rect that is the whole region and the export draws **nothing** —
/// the same defect, and the same picture, as the canvas bug this pairs with.
///
/// ⚠️ **Measured in a real browser rather than read off the spec**, on a
/// hand-written pair of filters differing only in this rectangle: region at the
/// shape gives `(255, 255, 255, 255)` at every sample down from the edge, region
/// padded gives 165, 187, 207, 222, 234, 242, 248, 251, 253, 254 — **the same ten
/// bytes `ondin-render`'s `an_inner_shadow_on_a_rect_reaches_all_four_edges`
/// reads off our own renderer.** Two independent implementations agreeing to the
/// byte is a better check on the repair than either one alone.
///
/// ⚠️ **The eleventh sample — the boundary row — does not agree**, 172 against
/// our 140, which is the shape's own antialiased edge under two rasterizers
/// rather than anything the filter does. It is recorded because a first attempt
/// at this sentence said *"byte for byte"* over a run that started one pixel in,
/// and the sample it omitted is the only one that disagrees.
///
/// The fixture's rect is 80×40 at world (30, 30) with a 2px centred stroke, so
/// its box is 29…111 by 29…71 and the unpadded region was exactly that. Asserted
/// on the region being *outside* the box rather than on the pad's own value,
/// which is `inner_working_pad`'s business and deliberately generous.
///
/// ⚠️ Flipped by dropping the `+ Insets::uniform(...)` in `effect_region`, where
/// `x` comes back to 29 and this fails on the first half of the condition.
#[test]
fn an_inner_shadows_filter_region_leaves_room_to_invert_the_silhouette() {
    let out = with_effects(vec![Effect::new(EffectKind::InnerShadow(shadow(6.0, 2.0)))]);
    let f = out
        .lines()
        .find(|l| l.contains("<filter "))
        .expect("a filter");
    let read = |k: &str| -> f64 {
        f.split(&format!("{k}=\""))
            .nth(1)
            .and_then(|s| s.split('"').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("{k} in {f}"))
    };
    assert!(
        read("x") < 29.0 && read("width") > 82.0,
        "the region is padded past the layer's own 29…111 box: {f}\n{out}"
    );
}

/// A stack that cannot change a pixel must leave the file exactly as it was.
///
/// ⚠️ **The failure this guards is not a missing effect, it is a missing layer.**
/// An SVG `<filter>` containing no primitives renders its element as *nothing at
/// all*, so the obvious implementation — emit the filter, skip the entries that
/// do nothing — deletes the artwork. Adding an effect row and switching it off is
/// an ordinary thing to do.
#[test]
fn a_stack_that_draws_nothing_leaves_the_markup_untouched() {
    let plain = with_effects(vec![]);
    assert!(!plain.contains("fx-"), "no stack, no filter:\n{plain}");
    for (label, stack) in [
        (
            "hidden",
            vec![Effect {
                kind: EffectKind::DropShadow(shadow(12.0, 4.0)),
                visible: false,
            }],
        ),
        (
            "neutral filters",
            vec![Effect::new(EffectKind::Filters(Filters::default()))],
        ),
        (
            "zero blur",
            vec![Effect::new(EffectKind::LayerBlur { radius: 0.0 })],
        ),
    ] {
        let out = with_effects(stack);
        assert_eq!(
            out, plain,
            "{label}: a stack with no ink must not change the file"
        );
    }
}

/// **A clipping frame's shadow falls outside the frame**, which SVG makes easy to
/// get wrong: `filter` is applied *before* `clip-path` on the same element, so an
/// attribute on the frame's own group would have the shadow drawn and then cut
/// off at the frame's edge — the one place a shadow most obviously must escape.
///
/// Asserted structurally: the filter group has to *open before* the element
/// carrying the clip. Flipped against putting the attribute on the frame's own
/// `<g>`, where both strings are still present and the order is reversed.
#[test]
fn a_clipping_frames_filter_wraps_its_clip_rather_than_sharing_an_element() {
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: f.artboard,
            effects: vec![Effect::new(EffectKind::DropShadow(shadow(10.0, 5.0)))],
        }]))
        .unwrap();
    f.res = Resolved::rebuild(&f.doc);
    let out = svg(&f.doc, &f.res, Some(f.artboard));
    let fx = out.find("<g filter=\"url(#fx-").expect("a filter group");
    let clip = out.find("clip-path=\"url(#clip-").expect("a clip");
    assert!(
        fx < clip,
        "the filter must open outside the clip, not on it:\n{out}"
    );
}

/// **A frame writes its whole fill stack, in list order, before its children**
/// (§15 D400) — the markup half of
/// `render::a_frames_fill_stack_paints_in_order_behind_its_children`, and the pair
/// is the point: this writer and the scene walk are two implementations of one
/// drawing, so a stack that stacks on the canvas and flattens in the file is
/// exactly the kind of divergence goldens alone would not name.
///
/// `emit_node` used to write **one** `<rect>` here, out of an `if let
/// NodeKind::Artboard { background: Some(bg), .. }`. Three things could not be said
/// in that shape and are asserted below: a second fill, its *order* relative to the
/// first, and a hidden one drawing nothing.
///
/// ⚠️ **The assertion is on positions in the string, not on `contains`.** Every
/// `contains` here passes against a writer that emits the two rects in either
/// order, or after the children — and "the ground is on top of the artwork" is the
/// failure that would actually be reported. Flipped by reversing `visible_fills` at
/// the frame arm: `ground < over` fails.
#[test]
fn a_frames_fill_stack_is_written_in_order_before_its_children() {
    use ondin_core::Fill;
    let mut f = common::fixture();
    let solid = |r, g, b, visible| Fill {
        brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(r, g, b)),
        visible,
    };
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: f.artboard,
            fills: vec![
                solid(0x01, 0x02, 0x03, true),
                solid(0x04, 0x05, 0x06, false),
                solid(0x07, 0x08, 0x09, true),
            ],
        }]))
        .expect("a frame with a fill stack");
    f.res = Resolved::rebuild(&f.doc);
    let out = svg(&f.doc, &f.res, None);

    let at = |hex: &str| out.find(&format!(r##"fill="{hex}""##));
    let ground = at("#010203").unwrap_or_else(|| panic!("the bottom fill is missing:\n{out}"));
    let over = at("#070809").unwrap_or_else(|| panic!("the top fill is missing:\n{out}"));
    assert!(
        at("#040506").is_none(),
        "a hidden fill writes nothing:\n{out}"
    );
    assert!(ground < over, "bottom of the stack first:\n{out}");

    // And both are behind the contents. The fixture's rect is the frame's first
    // child; whatever it is filled with has to come after both grounds.
    let child = out
        .find(&format!(
            r#"width="{}""#,
            match f.doc.get(f.rect).unwrap().kind() {
                ondin_core::NodeKind::Rect { size, .. } => size.width,
                other => panic!("the fixture's rect is a {other:?}"),
            }
        ))
        .unwrap_or_else(|| panic!("the frame's child is missing:\n{out}"));
    assert!(over < child, "the fills are behind the children:\n{out}");
}

/// **Type on a rail exports as `<textPath>` over a rail in `<defs>`** (§15 D405).
///
/// Three claims, and they fail in three different ways:
///
/// - **The rail reaches the file at all.** It lives on the text node rather than
///   in a layer of its own, so nothing in the node walk would emit it — it is
///   added by `collect_defs`, which is otherwise entirely about paint servers.
///   Forget that and the `href` below points at nothing and the text vanishes in
///   every viewer.
/// - **The reference resolves.** The id is built twice, in two functions, from
///   the same node — `collect_defs` has the `NodeId` and `write_element` only has
///   the `Node`. A mismatch is invisible in our own tests and fatal in a browser,
///   which is exactly the shape of the two-inverse-functions risk §15 D394 named.
/// - **The flat path is not taken.** A railed node must not also emit the
///   one-`<text>`-per-line form, or the type is in the file twice, once bent and
///   once straight through the middle of it.
#[test]
fn type_on_a_rail_exports_as_a_text_path() {
    use ondin_core::kurbo::BezPath;
    use ondin_core::{GeometryPatch, Resolved, Transaction};

    let mut rail = BezPath::new();
    rail.move_to((0.0, 0.0));
    rail.curve_to((60.0, -40.0), (140.0, 40.0), (200.0, 0.0));

    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetGeometry {
            id: f.text,
            geometry: GeometryPatch::TextPath(Some(rail)),
        }]))
        .expect("a text node takes a rail");
    let res = Resolved::rebuild(&f.doc);
    let out = svg(&f.doc, &res, None);

    let rail_def = out
        .lines()
        .find(|l| l.contains("<path id=\"rail-"))
        .unwrap_or_else(|| panic!("the rail must reach <defs>:\n{out}"));
    // The id as the def spells it, which is what the reference has to match.
    let start = rail_def.find("id=\"").expect("an id") + 4;
    let name = &rail_def[start..][..rail_def[start..].find('"').expect("a quote")];

    assert!(
        out.contains(&format!("href=\"#{name}\"")),
        "the textPath must name the def that exists — {name}:\n{out}"
    );
    assert_eq!(
        out.matches("<textPath").count(),
        1,
        "one textPath for one railed node:\n{out}"
    );
    // The flat form writes an `x=` and a `y=` on the `<text>`; the railed form
    // must not, or the node is in the file twice.
    let texts: Vec<&str> = out.lines().filter(|l| l.contains("<text ")).collect();
    assert_eq!(
        texts.len(),
        1,
        "a railed node emits one text element, not one per line:\n{out}"
    );
    assert!(
        !texts[0].contains(" x=\""),
        "a railed text element is placed by its rail, not by a baseline:\n{}",
        texts[0]
    );
    // And the content is still there to be selected and searched, which is the
    // whole reason this is `<textPath>` rather than an outlined `<path>`.
    assert!(
        out.contains("<tspan"),
        "the characters must survive as text:\n{out}"
    );
    // ⚠️ **This output was checked against a real browser, which is the only thing
    // that disagrees with a hand-written expectation about what SVG looks like**
    // (§15 D394's third amendment). Served over HTTP and laid over our own CPU
    // render of the same node at 50%: the rail's start point, its tangent and
    // every rotation agree; the per-glyph *advances* drift toward the end of the
    // word, because the viewer re-shapes — with a font it may not even have — and
    // its advance widths are not parley's. That drift is the cost `<textPath>`
    // was chosen for and is written up on the writer's own arm above.
    let _ = &out;
}

/// **The SVG writer is §15 D455's third door, and it was the one that was missed.**
///
/// `[S11.1-L1-02]`. D455 clamps a descending stop list up to the running maximum
/// at the importer and at the render boundary — but **nothing clamps on load**, so
/// a hand-edited `.ondin`, or any file imported before D455 landed, still holds
/// one in the model. The canvas and a PNG export were right, both going through
/// `color::brush_to_backend`; `gradient_def` wrote `g.stops` verbatim. So the app
/// drew the ramp correctly and handed a browser **the flat fill** — the finding's
/// own symptom, exported. Found by `arch-scribe` reading D455's brief against the
/// code, which is the eighth brief in a row it has found something in.
///
/// ⚠️ **The fixture has to bypass the importer**, or it is testing D455's first
/// door over again: the stops are written straight onto a `GradientBrush` and
/// committed with `SetFills`, which is exactly the shape a hand-edited file
/// arrives in.
///
/// ⚠️ **Asserted on the emitted offsets, not on the picture.** What a browser does
/// with a descending list is the browser's business — it clamps, per spec — and
/// the point here is that we no longer hand it one to clamp. Two controls: an
/// ascending list is written unchanged, and the model is **not** modified by
/// having been exported.
///
/// ⚠️ **Flipped** by writing `g.stops` instead of the clamped copy: fails on the
/// third offset, `0.2` against `0.8`.
#[test]
fn an_svg_export_clamps_a_descending_ramp_the_model_still_holds() {
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{
        Document, Fill, IdSource, NodeKind, Operation, Resolved, Transaction, peniko,
    };

    fn exported(offsets: [f32; 4]) -> (String, Document, ondin_core::NodeId) {
        let mut ids = IdSource::new(0x5A5A);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, rect) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: ab,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
        ]))
        .unwrap();
        let mut g = peniko::Gradient::new_linear((0.0, 0.0), (100.0, 0.0));
        g.stops.clear();
        for (i, &offset) in offsets.iter().enumerate() {
            g.stops.push(peniko::ColorStop {
                offset,
                color: peniko::color::DynamicColor::from_alpha_color(Color::from_rgba8(
                    (i as u8) * 60,
                    0,
                    0,
                    255,
                )),
            });
        }
        doc.apply(&Transaction(vec![Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: ondin_core::Brush::Gradient(g.into()),
                visible: true,
            }],
        }]))
        .unwrap();
        let out = ondin_export::svg::svg(&doc, &Resolved::rebuild(&doc), None);
        (out, doc, rect)
    }

    fn offsets_in(svg: &str) -> Vec<f64> {
        svg.match_indices(r#"<stop offset=""#)
            .map(|(at, tag)| {
                let rest = &svg[at + tag.len()..];
                rest[..rest.find('"').expect("a closed attribute")]
                    .parse()
                    .expect("a number")
            })
            .collect()
    }

    let (out, doc, rect) = exported([0.0, 0.8, 0.2, 1.0]);
    assert_eq!(
        offsets_in(&out),
        vec![0.0, 0.8, 0.8, 1.0],
        "the writer must hand a browser what the ramp means, not a list to clamp:\n{out}"
    );

    // The model is untouched: exporting is a read.
    let ondin_core::Brush::Gradient(g) = &doc.get(rect).unwrap().paint().fills[0].brush else {
        panic!("the fixture's fill is a gradient")
    };
    assert_eq!(
        g.gradient
            .stops
            .iter()
            .map(|s| s.offset)
            .collect::<Vec<_>>(),
        vec![0.0, 0.8, 0.2, 1.0],
        "a document must not change because it was exported"
    );

    // And an ascending list is written exactly as authored.
    let (ok, _, _) = exported([0.0, 0.2, 0.8, 1.0]);
    assert_eq!(offsets_in(&ok), vec![0.0, 0.2, 0.8, 1.0]);
}

/// **An alpha mask's region is written in the space the mask is referenced in.**
///
/// `[S8.1-L1-01]`, §15 D458. `<mask>` was emitted with `maskUnits="userSpaceOnUse"`
/// and **no** `x`/`y`/`width`/`height`. SVG's defaults for those are `-10%`,
/// `-10%`, `120%`, `120%`, and under `userSpaceOnUse` a percentage is a fraction
/// of the **viewport** read as a length in the *current* user space — which for a
/// mask inside a transformed `<g>` is that group's local space, not the page.
/// Everything outside is mask value 0, i.e. erased.
///
/// Measured in Chrome over HTTP on a 400×400 artboard holding a group with an
/// alpha mask and a coincident red rectangle, which the canvas draws whole in
/// every case: opaque to x=399 at identity, **x=219** under `scale(0.5)`,
/// **x=43** under `scale(0.1)`, and **nothing at all** under `translate(-1000)`
/// with the children at local `+1000`. The three numbers are exactly
/// `1.1 × 400 × scale`, which pins the mechanism rather than merely observing a
/// loss.
///
/// ⚠️ **The fixture is the `translate` row, because it is the one that loses
/// everything** and because it is the one a *scale*-shaped fix would miss: the
/// group sits at `-1000` and its children at local `+1000`, so the content is
/// past 110% of the viewport in the group's own coordinates at **any** zoom.
///
/// ⚠️ **Asserted against the content's own box, not against a literal.** A
/// literal region would pass for a writer that emitted the right numbers by
/// coincidence in this one fixture; what the region has to do is *contain the
/// mask's ink*, and that is what is checked. The mask ellipse is 200×200 at local
/// (1000, 1000).
///
/// ⚠️ **Flipped** by deleting the `region` interpolation from the `writeln!`:
/// fails on the first assertion, on the **absence** of a `width` attribute rather
/// than on a number — which is the state the finding measured.
///
/// ⚠️ **And confirmed in a real browser, which is the only thing that disagrees
/// with a hand-written expectation about what SVG looks like** (§15 D394's third
/// amendment). Both writers' output for this fixture — a 400×400 red rect masked
/// by a 300-diameter white ellipse, inside the same `translate(-1000)` group —
/// served over HTTP through `.claude/launch.json`'s `probe-static` and counted
/// off a canvas in Chrome:
///
/// | writer | red pixels | rightmost red |
/// | --- | --- | --- |
/// | before D458 | **0** | — |
/// | after | 70 456 | x = 349 |
///
/// π·150² is 70 686, and the ellipse spans page x = 50…350, so both numbers are
/// the drawing. **Zero is the whole finding**: the file opens, validates, and
/// looks deliberate.
#[test]
fn an_alpha_masks_region_is_written_in_the_space_it_is_referenced_in() {
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, IdSource, MaskMode, NodeKind, Operation, Resolved, Transaction};

    let mut ids = IdSource::new(0x5A5A);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, group, mask, over) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: None,
            name: None,
        },
        // The group is a thousand units to the left; its children are a thousand
        // to the right of *it*, so they land back on the page.
        Operation::CreateNode {
            id: group,
            parent: ab,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((-1000.0, -1000.0))),
            name: None,
        },
        Operation::CreateNode {
            id: mask,
            parent: group,
            index: 0,
            kind: NodeKind::Ellipse {
                size: Size::new(200.0, 200.0),
            },
            transform: Some(Affine::translate((1000.0, 1000.0))),
            name: None,
        },
        Operation::CreateNode {
            id: over,
            parent: group,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(200.0, 200.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((1000.0, 1000.0))),
            name: None,
        },
        Operation::SetMask {
            id: mask,
            mask: true,
        },
        Operation::SetMaskMode {
            id: mask,
            mode: MaskMode::Alpha,
        },
    ]))
    .unwrap();

    let out = ondin_export::svg::svg(&doc, &Resolved::rebuild(&doc), None);
    let wire = mask.to_wire().replace(':', "_");
    let open = out
        .find(&format!(r#"<mask id="mask-{wire}""#))
        .expect("the alpha mask is written");
    let tag = &out[open..open + out[open..].find('>').expect("a closed tag")];

    let attr = |name: &str| -> f64 {
        let at = tag.find(&format!(r#" {name}=""#)).unwrap_or_else(|| {
            panic!("no `{name}` on the mask tag — the state the finding measured:\n{tag}")
        });
        let rest = &tag[at + name.len() + 3..];
        rest[..rest.find('"').expect("a closed attribute")]
            .parse()
            .expect("a number")
    };

    // The mask's ink is the 200×200 ellipse at local (1000, 1000). The region must
    // contain it, in the group's own space — where the default `-10%..120%` of a
    // 400-unit viewport does not reach at all.
    let (x, y, w, h) = (attr("x"), attr("y"), attr("width"), attr("height"));
    assert!(
        x <= 1000.0 && y <= 1000.0 && x + w >= 1200.0 && y + h >= 1200.0,
        "the region must contain the mask's own ink at (1000,1000)–(1200,1200), \
         got ({x}, {y}) {w}×{h}"
    );
    // And it is expressed in that space rather than the page's, which is the whole
    // defect: a region computed against the viewport would sit near the origin.
    assert!(
        x > 400.0,
        "a region in page coordinates would be near 0; this one must be near 1000, got {x}"
    );
}

/// **A number that is not a number never reaches the markup** (`[S8.1-L1-03]`,
/// §15 D508).
///
/// `fmt` is the single formatter every number in this writer goes through, and
/// both of its arms printed a non-finite one verbatim: the fast path's
/// `fract() == 0.0` is false for `NaN` and `inf`, and `format!("{v:.4}")` emits
/// the literal strings `NaN` and `inf`. Neither is a valid SVG `<number>`.
///
/// **Two reachable inputs, and this test drives the one that needs no damaged
/// document at all.** `effect_region` computes `transform * world.inverse()`, and
/// `Affine::inverse` divides by the determinant — so a node scaled flat, which is
/// a resize handle dragged onto the opposite edge, wrote
/// `x="NaN" y="NaN" width="NaN" height="NaN"` on its `<filter>`. Per the spec an
/// unparseable filter region means the **referencing element is not rendered**,
/// so one layer went silently missing from a file that was otherwise correct.
///
/// ⚠️ **The sibling is not decoration.** With a normal layer beside it the
/// `viewBox` is honest and the file is plainly usable apart from the flattened
/// node, which is what makes this the *worse* of the finding's two outcomes:
/// nothing about the output looks wrong.
///
/// **Two assertions, because the two halves of the fix answer differently.** No
/// `NaN` or `inf` anywhere is `fmt`'s floor. No `<filter>` at all is
/// `effect_region`'s refusal — `fmt` alone would have written a region of `0`,
/// which is a filter region of no area and the same silent drop with a parseable
/// spelling.
///
/// **Two flips run.** `effect_region`'s determinant guard removed: fails on *"no
/// filter is written for a node with no area"* — the predicted site — and the
/// `NaN` assertion **stays green**, because `fmt`'s floor turns them into `0`.
/// `fmt`'s `is_finite` arm removed as well: fails on the `NaN` assertion, at
/// `x="NaN"`. So each half is asserted by exactly one of the two, which is what
/// says they are independent rather than one written twice.
#[test]
fn a_flattened_node_with_an_effect_exports_a_file_with_no_nan_in_it() {
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![
            Operation::SetEffects {
                id: f.rect,
                effects: vec![Effect::new(EffectKind::DropShadow(Shadow {
                    blur: 8.0,
                    ..Shadow::default()
                }))],
            },
            // A handle dragged onto the opposite edge: the node still has a
            // transform, and it is singular.
            Operation::SetTransform {
                id: f.rect,
                transform: ondin_core::kurbo::Affine::scale_non_uniform(1.0, 0.0),
            },
        ]))
        .expect("a flattened node is a legal document");
    f.res = Resolved::rebuild(&f.doc);

    let out = ondin_export::svg::svg(&f.doc, &f.res, None);
    assert!(
        !out.contains("NaN") && !out.contains("inf"),
        "no attribute may carry a non-number:\n{out}"
    );
    assert!(
        !out.contains("<filter"),
        "no filter is written for a node with no area — the artwork exports \
         unfiltered rather than not at all:\n{out}"
    );
    // The control: the sibling is untouched, so the file is a real drawing rather
    // than an empty one that passes both assertions for free.
    assert!(
        out.contains("<text"),
        "the rest of the drawing is still there:\n{out}"
    );
}

/// A document of `n` gradient-filled rects on the root, each gradient distinct so
/// every one earns its own def.
fn gradient_document(n: usize) -> (ondin_core::Document, ondin_core::Resolved) {
    use ondin_core::kurbo::{Point, Size};
    use ondin_core::peniko::{Color, Gradient};
    use ondin_core::{Brush, Document, Fill, IdSource, NodeKind, Operation, Resolved, Transaction};

    let mut ids = IdSource::new(0xDEF5);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut ops = Vec::with_capacity(n * 2);
    for i in 0..n {
        let id = ids.mint();
        ops.push(Operation::CreateNode {
            id,
            parent: root,
            index: i,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        });
        // A different end colour per rect, so no two gradients are the same paint
        // — the ids are per node and per slot either way, but a reader should not
        // have to check that to believe the def count.
        let g = Gradient::new_linear(Point::new(0.0, 0.0), Point::new(10.0, 10.0)).with_stops([
            (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
            (1.0_f32, Color::from_rgba8((i % 256) as u8, 0, 255, 255)),
        ]);
        ops.push(Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Gradient(g.into()),
                visible: true,
            }],
        });
    }
    doc.apply(&Transaction(ops)).expect("the fixture");
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// **Collecting defs is linear in the number of defs** (§15 D593,
/// `[A4-L4-02]`).
///
/// `Defs` deduped by scanning a `Vec`, and `collect_defs` calls `add` once per
/// gradient fill, per gradient stroke, per image (three ids), per effect stack and
/// per text rail — Θ(D²) string comparisons over ids sharing a long common
/// prefix. Measured with the node count held fixed at 16,000 so only the def count
/// moved: 21.3 ms at D=1,000 rising to **166.9 ms** at D=16,000, about 87% of
/// export time, while the emitted bytes grew exactly linearly.
///
/// ⚠️ **A ratio, not a threshold, and that is the whole design of this test.**
/// This project measures a 40× spread between debug and release on the same
/// work (`fonts::preview::FRAME_BUDGET`'s own note), so a *"under 200 ms"*
/// assertion is a measurement of the machine. Quadratic growth predicts ≈16× for
/// a 4× document and linear predicts ≈4× — the emitted bytes grow linearly too,
/// so even the fixed version is not flat — and the bound sits between them with
/// room for noise on both sides.
///
/// ⚠️ **The fixture assertions come first**: the two documents must actually
/// contain the defs they claim to, or a build that emitted nothing would have the
/// flattest curve of all.
///
/// **Flip run**, `Defs::add` put back to `self.entries.iter().any(…)`: **8.3×**
/// against the fixed **4.06×**, three runs each and stable to two decimal places.
/// It is the *only* thing that fails — the emitted bytes are identical either
/// way, which is why the §11 goldens could not see this and cannot be the
/// regression test for it.
#[test]
fn collecting_defs_does_not_grow_with_the_square_of_their_count() {
    use ondin_export::svg::svg;

    // ⚠️ **The sizes are chosen for *separation*, not for speed.** At 1,000/4,000
    // the two versions measure 4.1× and 7.0×, which leaves a bound very little
    // room on either side; at 1,500/6,000 they are **4.06×** and **8.3×**, and the
    // 6.0 below sits with 45% headroom above the first and 28% below the second.
    // The extra five seconds of debug fixture-building buys that margin, which is
    // what stops this becoming a test that fails on somebody else's machine.
    const SMALL: usize = 1_500;
    const LARGE: usize = 6_000;

    let (small_doc, small_res) = gradient_document(SMALL);
    let (large_doc, large_res) = gradient_document(LARGE);

    // The fixture: one def per rect, on both.
    let small_out = svg(&small_doc, &small_res, None);
    let large_out = svg(&large_doc, &large_res, None);
    assert_eq!(
        small_out.matches("<linearGradient").count(),
        SMALL,
        "the fixture: every rect must earn a def"
    );
    assert_eq!(large_out.matches("<linearGradient").count(), LARGE);

    // Warm: the first export of a process pays for allocator growth and for
    // whatever the fixture left cold, and that lands entirely on whichever
    // document goes first.
    let _ = svg(&small_doc, &small_res, None);
    let _ = svg(&large_doc, &large_res, None);

    // 🚨 **Best of three, not one** (§15 D705). This took a single sample of
    // each and went red in a full `cargo test --workspace` at **7.04×** against
    // the 6.0 bound, then passed 3/3 in isolation and clean on two subsequent
    // full runs. Nothing was wrong with the code: `cargo test` runs each crate's
    // binaries in parallel, the absolute times here are ~0.009 s and ~0.066 s,
    // and one scheduler stall inside a nine-millisecond window moves the
    // *ratio* far more than it moves either measurement.
    //
    // **The minimum is the right statistic for this and the mean is not.**
    // Contention can only ever make a sample *slower*, so the fastest of a few
    // is the closest estimate of the work's own cost, where an average carries
    // whatever the machine was doing. It is not a widened threshold: widening
    // toward the 8.3× the broken version measures is precisely the
    // discrimination this test exists for.
    //
    // **Re-measured under best-of-three, both sides**: the set gives **3.97×**
    // and the linear scan **8.12×** (0.0082 s / 0.0327 s against 0.0117 s /
    // 0.0949 s). So the margin is 51% above the fixed version and 26% below the
    // broken one, near enough what the single-sample note below claims — the
    // change buys stability without costing the discrimination the test is for,
    // which was the thing to check rather than assume.
    //
    // ⚠️ **It does not make the test immune, and pretending otherwise is how a
    // flake comes back as a mystery.** Three stalls in a row still fail it. The
    // failure message prints both times so the next reader can tell a stall
    // (both times inflated, ratio near 4) from the square coming back (the
    // large one alone inflated).
    let best_of = |doc: &_, res: &_| {
        (0..3)
            .map(|_| {
                let at = std::time::Instant::now();
                let _ = svg(doc, res, None);
                at.elapsed().as_secs_f64()
            })
            .fold(f64::INFINITY, f64::min)
    };
    let small_ms = best_of(&small_doc, &small_res);
    let large_ms = best_of(&large_doc, &large_res);

    let ratio = large_ms / small_ms.max(f64::EPSILON);
    assert!(
        ratio < 6.0,
        "a 4x document cost {ratio:.2}x — measured 4.06x with the set and 8.3x \
         with the scan, so this is the square coming back ({small_ms:.4}s vs \
         {large_ms:.4}s)"
    );
}

/// `n` rects, all filled with one 4 KB picture, and the exported SVG.
fn shared_image_export(n: usize) -> String {
    use ondin_core::kurbo::Size;
    use ondin_core::{
        Brush, Document, Fill, IdSource, ImageBrush, ImageEntry, ImageFormat, ImageId, ImageRef,
        ImageSource, NodeKind, Operation, Resolved, Transaction,
    };
    use ondin_export::svg::svg;

    let mut ids = IdSource::new(0x1A6E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let pic = ImageId("shared".into());
    // Big enough that the payload dominates the markup, so the ratio is about
    // the picture rather than about the `<rect>`s.
    let mut ops = vec![Operation::AddImage {
        id: pic.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(vec![0xAB; 4096].into()),
            format: ImageFormat::Png,
            width: 64,
            height: 64,
        },
    }];
    for i in 0..n {
        let id = ids.mint();
        ops.push(Operation::CreateNode {
            id,
            parent: root,
            index: i,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: Some(ondin_core::kurbo::Affine::translate((30.0 * i as f64, 0.0))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Image(ImageBrush {
                    image: ImageRef::new(pic.clone()),
                    sampler: Default::default(),
                }),
                visible: true,
            }],
        });
    }
    doc.apply(&Transaction(ops)).expect("the fixture");
    let res = Resolved::rebuild(&doc);
    svg(&doc, &res, None)
}

/// **One picture on ten shapes is written once** (§15 D595, `[S9.1-L4-03]`).
///
/// Every def was keyed per node per slot, so N shapes sharing one photograph
/// wrote N full base64 copies. Measured in release on one embedded 200 KB image:
/// one shape gave a 0.27 MB export from a 0.27 MB document; ten gave 2.67 MB from
/// 0.28; **thirty gave 8.01 MB from 0.30 — 26.6×**. The document side is flat
/// because `ImageId` is a content hash and the table holds the blob once; the
/// export threw that away exactly.
///
/// ⚠️ **Ten `<pattern>`s and one `<image>` is the shape being asserted, not
/// "fewer defs".** The per-node pattern is *not* the duplication and must stay —
/// its `patternTransform` carries that node's own `ImageRef::framing`, which
/// genuinely differs per node. A fix that hoisted the pattern too would pass a
/// byte-ratio assertion and lose every framing.
///
/// ⚠️ **Nothing in this workspace renders SVG, so this markup change was checked
/// against a real browser** — the technique `CLAUDE.md` records for imports,
/// used here on the writer. A three-rect document sharing one 4×4 PNG was
/// exported, a copy of it rewritten by hand into the old inline form, and both
/// drawn into canvases and differenced: **10,800 opaque pixels (3 × 60 × 60), 0
/// channels differing, worst delta 0.** That is what says `<use>` of an `<image>`
/// in `<defs>` is not merely valid but identical, which matters because the
/// round-trip suite cannot check it — `svg_in` has no `<pattern>` handling at all
/// (`[A5-L6-01]`) and the §11 golden contains no image.
///
/// **Flip run**, the `<use>` put back to an inline `<image href="data:…">`:
/// fails on *"the payload is written once"* at **11 against 1** — eleven and not
/// ten, because the hoisted def is still emitted under that flip and the ten
/// inline copies are added to it, which is the honest picture of a half-applied
/// fix.
#[test]
fn one_picture_on_many_shapes_is_written_once() {
    let one = shared_image_export(1);
    let ten = shared_image_export(10);

    // The fixture: both really do carry the picture, and the payload really is
    // the bulk of the small one.
    assert!(
        one.contains("data:image/png;base64,"),
        "the fixture: the single-shape export must embed the picture"
    );
    assert!(
        one.len() > 4_000,
        "the fixture: the payload must dominate, or the ratio is about markup"
    );

    assert_eq!(
        ten.matches("<pattern id=").count(),
        10,
        "each node keeps its own pattern — that is where its framing lives"
    );
    assert_eq!(
        ten.matches("data:image/png;base64,").count(),
        1,
        "and the payload is written once"
    );
    assert_eq!(
        ten.matches("<use href=\"#pix-").count(),
        10,
        "each pattern referencing the one payload"
    );

    let ratio = ten.len() as f64 / one.len() as f64;
    assert!(
        ratio < 1.5,
        "ten shapes on one picture cost {ratio:.1}x the bytes of one ({} vs {})",
        one.len(),
        ten.len()
    );
}

/// **A sweep gradient's approximated radius is measured off the shape**, and off
/// the *far* corner of it, so the last stop reaches the whole drawing wherever
/// the centre sits (§15 D647, `[S8.1-L1-06]`).
///
/// A sweep has no SVG 1.1 equivalent and is written as a `<radialGradient>`; a
/// sweep's payload is a centre and two angles and carries no radius at all, so
/// the number has to be invented. It used to be the literal `50` — which is a
/// 50-unit blob at the centre of a 1 000-unit shape with the last stop flooding
/// everything else, and an overflow in the other direction on a 10-unit one.
///
/// **Two centres in one test, and that is the point of it.** Both are on the
/// same 1 000 x 600 rect:
///
/// - centred at `(500, 300)`, the box's own middle, where the answer is half the
///   diagonal, `583.1`;
/// - centred at `(0, 0)`, a corner, where it is the whole diagonal, `1166.2`.
///
/// The obvious implementation — half the frame's diagonal, which is what the
/// finding's own fix sketch proposed — is right for the first and short by a
/// factor of two for the second, which is the case a sweep is most often put in.
///
/// ⚠️ **Flip run.** Predicted failing assertion: the corner one. Restoring the
/// `r="50"` literal fails **both**, at `50` against 583.1 and 1166.2 — so the
/// first assertion has teeth on the old behaviour and only the second has teeth
/// on the plausible wrong version. Replacing the corner walk with
/// `frame.diagonal_len() / 2.0` leaves the centred case green at 583.1 and fails
/// the corner one at 583.1 against 1166.2, which is the flip that matters.
#[test]
fn a_sweep_gradients_radius_is_the_shapes_and_is_measured_from_its_centre() {
    use ondin_core::Brush;
    use ondin_core::kurbo::{Point, Size};
    use ondin_core::peniko::{Color, Gradient};
    use ondin_core::{Fill, GeometryPatch};

    // The `r="…"` of the one `<radialGradient>` a sweep at `centre` produces on a
    // 1 000 x 600 rect.
    let radius_for = |centre: Point| -> f64 {
        let mut f = common::fixture();
        let gradient = Gradient::new_sweep(centre, 0.0, std::f32::consts::TAU).with_stops([
            (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
            (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
        ]);
        f.doc
            .apply(&Transaction(vec![
                Operation::SetGeometry {
                    id: f.rect,
                    geometry: GeometryPatch::Size(Size::new(1000.0, 600.0)),
                },
                Operation::SetFills {
                    id: f.rect,
                    fills: vec![Fill {
                        brush: Brush::Gradient(gradient.into()),
                        visible: true,
                    }],
                },
            ]))
            .unwrap();
        let res = Resolved::rebuild(&f.doc);
        let out = svg(&f.doc, &res, None);

        // The fixture, before the number: a sweep really does reach the file as a
        // radial approximation, and exactly one of them.
        assert_eq!(
            out.matches("<radialGradient").count(),
            1,
            "the fixture: one radial approximation for the one sweep: {out}"
        );
        let tail = out.split(r#"<radialGradient"#).nth(1).unwrap();
        let r = tail.split(r#" r=""#).nth(1).unwrap();
        r.split('"').next().unwrap().parse::<f64>().unwrap()
    };

    let centred = radius_for(Point::new(500.0, 300.0));
    let cornered = radius_for(Point::new(0.0, 0.0));

    // Half the diagonal, because from the middle every corner is that far.
    assert!(
        (centred - 583.095).abs() < 0.5,
        "a sweep at the box's middle reaches its corners at half the diagonal, got {centred}"
    );
    // The whole diagonal, because the far corner is a whole diagonal away — the
    // half-diagonal answer would draw the last stop's flood across most of it.
    assert!(
        (cornered - 1166.19).abs() < 0.5,
        "a sweep at a corner has to reach the opposite one, got {cornered}"
    );
}

/// **A hidden mask masks nothing, in both modes** — §15 D566's rule, which the
/// canvas got and this writer did not (§15 D648, `[S8.1-L3-07]`).
///
/// D282: *"`None` means masks nothing, never 'clip everything away': a **hidden
/// mask**, an empty text node, an empty group and a cancelled boolean all answer
/// it"*. The `Shape` arm honours that through `ondin_core::Resolved::mask_path`,
/// whose first act after the lookup is a visibility test. The `Alpha` arm did not: it emitted a
/// `<mask>` and filled it by calling `emit_node` on the mask, and `emit_node`
/// returns early for an invisible node — so what reached the file was an **empty**
/// `<mask>` over the run, and a browser drew nothing at all. Switching a mask off
/// erased the artwork, which is the one reading D282 forbids.
///
/// The strong assertion is the last one: with the mask hidden the file is
/// **byte-identical** to the same document with the mask flag simply off. A
/// hidden mask is not a weaker mask, it is not a mask.
///
/// ⚠️ **Flip run**, the `if child_node.visible()` guard removed. Predicted failing
/// assertion: the `<g mask=` one, and that is what happens — `Alpha`'s
/// *"a hidden mask leaves the run unwrapped"* fails first, with the identity
/// assertion behind it. `Shape` stays green under the same flip, because its
/// arm never reads the flag directly; that is the asymmetry the finding is about.
#[test]
fn a_hidden_mask_leaves_the_run_unwrapped_in_both_modes() {
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{IdSource, MaskMode, NodeKind, Resolved};

    // The export of the fixture with one ellipse in front of everything, set to
    // mask in `mode`, `mask` on or off and `visible` on or off.
    let export = |mode: MaskMode, is_mask: bool, visible: bool| -> String {
        let mut f = common::fixture();
        let mut ids = IdSource::new(0x81DE);
        let mask = ids.mint();
        f.doc
            .apply(&Transaction(vec![
                Operation::CreateNode {
                    id: mask,
                    parent: f.artboard,
                    index: 0,
                    kind: NodeKind::Ellipse {
                        size: Size::new(64.0, 48.0),
                    },
                    transform: Some(Affine::translate((7.0, 9.0))),
                    name: None,
                },
                Operation::SetMaskMode { id: mask, mode },
                Operation::SetMask {
                    id: mask,
                    mask: is_mask,
                },
                Operation::SetVisible { id: mask, visible },
            ]))
            .unwrap();
        svg(&f.doc, &Resolved::rebuild(&f.doc), None)
    };

    for (mode, element) in [(MaskMode::Shape, "clip-path"), (MaskMode::Alpha, "mask")] {
        // The fixture reaches the state it names: a *visible* mask of this mode
        // really does wrap the siblings above it.
        let live = export(mode, true, true);
        assert!(
            live.contains(&format!(r#"<g {element}="url(#mask-"#)),
            "the fixture: a visible {mode:?} mask wraps the run:\n{live}"
        );

        // The loss, first: hidden, nothing is wrapped — under the old `Alpha`
        // arm the whole drawing sat inside a `<g mask>` whose `<mask>` was empty,
        // so the file opened and showed an empty page.
        let hidden = export(mode, true, false);
        assert!(
            !hidden.contains(r#"<g mask="url(#mask-"#)
                && !hidden.contains(r#"<g clip-path="url(#mask-"#),
            "a hidden {mode:?} mask leaves the run unwrapped:\n{hidden}"
        );

        // And nothing else of it reaches the file either.
        assert_eq!(
            hidden,
            export(mode, false, false),
            "a hidden {mode:?} mask is not a weaker mask — it contributes exactly \
             what a hidden non-mask does, which is nothing"
        );
    }
}
