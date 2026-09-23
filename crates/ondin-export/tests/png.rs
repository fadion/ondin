//! PNG raster export + the color-boundary verification (§7.2, invariant 7):
//! a known sRGB fill must produce the expected pixel through the CPU renderer.

mod common;

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Fill, IdSource, ImageEntry, ImageFormat, ImageId, ImageSource, NodeId, NodeKind,
    Operation, Resolved, StrokeAlign, Transaction, image_brush,
};
use ondin_export::png::png;
use ondin_render::{VelloCpuRenderer, Viewport};

/// A 100×100 document: an artboard filled by a red rectangle covering it.
fn red_square_doc() -> (Document, Resolved) {
    let mut ids = IdSource::new(7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let rect = ids.mint();
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
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: None,
        },
    ]))
    .unwrap();
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: rect,
        fills: vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(220, 30, 40, 255)),
            visible: true,
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

fn viewport() -> Viewport {
    Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    }
}

#[test]
fn color_boundary_produces_expected_pixel() {
    let (doc, res) = red_square_doc();
    let (rgba, w, h) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );
    assert_eq!((w, h), (100, 100));
    assert_eq!(rgba.len(), 100 * 100 * 4);

    // Sample the center pixel — it must be the sRGB fill we authored, proving
    // the color path did NOT distort it (no accidental linear/premul conversion).
    let idx = ((50 * 100) + 50) * 4;
    let px = &rgba[idx..idx + 4];
    // Allow ±2 for rounding through premultiply/un-premultiply.
    assert!((px[0] as i32 - 220).abs() <= 2, "R was {}", px[0]);
    assert!((px[1] as i32 - 30).abs() <= 2, "G was {}", px[1]);
    assert!((px[2] as i32 - 40).abs() <= 2, "B was {}", px[2]);
    assert_eq!(px[3], 255, "opaque");
}

#[test]
fn png_encodes_valid_signature_and_size() {
    let f = common::fixture();
    let vp = Viewport {
        view: f.res.world_bounds(f.artboard).unwrap(),
        pixel_size: (200, 150),
    };
    let bytes = png(&f.doc, &f.res, &vp);
    // PNG magic number.
    assert_eq!(
        &bytes[..8],
        &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
    );

    // Decode it back and check dimensions.
    let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    let reader = decoder.read_info().expect("valid png");
    let info = reader.info();
    assert_eq!((info.width, info.height), (200, 150));
}

#[test]
fn text_rasterizes_ink_pixels() {
    // A large black glyph on a white artboard must leave dark pixels — proving
    // the parley→glyph→vello_cpu path actually rasterizes text (not just shapes).
    use ondin_core::node::{TextSizing, TextStyle};
    let mut ids = IdSource::new(9);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let text = ids.mint();
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
        Operation::SetFills {
            id: ab,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(255, 255, 255, 255)),
                visible: true,
            }],
        },
        Operation::CreateNode {
            id: text,
            parent: ab,
            index: 0,
            kind: NodeKind::Text {
                content: "E".into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: 90.0,
                    weight: 800,
                    italic: false,
                    line_height: Some(ondin_core::Length::Em(1.0)),
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
            transform: None,
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );

    let dark = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] > 128 && p[0] < 100 && p[1] < 100 && p[2] < 100)
        .count();
    assert!(dark > 50, "expected glyph ink pixels, found {dark}");
}

#[test]
fn empty_viewport_region_is_transparent() {
    // Render a region far from any geometry: fully transparent.
    let (doc, res) = red_square_doc();
    let vp = Viewport {
        view: Rect::new(1000.0, 1000.0, 1100.0, 1100.0),
        pixel_size: (16, 16),
    };
    let (rgba, _, _) =
        VelloCpuRenderer::new().render_to_rgba(&doc, &res, &vp, &ondin_render::ImageStore::new());
    assert!(
        rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0),
        "all pixels transparent"
    );
}

// --- stroke alignment -----------------------------------------------------

/// A 100×100 artboard with a 40×40 rect at (30,30), stroked 10 wide in blue
/// with `align`, no fill. Nothing else paints, so every coloured pixel is
/// stroke and its extent can be measured directly.
fn stroked_rect_doc(align: StrokeAlign) -> (Document, Resolved) {
    let mut ids = IdSource::new(21);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let rect = ids.mint();
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
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(ondin_core::kurbo::Affine::translate((30.0, 30.0))),
            name: None,
        },
    ]))
    .unwrap();
    doc.apply(&Transaction(vec![
        Operation::SetFills {
            id: rect,
            fills: Vec::new(),
        },
        Operation::SetStrokes {
            id: rect,
            strokes: vec![ondin_core::Stroke {
                brush: Brush::Solid(Color::from_rgba8(0, 0, 255, 255)),
                width: 10.0,
                join: ondin_core::kurbo::Join::Miter,
                cap: ondin_core::kurbo::Cap::Butt,
                dashes: Vec::new(),
                dash_offset: 0.0,
                sides: Default::default(),
                miter_limit: 4.0,
                dash_fit: false,
                align,
                visible: true,
            }],
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// Whether the pixel at `(x, y)` has any ink in it.
fn inked(rgba: &[u8], x: usize, y: usize) -> bool {
    rgba[((y * 100) + x) * 4 + 3] > 20
}

/// The construction under test is "stroke at double width, clip away the half
/// you do not want", and what it has to produce is a stroke that occupies
/// exactly one side of the outline. These sample points are 2px clear of every
/// boundary so antialiasing cannot decide the result.
///
/// The rect's left edge is at x=30, so with a 10-wide stroke:
///   centre  covers x 25..35
///   inside  covers x 30..40
///   outside covers x 20..30
#[test]
fn stroke_alignment_puts_the_paint_on_the_right_side_of_the_outline() {
    let y = 50; // mid-height, so only the left/right edges matter

    let (doc, res) = stroked_rect_doc(StrokeAlign::Center);
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );
    assert!(inked(&rgba, 27, y), "centre: paints outside the outline");
    assert!(inked(&rgba, 32, y), "centre: paints inside the outline");
    assert!(!inked(&rgba, 22, y), "centre: nothing a full width out");
    assert!(!inked(&rgba, 37, y), "centre: nothing a full width in");

    let (doc, res) = stroked_rect_doc(StrokeAlign::Inside);
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );
    assert!(
        inked(&rgba, 32, y),
        "inside: paints just within the outline"
    );
    assert!(inked(&rgba, 37, y), "inside: reaches a full width in");
    assert!(!inked(&rgba, 27, y), "inside: must not cross the outline");
    assert!(!inked(&rgba, 42, y), "inside: must not exceed its width");

    let (doc, res) = stroked_rect_doc(StrokeAlign::Outside);
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );
    assert!(
        inked(&rgba, 27, y),
        "outside: paints just beyond the outline"
    );
    assert!(inked(&rgba, 22, y), "outside: reaches a full width out");
    assert!(!inked(&rgba, 32, y), "outside: must not cross the outline");
    assert!(!inked(&rgba, 18, y), "outside: must not exceed its width");
}

/// The clip must not leak into whatever is drawn next. An inside stroke pushes
/// a clip layer; a sibling painted afterwards has to be unaffected by it.
#[test]
fn an_aligned_stroke_does_not_clip_its_siblings() {
    let (mut doc, _) = stroked_rect_doc(StrokeAlign::Inside);
    let ab = doc.get(doc.root()).unwrap().children()[0];
    let mut ids = IdSource::new(99);
    let sibling = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: sibling,
        parent: ab,
        index: 1,
        kind: NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: RoundedRectRadii::default(),
        },
        transform: Some(ondin_core::kurbo::Affine::translate((5.0, 5.0))),
        name: None,
    }]))
    .unwrap();
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: sibling,
        fills: vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(0, 200, 0, 255)),
            visible: true,
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );
    assert!(
        inked(&rgba, 10, 10),
        "the sibling outside the stroked rect was clipped away"
    );
}

/// An open shape has no inside, so alignment cannot apply — and must not chop
/// the stroke in half trying. A line drawn `Inside` looks like a centred one.
#[test]
fn an_open_shape_ignores_alignment_rather_than_half_drawing_itself() {
    let mut ids = IdSource::new(31);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let line = ids.mint();
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
            id: line,
            parent: ab,
            index: 0,
            kind: NodeKind::Line {
                end: ondin_core::kurbo::Point::new(60.0, 0.0),
            },
            transform: Some(ondin_core::kurbo::Affine::translate((20.0, 50.0))),
            name: None,
        },
    ]))
    .unwrap();
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: line,
        strokes: vec![ondin_core::Stroke {
            brush: Brush::Solid(Color::from_rgba8(0, 0, 255, 255)),
            width: 10.0,
            join: ondin_core::kurbo::Join::Miter,
            cap: ondin_core::kurbo::Cap::Butt,
            dashes: Vec::new(),
            dash_offset: 0.0,
            sides: Default::default(),
            miter_limit: 4.0,
            dash_fit: false,
            align: StrokeAlign::Inside,
            visible: true,
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );
    // Centred on y=50 means ink both above and below the line's own y.
    assert!(inked(&rgba, 50, 47), "above the line");
    assert!(inked(&rgba, 50, 53), "below the line");
}

