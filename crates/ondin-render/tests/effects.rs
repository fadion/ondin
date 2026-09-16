//! Effects, through the whole CPU path: document → scene walk → offscreen layer
//! → passes → composite (§5.3a, §6.4).
//!
//! **Pixels, not markup.** `ondin-export`'s SVG tests read the filter graph, which
//! proves what the file *says*; these prove what the renderer *draws*. The two
//! writers are meant to agree, and neither test would notice if only the other
//! were right — so both exist, and this is the one that would catch a blur going
//! the wrong way or a shadow painted in the layer's colour.

use ondin_core::kurbo::{Affine, Rect, Size, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Effect, EffectKind, Fill, Filters, IdSource, NodeKind, Operation, Resolved,
    Shadow, Transaction,
};
use ondin_render::{ImageStore, VelloCpuRenderer, Viewport};

/// A white 40×40 square at (30, 30) on a 100×100 page, carrying `effects`.
fn render(effects: Vec<Effect>) -> (Vec<u8>, usize) {
    render_kind(
        NodeKind::Rect {
            size: Size::new(40.0, 40.0),
            corner_radii: Default::default(),
        },
        effects,
    )
}

/// The same page with the shape's kind chosen — for the one test that needs a
/// layer whose **bounding box is not its coverage**.
fn render_kind(kind: NodeKind, effects: Vec<Effect>) -> (Vec<u8>, usize) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let r = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: r,
            parent: root,
            index: 0,
            kind,
            transform: Some(Affine::translate((30.0, 30.0))),
            name: None,
        },
        Operation::SetFills {
            id: r,
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        },
        Operation::SetEffects { id: r, effects },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    };
    let (px, w, _) =
        VelloCpuRenderer::new().render_to_rgba(&doc, &res, &vp, &ImageStore::default());
    (px, w as usize)
}

fn at(px: &[u8], w: usize, x: usize, y: usize) -> (u8, u8, u8, u8) {
    let p = (y * w + x) * 4;
    (px[p], px[p + 1], px[p + 2], px[p + 3])
}

fn red_shadow(dy: f64, blur: f64) -> Effect {
    Effect::new(EffectKind::DropShadow(Shadow {
        offset: Vec2::new(0.0, dy),
        blur,
        spread: 0.0,
        color: Color::from_rgba8(255, 0, 0, 255),
    }))
}

/// **The shadow is drawn, in its own colour, on the side its offset points at.**
///
/// The square covers y 30…70, so a shadow 10 down covers 40…80 before blurring.
/// A red shadow under a white square is deliberate: a shadow painted in the
/// *layer's* colour — the mistake of compositing the layer twice instead of
/// building a silhouette — is invisible against a black page and obvious here.
///
/// ⚠️ Flipped by negating the offset, which moves every one of these assertions
/// to the other side and fails the first.
#[test]
fn a_drop_shadow_lands_below_the_layer_in_its_own_colour() {
    let (px, w) = render(vec![red_shadow(10.0, 8.0)]);
    let below = at(&px, w, 50, 78);
    assert!(below.3 > 0, "there is ink below the square: {below:?}");
    assert!(
        below.0 > 200 && below.1 < 40 && below.2 < 40,
        "and it is the shadow's red, not the layer's white: {below:?}"
    );
    assert_eq!(
        at(&px, w, 50, 20).3,
        0,
        "nothing above it — the offset points down"
    );
    assert_eq!(
        at(&px, w, 50, 50),
        (255, 255, 255, 255),
        "and the layer itself is untouched, drawn over its own shadow"
    );
}

/// The blur is what makes a shadow a shadow, so its falloff is the assertion.
///
/// A shadow 10 down with an 8 blur has a deviation of 4 and reaches 12 past its
/// silhouette's edge at y=80: solid well inside, weaker at the edge, gone by 92.
///
/// ⚠️ Flipped against a zero deviation, where the first two are equal and the
/// third is still 0 — so the *middle* assertion is the one carrying the test, and
/// a version that only checked "something is there and something is not" would
/// pass against a hard-edged offset copy.
#[test]
fn a_shadows_alpha_falls_off_across_the_blur_rather_than_stopping_at_an_edge() {
    let (px, w) = render(vec![red_shadow(10.0, 8.0)]);
    let solid = at(&px, w, 50, 74).3;
    let edge = at(&px, w, 50, 80).3;
    let past = at(&px, w, 50, 95).3;
    assert!(
        solid > edge,
        "{solid} inside should beat {edge} at the edge"
    );
    assert!(
        edge > 40 && edge < solid - 20,
        "the edge is a partial value, not a step: {edge} against {solid}"
    );
    assert_eq!(past, 0, "and it has faded out well before 15px past");
}

/// **A layer blur softens the layer's own edge**, which is the case that proves
/// the offscreen buffer is big enough: the blur reaches outside the square, so a
/// buffer sized to the geometry rather than to `ink_bounds` would clip it square.
#[test]
fn a_layer_blur_spreads_the_layer_past_its_own_edge() {
    let (px, w) = render(vec![Effect::new(EffectKind::LayerBlur { radius: 8.0 })]);
    let outside = at(&px, w, 50, 74).3;
    assert!(
        outside > 0,
        "the blurred edge reaches past the square's own 70: {outside}"
    );
    let centre = at(&px, w, 50, 50).3;
    assert_eq!(centre, 255, "and the middle is still solid: {centre}");
}

/// A `Filters` row is per-pixel, so it must change the layer and move nothing.
///
/// Desaturating white leaves white, so the fixture uses **brightness**, which a
/// neutral-matrix bug would leave at 255 and a matrix applied to premultiplied
/// data would get subtly wrong only at the edges.
#[test]
fn a_filters_row_changes_the_layers_colour_and_not_its_extent() {
    let (px, w) = render(vec![Effect::new(EffectKind::Filters(Filters {
        brightness: 0.5,
        ..Filters::default()
    }))]);
    let c = at(&px, w, 50, 50);
    assert!(
        (100..=160).contains(&c.0) && c.0 == c.1 && c.1 == c.2,
        "white halved is a mid grey: {c:?}"
    );
    assert_eq!(c.3, 255, "and fully opaque");
    assert_eq!(
        at(&px, w, 50, 74).3,
        0,
        "a per-pixel effect reaches nothing outside the layer"
    );
}

/// **Spread grows the shadow's silhouette before it is blurred**, so a positive
/// spread with no blur and no offset shows the shadow as a hard border sticking
/// out all round the layer.
///
/// ⚠️ The sign is the whole of what this catches, and it is a one-character
/// mistake: `erode` where `dilate` belongs leaves the shadow entirely hidden
/// behind the layer, which reads as "spread does nothing" rather than as
/// backwards. Asserted at 6px out from a 5px spread — inside the reach of the
/// grown silhouette and outside the layer — plus a matching check that 12px out
/// is still empty, or a version that simply blurred would also pass.
#[test]
fn a_positive_spread_grows_the_shadow_beyond_the_layer() {
    let (px, w) = render(vec![Effect::new(EffectKind::DropShadow(Shadow {
        offset: Vec2::new(0.0, 0.0),
        blur: 0.0,
        spread: 5.0,
        color: Color::from_rgba8(255, 0, 0, 255),
    }))]);
    let out = at(&px, w, 50, 73);
    assert!(
        out.3 > 200 && out.0 > 200 && out.1 < 40,
        "6px below the layer is solid shadow red: {out:?}"
    );
    assert_eq!(
        at(&px, w, 50, 79).3,
        0,
        "and 9px out is past the spread, so empty"
    );
    // Sideways too — a spread that only ran along one axis would pass above.
    let side = at(&px, w, 73, 50);
    assert!(side.3 > 200, "and it grows sideways as well: {side:?}");
}