/// **A per-side stroke has to put ink on that side and nowhere else.**
///
/// The end-to-end assertion for the whole feature, through the CPU backend that
/// shares the GPU canvas's scene walk: the model says "top only", so the top edge
/// darkens and the other three do not. Nothing short of this catches a side path
/// that came out as two `MoveTo`s and drew nothing — which is exactly the bug
/// `geometry::side_path` shipped with for one round, because kurbo reads
/// `BezPath::is_empty` as "no *segments*" and a lone `MoveTo` satisfies it.
#[test]
fn a_per_side_stroke_inks_only_that_side() {
    use ondin_core::{Operation, Resolved, Stroke, StrokeSides, Transaction};

    /// The darkest pixel along a horizontal run — the stroke is black on a red
    /// fill, so "dark" is unambiguous.
    fn darkest(rgba: &[u8], w: usize, y: usize, xs: std::ops::Range<usize>) -> u8 {
        xs.map(|x| rgba[(y * w + x) * 4]).min().unwrap_or(255)
    }
    fn darkest_col(rgba: &[u8], w: usize, x: usize, ys: std::ops::Range<usize>) -> u8 {
        ys.map(|y| rgba[(y * w + x) * 4]).min().unwrap_or(255)
    }

    let stroked = |sides: StrokeSides| {
        let (mut doc, _) = red_square_doc();
        // The one rect in the fixture — the artboard's only child.
        let ab = doc.get(doc.root()).unwrap().children()[0];
        let rect = doc.get(ab).unwrap().children()[0];
        doc.apply(&Transaction(vec![Operation::SetStrokes {
            id: rect,
            strokes: vec![Stroke {
                brush: Brush::Solid(Color::from_rgba8(0, 0, 0, 255)),
                width: 8.0,
                // Inside, so the whole stroke lands within the 100×100 viewport
                // rather than half of it falling off the edge.
                align: StrokeAlign::Inside,
                sides,
                ..Default::default()
            }],
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        VelloCpuRenderer::new().render_to_rgba(
            &doc,
            &res,
            &viewport(),
            &ondin_render::ImageStore::new(),
        )
    };

    // The whole outline first, as the control: all four edges dark.
    let (all, w, _) = stroked(StrokeSides::All);
    let w = w as usize;
    for (name, got) in [
        ("top", darkest(&all, w, 2, 20..80)),
        ("bottom", darkest(&all, w, 97, 20..80)),
        ("left", darkest_col(&all, w, 2, 20..80)),
        ("right", darkest_col(&all, w, 97, 20..80)),
    ] {
        assert!(
            got < 60,
            "All sides: the {name} edge came out {got}, not ink"
        );
    }

    // Top only: the top edge inks and the other three keep the red fill.
    let (top, _, _) = stroked(StrokeSides::Top);
    assert!(
        darkest(&top, w, 2, 20..80) < 60,
        "the top edge drew nothing: {}",
        darkest(&top, w, 2, 20..80)
    );
    for (name, got) in [
        ("bottom", darkest(&top, w, 97, 20..80)),
        ("left", darkest_col(&top, w, 2, 20..80)),
        ("right", darkest_col(&top, w, 97, 20..80)),
    ] {
        assert!(
            got > 150,
            "Top side: the {name} edge came out {got} — it should still be the red fill"
        );
    }

    // And a `Custom` set honours each side's own width: a 2px top against a 20px
    // bottom, so a row well inside the shape is ink at the bottom and fill at the
    // top.
    let (custom, _, _) = stroked(StrokeSides::Custom {
        top: 2.0,
        right: 0.0,
        bottom: 20.0,
        left: 0.0,
    });
    assert!(
        darkest(&custom, w, 10, 20..80) > 150,
        "a 2px top must not reach 10px in: {}",
        darkest(&custom, w, 10, 20..80)
    );
    assert!(
        darkest(&custom, w, 88, 20..80) < 60,
        "a 20px bottom must reach 12px up: {}",
        darkest(&custom, w, 88, 20..80)
    );
    assert!(
        darkest_col(&custom, w, 2, 30..70) > 150,
        "a side set to 0 must draw nothing at all"
    );
}

/// A dotted stroke has to come out as *dots* through the raster path too — the
/// zero-length dash the model stores draws nothing under a butt cap, so the
/// substitution has to happen before the backend sees it.
///
/// Measured as gaps along the edge: a solid line has none.
#[test]
fn a_dotted_stroke_rasterizes_as_dots_not_a_line() {
    use ondin_core::kurbo::Cap;
    use ondin_core::{Operation, Resolved, Stroke, Transaction};

    let (mut doc, _) = red_square_doc();
    let ab = doc.get(doc.root()).unwrap().children()[0];
    let rect = doc.get(ab).unwrap().children()[0];
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: rect,
        strokes: vec![Stroke {
            brush: Brush::Solid(Color::from_rgba8(0, 0, 0, 255)),
            width: 4.0,
            align: StrokeAlign::Inside,
            // A dot every 12 units. Butt-capped, which is the cap that cannot draw
            // a zero-length dash at all.
            dashes: vec![0.0, 12.0],
            cap: Cap::Butt,
            ..Default::default()
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );
    let w = w as usize;

    // Along the top edge: some ink, and some gaps between it.
    let row: Vec<u8> = (5..95).map(|x| rgba[(2 * w + x) * 4]).collect();
    assert!(
        row.iter().any(|r| *r < 60),
        "the dotted stroke drew nothing at all — the zero-length dash reached the \
         backend unsubstituted"
    );
    assert!(
        row.iter().any(|r| *r > 150),
        "the dotted stroke drew a solid line: {row:?}"
    );
}

/// A text node with `content` and `style`, on a white 100×100 artboard.
fn decorated_doc(content: &str, style: ondin_core::TextStyle) -> (Document, Resolved) {
    use ondin_core::node::TextSizing;
    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let text = ids.mint();
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
        Operation::SetFills {
            id: ab,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(255, 255, 255, 255)),
                visible: true,
            }],
        },
        Operation::CreateNode {
            id: text,
            parent: ab,
            index: 0,
            kind: NodeKind::Text {
                content: content.into(),
                style: Box::new(style),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

fn dark_pixels(doc: &Document, res: &Resolved) -> usize {
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        doc,
        res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );
    rgba.as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] > 128 && p[0] < 100 && p[1] < 100 && p[2] < 100)
        .count()
}

/// Decoration ink reaches the raster, and the four line styles are four
/// different amounts of it.
///
/// **Counting ink, not comparing images.** The point of the four styles is that
/// they break the band up differently, so a dotted underline must lay down less
/// ink than a solid one and a wavy one must lay down more than a dotted one. An
/// image comparison would pass on a solid line drawn four times.
#[test]
fn the_four_decoration_styles_lay_down_different_amounts_of_ink() {
    use ondin_core::{Decoration, Length, LineStyle, TextStyle};
    let base = TextStyle {
        font_family: "Inter".into(),
        font_size: 40.0,
        line_height: Some(Length::Em(1.0)),
        ..Default::default()
    };
    let plain = {
        let (doc, res) = decorated_doc("mmm", base.clone());
        dark_pixels(&doc, &res)
    };
    let mut ink = Vec::new();
    for style in LineStyle::ALL {
        let s = TextStyle {
            underline: Some(Decoration {
                // A thick band, so the difference between the patterns is many
                // pixels rather than a handful.
                thickness: Some(Length::Px(4.0)),
                style,
                ..Decoration::default()
            }),
            ..base.clone()
        };
        let (doc, res) = decorated_doc("mmm", s);
        ink.push((style, dark_pixels(&doc, &res)));
    }
    for (style, count) in &ink {
        assert!(
            *count > plain,
            "{style:?} added no ink: {count} vs {plain} undecorated"
        );
    }
    let of = |want: LineStyle| ink.iter().find(|(s, _)| *s == want).unwrap().1;
    assert!(
        of(LineStyle::Dotted) < of(LineStyle::Dashed),
        "dots are shorter than dashes: {:?}",
        ink
    );
    assert!(
        of(LineStyle::Dashed) < of(LineStyle::Solid),
        "a broken line is less ink than a whole one: {:?}",
        ink
    );
    assert!(
        of(LineStyle::Wavy) > of(LineStyle::Dotted),
        "a sine ribbon is continuous: {:?}",
        ink
    );
}

/// A decoration's own colour wins over the text's, and absence inherits it.
#[test]
fn a_decoration_colour_overrides_the_text_ink() {
    use ondin_core::{Decoration, Fill, Length, TextStyle};
    let style = TextStyle {
        font_family: "Inter".into(),
        font_size: 40.0,
        line_height: Some(Length::Em(1.0)),
        underline: Some(Decoration {
            thickness: Some(Length::Px(6.0)),
            color: Some(Color::from_rgba8(255, 0, 0, 255)),
            ..Decoration::default()
        }),
        ..Default::default()
    };
    let (mut doc, _) = decorated_doc("mmm", style);
    // Paint the glyphs blue, so red pixels can only be the decoration.
    let text = doc
        .get(doc.root())
        .unwrap()
        .children()
        .iter()
        .flat_map(|ab| doc.get(*ab).unwrap().children().iter().copied())
        .next()
        .unwrap();
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: text,
        fills: vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(0, 0, 255, 255)),
            visible: true,
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );
    let red = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60)
        .count();
    let blue = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[2] > 200 && p[0] < 60 && p[1] < 60)
        .count();
    assert!(red > 50, "expected a red underline, found {red} red pixels");
    assert!(blue > 50, "expected blue glyphs, found {blue} blue pixels");
}

/// **A very wide stroke must not leave a hole in the middle of its own ink.**
///
/// Reported with a 309pt centred stroke on a 200×120 shape: the ink stopped short of
/// its own centre and the fill showed through. The mechanism is the inward offset
/// turning inside out — past a point the inner contour passes out through the far
/// side of the shape, and the region it then encloses nets to winding zero — and the
/// answer is `scene::lay_stroke`, which expands the outline itself, drops the inner
/// contour and winds what is left alike so a non-zero fill is a plain union.
///
/// **The numbers are the reported ones, and the shape of the failure is in the
/// message.** Without the fix an ellipse holes from 202pt and a rect from 330pt, with
/// the hole's half-width growing as `width/2 − 100` and `width/2 − 160` — so 309pt on
/// a 200×120 ellipse loses a 108pt band at the centre. The stroke is drawn with **no
/// fill at all**, which is what makes the assertion about the stroke's own coverage
/// rather than about what is behind it, and it is checked at five widths spread
/// either side of both onsets because the failure was measured to be a band rather
/// than a threshold.
#[test]
fn a_very_wide_stroke_buries_the_middle_of_the_shape() {
    /// Canvas big enough for the widest stroke below to fit inside it.
    const N: u32 = 800;
    let shape_at = |kind: NodeKind, width: f64| {
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        let shape = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(f64::from(N), f64::from(N)),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: shape,
                parent: ab,
                index: 0,
                kind,
                transform: Some(ondin_core::kurbo::Affine::translate((300.0, 340.0))),
                name: None,
            },
            Operation::SetStrokes {
                id: shape,
                strokes: vec![ondin_core::Stroke {
                    brush: Brush::Solid(Color::from_rgba8(255, 0, 0, 255)),
                    width,
                    join: ondin_core::kurbo::Join::Round,
                    align: StrokeAlign::Center,
                    ..Default::default()
                }],
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(
            &doc,
            &res,
            &Viewport {
                view: Rect::new(0.0, 0.0, f64::from(N), f64::from(N)),
                pixel_size: (N, N),
            },
            &ondin_render::ImageStore::new(),
        );
        // The scanline through the shape's centre: the row the hole opens on.
        (0..w as usize)
            .map(|x| rgba[(400 * w as usize + x) * 4 + 3] != 0)
            .collect::<Vec<bool>>()
    };
    let shapes = [
        (
            "ellipse",
            NodeKind::Ellipse {
                size: Size::new(200.0, 120.0),
            },
        ),
        (
            "rect",
            NodeKind::Rect {
                size: Size::new(200.0, 120.0),
                corner_radii: RoundedRectRadii::default(),
            },
        ),
    ];
    for (name, kind) in shapes {
        for width in [130.0, 220.0, 309.0, 400.0, 600.0] {
            let row = shape_at(kind.clone(), width);
            let first = row.iter().position(|on| *on).expect("some ink");
            let last = row.iter().rposition(|on| *on).unwrap();
            let holes = row[first..=last].iter().filter(|on| !**on).count();
            assert_eq!(
                holes, 0,
                "{name} with a {width}pt centred stroke left {holes}pt of its own \
                 middle unpainted, between x={first} and x={last}"
            );
            // And the ink still reaches as far as it should, so the fix cannot pass
            // by drawing something smaller and solid.
            let want = (300.0 - width / 2.0).max(0.0) as usize;
            assert!(
                first.abs_diff(want) <= 1,
                "{name} at {width}pt: ink starts at {first}, expected {want}"
            );
        }
    }
}

// --- image fills (§5.5a) ----------------------------------------------------

/// A 2×2 PNG — red, green / blue, transparent — encoded here so the test carries
/// its own fixture and the pixels are known.
fn png_2x2() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 2, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            0, 0, 0, 0, // transparent
        ])
        .unwrap();
    }
    out
}

/// A 100×100 document whose 100×100 rectangle is filled with the 2×2 image,
/// scaled up by the framing so each source pixel covers a 50×50 quadrant.
fn image_doc(id: &ImageId) -> (Document, Resolved) {
    framed_doc(id, Size::new(100.0, 100.0), |_| {})
}

/// `image_doc` with the two things the framing tests vary: how big the shape
/// is, and how the picture sits in it.
///
/// The framing arrives as a closure over the whole `ondin_core::ImageRef`
/// rather than as a fit, because two of the four modes read a parameter beside
/// the mode — a tile without its scale is not a case anyone means to test.
///
/// **The artboard is sized to the shape**, because an artboard clips by default:
/// left at 100×100 it cut a 200-wide shape in half, and the letterbox test read
/// the clip as an empty band and passed for the wrong reason.
fn framed_doc(
    id: &ImageId,
    size: Size,
    frame: impl FnOnce(&mut ondin_core::ImageRef),
) -> (Document, Resolved) {
    let mut brush = image_brush(id.clone());
    let ondin_core::peniko::Brush::Image(img) = &mut brush else {
        unreachable!("image_brush builds an image brush")
    };
    frame(&mut img.image);
    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let rect = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard { size },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size,
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: None,
        },
        Operation::AddImage {
            id: id.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(png_2x2().into()),
                format: ImageFormat::Png,
                width: 2,
                height: 2,
            },
        },
        Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush,
                visible: true,
            }],
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

fn pixel(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// **An image fill paints the image's own pixels**, end to end: encoded bytes in
/// the document, decoded once by the store, resolved through
/// `color::brush_to_backend`, premultiplied into a `vello_cpu::ImageSource`, and
/// rasterized — **framed** across the shape rather than laid at one unit per
/// source pixel in its corner.
///
/// **Read at the quadrant centres, and that is the assertion.** A 2×2 picture in
/// a 100×100 shape is the case that separates a framing from the placeholder that
/// preceded it: unframed it draws two units square in the top-left with
/// `Extend::Pad` smearing the edge pixels over the other ninety-eight, so red
/// covers the whole left half and blue the whole bottom-left. Sampling at
/// (25,25) and (75,75) passes only when each source pixel has been scaled to its
/// own 50×50 quadrant.
#[test]
fn an_image_fill_is_framed_across_its_shape() {
    let id = ImageId("sha256:swatch".into());
    let (doc, res) = image_doc(&id);
    let mut images = ondin_render::ImageStore::new();
    images.prepare(&doc);
    assert_eq!(images.len(), 1, "the store decoded the document's image");

    let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(&doc, &res, &viewport(), &images);

    let [r, g, b, a] = pixel(&rgba, w, 25, 25);
    assert!(
        r > 200 && g < 60 && b < 60 && a > 200,
        "the top-left quadrant should be the image's red, got {:?}",
        [r, g, b, a]
    );
    let [r, g, b, _] = pixel(&rgba, w, 75, 25);
    assert!(
        g > 200 && r < 60 && b < 60,
        "the top-right quadrant should be the image's green, got {:?}",
        [r, g, b]
    );
    let [r, g, b, _] = pixel(&rgba, w, 25, 75);
    assert!(
        b > 200 && r < 60 && g < 60,
        "the bottom-left quadrant should be the image's blue, got {:?}",
        [r, g, b]
    );
    // The transparent source pixel stays transparent: alpha survived the decode
    // and the premultiply, which is the half of this a solid-colour image would
    // not have caught.
    let [_, _, _, a] = pixel(&rgba, w, 75, 75);
    assert!(
        a < 40,
        "the bottom-right quadrant should be transparent, got alpha {a}"
    );
}

/// **Fit leaves the frame empty where the picture does not reach — it does not
/// smear into it.**
///
/// The one thing about framing that is not arithmetic. peniko's `Extend` is pad,
/// repeat or reflect with no transparent option, so a contained picture's
/// letterbox is whatever its edge row happens to be unless the ink is clipped to
/// the picture's own rectangle. Asserted on a **2:1 shape with a square source**,
/// where the bands are 50 units wide either side and the smear would be a solid
/// red column on the left and a solid green one on the right — which is exactly
/// what the wrong version draws, and which no test of the middle would notice.
#[test]
fn a_fitted_image_leaves_its_letterbox_empty() {
    let id = ImageId("sha256:swatch".into());
    let (doc, res) = framed_doc(&id, Size::new(200.0, 100.0), |r| {
        r.fit = ondin_core::ImageFit::Fit;
    });
    let mut images = ondin_render::ImageStore::new();
    images.prepare(&doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 200.0, 100.0),
        pixel_size: (200, 100),
    };
    let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(&doc, &res, &vp, &images);

    // The picture is 100×100, centred at x ∈ 50..150. Its own quadrants first,
    // so the test is known to be looking at a drawing at all.
    let [r, g, b, _] = pixel(&rgba, w, 75, 25);
    assert!(
        r > 200 && g < 60 && b < 60,
        "the picture's red should sit at x=75, got {:?}",
        [r, g, b]
    );
    let [r, g, b, _] = pixel(&rgba, w, 125, 25);
    assert!(
        g > 200 && r < 60 && b < 60,
        "the picture's green should sit at x=125, got {:?}",
        [r, g, b]
    );
    // And the bands either side of it are untouched.
    for (x, y) in [(5, 25), (5, 75), (195, 25), (195, 75), (49, 50)] {
        let px = pixel(&rgba, w, x, y);
        assert_eq!(
            px[3], 0,
            "({x},{y}) is outside a fitted picture and must stay empty, got {px:?}"
        );
    }
}