/// ⚠️ **A switched-off stack must draw exactly what no stack draws.**
///
/// The failure this guards is not a missing shadow but a changed layer: the
/// offscreen path rasterizes into its own buffer and composites the result, so
/// any rounding it introduces shows up on every layer that has ever had an effect
/// added and turned off. Byte equality, because "looks the same" is what let it
/// through the first time.
#[test]
fn a_stack_with_no_ink_composites_identically_to_no_stack_at_all() {
    let (plain, _) = render(vec![]);
    for (label, stack) in [
        (
            "hidden",
            vec![Effect {
                kind: EffectKind::DropShadow(Shadow::default()),
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
        let (out, _) = render(stack);
        assert!(out == plain, "{label}: the page must be byte-identical");
    }
}

/// An inner shadow is drawn **inside the layer's ink** and nowhere else — the one
/// property that distinguishes it from a drop shadow with a negative offset.
///
/// ⚠️ **The fixture is an ellipse, and a square version of this test is
/// vacuous.** An inner shadow's escape is zero, so the offscreen buffer is
/// exactly the layer's bounding box; on a square that box *is* the ink, and every
/// point outside the shape is also outside the buffer and transparent for a
/// reason that has nothing to do with the effect. Removing the confine step
/// entirely — which paints the inverted silhouette across the whole buffer — left
/// that version passing. An ellipse's bounding-box corners are inside the buffer
/// and outside the shape, which is the only place the mistake is visible.
///
/// Flipped against exactly that omission: `(35, 35)` goes red and the assertion
/// below fails.
#[test]
fn an_inner_shadow_stays_within_the_layers_ink_not_merely_its_box() {
    let (px, w) = render_kind(
        NodeKind::Ellipse {
            size: Size::new(40.0, 40.0),
        },
        vec![Effect::new(EffectKind::InnerShadow(Shadow {
            offset: Vec2::new(0.0, 8.0),
            blur: 4.0,
            spread: 0.0,
            color: Color::from_rgba8(255, 0, 0, 255),
        }))],
    );
    // Inside the top of the ellipse, where a shadow cast downwards piles up.
    let top = at(&px, w, 50, 34);
    assert!(
        top.0 > top.1 + 40,
        "the inside of the top edge is tinted red: {top:?}"
    );
    assert_eq!(
        at(&px, w, 50, 50),
        (255, 255, 255, 255),
        "the middle is untouched"
    );
    // Inside the bounding box, well outside the ellipse: pixel (32, 32) centres
    // 24.7px from a 20px radius. **The fixture is asserted before the claim is** —
    // (35, 35) sits 20.5px out and carries the ellipse's own antialiased edge at
    // alpha 18, so a sample chosen by eye would have been measuring the shape
    // rather than the shadow.
    let (plain, _) = render_kind(
        NodeKind::Ellipse {
            size: Size::new(40.0, 40.0),
        },
        vec![],
    );
    assert_eq!(
        at(&plain, w, 32, 32).3,
        0,
        "the sample point really is outside the ellipse"
    );
    let corner = at(&px, w, 32, 32);
    assert_eq!(
        corner.3, 0,
        "the box's corner is outside the ink and must stay empty: {corner:?}"
    );
}

/// **The shape the test above calls vacuous is the one that draws nothing at
/// all**, and a rectangle is most of what is on a page (`[S10.2-L1-01]`).
///
/// An inner shadow's escape is `Insets::ZERO` (§5.3a), so the offscreen buffer is
/// exactly the layer's bounding box — and on a rect the ink *is* that box, so the
/// inverted silhouette is zero in every pixel the buffer holds. At the panel's
/// own default offset the only thing that rescued it was the offset **lookup**'s
/// "outside the buffer is fully casting" rule, which draws a band `|dy|` wide on
/// one side; at offset `(0, 0)` that branch never fires and the render is
/// pixel-identical to no effect at all. Measured, before the fix, at all four
/// samples below: `(255, 255, 255)`.
///
/// The neighbour above is the anti-vacuity control: it keeps the *confine* step
/// honest on a shape whose box is not its ink, which this one cannot see.
///
/// ⚠️ Flipped by restoring the old edge rule — reading outside the buffer as
/// transparent for the passes while the lookup calls it solid — which puts every
/// sample back to white and fails on the first.
#[test]
fn an_inner_shadow_on_a_rect_reaches_all_four_edges() {
    let inner = vec![Effect::new(EffectKind::InnerShadow(Shadow {
        offset: Vec2::new(0.0, 0.0),
        blur: 8.0,
        spread: 0.0,
        color: Color::from_rgba8(255, 0, 0, 255),
    }))];
    let (px, w) = render(inner);
    let (plain, _) = render(vec![]);

    // The square covers 30…70, so one pixel in on each side is inside the ink and
    // near the peak of the shadow. **The threshold is off the measured profile**,
    // not chosen: down each edge the green channel reads 140 at the first pixel,
    // 165, 187, 207, 222 … and 255 by ten pixels in, so 200 sits between the
    // fourth and fifth samples and no plausible falloff passes by accident.
    for (x, y, side) in [
        (50, 31, "top"),
        (50, 69, "bottom"),
        (31, 50, "left"),
        (69, 50, "right"),
    ] {
        let p = at(&px, w, x, y);
        assert!(
            p.0 > 200 && p.1 < 200 && p.2 < 200,
            "the {side} edge is tinted by the shadow: {p:?}"
        );
    }
    let (near, far) = (at(&px, w, 50, 31).1, at(&px, w, 50, 36).1);
    assert!(
        near < far,
        "and it falls off inwards rather than filling the shape: {near} then {far}"
    );
    assert_eq!(
        at(&px, w, 50, 50),
        (255, 255, 255, 255),
        "and the middle is untouched — a shadow that reached it would be a wash"
    );
    assert_ne!(
        px, plain,
        "the whole point: with the old edge rule these two buffers were equal"
    );
}

/// **A positive spread thickens an inner shadow inwards**, which is the direction
/// nothing pinned and which was drawn backwards until `[S10.2-L1-01]`'s repair.
///
/// `spread_alpha`'s `grow` flag reads `(spread > 0) != inner` — grow the *shape's*
/// alpha for a drop shadow, erode it for an inner one — and it was being applied
/// to a buffer that had already been complemented, i.e. to the opposite operand.
/// Complementing at the tint instead puts the flag back on the value it was
/// written for.
///
/// With no blur and no offset the assertion is a hard band and needs no
/// threshold: a rect's inner silhouette is solid, eroding it by 5 leaves a 5px
/// rim of "outside the shape", and complementing that is the shadow. Zero spread
/// casts **nothing** — a shape with no offset, no blur and no spread covers its
/// own silhouette exactly — which is the fixture assertion that stops this test
/// measuring the edge of the rect instead of the effect.
///
/// ⚠️ Flipped by negating `grow`, which makes the +5 case cast nothing and fails
/// on the first assertion. The −5 case passes under both, so it is a control on
/// the *sign* and not on the mechanism.
#[test]
fn a_positive_spread_thickens_an_inner_shadow_inwards() {
    let spread_of = |spread: f64| {
        render(vec![Effect::new(EffectKind::InnerShadow(Shadow {
            offset: Vec2::new(0.0, 0.0),
            blur: 0.0,
            spread,
            color: Color::from_rgba8(255, 0, 0, 255),
        }))])
    };
    let (none, w) = spread_of(0.0);
    assert_eq!(
        at(&none, w, 50, 32),
        (255, 255, 255, 255),
        "no offset, no blur, no spread: the shape covers its own silhouette"
    );

    let (grown, _) = spread_of(5.0);
    let band = at(&grown, w, 50, 32);
    assert_eq!(
        (band.0, band.1, band.2),
        (255, 0, 0),
        "two pixels in is inside the 5px rim and is solid shadow: {band:?}"
    );
    assert_eq!(
        at(&grown, w, 50, 50),
        (255, 255, 255, 255),
        "and the middle is past the rim, untouched"
    );

    let (eroded, _) = spread_of(-5.0);
    assert_eq!(
        at(&eroded, w, 50, 32),
        (255, 255, 255, 255),
        "a negative spread pulls the silhouette wider than the shape: still nothing"
    );
}

/// **An effect layer and an ordinary layer nest, and the pops have to pair
/// correctly** (§15 D338).
///
/// The walk opens the effect layer *outside* the clip and the opacity (§6.4), so
/// any layer with a shadow and `opacity < 1` — or a frame with a shadow and
/// `clip` — has an ordinary layer inside the effect one. A `pop_layer` that
/// closed whichever effect frame was open would close it on the *inner* pop, and
/// `vello_cpu` would then reach `flush` with a layer still open.
///
/// **It panicked rather than drawing wrongly**, which is why this asserts on a
/// pixel rather than on a comparison: before the fix, `render_to_rgba` never
/// returned at all — *"some layers haven't been popped yet"*. Reachable from the
/// panel in two clicks (add a shadow, drag the opacity down) and from
/// `ondin export --png` on the same file.
#[test]
fn a_shadow_survives_the_layer_being_faded() {
    let (px, w) = render_with(0.5, false, vec![red_shadow(12.0, 0.0)]);
    // The shadow is 12 down from a square whose bottom edge is at y = 70, so a
    // sample at y = 78 is in the shadow and clear of the shape.
    let shadow = at(&px, w, 50, 78);
    assert!(
        shadow.0 > 100 && shadow.1 < 40 && shadow.3 > 0,
        "the shadow is drawn, in red: {shadow:?}"
    );
    // **Faded, and the fade reaches the shadow.** The effect layer is outside the
    // opacity, so the shadow is cast from the layer as it appears — half an alpha
    // in the silhouette *and* half in the composite would be a quarter, which is
    // the reading this number rules out.
    let opaque = render_with(1.0, false, vec![red_shadow(12.0, 0.0)]).0;
    let full = at(&opaque, w, 50, 78);
    assert!(
        shadow.3 < full.3,
        "a faded layer casts a fainter shadow: {shadow:?} against {full:?}"
    );
    // **Sampled where the shadow is not.** The shape spans y 30..70 and the
    // shadow y 42..82, so at y = 50 the two overlap and the pixel reads
    // `(255, 170, 170, 192)` — half a white square over half a red shadow, which
    // is right and is not a number about the shape's own opacity. y = 35 is
    // inside the shape and above the shadow.
    let bare = at(&px, w, 50, 35);
    assert!(
        (100..=160).contains(&bare.3),
        "and the shape itself is half there: {bare:?}"
    );
}

/// The other way in: a **frame that clips**, which opens the same ordinary layer
/// for a different reason. Asserted separately because the two arms of
/// `needs_layer` are two conditions, and a fix that only counted one of them
/// would pass the test above.
#[test]
fn a_shadow_survives_the_layer_clipping() {
    let (px, w) = render_with(1.0, true, vec![red_shadow(12.0, 0.0)]);
    let shadow = at(&px, w, 50, 78);
    assert!(
        shadow.0 > 100 && shadow.1 < 40 && shadow.3 > 0,
        "a clipping frame still casts its shadow, and outside its own clip: \
         {shadow:?}"
    );
}

/// A 40×40 shape at (30, 30) on a 100×100 page carrying `effects`, at `opacity`,
/// as a clipping **frame** when `clip` — the two states that make the walk open a
/// layer inside the effect layer.
fn render_with(opacity: f32, clip: bool, effects: Vec<Effect>) -> (Vec<u8>, usize) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let r = ids.mint();
    let kind = if clip {
        NodeKind::Artboard {
            size: Size::new(40.0, 40.0),
        }
    } else {
        NodeKind::Rect {
            size: Size::new(40.0, 40.0),
            corner_radii: Default::default(),
        }
    };
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: r,
        parent: root,
        index: 0,
        kind,
        transform: Some(Affine::translate((30.0, 30.0))),
        name: None,
    }]))
    .unwrap();
    let mut ops = vec![Operation::SetEffects { id: r, effects }];
    if !clip {
        ops.push(Operation::SetFills {
            id: r,
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        });
    } else {
        ops.push(Operation::SetFills {
            id: r,
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        });
        ops.push(Operation::SetClip { id: r, clip: true });
    }
    if opacity < 1.0 {
        ops.push(Operation::SetOpacity { id: r, opacity });
    }
    doc.apply(&Transaction(ops)).unwrap();
    let res = Resolved::rebuild(&doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    };
    let (px, w, _) =
        VelloCpuRenderer::new().render_to_rgba(&doc, &res, &vp, &ImageStore::default());
    (px, w as usize)
}