/// **Tile repeats, which is the one fit that needs the sampler and not just the
/// transform.**
///
/// At scale 25 a 2×2 picture is a 50-unit tile, so the 100×100 shape holds two
/// across and two down and each source pixel is 25 units.
///
/// **Two wrong versions to fail against, and the coordinates are chosen to catch
/// both.** A framing that forgot the *sampler* pads instead of repeating, so the
/// second tile is a smear of the first's right-hand column — caught by (61,11)
/// and (11,61) being red rather than green and blue. A framing that forgot the
/// *transform* tiles at one unit per source pixel, a 2-unit tile, which lands on
/// the right colour at any even coordinate; so every read here is at an **odd**
/// one, where the untransformed tiling gives a different answer than the scaled
/// one at every single sample.
#[test]
fn a_tiled_image_repeats_across_its_shape() {
    let id = ImageId("sha256:swatch".into());
    let (doc, res) = framed_doc(&id, Size::new(100.0, 100.0), |r| {
        r.fit = ondin_core::ImageFit::Tile;
        r.tile_scale = 25.0;
    });
    let mut images = ondin_render::ImageStore::new();
    images.prepare(&doc);
    let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(&doc, &res, &viewport(), &images);

    for (x, y) in [(11, 11), (61, 11), (11, 61), (61, 61)] {
        let [r, g, b, _] = pixel(&rgba, w, x, y);
        assert!(
            r > 200 && g < 60 && b < 60,
            "({x},{y}) is a tile's red corner, got {:?}",
            [r, g, b]
        );
    }
    let [r, g, b, _] = pixel(&rgba, w, 39, 11);
    assert!(
        g > 200 && r < 60 && b < 60,
        "(39,11) is the first tile's green, got {:?}",
        [r, g, b]
    );
    let [_, _, _, a] = pixel(&rgba, w, 89, 89);
    assert!(
        a < 40,
        "(89,89) is the last tile's transparent corner, got alpha {a}"
    );
}

/// **A fill naming an image the store cannot produce draws the placeholder** —
/// which is grey and a cross, and is emphatically not black and not nothing.
///
/// This is the missing-image path: a dangling reference, a link whose file has
/// moved, bytes that would not decode. All three end at the same "no pixels" and
/// D179 makes them one drawing. Two failures are asserted against at once,
/// because the entry has been on both sides of it: a **black** rectangle, which
/// is what an opaque default fallback would paint, and **nothing at all**, which
/// is what the renderer did until this — §5.5a's "silent blank", and the
/// one that gets exported into a deliverable without anyone noticing.
///
/// It is asserted on `--png` rather than on the canvas deliberately: an export
/// is where a hole does the most damage, and the CPU backend shares the walk.
#[test]
fn an_unresolvable_image_fill_draws_the_placeholder_rather_than_a_hole() {
    let id = ImageId("sha256:swatch".into());
    let (doc, res) = image_doc(&id);
    // A store that never saw the document: every reference dangles.
    let empty = ondin_render::ImageStore::new();
    let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(&doc, &res, &viewport(), &empty);

    // A point in the left quadrant that is off *both* diagonals — (25,75) sits
    // exactly on the anti-diagonal, which is how the first spelling of this read
    // the cross and reported it as the ground. Well clear of the rim too.
    let px = pixel(&rgba, w, 25, 40);
    assert!(px[3] > 250, "the ground has to be opaque, got {px:?}");
    // **Against the one description, not against a literal**: that is what makes
    // this an end-to-end check — core's colour has to survive the walk, the
    // premultiply and the raster to land on a pixel. The `> 100` beside it is the
    // half that is about the *choice*: the entry this replaced was called
    // "…rather_than_black", and a placeholder that went dark would pass every
    // other assertion here.
    let want = ground_colour();
    assert!(
        px[..3].iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2),
        "the ground should be the placeholder's own {want:?}, got {px:?}"
    );
    assert!(want[0] > 100, "and that colour has to be light: {want:?}");
    // The cross runs corner to corner, so the diagonal is darker than the ground
    // it is drawn on — this is what separates a placeholder from a grey fill
    // somebody chose.
    let on_cross = pixel(&rgba, w, 50, 50);
    assert!(
        on_cross[0] < px[0],
        "(50,50) is on the diagonal and should be darker than the ground {px:?}, got {on_cross:?}"
    );
}

/// **Corrupt bytes are the same state as a missing image, not a crash.**
///
/// The document says PNG and carries something else — the case a truncated write
/// or a bad hand-edit produces. It has to reach the same unresolvable-image path
/// as a dead link rather than taking the renderer down or drawing noise, and
/// **that path now draws the placeholder** (§15 D179): the whole point of the
/// three states being one state is that this is the same picture a dead link
/// gets, so the assertion here is deliberately the same one.
#[test]
fn an_image_whose_bytes_do_not_decode_draws_the_placeholder_too() {
    let id = ImageId("sha256:broken".into());
    let (mut doc, _) = image_doc(&id);
    doc.apply(&Transaction(vec![
        Operation::RemoveImage { id: id.clone() },
        Operation::AddImage {
            id: id.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(b"PNG? no.".to_vec().into()),
                format: ImageFormat::Png,
                width: 2,
                height: 2,
            },
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let mut images = ondin_render::ImageStore::new();
    images.prepare(&doc);
    assert!(images.is_empty(), "nothing decoded");

    let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(&doc, &res, &viewport(), &images);
    let px = pixel(&rgba, w, 25, 40);
    assert!(px[3] > 250, "the ground has to be opaque, got {px:?}");
    let want = ground_colour();
    assert!(
        px[..3].iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2),
        "bytes that will not decode get the same {want:?} a dead link does, got {px:?}"
    );
}

/// The placeholder's ground, as the one description gives it (§15 D179).
fn ground_colour() -> [u8; 3] {
    let c = ondin_core::missing_placeholder(Rect::new(0.0, 0.0, 100.0, 100.0))
        .expect("a placeholder for a real box")
        .ground
        .to_rgba8();
    [c.r, c.g, c.b]
}

/// A boolean of `n` circles of radius 90 spaced round a circle of radius 120 —
/// the ring `boolean`'s module docs record, in `Ellipse` nodes and **with no paint
/// on any of it**.
fn ring_doc(op: ondin_core::BoolOp, n: usize) -> (Document, NodeId) {
    let mut ids = IdSource::new(0x0B00);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let b = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: b,
        parent: root,
        index: 0,
        kind: NodeKind::Boolean { op },
        transform: None,
        name: None,
    }];
    for i in 0..n {
        let a = i as f64 / n as f64 * std::f64::consts::TAU;
        let (cx, cy) = (200.0 + 120.0 * a.cos(), 200.0 + 120.0 * a.sin());
        ops.push(Operation::CreateNode {
            id: ids.mint(),
            parent: b,
            index: i,
            kind: NodeKind::Ellipse {
                size: Size::new(180.0, 180.0),
            },
            transform: Some(Affine::translate((cx - 90.0, cy - 90.0))),
            name: None,
        });
    }
    doc.apply(&Transaction(ops)).expect("build the ring");
    (doc, b)
}

/// **The canvas half of §15 D298, on real pixels**: a boolean the arithmetic gave
/// up on paints the placeholder, and one that is correctly empty still paints
/// nothing.
///
/// The `--png` path shares `scene.rs`'s walk with the GPU canvas, so this is the
/// closest a test gets to the thing the user looks at — and it is the assertion
/// the SVG test cannot make, since that writer is a second implementation rather
/// than the same one.
///
/// **The ring carries no fills and no strokes at all**, which is what makes the
/// first half say something. A boolean with no paint draws nothing however well
/// its arithmetic went, so ink at the centre can only have come from the
/// placeholder branch — it is drawn *instead of* the node's paint rather than as
/// part of it, which is the shape of the mistake that would put a grey slab under
/// every working boolean too.
#[test]
fn a_boolean_the_arithmetic_gave_up_on_paints_the_placeholder() {
    // The operands reach 210 in every direction from (200, 200), so this frames
    // the whole box at one pixel per two units.
    let viewport = Viewport {
        view: Rect::new(-10.0, -10.0, 410.0, 410.0),
        pixel_size: (210, 210),
    };
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let (doc, b) = ring_doc(ondin_core::BoolOp::Exclude, 40);
    // **Injected since flo_curves 0.8.1 fixed the defect this ring used to trip**
    // (`boolean::poison_next`, §15 D239). The assertion below is unchanged and is
    // still the one that matters: it says the fixture reached the failed state, by
    // whatever route, before anything is measured about pixels.
    ondin_core::boolean::poison_next(1);
    let res = Resolved::rebuild(&doc);
    std::panic::set_hook(hook);
    assert!(
        res.boolean_failed(b),
        "the ring has to be in the failed state, or this test is about nothing"
    );

    let store = ondin_render::ImageStore::new();
    let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(&doc, &res, &viewport, &store);
    // World (100, 200), which is fifty units clear of **both** diagonals — the
    // centre of the box is where the cross is, and sampling there measured the ink
    // and called it a missing ground.
    let px = pixel(&rgba, w, 55, 105);
    assert!(px[3] > 250, "the ground has to be opaque, got {px:?}");
    let want = ground_colour();
    assert!(
        px[..3].iter().zip(want).all(|(a, c)| a.abs_diff(c) <= 2),
        "a boolean that gave up gets the same {want:?} a dead link does, got {px:?}"
    );

    // The control, and it is the one that keeps this a rule rather than "paint grey
    // wherever a boolean has no outline": two circles 500 apart intersect to
    // nothing, correctly, and must stay invisible.
    let (mut doc, b) = ring_doc(ondin_core::BoolOp::Intersect, 0);
    let mut ids = IdSource::new(0x0C00);
    doc.apply(&Transaction(
        [0.0, 500.0]
            .into_iter()
            .enumerate()
            .map(|(i, x)| Operation::CreateNode {
                id: ids.mint(),
                parent: b,
                index: i,
                kind: NodeKind::Ellipse {
                    size: Size::new(20.0, 20.0),
                },
                transform: Some(Affine::translate((x, 0.0))),
                name: None,
            })
            .collect(),
    ))
    .expect("two circles far apart");
    let res = Resolved::rebuild(&doc);
    assert!(!res.boolean_failed(b), "nothing panicked here");
    let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(&doc, &res, &viewport, &store);
    assert_eq!(
        pixel(&rgba, w, 10, 10)[3],
        0,
        "an empty intersection is still empty — a placeholder here would cover \
         artwork behind a shape the user meant to be invisible"
    );
}

/// **The eight orientations are eight different drawings, on real pixels.**
///
/// `ImageOrient`'s own tests are about matrices; this is the one that says the
/// matrix reaches the raster. It runs end to end — the reference in the document,
/// the decode cache, `brush_to_backend`, the premultiplied `ImageSource` and the
/// rasterizer — for all eight symmetries, and the 2×2 fixture is what makes it
/// legible: each source pixel is a 50×50 quadrant, so a flip or a turn *permutes
/// the colours on screen* and the expected grid can be written out by hand rather
/// than derived from the code under test.
///
/// **The grids are hand-derived, which is the point.** Computing them from
/// `ImageOrient::affine` would assert that the code agrees with itself. Written
/// out, each one is a picture: `q1` turned clockwise takes the bottom-left blue
/// to the top-left, `q2` is the half turn, `q3 + mirror` is the transpose about
/// the main diagonal and therefore the only non-identity one that leaves red
/// where it is.
///
/// The eight being **distinct** is asserted too. A version that composed the
/// orientation into the brush transform but resolved it from the wrong field —
/// or dropped the mirror — collapses pairs of these onto each other, which every
/// individual grid would still catch but which is worth naming as the failure.
#[test]
fn each_of_the_eight_orientations_permutes_the_pixels_on_screen() {
    // The fixture, as it sits unturned: red and green on top, blue and a
    // transparent pixel below.
    const R: char = 'R';
    const G: char = 'G';
    const B: char = 'B';
    const T: char = 'T';
    // (quarters, mirrored) → the quadrants, top-left, top-right, bottom-left,
    // bottom-right.
    let cases: [(u8, bool, [char; 4]); 8] = [
        (0, false, [R, G, B, T]),
        (0, true, [G, R, T, B]),
        (1, false, [B, R, T, G]),
        (1, true, [T, G, B, R]),
        (2, false, [T, B, G, R]),
        (2, true, [B, T, R, G]),
        (3, false, [G, T, R, B]),
        (3, true, [R, B, G, T]),
    ];
    // Every grid has to be different, or the test would pass with two
    // orientations drawing the same thing.
    for (i, a) in cases.iter().enumerate() {
        for b in &cases[i + 1..] {
            assert_ne!(a.2, b.2, "two of the expected grids are the same picture");
        }
    }

    let id = ImageId("sha256:swatch".into());
    for (quarters, mirrored, want) in cases {
        let (doc, res) = framed_doc(&id, Size::new(100.0, 100.0), |r| {
            r.orient = ondin_core::ImageOrient { quarters, mirrored };
        });
        let mut images = ondin_render::ImageStore::new();
        images.prepare(&doc);
        let (rgba, w, _) = VelloCpuRenderer::new().render_to_rgba(&doc, &res, &viewport(), &images);

        for (i, (x, y)) in [(25, 25), (75, 25), (25, 75), (75, 75)]
            .into_iter()
            .enumerate()
        {
            let [r, g, b, a] = pixel(&rgba, w, x, y);
            let got = match (r, g, b, a) {
                (_, _, _, a) if a < 40 => T,
                (r, g, b, _) if r > 200 && g < 60 && b < 60 => R,
                (r, g, b, _) if g > 200 && r < 60 && b < 60 => G,
                (r, g, b, _) if b > 200 && r < 60 && g < 60 => B,
                _ => '?',
            };
            assert_eq!(
                got,
                want[i],
                "at {quarters} quarters, mirrored {mirrored}: quadrant ({x},{y}) \
                 drew {:?} — expected {} in the grid {want:?}",
                [r, g, b, a],
                want[i]
            );
        }
    }
}

// --- image adjustments (§5.5a) ---------------------------------------------

/// **An adjustment reaches the exported pixels**, end to end and through the
/// public entry point: a value on the fill, resolved into a pipeline by core,
/// applied to a derived buffer by the store `png` builds for itself,
/// premultiplied into `vello_cpu`'s own handle, rasterized, encoded, and read
/// back out of the file.
///
/// **The numbers are exact, not a direction.** Half a stop of light on a
/// saturated red is `255 × 2⁻¹ ≈ 128` in red and nothing anywhere else, and
/// asserting "it got darker" would pass against an adjustment applied to the
/// wrong channel, applied twice, or applied to a picture the framing then drew at
/// the wrong scale. The green quadrant is checked as well, because a matrix
/// composed with its rows in the wrong order dims red correctly and moves green
/// somewhere it should not go.
///
/// The transparent quadrant is the third claim: the pipeline runs on **straight**
/// alpha and leaves the alpha channel alone, so a picture with a soft edge does
/// not acquire a fringe.
#[test]
fn an_adjusted_image_fill_exports_the_adjusted_pixels() {
    let id = ImageId("sha256:swatch".into());
    let (doc, res) = framed_doc(&id, Size::new(100.0, 100.0), |r| {
        r.adjust.exposure = -1.0;
    });
    let (rgba, w) = decode_png(&png(&doc, &res, &viewport()));

    let [r, g, b, a] = pixel(&rgba, w, 25, 25);
    assert!(
        (118..=138).contains(&r) && g < 12 && b < 12 && a > 200,
        "half a stop of light on the image's red should be about 128, got {:?}",
        [r, g, b, a]
    );
    let [r, g, b, _] = pixel(&rgba, w, 75, 25);
    assert!(
        (118..=138).contains(&g) && r < 12 && b < 12,
        "and the green quadrant halves in green alone, got {:?}",
        [r, g, b]
    );
    let [_, _, _, a] = pixel(&rgba, w, 75, 75);
    assert!(
        a < 40,
        "the adjustment must not touch alpha, got {a} where the source is clear"
    );
}

/// **Two fills of one photograph are adjusted independently**, which is what
/// "per fill, not per image" means and is the reason the derived buffers are
/// keyed by the adjustment as well as by the id.
///
/// The failure this catches is a cache keyed too loosely: both layers paint from
/// whichever buffer was built first, so the *second* one silently wears the
/// first's settings — and on a document where two copies of a picture are meant
/// to look different, that is a bug that looks like the panel not committing.
#[test]
fn two_fills_of_one_picture_carry_their_own_adjustments() {
    let id = ImageId("sha256:swatch".into());
    let (mut doc, _) = framed_doc(&id, Size::new(100.0, 100.0), |r| {
        r.adjust.exposure = -1.0;
    });
    // A second rectangle over the bottom half of the artboard, showing the same
    // picture, untouched — and framed by `Crop` rather than `Fill`, so the
    // source's box maps onto the frame exactly and each texel has a centre the
    // probe can sit on. Under `Fill` the picture overflows a 2:1 frame and every
    // sample is a blend of two texels, which is how this test first read 190 for
    // a pure red.
    let ab = doc
        .get(doc.root())
        .and_then(|r| r.children().first().copied())
        .expect("the artboard");
    let mut ids = IdSource::new(400);
    let second = ids.mint();
    let mut brush = image_brush(id.clone());
    let ondin_core::peniko::Brush::Image(img) = &mut brush else {
        unreachable!("image_brush builds an image brush")
    };
    img.image.fit = ondin_core::ImageFit::Crop;
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: second,
            parent: ab,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 50.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((0.0, 50.0))),
            name: None,
        },
        Operation::SetFills {
            id: second,
            fills: vec![Fill {
                brush,
                visible: true,
            }],
        },
    ]))
    .expect("a second layer showing the same picture");
    let res = Resolved::rebuild(&doc);
    let (rgba, w) = decode_png(&png(&doc, &res, &viewport()));

    // Above: the dimmed copy's red quadrant. Below: the untouched copy's, which
    // covers the bottom half of the artboard at its own framing.
    let dim = pixel(&rgba, w, 25, 25)[0];
    let bright = pixel(&rgba, w, 25, 62)[0];
    assert!(
        (118..=138).contains(&dim),
        "the adjusted layer should be about half-lit, got {dim}"
    );
    assert!(
        bright > 240,
        "the untouched layer picked up the other one's exposure, got {bright}"
    );
}