/// **A layer scrolled out of the viewport still casts its shadow into it**
/// (§15 D342).
///
/// The effect layer's buffer is clamped to the target — an effect on a layer
/// mostly off-screen should cost the visible part — and the buffer is *also* what
/// the silhouette is read from. Clamped to the bare target, a layer scrolled off
/// the top cast a shadow computed from the sliver still showing, and once it was
/// fully off it cast none at all while its shadow was still reaching in.
///
/// **Absolute, not differential.** Both backends had the same clamp, so
/// `gpu_effects::both_backends_draw_the_same_shadow` agreed perfectly on the wrong
/// answer; what found this was asking where the ink actually landed. Reported from
/// the machine as something wrong "when panning and the shape is off screen".
#[test]
fn a_layer_scrolled_off_the_top_still_casts_into_view() {
    // The square is world y 30…70 and the shadow is 20 down with a 6 blur, so its
    // ink runs to y 99 — well past the square's own box.
    let effects = vec![red_shadow(20.0, 6.0)];
    let full = render_panned(&effects, 0.0);
    let bottom = ink_rows(&full)
        .1
        .expect("the fixture draws a shadow below the square");
    assert!(
        bottom > 80,
        "the fixture's shadow reaches past y=80, or the pans below prove nothing: {bottom}"
    );

    // Panned so the square's own box (30…70) is entirely above the view.
    for dy in [72.0, 80.0, 88.0] {
        let px = render_panned(&effects, dy);
        let (top, low) = ink_rows(&px);
        let top = top
            .unwrap_or_else(|| panic!("dy={dy}: the layer is off the top and cast nothing at all"));
        assert_eq!(top, 0, "dy={dy}: the shadow starts at the top edge");
        let low = low.expect("some ink");
        assert_eq!(
            low as f64,
            bottom as f64 - dy,
            "dy={dy}: the shadow's far edge moved with the pan and nothing else"
        );
    }
}