/// An artboard on an opaque blue ground holding two overlapping layers: a red
/// **ellipse** at (10,10) 40×40, and a green rect at (30,30) 40×40 painted after
/// it. Returns `(doc, res, ellipse)`.
///
/// Every part of that is load-bearing for the test below. The rect **overlaps the
/// ellipse's box and is on top of it**, so a walk that framed a viewport on the
/// ellipse rather than scoping to it paints green where the ellipse should be red.
/// The ground is opaque, so the same walk fills the corners the ellipse leaves
/// empty. And the ellipse is an ellipse rather than a rect precisely so it *has*
/// corners it does not cover — a rect fills its own bounds and there would be
/// nowhere for the background to show through.
fn overlapping_siblings() -> (Document, Resolved, ondin_core::NodeId) {
    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let ellipse = ids.mint();
    let rect = ids.mint();
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
        Operation::SetFills {
            id: ab,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(0, 0, 255, 255)),
                visible: true,
            }],
        },
        Operation::CreateNode {
            id: ellipse,
            parent: ab,
            index: 0,
            kind: NodeKind::Ellipse {
                size: Size::new(40.0, 40.0),
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: ab,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((30.0, 30.0))),
            name: None,
        },
    ]))
    .unwrap();
    doc.apply(&Transaction(vec![
        Operation::SetFills {
            id: ellipse,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(220, 30, 40, 255)),
                visible: true,
            }],
        },
        Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(30, 200, 60, 255)),
                visible: true,
            }],
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, ellipse)
}

/// `png_of` paints the layers it was given and **nothing else** — not the sibling
/// on top of them, not the ground behind.
///
/// This is the whole reason the scoped walk exists, and it is the objection
/// `Item::CopyAsSvg` recorded against a *Copy as PNG* — "the raster path is
/// viewport-scoped with no way to say 'only these layers'". It is two faults under
/// one report and each needs its own assertion: framing a viewport on the ellipse
/// and rendering the page picks up the rect **in front** at one pixel and the
/// artboard **behind** at another, and a walk that fixed only one of those would
/// pass half of this.
#[test]
fn a_subtree_export_paints_neither_its_neighbours_nor_the_ground() {
    let (doc, res, ellipse) = overlapping_siblings();
    let bytes = ondin_export::png::png_of(&doc, &res, &[ellipse]).expect("the ellipse has bounds");
    let (rgba, w) = decode_png(&bytes);

    // Framed on the ellipse's own bounds at 1:1, so the image is exactly its size
    // and its local pixel (x, y) is world (10 + x, 10 + y).
    assert_eq!(w, 40, "the frame is the ellipse's bounds at 1:1");

    // World (35, 35): inside the ellipse (centre (30,30), radii 20 — the point is
    // ~7 units out) *and* inside the rect, which is painted after it. Red is the
    // scoped answer; green is what the whole-page walk draws over it.
    let under_the_rect = pixel(&rgba, w, 25, 25);
    assert!(
        under_the_rect[0] > 150 && under_the_rect[1] < 100,
        "the sibling in front was painted into a subtree export: {under_the_rect:?}"
    );

    // A corner of the box the ellipse does not cover. Transparent, because nothing
    // was asked for there — with the artboard in the walk it is solid blue.
    let corner = pixel(&rgba, w, 1, 1);
    assert_eq!(
        corner[3], 0,
        "the ground behind was painted into a subtree export: {corner:?}"
    );
}

/// A PNG this crate wrote, back to `(rgba, width)`.
fn decode_png(bytes: &[u8]) -> (Vec<u8>, u32) {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("a readable PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("a bounded buffer")];
    let info = reader.next_frame(&mut buf).expect("one frame");
    buf.truncate(info.buffer_size());
    (buf, info.width)
}

/// A mask reaches the pixels: the red square is kept where the mask covers it
/// and gone where it does not.
///
/// **The one mask test that goes all the way to a rasterizer**, which is what
/// makes it worth its runtime: the recording painter in `ondin-render` can say a
/// layer was pushed with a clip on it, and cannot say the clip was the right
/// shape, in the right space, or popped in the right order. Three samples, and
/// each answers a different way of getting it wrong — inside the mask (the clip
/// is not simply swallowing everything), outside it but inside the square (it
/// actually clips), and the mask's own ground (a mask paints nothing, so its
/// half is red-or-empty by what it lets through rather than by its own fill).
#[test]
fn a_mask_keeps_the_ink_it_covers_and_removes_the_rest() {
    let (mut doc, _res) = red_square_doc();
    let ab = doc.get(doc.root()).unwrap().children()[0];
    let square = doc.get(ab).unwrap().children()[0];

    // A 40×40 mask in the top-left corner, *under* the square that fills the
    // whole 100×100 board.
    let mut ids = IdSource::new(0x4A5C);
    let mask = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: mask,
        parent: ab,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(40.0, 40.0),
            corner_radii: RoundedRectRadii::default(),
        },
        transform: None,
        name: None,
    }]))
    .unwrap();
    assert_eq!(
        doc.get(ab).unwrap().children(),
        &[mask, square],
        "fixture: the mask is below the square it masks"
    );

    let render = |doc: &Document| {
        let res = Resolved::rebuild(doc);
        VelloCpuRenderer::new()
            .render_to_rgba(doc, &res, &viewport(), &ondin_render::ImageStore::new())
            .0
    };

    let before = render(&doc);
    assert!(
        pixel(&before, 100, 80, 80)[3] > 200,
        "fixture: the square covers the whole board until the flag is set"
    );

    doc.apply(&Transaction(vec![Operation::SetMask {
        id: mask,
        mask: true,
    }]))
    .unwrap();
    let after = render(&doc);

    let inside = pixel(&after, 100, 20, 20);
    assert!(
        (inside[0] as i32 - 220).abs() <= 2 && inside[3] > 200,
        "inside the mask the square's own red survives: {inside:?}"
    );
    for (x, y) in [(80u32, 80u32), (20, 80), (80, 20)] {
        let px = pixel(&after, 100, x, y);
        assert_eq!(
            px[3], 0,
            "outside the mask there is nothing left at ({x},{y}): {px:?}"
        );
    }
}

/// **An alpha mask fades what it masks instead of cutting it**, and reads only
/// the mask's alpha.
///
/// The same fixture as the shape-mask test above with one operation added, so the
/// difference in the pixels is the mode and nothing else. Three samples, each
/// answering a way of getting it wrong:
///
/// - **Inside, at 128.** Not 255 and not 0 — the content survives *in proportion*
///   to the mask's alpha, which is the whole difference from a clip. A shape mask
///   passes every other assertion here and fails this one.
/// - **Still red.** The mask is filled a half-transparent **blue**; if its colour
///   reached the page, this would be purple. It does not: an alpha mask reads
///   alpha, and that is also what separates it from the luminance mode this app
///   does not have (§15 D288) — a luminance mask would have read that blue as a
///   brightness and produced a third number again.
/// - **Outside, gone.** The mask still bounds the run; it fades within its own
///   shape rather than everywhere.
#[test]
fn an_alpha_mask_fades_what_it_masks_and_reads_only_its_alpha() {
    let (mut doc, _res) = red_square_doc();
    let ab = doc.get(doc.root()).unwrap().children()[0];
    let square = doc.get(ab).unwrap().children()[0];

    let mut ids = IdSource::new(0xA19A);
    let mask = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: mask,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: None,
        },
        // Half-transparent, and **blue** — the colour is the control.
        Operation::SetFills {
            id: mask,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(0, 0, 255, 128)),
                visible: true,
            }],
        },
        Operation::SetMask {
            id: mask,
            mask: true,
        },
        Operation::SetMaskMode {
            id: mask,
            mode: ondin_core::MaskMode::Alpha,
        },
    ]))
    .unwrap();
    assert_eq!(
        doc.get(ab).unwrap().children(),
        &[mask, square],
        "fixture: the mask is below the square it masks"
    );

    let res = Resolved::rebuild(&doc);
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &viewport(),
        &ondin_render::ImageStore::new(),
    );

    let inside = pixel(&rgba, 100, 20, 20);
    assert!(
        (inside[3] as i32 - 128).abs() <= 2,
        "the content survives at the mask's own alpha, not whole and not gone: {inside:?}"
    );
    assert!(
        inside[0] > 200 && inside[2] < 60,
        "and in its own red — the mask's blue is not composited, only its alpha: {inside:?}"
    );
    assert_eq!(
        pixel(&rgba, 100, 80, 80)[3],
        0,
        "outside the mask's own shape there is still nothing"
    );
}