/// The same question with the offset **sideways**, because the arithmetic that
/// grows the clamp swaps the two sides — ink at the right edge is cast from
/// silhouette to the *left* of it — and a version that grew the wrong side would
/// pass the vertical case above and fail here.
#[test]
fn a_layer_scrolled_off_the_left_still_casts_to_the_right() {
    let effects = vec![Effect::new(EffectKind::DropShadow(Shadow {
        offset: Vec2::new(30.0, 0.0),
        blur: 4.0,
        spread: 0.0,
        color: Color::from_rgba8(255, 0, 0, 255),
    }))];
    // The square is world x 30…70; its shadow runs to x 106. Panned right by 75,
    // the square is off the left edge and the shadow's tail is still in view.
    let px = render_panned_x(&effects, 75.0);
    let lit = (0..100).any(|x| at(&px, 100, x, 50).3 > 0);
    assert!(
        lit,
        "a layer off the left edge still throws its shadow to the right"
    );
}

fn ink_rows(px: &[u8]) -> (Option<usize>, Option<usize>) {
    let rows: Vec<usize> = (0..100)
        .filter(|y| (0..100).any(|x| at(px, 100, x, *y).3 > 0))
        .collect();
    (rows.first().copied(), rows.last().copied())
}

fn render_panned(effects: &[Effect], dy: f64) -> Vec<u8> {
    render_view(effects, Rect::new(0.0, dy, 100.0, 100.0 + dy))
}

fn render_panned_x(effects: &[Effect], dx: f64) -> Vec<u8> {
    render_view(effects, Rect::new(dx, 0.0, 100.0 + dx, 100.0))
}

fn render_view(effects: &[Effect], view: Rect) -> Vec<u8> {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let r = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: r,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((30.0, 30.0))),
            name: None,
        },
        Operation::SetFills {
            id: r,
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        },
        Operation::SetEffects {
            id: r,
            effects: effects.to_vec(),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let vp = Viewport {
        view,
        pixel_size: (100, 100),
    };
    VelloCpuRenderer::new()
        .render_to_rgba(&doc, &res, &vp, &ImageStore::default())
        .0
}