/// **A gradient on railed type is one gradient across the whole node, not one
/// per letter** (§15 D405).
///
/// **This goes to the pixels because nothing shallower can see it.** A bent glyph
/// is drawn under its own transform — that is what carries the rotation — and a
/// brush is authored in the node's local space, so drawing a bent run *as glyphs*
/// hands every letter the gradient's own origin and every letter comes out in the
/// first stop. `scene::draw_run` fills the run's outline instead when the brush is
/// not a solid, and the only statement of the difference is what colour the ink
/// actually is.
///
/// ⚠️ **Measured by flipping it, not predicted.** With the outline arm disabled,
/// this fixture renders the entire word in flat red — the first stop, repeated
/// thirty times — where the correct build runs red to blue along the rail. The
/// two ends are sampled rather than the middle for exactly that reason: a
/// per-glyph gradient is not *subtly* wrong, it is the start colour everywhere,
/// so the end of the word is where the two builds differ most.
#[test]
fn a_gradient_on_railed_type_runs_across_the_whole_node() {
    use ondin_core::GeometryPatch;
    use ondin_core::kurbo::{BezPath, Point};
    use ondin_core::peniko::Gradient;

    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let text = ids.mint();

    // A rail straight across the middle of the board: straight so that the
    // *placement* cannot be what this test is about, while the glyphs still take
    // the bent arm — `draw_run` splits on the brush, and a straight rail leaves
    // every rotation at zero, so this fixture would take the fast path and prove
    // nothing. Hence the gentle curve.
    let mut rail = BezPath::new();
    rail.move_to((10.0, 60.0));
    rail.curve_to((60.0, 40.0), (140.0, 80.0), (190.0, 60.0));

    let gradient = Gradient::new_linear(Point::new(10.0, 60.0), Point::new(190.0, 60.0))
        .with_stops([
            (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
            (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
        ]);

    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(200.0, 120.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: text,
            parent: ab,
            index: 0,
            kind: NodeKind::Text {
                content: "MMMMMMMMMMMMMM".into(),
                style: Box::new(ondin_core::TextStyle {
                    font_family: "Inter".into(),
                    font_size: 18.0,
                    weight: 700,
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: ondin_core::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetGeometry {
            id: text,
            geometry: GeometryPatch::TextPath(Some(rail)),
        },
        Operation::SetFills {
            id: text,
            fills: vec![Fill {
                brush: Brush::Gradient(gradient.into()),
                visible: true,
            }],
        },
    ]))
    .expect("a railed, gradient-filled text node");
    let res = Resolved::rebuild(&doc);

    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 200.0, 120.0),
        pixel_size: (400, 240),
    };
    let (rgba, w) = decode_png(&png(&doc, &res, &vp));

    // The reddest and the bluest pixel anywhere in the frame. Sampling extremes
    // rather than named coordinates keeps this from depending on where a
    // particular `M` landed, which is the shaper's business and not this test's.
    let (mut most_red, mut most_blue) = (0_i32, 0_i32);
    for y in 0..(vp.pixel_size.1) {
        for x in 0..w {
            let p = pixel(&rgba, w, x, y);
            if p[3] < 200 {
                continue;
            }
            most_red = most_red.max(i32::from(p[0]) - i32::from(p[2]));
            most_blue = most_blue.max(i32::from(p[2]) - i32::from(p[0]));
        }
    }
    assert!(
        most_red > 100,
        "the start of the word must be red: {most_red}"
    );
    assert!(
        most_blue > 100,
        "and the end of it blue — a per-glyph gradient makes every letter the \
         first stop, so this is the assertion that fails: {most_blue}"
    );
}

/// **An elliptical radial gradient is painted elliptical** (§15 D412), which is
/// the only assertion that can say the feature works: everything else about it —
/// the field, the serde shape, the reader — is machinery in service of pixels.
///
/// The gradient is a *circle* of radius 20 centred at `(50, 50)` in its own
/// space, under a squash **about that same point**. So what should reach the page
/// is an ellipse centred at `(50, 50)` with an x-radius of 20 and a y-radius of
/// 40, and the measurement is the half-ramp point along each axis: red falls
/// below half at **10** pixels out horizontally and **20** vertically.
///
/// ⚠️ **Asserted as the *ratio*, not as either distance.** The absolute numbers
/// depend on the ramp's interpolation and on where vello puts the boundary
/// texel, neither of which this test is about; the ratio is exactly 2 by
/// construction and survives both. Rendered through the CPU backend because that
/// is the one a test can run — and it is also the backend where the bug would
/// have been silent, since `ondin export` has no watchdog and no eye on it.
///
/// ⚠️ **The squash is centred on the sample point on purpose, and the first
/// version of this test was not.** With a bare `scale(1, 2)` over a circle at
/// `(50, 25)` the transform *moves* the gradient as well as shaping it, so
/// dropping it in `brush_to_backend` failed the **guard** — nothing reddish
/// anywhere near the centre — rather than the ratio. A true failure, and about
/// the wrong thing: it would have passed a backend that applied the translation
/// and dropped the scale. Centred, the flip changes only the shape, and it fails
/// where it should, at `ratio 1.000`.
#[test]
fn a_squashed_radial_gradient_paints_an_ellipse_rather_than_a_circle() {
    use ondin_core::GradientBrush;
    use ondin_core::peniko::Gradient;

    let mut ids = IdSource::new(0xE117);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, rect) = (ids.mint(), ids.mint());
    let square = |id: NodeId, parent: NodeId, kind: NodeKind| Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind,
        transform: None,
        name: None,
    };
    doc.apply(&Transaction(vec![
        square(
            ab,
            root,
            NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
        ),
        square(
            rect,
            ab,
            NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
            },
        ),
        Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: Brush::Gradient(GradientBrush {
                    gradient: Gradient::new_radial((50.0, 50.0), 20.0).with_stops([
                        (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
                        (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
                    ]),
                    transform: Affine::translate((0.0, 50.0))
                        * Affine::scale_non_uniform(1.0, 2.0)
                        * Affine::translate((0.0, -50.0)),
                    opacity: 1.0,
                }),
                visible: true,
            }],
        },
    ]))
    .expect("a squashed radial gradient over the whole artboard");
    let res = Resolved::rebuild(&doc);
    let (rgba, w) = decode_png(&png(&doc, &res, &viewport()));

    // How far from the centre the ramp passes its half-way point, along each
    // axis. Walking out rather than sampling one texel keeps this off the exact
    // pixel the boundary lands in.
    let reddish = |x: u32, y: u32| pixel(&rgba, w, x, y)[0] > 128;
    let across = (1..50).take_while(|d| reddish(50 + d, 50)).count();
    let down = (1..50).take_while(|d| reddish(50, 50 + d)).count();
    assert!(
        across > 5 && down > 5,
        "both axes must be inside the ramp at all: {across} across, {down} down"
    );
    let ratio = down as f64 / across as f64;
    assert!(
        (ratio - 2.0).abs() < 0.15,
        "the gradient is twice as tall as it is wide, which is what the brush \
         transform is for — a circle would give 1.0: {across} across, {down} \
         down, ratio {ratio:.3}"
    );
}

/// A `Union` boolean of two overlapping 100×100 rects, then a 60×60 rect painted
/// **after** it. Both filled, so both have ink to count.
fn boolean_then_sibling(effects: Vec<ondin_core::Effect>) -> (Document, Resolved) {
    let mut ids = IdSource::new(0xB001);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (b, a1, a2, after) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    let fill = |id| Operation::SetFills {
        id,
        fills: vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(220, 30, 40, 255)),
            visible: true,
        }],
    };
    let rect = |id, parent, index, w: f64, h: f64, at: (f64, f64)| Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
        },
        transform: Some(Affine::translate(at)),
        name: None,
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: b,
            parent: root,
            index: 0,
            kind: NodeKind::Boolean {
                op: ondin_core::BoolOp::Union,
            },
            transform: None,
            name: None,
        },
        rect(a1, b, 0, 100.0, 100.0, (10.0, 10.0)),
        rect(a2, b, 1, 100.0, 100.0, (60.0, 10.0)),
        rect(after, root, 1, 60.0, 60.0, (20.0, 130.0)),
        fill(b),
        fill(after),
        Operation::SetEffects { id: b, effects },
    ]))
    .expect("build");
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// **`[S11.1-L1-01]`'s loss: a drop shadow on a boolean took the boolean *and
/// every layer painted after it* off the canvas.**
///
/// `paint_node` pushes a node's effect layer near the top and pops it at the very
/// end; the `Boolean` arm returns from the middle, and it unwound the clip layer,
/// the mask run and (by a `debug_assert`) the outer opacity layer — three of the
/// four. Everything drawn afterwards therefore landed inside a layer whose bounds
/// were the boolean's own ink box and which was never composited.
///
/// ⚠️ **The sibling is the assertion that names the damage.** A boolean that
/// vanishes reads as "the effect is broken"; a boolean that takes an unrelated
/// rectangle with it is a different bug, and it is the one that was happening.
/// So it is asserted first.
///
/// **The control is the same effect on a `Rect`**, and it is what makes this the
/// early `return` rather than `any_ink` or `effect_bounds`: the identical stack
/// balances and draws on any other kind.
///
/// Flip-check, run: taking the `pop_layer` back out of the boolean arm fails at
/// *"the layer painted after a boolean is still on the canvas (control drew
/// 3600)"* — and the boolean's own assertion under it would fail too, so the
/// order is what decides which of the two the next reader is shown.
#[test]
fn an_effect_on_a_boolean_does_not_take_the_rest_of_the_canvas_with_it() {
    let shadow = ondin_core::Effect::new(ondin_core::effect::EffectKind::DropShadow(
        ondin_core::effect::Shadow {
            offset: ondin_core::kurbo::Vec2::new(0.0, 4.0),
            blur: 4.0,
            spread: 0.0,
            color: Color::from_rgba8(0, 0, 0, 128),
        },
    ));
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 200.0, 200.0),
        pixel_size: (200, 200),
    };
    // Counted in two boxes, so the boolean's ink and the sibling's are separable
    // however the shadow spreads: the union sits in y 10..110 and the sibling in
    // y 130..190.
    let ink = |rgba: &[u8], y0: usize, y1: usize| {
        (y0..y1)
            .flat_map(|y| (0..200).map(move |x| (x, y)))
            .filter(|(x, y)| rgba[((y * 200) + x) * 4 + 3] > 20)
            .count()
    };
    let count = |effects: Vec<ondin_core::Effect>| {
        let (doc, res) = boolean_then_sibling(effects);
        let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
            &doc,
            &res,
            &vp,
            &ondin_render::ImageStore::new(),
        );
        (ink(&rgba, 0, 120), ink(&rgba, 125, 200))
    };

    let (plain_boolean, plain_after) = count(Vec::new());
    assert!(
        plain_boolean > 1000 && plain_after > 1000,
        "the control draws"
    );

    let (with_effect, after_effect) = count(vec![shadow]);
    assert!(
        after_effect > 0,
        "the layer painted after a boolean is still on the canvas \
         (control drew {plain_after})"
    );
    assert!(
        with_effect > 0,
        "and so is the boolean itself (control drew {plain_boolean})"
    );
}

/// **`[S11.2-L1-03]`'s loss: the CPU backend drew type it could not paint in the
/// *previous* node's colour.**
///
/// `apply_paint` answers `false` when a brush resolves to nothing — a fill whose
/// image the store has no pixels for — and `fill_path` and `stroke_path` both
/// return on it. `draw_text` called it as a statement. `vello_cpu`'s paint is
/// context state, so the word was drawn in whatever the last `set_paint` left
/// there: the red rectangle above it.
///
/// ⚠️ **The colour is the assertion, not the pixel count.** "The text drew
/// nothing" and "the text drew in its own missing-image colour" are both
/// plausible right answers to the same count; only *whose* red it is says the
/// paint leaked from the node before. So the fixture puts a red rect above and
/// counts red below it, and the count that matters is `0`.
///
/// The GPU backend already had this guard, and its comment states the rule —
/// type filled with an unresolvable image draws nothing, as a shape does
/// (§15 D179). This is the same guard twice next door: in the sibling method and
/// in the sibling backend.
///
/// Flip-check, run: calling `apply_paint` as a statement again fails at *"no
/// pixel below the rect may be the rect's red"*, `left: 4820 right: 0` — the
/// leaked ink, counted, in the message.
#[test]
fn text_whose_picture_is_missing_draws_nothing_rather_than_the_last_paint() {
    use ondin_core::node::{TextSizing, TextStyle};
    let mut ids = IdSource::new(0x7E27);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (rect, text) = (ids.mint(), ids.mint());
    let missing = ImageId("no-such-image".into());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(300.0, 60.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(255, 0, 0, 255)),
                visible: true,
            }],
        },
        Operation::CreateNode {
            id: text,
            parent: root,
            index: 1,
            kind: NodeKind::Text {
                content: "MISSING".into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: 48.0,
                    weight: 800,
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
            transform: Some(Affine::translate((0.0, 80.0))),
            name: None,
        },
        // The node's *only* fill is a picture nothing can supply.
        Operation::SetFills {
            id: text,
            fills: vec![Fill {
                brush: image_brush(missing),
                visible: true,
            }],
        },
    ]))
    .expect("build");
    let res = Resolved::rebuild(&doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 300.0, 200.0),
        pixel_size: (300, 200),
    };
    let (rgba, _, _) =
        VelloCpuRenderer::new().render_to_rgba(&doc, &res, &vp, &ondin_render::ImageStore::new());
    let at = |x: usize, y: usize| {
        let i = ((y * 300) + x) * 4;
        (rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3])
    };
    // The fixture is in the state under test: the rect really did paint red.
    assert_eq!(at(150, 30), (255, 0, 0, 255), "the rect is the control");

    let red_below = (70..200)
        .flat_map(|y| (0..300).map(move |x| at(x, y)))
        .filter(|&(r, g, b, a)| a > 20 && r > 200 && g < 60 && b < 60)
        .count();
    assert_eq!(
        red_below, 0,
        "no pixel below the rect may be the rect's red"
    );
}

/// **A gradient whose stop offsets descend is not a wrong ramp, it is *no* ramp.**
///
/// `[S11.1-L1-02]` (High), and the settling of `[S7.2-L…]`'s hand-off: that pass
/// measured that `<stop offset="0"/><stop offset="0.8"/><stop offset="0.2"/><stop
/// offset="1"/>` imported as `[0, 0.8, 0.2, 1]` where SVG requires each offset to
/// be clamped up to the running maximum, and could not measure the picture. The
/// picture was **a flat fill of the first stop's colour in all 100 columns** — a
/// descent of one part in ten million was enough, while *equal* offsets (exactly
/// what SVG's clamp produces) were always fine.
///
/// The two rows this asserts identical are the same file read two ways: what
/// Ondin stored, and what SVG says the file means. §15 D455 makes the first
/// become the second at both doors — `svg_in` for an import, and
/// `color::brush_to_backend` for a hand-edited `.ondin` and every file imported
/// before today.
///
/// ⚠️ **Three controls, and the middle one is the one with teeth.** An ascending
/// ramp must be untouched (nothing here may reorder a legitimate gradient); a
/// ramp with *equal* offsets must be untouched (it is already monotonic, and it
/// is what the fix produces, so this says the fix is idempotent); and the sorted
/// reading `[0, 0.2, 0.8, 1]` — what `panels::paint::with_stops` produces if the
/// user touches a colour — must be **different**, because a sort and a clamp are
/// two different pictures and picking the clamp is the decision D455 records.
///
/// ⚠️ **Flipped** by removing the `make_stops_monotonic` call in
/// `color::brush_to_backend`: fails at **column 0**, `(255, 0, 0)` against the
/// ramp's `(253, 2, 0)`. Column 0 rather than somewhere in the middle, which is
/// worth knowing: the flat fill is the *first stop's* colour, so it agrees with
/// the correct ramp nowhere at all, not even where the ramp starts — the ramp has
/// already left the pure stop colour by its first pixel.
///
/// ⚠️ **This does not exercise the importer's half of D455.** Both doors apply
/// the same rule and the render one is downstream, so this test is green with
/// `svg_in`'s call deleted; `a_descending_stop_list_is_clamped_up_on_import` is
/// the one that covers it.
#[test]
fn a_gradient_with_descending_stops_draws_the_ramp_svg_says_it_means() {
    fn row_of(offsets: [f32; 4]) -> Vec<(u8, u8, u8)> {
        let mut ids = IdSource::new(11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        let rect = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 20.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: ab,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: None,
                name: None,
            },
        ]))
        .unwrap();

        let colors = [
            Color::from_rgba8(255, 0, 0, 255),
            Color::from_rgba8(0, 255, 0, 255),
            Color::from_rgba8(0, 0, 255, 255),
            Color::from_rgba8(255, 255, 255, 255),
        ];
        let mut g = ondin_core::peniko::Gradient::new_linear((0.0, 0.0), (100.0, 0.0));
        g.stops.clear();
        for (&offset, color) in offsets.iter().zip(colors) {
            g.stops.push(ondin_core::peniko::ColorStop {
                offset,
                color: ondin_core::peniko::color::DynamicColor::from_alpha_color(color),
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

        let res = Resolved::rebuild(&doc);
        let vp = Viewport {
            view: Rect::new(0.0, 0.0, 100.0, 20.0),
            pixel_size: (100, 20),
        };
        let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
            &doc,
            &res,
            &vp,
            &ondin_render::ImageStore::new(),
        );
        (0..100)
            .map(|x| {
                let i = ((10 * 100) + x) * 4;
                (rgba[i], rgba[i + 1], rgba[i + 2])
            })
            .collect()
    }

    let stored = row_of([0.0, 0.8, 0.2, 1.0]);
    let meant = row_of([0.0, 0.8, 0.8, 1.0]);

    // The fixture is in the state this test is about, or it is about nothing:
    // the SVG-correct reading must actually be a ramp rather than a flat fill.
    assert!(
        meant[0] != meant[50] && meant[50] != meant[99],
        "fixture: the correct reading must vary across the row, got {:?} {:?} {:?}",
        meant[0],
        meant[50],
        meant[99]
    );

    for (x, (got, want)) in stored.iter().zip(&meant).enumerate() {
        assert_eq!(
            got, want,
            "column {x}: a descending ramp must draw what SVG says the file means. \
             Before D455 every column of `stored` was the first stop's colour."
        );
    }

    // An ascending ramp and an already-equal one are both untouched, and the
    // sorted reading is deliberately a different picture from the clamped one.
    let ascending = row_of([0.0, 0.2, 0.8, 1.0]);
    assert_ne!(
        ascending, meant,
        "a sort and a clamp disagree on this input, and D455 picks the clamp"
    );
    assert_eq!(row_of([0.0, 0.8, 0.8, 1.0]), meant, "idempotent");
}
