//! Code export's fidelity bar: each fixture's page, screenshotted by a headless
//! Chrome, against the CPU renderer's raster of the same layers. Anything the
//! export does not report as approximated must match.
//!
//! The browser is `ONDIN_BROWSER`, else Chrome, Chromium or Edge where they are
//! usually installed. With none found the comparisons are skipped with a line on
//! stderr — unless `CI` is set, where a missing browser fails the run.
//!
//! The comparison composites both images over white and takes the mean absolute
//! channel error per 8×8 cell, so an edge antialiased differently by the two
//! rasterizers is a few low cells while a misplaced or miscoloured layer is a run
//! of high ones.

use ondin_core::container::{
    AlignItems, Display, Flex, FlexDirection, Grid, Insets, JustifyContent, LayoutItem, LengthPct,
    Track, TrackBreadth, TrackSize,
};
use ondin_core::kurbo::{Affine, Join, Point, RoundedRectRadii, Size, Vec2};
use ondin_core::peniko::{
    Color, ColorStop, Extend, Gradient, GradientKind, LinearGradientPosition,
    RadialGradientPosition, SweepGradientPosition,
};
use ondin_core::variant::{self, PropKind, PropValue};
use ondin_core::{
    Brush, CharAttr, Document, Effect, EffectKind, Fill, GradientBrush, IdSource, NodeId, NodeKind,
    Operation, Resolved, Shadow, Stroke, StrokeAlign, StrokeSides, TextAlign, TextSizing,
    TextStyle, Transaction, VerticalAlign, keyed_by_position,
};
use ondin_export::code::CodeExport;
use ondin_export::code::fidelity::{
    self, Image, MAX_CELL_ERROR, MAX_MEAN_ERROR, blurred, cell_errors,
};
use ondin_export::code::html::{html_of, html_of_components};
use std::path::PathBuf;

// The limits are `fidelity`'s, which the `ondin check code` command holds a page
// to as well. Matching pages peak at 14.6 (an inline-SVG inner shadow) and 12.1
// (rotated edges). flip: a blur written as its radius → 23.6 in `effects`; a sweep
// started at +x → 126 in `gradients`; a centred stroke written as an outside one →
// 115 in `strokes`.

/// A document under construction: a root and helpers that apply one op each.
struct Doc {
    doc: Document,
    ids: IdSource,
}

impl Doc {
    fn new() -> Self {
        let mut ids = IdSource::new(0xC0DE);
        let root = ids.mint();
        Doc {
            doc: Document::new(root),
            ids,
        }
    }

    fn apply(&mut self, ops: Vec<Operation>) {
        self.doc
            .apply(&Transaction(ops))
            .expect("fixture op applies");
    }

    fn add(&mut self, parent: Option<NodeId>, kind: NodeKind, at: Affine, name: &str) -> NodeId {
        let id = self.ids.mint();
        let parent = parent.unwrap_or(self.doc.first_page());
        let index = self.doc.get(parent).map_or(0, |p| p.children().len());
        self.apply(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(at),
            name: Some(name.into()),
        }]);
        id
    }

    fn frame(
        &mut self,
        parent: Option<NodeId>,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        name: &str,
    ) -> NodeId {
        let id = self.add(
            parent,
            NodeKind::Artboard {
                size: Size::new(w, h),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            Affine::translate((x, y)),
            name,
        );
        self.fill(id, solid(240, 240, 236, 255));
        id
    }

    fn rect(&mut self, parent: NodeId, x: f64, y: f64, w: f64, h: f64, r: f64) -> NodeId {
        self.add(
            Some(parent),
            NodeKind::Rect {
                size: Size::new(w, h),
                corner_radii: RoundedRectRadii::from_single_radius(r),
                corner_smoothing: 0.0,
            },
            Affine::translate((x, y)),
            "Rect",
        )
    }

    fn fill(&mut self, id: NodeId, brush: Brush) {
        self.fills(id, vec![brush]);
    }

    fn fills(&mut self, id: NodeId, brushes: Vec<Brush>) {
        self.apply(vec![Operation::SetFills {
            id,
            fills: keyed_by_position(brushes.into_iter().map(|b| Fill {
                brush: b.into(),
                visible: true,
            })),
        }]);
    }

    fn stroke(&mut self, id: NodeId, width: f64, align: StrokeAlign, sides: StrokeSides) {
        self.apply(vec![Operation::SetStrokes {
            id,
            strokes: keyed_by_position([Stroke {
                brush: solid(20, 20, 30, 255).into(),
                width,
                width_var: None,
                join: ondin_core::kurbo::Join::Miter,
                cap: ondin_core::kurbo::Cap::Butt,
                dashes: vec![],
                dash_offset: 0.0,
                sides,
                miter_limit: 4.0,
                dash_fit: false,
                align,
                visible: true,
            }]),
        }]);
    }

    /// `Self::stroke` on all sides, with `join` at its corners.
    fn stroke_joined(&mut self, id: NodeId, width: f64, align: StrokeAlign, join: Join) {
        self.stroke(id, width, align, StrokeSides::All);
        let mut strokes = self.doc.get(id).expect("stroked").paint().strokes.clone();
        strokes[0].value.join = join;
        self.apply(vec![Operation::SetStrokes { id, strokes }]);
    }

    fn effects(&mut self, id: NodeId, kinds: Vec<EffectKind>) {
        self.apply(vec![Operation::SetEffects {
            id,
            effects: keyed_by_position(kinds.into_iter().map(Effect::new)),
        }]);
    }

    fn done(self) -> (Document, Resolved) {
        let res = Resolved::rebuild(&self.doc);
        (self.doc, res)
    }
}

fn solid(r: u8, g: u8, b: u8, a: u8) -> Brush {
    Brush::Solid(Color::from_rgba8(r, g, b, a))
}

fn ramp(kind: GradientKind, extend: Extend) -> Brush {
    Brush::Gradient(GradientBrush {
        gradient: Gradient {
            kind,
            extend,
            stops: [
                ColorStop {
                    offset: 0.0,
                    color: Color::from_rgba8(230, 60, 40, 255).into(),
                },
                ColorStop {
                    offset: 0.5,
                    color: Color::from_rgba8(250, 210, 60, 255).into(),
                },
                ColorStop {
                    offset: 1.0,
                    color: Color::from_rgba8(40, 90, 220, 255).into(),
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

// ---------------------------------------------------------------------------
// The browser
// ---------------------------------------------------------------------------

/// The browser, or `None` after saying why the comparison is skipped. Fails on CI.
fn browser_or_skip(test: &str) -> Option<PathBuf> {
    let b = fidelity::browser();
    if b.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "{test}: no headless browser found (set ONDIN_BROWSER)"
        );
        eprintln!("{test}: skipped, no headless browser found (set ONDIN_BROWSER)");
    }
    b
}

/// A fresh folder under the system temp dir for one test.
fn scratch(test: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ondin-code-fidelity-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// The worst cells, worst first, and the whole image's mean channel error.
fn compare(a: &Image, b: &Image) -> (Vec<fidelity::Cell>, f64) {
    let (mut cells, mean) = cell_errors(a, b);
    cells.sort_by(|x, y| y.0.total_cmp(&x.0));
    cells.truncate(5);
    (cells, mean)
}

/// Export the whole document, screenshot it, and hold it to the canvas.
fn holds(test: &str, doc: &Document, res: &Resolved) {
    holds_within(test, doc, res, 0, MAX_CELL_ERROR);
}

/// Text's limit, after two passes of `blurred`: Chrome and vello antialias glyph
/// edges differently even where every glyph is placed alike, and each platform's
/// Chrome differently again — Linux peaks at 9.1 and macOS at 19.1 on fixtures
/// Windows holds under 6.0. Only Windows is tight enough for a one-pixel drop of a
/// line to show; elsewhere `MAX_TEXT_MEAN_ERROR` is what still catches a layout
/// shift. flip (Windows): an automatic line height written as `normal` → 9.8.
const MAX_TEXT_CELL_ERROR: f64 = if cfg!(target_os = "macos") {
    22.0
} else if cfg!(target_os = "linux") {
    12.0
} else {
    8.0
};

/// The mean limit for a blurred comparison, which only text takes. macOS Chrome's
/// glyph smoothing adds about 0.8 to every text fixture's mean, which carries
/// `decorations` (1.2 on Windows) past `MAX_MEAN_ERROR`.
const MAX_TEXT_MEAN_ERROR: f64 = if cfg!(target_os = "macos") {
    2.5
} else {
    MAX_MEAN_ERROR
};

/// `holds`, comparing after `passes` of `blurred` with `limit` per cell.
fn holds_within(test: &str, doc: &Document, res: &Resolved, passes: usize, limit: f64) {
    holds_as(test, doc, res, passes, limit, html_of);
}

/// `holds_within` for the page `export` writes.
fn holds_as(
    test: &str,
    doc: &Document,
    res: &Resolved,
    passes: usize,
    limit: f64,
    export: fn(&Document, &Resolved, &[NodeId]) -> CodeExport,
) {
    let origins: Vec<NodeId> = doc.get(doc.first_page()).expect("root").children().to_vec();
    let export = export(doc, res, &origins);
    assert!(
        export.approximated.is_empty(),
        "{test}: the fixture is meant to be exact, but the export reports {:?}",
        export.approximated
    );
    let Some(browser) = browser_or_skip(test) else {
        return;
    };
    let dir = scratch(test);
    fidelity::write_page(&export, &dir).expect("the page is written");
    let cpu = ondin_export::png::raster_of(doc, res, &origins, &Default::default())
        .expect("the fixture has ink");
    let cpu = Image {
        rgba: cpu.rgba,
        width: cpu.width,
        height: cpu.height,
    };
    std::fs::write(
        dir.join("cpu.png"),
        ondin_export::png::encode_rgba8(&cpu.rgba, cpu.width, cpu.height),
    )
    .expect("cpu png");
    let page = fidelity::screenshot(&browser, &dir, cpu.width, cpu.height)
        .unwrap_or_else(|e| panic!("{test}: {e}"));
    assert_eq!(
        (page.width, page.height),
        (cpu.width, cpu.height),
        "{test}: the screenshot is not the page's size"
    );
    let ((cells, mean), mean_limit) = if passes == 0 {
        (compare(&cpu, &page), MAX_MEAN_ERROR)
    } else {
        (
            compare(&blurred(&cpu, passes), &blurred(&page, passes)),
            MAX_TEXT_MEAN_ERROR,
        )
    };
    let (worst, at) = cells.first().copied().unwrap_or_default();
    eprintln!("{test}: worst cell {worst:.1}/255 at {at:?}, mean {mean:.2}/255");
    assert!(
        worst <= limit && mean <= mean_limit,
        "{test}: the page differs from the canvas — worst cells {:?}, mean {mean:.2}/255 \
         (see {})",
        cells
            .iter()
            .map(|(e, at)| format!("{e:.1} at {at:?}"))
            .collect::<Vec<_>>(),
        dir.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Fills, corner radii, opacity and z-order on plain shapes in a frame.
#[test]
fn shapes_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 220.0, 160.0, "Board");
    let a = d.rect(f, 20.0, 20.0, 100.0, 70.0, 12.0);
    d.fill(a, solid(220, 50, 60, 255));
    let b = d.add(
        Some(f),
        NodeKind::Ellipse {
            size: Size::new(90.0, 60.0),
            arc: ondin_core::EllipseArc::default(),
        },
        Affine::translate((90.0, 60.0)),
        "Dot",
    );
    d.fill(b, solid(40, 110, 230, 255));
    let c = d.rect(f, 60.0, 40.0, 120.0, 40.0, 0.0);
    d.fills(c, vec![solid(20, 160, 90, 255), solid(255, 255, 255, 128)]);
    d.apply(vec![Operation::SetOpacity {
        id: c,
        opacity: 0.6,
    }]);
    let (doc, res) = d.done();
    holds("shapes", &doc, &res);
}

/// `fidelity::check`, the command's door, finds nothing on a page that matches —
/// white type on a blue fill included, which Chrome draws heavier — names the
/// layers of one a stray rule moved, and names the type by its ink when a rule
/// spaces its letters.
///
/// flip: `compare_page` ignoring cells past the limit → fails at "the frame is
/// named" (the page's mean still fails it). flip: `misplaced_ink` reading ink
/// without the one-pixel allowance → fails at "a page that matches".
#[test]
fn the_check_names_what_a_page_draws_differently() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 240.0, 140.0, "Board");
    let a = d.rect(f, 20.0, 20.0, 120.0, 40.0, 0.0);
    d.fill(a, solid(37, 99, 235, 255));
    let white = text_node("Button", TextSizing::Auto, body_style(), |_| {});
    let w = d.add(Some(f), white, Affine::translate((40.0, 28.0)), "Label");
    d.fill(w, solid(255, 255, 255, 255));
    let dark = text_node("Heading", TextSizing::Auto, body_style(), |_| {});
    d.add(Some(f), dark, Affine::translate((20.0, 84.0)), "Heading");
    let (doc, res) = d.done();
    let Some(browser) = browser_or_skip("check") else {
        return;
    };
    let origins: Vec<NodeId> = doc.get(doc.first_page()).expect("root").children().to_vec();

    let dir = scratch("check");
    let found = fidelity::check(&doc, &res, &origins, &browser, &dir).expect("the check runs");
    assert!(found.holds(), "a page that matches: {found:?}");

    let mut export = html_of(&doc, &res, &origins);
    let css = export
        .files
        .iter_mut()
        .find(|f| f.path == "styles.css")
        .expect("a stylesheet");
    css.bytes
        .extend_from_slice(b"\nbody { margin: 0 0 0 12px !important; }\n");
    let dir = scratch("check-moved");
    let found = fidelity::check_export(&doc, &res, &origins, &export, &browser, &dir)
        .expect("the check runs");
    assert!(!found.holds(), "a moved page drifts: {found:?}");
    let labels: Vec<&str> = found.drifted.iter().map(|d| d.label.as_str()).collect();
    assert!(
        labels.iter().any(|l| l.contains("Board")),
        "the frame is named: {labels:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);

    let mut export = html_of(&doc, &res, &origins);
    let css = export
        .files
        .iter_mut()
        .find(|f| f.path == "styles.css")
        .expect("a stylesheet");
    css.bytes
        .extend_from_slice(b"\n* { letter-spacing: 4px !important; }\n");
    let dir = scratch("check-spaced");
    let found = fidelity::check_export(&doc, &res, &origins, &export, &browser, &dir)
        .expect("the check runs");
    // Wider type also spills past its own box, onto what is drawn there.
    let ink: Vec<&str> = found
        .drifted
        .iter()
        .filter(|d| matches!(d.how, fidelity::How::Ink { .. }))
        .map(|d| d.label.as_str())
        .collect();
    assert_eq!(
        ink,
        ["text “Label”", "text “Heading”"],
        "spaced type is named by its ink: {found:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `fidelity::check` holds a page of body text that matches, whose antialiasing
/// alone puts the whole page's mean past `MAX_MEAN_ERROR`.
///
/// flip: `compare_page` taking the mean over every pixel → fails at "a page of
/// type".
#[test]
fn the_check_holds_a_page_of_type() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 230.0, "Board");
    let body = "Nine lines of plain body text\n".repeat(9);
    let t = text_node(&body, TextSizing::Auto, body_style(), |_| {});
    d.add(Some(f), t, Affine::translate((10.0, 10.0)), "Body");
    let (doc, res) = d.done();
    let Some(browser) = browser_or_skip("type") else {
        return;
    };
    let dir = scratch("check-type");
    let found = fidelity::check(&doc, &res, &[f], &browser, &dir).expect("the check runs");
    assert!(found.holds(), "a page of type: {found:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Blend modes as the browser draws them: a separable and a non-separable mode
/// over the frame's fill, a group set to Normal whose Difference child hangs off
/// its white card onto the frame and has nothing to difference against there,
/// and a fill-less clipping frame whose Multiply child has nothing under it
/// inside the clip.
///
/// flip: `css_element` writing a clip as `overflow` alone → fails at "a clip
/// isolates".
#[test]
fn blend_modes_match_the_canvas() {
    use ondin_core::BlendMode as B;
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 400.0, 120.0, "Board");
    d.fill(f, solid(40, 90, 200, 255));
    let row = d.frame(Some(f), 300.0, 20.0, 80.0, 80.0, "Row");
    d.apply(vec![Operation::SetFills {
        id: row,
        fills: keyed_by_position(Vec::<ondin_core::Fill>::new()),
    }]);
    let through = d.rect(row, 10.0, 10.0, 60.0, 60.0, 0.0);
    d.fill(through, solid(200, 100, 50, 255));
    d.apply(vec![Operation::SetBlend {
        id: through,
        blend: B::Multiply,
    }]);
    let m = d.rect(f, 20.0, 20.0, 60.0, 80.0, 0.0);
    d.fill(m, solid(240, 160, 80, 255));
    let l = d.rect(f, 100.0, 20.0, 60.0, 80.0, 8.0);
    d.fill(l, solid(250, 230, 120, 255));
    let g = d.add(Some(f), NodeKind::Group, Affine::IDENTITY, "Isolated");
    let card = d.rect(g, 180.0, 20.0, 60.0, 80.0, 0.0);
    d.fill(card, solid(255, 255, 255, 255));
    let disc = d.rect(g, 210.0, 30.0, 70.0, 60.0, 30.0);
    d.fill(disc, solid(240, 160, 80, 255));
    d.apply(vec![
        Operation::SetBlend {
            id: m,
            blend: B::Multiply,
        },
        Operation::SetBlend {
            id: l,
            blend: B::Luminosity,
        },
        Operation::SetBlend {
            id: g,
            blend: B::Normal,
        },
        Operation::SetBlend {
            id: disc,
            blend: B::Difference,
        },
    ]);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    for decl in [
        "mix-blend-mode: multiply",
        "mix-blend-mode: luminosity",
        "mix-blend-mode: difference",
        "isolation: isolate",
    ] {
        assert!(css.contains(decl), "{decl} in {css}");
    }
    for rule in css.split('}').filter(|r| r.contains("overflow: hidden")) {
        assert!(
            rule.contains("isolation: isolate"),
            "a clip isolates: {rule}"
        );
    }
    holds("blend", &doc, &res);
}

/// Inside, centre and outside strokes, square and rounded, and a per-side border.
#[test]
fn strokes_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 260.0, 140.0, "Board");
    for (i, align) in [
        StrokeAlign::Inside,
        StrokeAlign::Center,
        StrokeAlign::Outside,
    ]
    .into_iter()
    .enumerate()
    {
        let x = 20.0 + i as f64 * 80.0;
        let sq = d.rect(f, x, 20.0, 50.0, 40.0, 0.0);
        d.fill(sq, solid(250, 200, 80, 255));
        d.stroke(sq, 6.0, align, StrokeSides::All);
        let rd = d.rect(f, x, 80.0, 50.0, 40.0, 14.0);
        d.fill(rd, solid(120, 200, 250, 255));
        d.stroke(rd, 6.0, align, StrokeSides::All);
    }
    let (doc, res) = d.done();
    holds("strokes", &doc, &res);
}

/// Per-side widths: borders on a square rect, SVG on a rounded one.
#[test]
fn a_per_side_stroke_matches_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 220.0, 100.0, "Board");
    let sides = StrokeSides::Custom {
        top: 8.0,
        right: 0.0,
        bottom: 2.0,
        left: 4.0,
    };
    for (x, r) in [(20.0, 0.0), (120.0, 10.0)] {
        let rect = d.rect(f, x, 20.0, 80.0, 60.0, r);
        d.fill(rect, solid(250, 230, 200, 255));
        d.stroke(rect, 4.0, StrokeAlign::Inside, sides);
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(
        css.contains("border-top: 8px solid"),
        "the square one is CSS:\n{css}"
    );
    assert_eq!(
        export.positioned,
        vec!["layer “Rect”: a per-side stroke CSS borders cannot draw, written as inline SVG"]
    );
    holds("per-side", &doc, &res);
}

/// A frame's stroke paints over its children, at each alignment: on a clipping
/// frame with a full-bleed child, and on one that does not clip, whose child
/// reaches past its edges.
///
/// flip: `paint` writing an inside or unclipped stroke on the frame itself → the
/// child covers it, 143.3 along the inside card's edges.
#[test]
fn a_frame_stroke_draws_over_its_children() {
    let mut d = Doc::new();
    let board = d.frame(None, 0.0, 0.0, 420.0, 260.0, "Board");
    for (i, align) in [
        StrokeAlign::Inside,
        StrokeAlign::Center,
        StrokeAlign::Outside,
    ]
    .into_iter()
    .enumerate()
    {
        for (j, clip) in [true, false].into_iter().enumerate() {
            let (x, y) = (20.0 + i as f64 * 135.0, 20.0 + j as f64 * 125.0);
            let card = d.frame(Some(board), x, y, 110.0, 90.0, "Card");
            d.apply(vec![Operation::SetClip { id: card, clip }]);
            d.stroke(card, 10.0, align, StrokeSides::All);
            let bleed = if clip { 0.0 } else { 8.0 };
            let child = d.rect(
                card,
                -bleed,
                -bleed,
                110.0 + 2.0 * bleed,
                90.0 + 2.0 * bleed,
                0.0,
            );
            d.fill(child, solid(90, 170, 240, 255));
        }
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[board]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(
        css.contains("outline: 5px solid") && css.contains("outline-offset: -5px"),
        "the clipping centred card splits its stroke at the edge:\n{css}"
    );
    holds("frame-stroke", &doc, &res);
}

/// A stroked circle at each alignment is CSS; a stroked ellipse that is not a
/// circle is inline SVG, an outline's offset radii not being its parallel curve.
///
/// flip: `stroke_blocker` letting every ellipse through → `positioned` is empty.
#[test]
fn stroked_ellipses_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 300.0, 260.0, "Board");
    for (i, align) in [
        StrokeAlign::Inside,
        StrokeAlign::Center,
        StrokeAlign::Outside,
    ]
    .into_iter()
    .enumerate()
    {
        let y = 20.0 + i as f64 * 80.0;
        let circle = d.add(
            Some(f),
            NodeKind::Ellipse {
                size: Size::new(50.0, 50.0),
                arc: ondin_core::EllipseArc::default(),
            },
            Affine::translate((20.0, y)),
            "Dot",
        );
        d.fill(circle, solid(250, 200, 80, 255));
        d.stroke(circle, 8.0, align, StrokeSides::All);
        let oval = d.add(
            Some(f),
            NodeKind::Ellipse {
                size: Size::new(180.0, 50.0),
                arc: ondin_core::EllipseArc::default(),
            },
            Affine::translate((100.0, y)),
            "Oval",
        );
        d.fill(oval, solid(120, 200, 250, 255));
        d.stroke(oval, 10.0, align, StrokeSides::All);
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(css.contains(".dot {"), "the circles are CSS:\n{css}");
    assert_eq!(
        export.positioned,
        vec!["layer “Oval”: a stroked ellipse that is not a circle, written as inline SVG"]
    );
    holds("ellipses", &doc, &res);
}

/// Round and bevel joins on a rect with one rounded corner and three square ones
/// are drawn as joined; the outline's square corners are always mitred.
///
/// flip: `stroke_blocker` testing the join only when every corner is square →
/// 134.6 at the square corners.
#[test]
fn a_join_on_mixed_corners_matches_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 140.0, "Board");
    for (i, join) in [Join::Round, Join::Bevel].into_iter().enumerate() {
        let r = d.add(
            Some(f),
            NodeKind::Rect {
                size: Size::new(120.0, 80.0),
                corner_radii: RoundedRectRadii::new(20.0, 0.0, 0.0, 0.0),
                corner_smoothing: 0.0,
            },
            Affine::translate((30.0 + i as f64 * 160.0, 30.0)),
            "Tab",
        );
        d.fill(r, solid(250, 230, 200, 255));
        d.stroke_joined(r, 12.0, StrokeAlign::Outside, join);
    }
    let (doc, res) = d.done();
    holds("mixed-joins", &doc, &res);
}

/// Linear, repeating, concentric radial and sweep gradients.
#[test]
fn gradients_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 340.0, 100.0, "Board");
    let a = d.rect(f, 10.0, 10.0, 70.0, 80.0, 0.0);
    d.fill(
        a,
        ramp(
            GradientKind::Linear(LinearGradientPosition {
                start: Point::new(5.0, 10.0),
                end: Point::new(60.0, 70.0),
            }),
            Extend::Pad,
        ),
    );
    let b = d.rect(f, 90.0, 10.0, 70.0, 80.0, 0.0);
    d.fill(
        b,
        ramp(
            GradientKind::Linear(LinearGradientPosition {
                start: Point::new(0.0, 20.0),
                end: Point::new(0.0, 45.0),
            }),
            Extend::Repeat,
        ),
    );
    let c = d.rect(f, 170.0, 10.0, 70.0, 80.0, 0.0);
    d.fill(
        c,
        ramp(
            GradientKind::Radial(RadialGradientPosition {
                start_center: Point::new(30.0, 40.0),
                start_radius: 0.0,
                end_center: Point::new(30.0, 40.0),
                end_radius: 45.0,
            }),
            Extend::Pad,
        ),
    );
    let e = d.rect(f, 250.0, 10.0, 80.0, 80.0, 0.0);
    d.fill(
        e,
        ramp(
            GradientKind::Sweep(SweepGradientPosition {
                center: Point::new(40.0, 40.0),
                start_angle: 0.0,
                end_angle: std::f32::consts::TAU,
            }),
            Extend::Pad,
        ),
    );
    let (doc, res) = d.done();
    holds("gradients", &doc, &res);
}

/// A full-turn sweep started at six o'clock goes all the way round from there.
///
/// flip: `css::gradient` writing the sweep's stops at absolute angles under a
/// `from` without the start → the quarter before the start pads its first colour.
#[test]
fn a_sweep_with_a_start_angle_matches_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let e = d.rect(f, 10.0, 10.0, 80.0, 80.0, 0.0);
    let start = std::f32::consts::FRAC_PI_2;
    d.fill(
        e,
        ramp(
            GradientKind::Sweep(SweepGradientPosition {
                center: Point::new(40.0, 40.0),
                start_angle: start,
                end_angle: start + std::f32::consts::TAU,
            }),
            Extend::Pad,
        ),
    );
    let (doc, res) = d.done();
    holds("sweep-start", &doc, &res);
}

/// A rotated rect, and a rotated frame whose child turns with it.
#[test]
fn rotation_matches_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 240.0, 180.0, "Board");
    let r = d.add(
        Some(f),
        NodeKind::Rect {
            size: Size::new(80.0, 40.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        Affine::translate((60.0, 40.0)) * Affine::rotate(0.4),
        "Tilted",
    );
    d.fill(r, solid(200, 60, 160, 255));
    let inner = d.add(
        Some(f),
        NodeKind::Artboard {
            size: Size::new(70.0, 60.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        Affine::translate((140.0, 90.0)) * Affine::rotate(-0.3),
        "Inner",
    );
    d.fill(inner, solid(60, 70, 90, 255));
    let dot = d.rect(inner, 10.0, 10.0, 30.0, 20.0, 4.0);
    d.fill(dot, solid(250, 250, 250, 255));
    let (doc, res) = d.done();
    holds("rotation", &doc, &res);
}

/// A layer blur and a drop shadow, one on a translucent fill.
#[test]
fn effects_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 260.0, 120.0, "Board");
    let a = d.rect(f, 20.0, 30.0, 60.0, 60.0, 8.0);
    d.fill(a, solid(40, 120, 220, 255));
    d.effects(a, vec![EffectKind::LayerBlur { radius: 8.0 }]);
    let b = d.rect(f, 100.0, 30.0, 60.0, 60.0, 8.0);
    d.fill(b, solid(250, 250, 250, 255));
    let shadow = |a| {
        EffectKind::DropShadow(Shadow {
            offset: Vec2::new(4.0, 6.0),
            blur: 10.0,
            spread: 0.0,
            color: Color::from_rgba8(0, 0, 0, a),
            blend: ondin_core::BlendMode::Normal,
        })
    };
    d.effects(b, vec![shadow(140)]);
    let c = d.rect(f, 180.0, 30.0, 60.0, 60.0, 30.0);
    d.fill(c, solid(240, 80, 60, 120));
    d.effects(c, vec![shadow(200)]);
    let (doc, res) = d.done();
    holds("effects", &doc, &res);
}

/// A flex row with gap, padding, a growing item and centred cross alignment,
/// and a two-column grid.
#[test]
fn layout_matches_the_canvas() {
    let mut d = Doc::new();
    let row = d.frame(None, 0.0, 0.0, 300.0, 80.0, "Row");
    d.apply(vec![Operation::SetDisplay {
        id: row,
        display: Some(Display::Flex(Flex {
            direction: FlexDirection::Row,
            justify_content: JustifyContent::Start,
            align_items: AlignItems::Center,
            row_gap: 0.0,
            column_gap: 12.0,
            padding: [10.0, 16.0, 10.0, 16.0],
            ..Default::default()
        })),
    }]);
    for (i, h) in [30.0, 50.0, 40.0].into_iter().enumerate() {
        let r = d.rect(row, 0.0, 0.0, 50.0, h, 6.0);
        d.fill(r, solid(60 + 60 * i as u8, 100, 200, 255));
        if i == 1 {
            d.apply(vec![Operation::SetLayoutItem {
                id: r,
                item: LayoutItem {
                    grow: 1.0,
                    ..Default::default()
                },
            }]);
        }
    }
    let grid = d.frame(None, 0.0, 100.0, 200.0, 120.0, "Grid");
    d.apply(vec![Operation::SetDisplay {
        id: grid,
        display: Some(Display::Grid(Grid {
            columns: vec![
                Track::Size(TrackSize::Breadth(TrackBreadth::Fr(1.0))),
                Track::Size(TrackSize::Breadth(TrackBreadth::Fr(2.0))),
            ],
            rows: vec![Track::Repeat {
                repeat: 2,
                tracks: vec![TrackSize::Breadth(TrackBreadth::Px(40.0))],
            }],
            row_gap: 8.0,
            column_gap: 8.0,
            padding: [12.0; 4],
            ..Default::default()
        })),
    }]);
    for i in 0..4 {
        let r = d.rect(grid, 0.0, 0.0, 30.0, 30.0, 0.0);
        d.fill(r, solid(230, 90 + 30 * i as u8, 60, 255));
    }
    let (doc, res) = d.done();
    holds("layout", &doc, &res);
}

/// A hugging group with a column layout inside a frame, and children pinned by
/// insets — one to the bottom right, one stretched across.
#[test]
fn insets_and_hugging_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 240.0, 160.0, "Board");
    let pin = d.rect(f, 0.0, 0.0, 40.0, 30.0, 0.0);
    d.fill(pin, solid(220, 60, 60, 255));
    d.apply(vec![Operation::SetInsets {
        id: pin,
        insets: Insets {
            right: Some(LengthPct::Px(12.0)),
            bottom: Some(LengthPct::Px(10.0)),
            ..Default::default()
        },
    }]);
    let bar = d.rect(f, 0.0, 10.0, 40.0, 16.0, 8.0);
    d.fill(bar, solid(60, 60, 220, 255));
    d.apply(vec![Operation::SetInsets {
        id: bar,
        insets: Insets {
            left: Some(LengthPct::Px(20.0)),
            right: Some(LengthPct::Percent(25.0)),
            ..Default::default()
        },
    }]);
    let stack = d.add(
        Some(f),
        NodeKind::Group,
        Affine::translate((20.0, 40.0)),
        "Stack",
    );
    d.apply(vec![Operation::SetDisplay {
        id: stack,
        display: Some(Display::Flex(Flex {
            direction: FlexDirection::Column,
            align_items: AlignItems::End,
            row_gap: 6.0,
            ..Default::default()
        })),
    }]);
    for w in [60.0, 30.0, 45.0] {
        let r = d.rect(stack, 0.0, 0.0, w, 14.0, 0.0);
        d.fill(r, solid(40, 160, 110, 255));
    }
    let (doc, res) = d.done();
    holds("insets", &doc, &res);
}

/// What CSS cannot draw goes to inline SVG and still matches: a star, a path and
/// an inner shadow, beside a group with no layout.
#[test]
fn pictures_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 260.0, 120.0, "Board");
    let star = d.add(
        Some(f),
        NodeKind::Star {
            size: Size::new(70.0, 70.0),
            points: 5,
            inner_ratio: 0.5,
            corner_radius: 0.0,
            corner_smoothing: 0.0,
        },
        Affine::translate((20.0, 20.0)),
        "Star",
    );
    d.fill(star, solid(250, 180, 30, 255));
    let mut p = ondin_core::kurbo::BezPath::new();
    p.move_to((0.0, 0.0));
    p.curve_to((40.0, -20.0), (60.0, 60.0), (80.0, 40.0));
    p.line_to((20.0, 60.0));
    p.close_path();
    let path = d.add(
        Some(f),
        NodeKind::Path {
            path: p,
            corner_radii: vec![],
            corner_smoothing: 0.0,
            mirror: ondin_core::symmetry::PathMirror::NONE,
        },
        Affine::translate((100.0, 30.0)),
        "Blob",
    );
    d.fill(path, solid(80, 160, 240, 255));
    let g = d.add(
        Some(f),
        NodeKind::Group,
        Affine::translate((190.0, 20.0)),
        "Pair",
    );
    let a = d.rect(g, 0.0, 0.0, 50.0, 40.0, 6.0);
    d.fill(a, solid(240, 240, 240, 255));
    d.effects(
        a,
        vec![EffectKind::InnerShadow(Shadow {
            offset: Vec2::new(0.0, 3.0),
            blur: 6.0,
            spread: 0.0,
            color: Color::from_rgba8(0, 0, 0, 160),
            blend: ondin_core::BlendMode::Normal,
        })],
    );
    let b = d.rect(g, 10.0, 50.0, 40.0, 30.0, 0.0);
    d.fill(b, solid(30, 30, 30, 255));
    let (doc, res) = d.done();
    holds("pictures", &doc, &res);
}

/// A rounded clipping frame with children strokes its rounded outline, its
/// stroke's overlay taking the frame's corners; an arc goes to inline SVG.
///
/// flip: the overlay stroke without `border-radius` → fails at "the overlay
/// follows the corners".
#[test]
fn a_rounded_frame_and_an_arc_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 260.0, 120.0, "Board");
    let card = d.frame(Some(f), 20.0, 20.0, 110.0, 80.0, "Card");
    d.fill(card, solid(80, 160, 240, 255));
    d.apply(vec![Operation::SetGeometry {
        id: card,
        geometry: ondin_core::GeometryPatch::CornerRadii(RoundedRectRadii::from_single_radius(
            16.0,
        )),
    }]);
    d.stroke(card, 4.0, StrokeAlign::Inside, StrokeSides::All);
    let inner = d.rect(card, 30.0, 25.0, 50.0, 30.0, 0.0);
    d.fill(inner, solid(250, 180, 30, 255));
    let arc = d.add(
        Some(f),
        NodeKind::Ellipse {
            size: Size::new(80.0, 80.0),
            arc: ondin_core::EllipseArc {
                start: 30.0,
                sweep: 270.0,
                ratio: 0.4,
            },
        },
        Affine::translate((160.0, 20.0)),
        "Ring",
    );
    d.fill(arc, solid(30, 30, 30, 255));
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    let overlay = css
        .split('}')
        .find(|r| r.contains("stroke {") || r.contains("stroke{"))
        .expect("the overlay's rule");
    assert!(
        overlay.contains("border-radius: inherit"),
        "the overlay follows the corners: {overlay}"
    );
    holds("rounded", &doc, &res);
}

/// A picture pinned by insets sits where layout puts it, not at its stored
/// transform.
///
/// flip: `css_element` placing an out-of-flow child by `Node::transform` → the
/// star is drawn at the frame's top-left.
#[test]
fn a_pinned_picture_sits_where_layout_puts_it() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 240.0, 160.0, "Board");
    let star = d.add(
        Some(f),
        NodeKind::Star {
            size: Size::new(50.0, 50.0),
            points: 5,
            inner_ratio: 0.5,
            corner_radius: 0.0,
            corner_smoothing: 0.0,
        },
        Affine::IDENTITY,
        "Star",
    );
    d.fill(star, solid(250, 180, 30, 255));
    d.apply(vec![Operation::SetInsets {
        id: star,
        insets: Insets {
            right: Some(LengthPct::Px(12.0)),
            bottom: Some(LengthPct::Px(10.0)),
            ..Default::default()
        },
    }]);
    let (doc, res) = d.done();
    assert_eq!(
        doc.get(star).unwrap().transform(),
        Affine::IDENTITY,
        "the stored transform is not where the star is drawn"
    );
    holds("pinned-picture", &doc, &res);
}

/// A text node with its own size and colour and a bold run, one wrapping at a
/// width with a second paragraph, and one centred in a fixed box.
#[test]
fn text_matches_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 300.0, 200.0, "Board");
    let text = |content: &str, sizing, style: TextStyle| NodeKind::Text {
        content: content.into(),
        style: Box::new(style),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    };
    let style = TextStyle {
        font_size: 20.0,
        line_height: Some(ondin_core::Length::Em(1.25)),
        ..Default::default()
    };
    let mut title = text("Hello, layers", TextSizing::Auto, style.clone());
    if let NodeKind::Text { spans, style, .. } = &mut title {
        spans.set(7..13, CharAttr::Weight(700), style);
    }
    let t = d.add(Some(f), title, Affine::translate((16.0, 12.0)), "Title");
    d.fill(t, solid(30, 40, 60, 255));
    let body = text(
        "Two short lines that wrap at the box.\nAnd a second paragraph.",
        TextSizing::AutoHeight(150.0),
        TextStyle {
            font_size: 13.0,
            line_height: Some(ondin_core::Length::Px(18.0)),
            ..Default::default()
        },
    );
    d.add(Some(f), body, Affine::translate((16.0, 50.0)), "Body");
    let mut centred = text(
        "Centred",
        TextSizing::Fixed(Size::new(110.0, 60.0)),
        TextStyle {
            font_size: 15.0,
            italic: true,
            ..Default::default()
        },
    );
    if let NodeKind::Text {
        paragraph, block, ..
    } = &mut centred
    {
        paragraph.align = TextAlign::Center;
        block.vertical_align = VerticalAlign::Middle;
    }
    let c = d.add(Some(f), centred, Affine::translate((180.0, 120.0)), "Badge");
    d.fill(c, solid(200, 40, 90, 255));
    let (doc, res) = d.done();
    holds_within("text", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
}

/// A text node of `content` in `style`, edited by `edit` before it is placed.
fn text_node(
    content: &str,
    sizing: TextSizing,
    style: TextStyle,
    edit: impl FnOnce(&mut NodeKind),
) -> NodeKind {
    let mut kind = NodeKind::Text {
        content: content.into(),
        style: Box::new(style),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    };
    edit(&mut kind);
    kind
}

/// The 18px Inter the text fixtures below are set in, on a 24px line.
fn body_style() -> TextStyle {
    TextStyle {
        font_size: 18.0,
        line_height: Some(ondin_core::Length::Px(24.0)),
        ..Default::default()
    }
}

/// Underlines and strikethroughs: the font's own, a thick coloured one that does
/// not skip ink, one moved off the baseline, both on one run, one on a middle run,
/// and a node's underline that one run takes off.
///
/// flip: `Walk::run` taking the node's underline for every run → 31.2 on the
/// runs whose underline differs from their node's.
#[test]
fn text_decorations_match_the_canvas() {
    use ondin_core::Decoration;
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 230.0, "Board");
    let under = |deco: Decoration| TextStyle {
        underline: Some(deco),
        ..body_style()
    };
    let red = Color::from_rgba8(220, 30, 40, 255);
    let lines = [
        ("Underlined gyp", under(Decoration::default())),
        (
            "Thick red, through gyp",
            under(Decoration {
                thickness: Some(ondin_core::Length::Px(4.0)),
                color: Some(red),
                skip_ink: false,
                ..Default::default()
            }),
        ),
        (
            "Moved down",
            under(Decoration {
                offset: Some(ondin_core::Length::Px(-6.0)),
                ..Default::default()
            }),
        ),
        (
            "Struck through",
            TextStyle {
                strikethrough: Some(Decoration::default()),
                ..body_style()
            },
        ),
        (
            "Both at once",
            TextStyle {
                underline: Some(Decoration::default()),
                strikethrough: Some(Decoration {
                    color: Some(red),
                    ..Default::default()
                }),
                ..body_style()
            },
        ),
    ];
    for (i, (content, style)) in lines.into_iter().enumerate() {
        let t = text_node(content, TextSizing::Auto, style, |_| {});
        d.add(
            Some(f),
            t,
            Affine::translate((16.0, 12.0 + i as f64 * 32.0)),
            "Line",
        );
    }
    let middle = text_node("plain marked plain", TextSizing::Auto, body_style(), |k| {
        if let NodeKind::Text { spans, style, .. } = k {
            spans.set(
                6..12,
                CharAttr::Underline(Some(Decoration::default())),
                style,
            );
        }
    });
    d.add(Some(f), middle, Affine::translate((16.0, 172.0)), "Middle");
    let off = text_node(
        "all but this one",
        TextSizing::Auto,
        under(Decoration::default()),
        |k| {
            if let NodeKind::Text { spans, style, .. } = k {
                spans.set(8..12, CharAttr::Underline(None), style);
            }
        },
    );
    d.add(Some(f), off, Affine::translate((16.0, 200.0)), "Off");
    let (doc, res) = d.done();
    holds_within("decorations", &doc, &res, 2, MAX_DECORATION_CELL_ERROR);
}

/// Decorations' limit, over text's: Chrome snaps a band to whole device pixels
/// where the canvas antialiases its fractional thickness and position, so a
/// 1.3px underline is a crisp 1px one on the page. The matching fixture peaks at
/// 9.7, and at 14.1 under macOS Chrome's glyph smoothing (see `MAX_TEXT_CELL_ERROR`).
const MAX_DECORATION_CELL_ERROR: f64 = if cfg!(target_os = "macos") {
    22.0
} else {
    12.0
};

/// A run's baseline shift moves its ink and leaves the lines where they are, a
/// whole node's shift moves all of it, and case is the canvas's: Title lowercases
/// the rest of each word and starts one after an apostrophe, Upper keeps `ß`.
///
/// flip: `Walk::run` writing a shift as `vertical-align` → 82.8, the shifted
/// run's line box growing and the second line dropping.
#[test]
fn baseline_shift_and_case_match_the_canvas() {
    use ondin_core::TextCase;
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 200.0, "Board");
    let shifted = text_node("x2 and\nmore below", TextSizing::Auto, body_style(), |k| {
        if let NodeKind::Text { spans, style, .. } = k {
            spans.set(
                1..2,
                CharAttr::BaselineShift(ondin_core::Length::Px(8.0)),
                style,
            );
        }
    });
    d.add(Some(f), shifted, Affine::translate((16.0, 12.0)), "Power");
    let whole = text_node(
        "Raised whole",
        TextSizing::Auto,
        TextStyle {
            baseline_shift: ondin_core::Length::Px(6.0),
            ..body_style()
        },
        |_| {},
    );
    d.add(Some(f), whole, Affine::translate((160.0, 12.0)), "Raised");
    for (i, (content, case)) in [
        ("HELLO WORLD it's", TextCase::Title),
        ("straße", TextCase::Upper),
        ("QUIET Words", TextCase::Lower),
    ]
    .into_iter()
    .enumerate()
    {
        let t = text_node(
            content,
            TextSizing::Auto,
            TextStyle {
                case,
                ..body_style()
            },
            |_| {},
        );
        d.add(
            Some(f),
            t,
            Affine::translate((16.0, 90.0 + i as f64 * 32.0)),
            "Cased",
        );
    }
    let (doc, res) = d.done();
    holds_within("shift-and-case", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
}

/// Paragraph breaking and alignment: no wrap, a justified paragraph with its last
/// line centred, break-all, a long word broken anywhere, and an RTL paragraph's
/// start edge on the right.
///
/// flip: `Walk::text` leaving `wrap` unread → 85.4, the no-wrap line wrapping on
/// the page.
#[test]
fn paragraph_breaking_matches_the_canvas() {
    use ondin_core::{JustifyLast, OverflowWrap, TextDirection, WordBreak, WrapMode};
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 420.0, 330.0, "Board");
    let paragraph = |edit: fn(&mut ondin_core::ParagraphStyle)| {
        move |k: &mut NodeKind| {
            if let NodeKind::Text { paragraph, .. } = k {
                edit(paragraph);
            }
        }
    };
    let cases = [
        (
            "A line that would wrap at the box",
            150.0,
            paragraph(|p| p.wrap = WrapMode::NoWrap),
        ),
        (
            "Justified text whose last line sits in the middle",
            180.0,
            paragraph(|p| {
                p.align = TextAlign::Justify;
                p.justify_last = JustifyLast::Center;
            }),
        ),
        (
            "Break all of these words",
            120.0,
            paragraph(|p| p.word_break = WordBreak::BreakAll),
        ),
        (
            "Antidisestablishmentarianism",
            120.0,
            paragraph(|p| p.overflow_wrap = OverflowWrap::Anywhere),
        ),
        (
            "Starts on the right",
            180.0,
            paragraph(|p| p.direction = TextDirection::Rtl),
        ),
    ];
    for (i, (content, width, edit)) in cases.into_iter().enumerate() {
        let (x, y) = (16.0 + (i % 2) as f64 * 200.0, 12.0 + (i / 2) as f64 * 100.0);
        let t = text_node(content, TextSizing::AutoHeight(width), body_style(), edit);
        d.add(Some(f), t, Affine::translate((x, y)), "Para");
    }
    let (doc, res) = d.done();
    holds_within("breaking", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
}

/// Letter and word spacing, a first-line indent, a start indent, space between
/// paragraphs, and a font feature.
///
/// flip: `face` dropping letter spacing → 71.7.
#[test]
fn text_spacing_and_features_match_the_canvas() {
    use ondin_core::{FeatureSetting, Tag};
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 200.0, "Board");
    let spaced = text_node(
        "Spaced out words",
        TextSizing::Auto,
        TextStyle {
            letter_spacing: ondin_core::Length::Px(2.0),
            word_spacing: ondin_core::Length::Px(6.0),
            ..body_style()
        },
        |_| {},
    );
    d.add(Some(f), spaced, Affine::translate((16.0, 12.0)), "Spaced");
    let indented = text_node(
        "First paragraph runs on.\nSecond one.",
        TextSizing::AutoHeight(220.0),
        body_style(),
        |k| {
            if let NodeKind::Text { paragraph, .. } = k {
                paragraph.indent = ondin_core::Length::Px(20.0);
                paragraph.indent_start = ondin_core::Length::Px(10.0);
                paragraph.spacing = ondin_core::Length::Px(8.0);
            }
        },
    );
    d.add(
        Some(f),
        indented,
        Affine::translate((16.0, 50.0)),
        "Indented",
    );
    let figures = text_node(
        "1111 0000",
        TextSizing::Auto,
        TextStyle {
            features: vec![FeatureSetting {
                tag: Tag::new(*b"tnum"),
                value: 1,
            }],
            ..body_style()
        },
        |_| {},
    );
    d.add(
        Some(f),
        figures,
        Affine::translate((16.0, 150.0)),
        "Figures",
    );
    let (doc, res) = d.done();
    holds_within("spacing", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
}

/// What the page cannot draw as the canvas does is reported: a wavy underline, a
/// strikethrough moved off the font's position, an ellipsis that fires, optical
/// margins and right-to-left content.
#[test]
fn text_the_page_cannot_match_is_reported() {
    use ondin_core::{Decoration, LineStyle, TextOverflow};
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 300.0, "Board");
    // The layer's name, its content, its style and an edit of its kind.
    type Case = (&'static str, &'static str, TextStyle, fn(&mut NodeKind));
    let cases: [Case; 5] = [
        (
            "Wavy",
            "wavy",
            TextStyle {
                underline: Some(Decoration {
                    style: LineStyle::Wavy,
                    ..Default::default()
                }),
                ..body_style()
            },
            |_| {},
        ),
        (
            "Moved",
            "moved strike",
            TextStyle {
                strikethrough: Some(Decoration {
                    offset: Some(ondin_core::Length::Px(3.0)),
                    ..Default::default()
                }),
                ..body_style()
            },
            |_| {},
        ),
        (
            "Clipped",
            "A long line that will not fit",
            body_style(),
            |k| {
                if let NodeKind::Text { block, sizing, .. } = k {
                    block.overflow = TextOverflow::Ellipsis;
                    *sizing = TextSizing::Fixed(Size::new(150.0, 24.0));
                }
            },
        ),
        ("Optical", "Optical", body_style(), |k| {
            if let NodeKind::Text { paragraph, .. } = k {
                paragraph.optical_margins = true;
            }
        }),
        ("Hebrew", "שלום", body_style(), |_| {}),
    ];
    for (i, (name, content, style, edit)) in cases.into_iter().enumerate() {
        let t = text_node(content, TextSizing::Auto, style, edit);
        d.add(
            Some(f),
            t,
            Affine::translate((16.0, 12.0 + i as f64 * 40.0)),
            name,
        );
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    assert_eq!(
        export.approximated,
        vec![
            "text “Wavy”: a dashed, dotted or wavy underline",
            "text “Moved”: a strikethrough moved off the font's position",
            "text “Clipped”: an ellipsis where the text overflows",
            "text “Optical”: optical margin alignment",
            "text “Hebrew”: right-to-left text",
        ]
    );
}

/// Variables as `var()` in two modes: one frame in the default, one set to Dark
/// with a Light frame inside. A fill bound through an alias (at full and half
/// alpha), and bound radius, gap, opacity, width, stroke width and shadow blur.
///
/// flip: `Tokens::css` declaring aliases on `:root` only → 136.7 in the dark
/// frame, whose alias keeps the light colour; its default block on `:root` alone
/// → 136.7 at the light frame nested in the dark one.
#[test]
fn variables_match_the_canvas_in_every_mode() {
    use ondin_core::BrushSource;
    use ondin_core::variable::{Cell, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let (light, dark) = (d.ids.mint_mode(), d.ids.mint_mode());
    d.apply(vec![
        Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id: light,
                name: "Light".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id: dark,
                name: "Dark".into(),
            },
            index: 1,
            cells: vec![],
        },
    ]);
    let var = |d: &mut Doc, name: &str, kind, a: Cell, b: Cell| {
        let id = d.ids.mint_var();
        d.apply(vec![Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind,
                cells: [(light, a), (dark, b)].into_iter().collect(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]);
        id
    };
    let brand = var(
        &mut d,
        "brand/500",
        VarKind::Color,
        Cell::Color(Color::from_rgba8(230, 70, 50, 255)),
        Cell::Color(Color::from_rgba8(60, 120, 240, 255)),
    );
    let accent = var(
        &mut d,
        "accent",
        VarKind::Color,
        Cell::Alias(brand),
        Cell::Alias(brand),
    );
    let radius = var(
        &mut d,
        "radius",
        VarKind::Number,
        Cell::Number(6.0),
        Cell::Number(18.0),
    );
    let gap = var(
        &mut d,
        "gap",
        VarKind::Number,
        Cell::Number(8.0),
        Cell::Number(24.0),
    );
    let fade = var(
        &mut d,
        "fade",
        VarKind::Number,
        Cell::Number(100.0),
        Cell::Number(40.0),
    );
    let num = |d: &mut Doc, name: &str, a: f64, b: f64| {
        var(d, name, VarKind::Number, Cell::Number(a), Cell::Number(b))
    };
    let wide = num(&mut d, "wide", 40.0, 70.0);
    let line = num(&mut d, "line", 2.0, 6.0);
    let soft = num(&mut d, "soft", 4.0, 12.0);

    for (i, mode) in [None, Some(dark)].into_iter().enumerate() {
        let f = d.frame(None, 0.0, i as f64 * 90.0, 260.0, 80.0, "Board");
        d.apply(vec![
            Operation::SetModes {
                id: f,
                modes: mode.into_iter().collect(),
            },
            Operation::SetDisplay {
                id: f,
                display: Some(Display::Flex(Flex {
                    align_items: AlignItems::Center,
                    padding: [10.0; 4],
                    ..Default::default()
                })),
            },
            Operation::SetSpacingVars {
                id: f,
                vars: [Some(gap), None, None, None, None, None],
            },
        ]);
        for j in 0..3 {
            let r = d.rect(f, 0.0, 0.0, 50.0, 50.0, 0.0);
            d.apply(vec![
                Operation::SetFills {
                    id: r,
                    fills: keyed_by_position([Fill {
                        brush: BrushSource::Bound {
                            var: accent,
                            alpha: if j == 1 { 0.5 } else { 1.0 },
                        },
                        visible: true,
                    }]),
                },
                Operation::SetCornerVars {
                    id: r,
                    vars: [Some(radius), Some(radius), None, Some(radius)],
                },
            ]);
            if j == 2 {
                d.apply(vec![
                    Operation::SetOpacityVar {
                        id: r,
                        var: Some(fade),
                    },
                    Operation::SetSizeVars {
                        id: r,
                        vars: [Some(wide), None],
                    },
                ]);
            }
            if j == 0 {
                d.stroke(r, 0.0, StrokeAlign::Center, StrokeSides::All);
                let mut strokes = d.doc.get(r).unwrap().paint().strokes.clone();
                strokes[0].value.width_var = Some(line);
                d.apply(vec![Operation::SetStrokes { id: r, strokes }]);
            }
            if j == 1 {
                let mut shadow = Effect::new(EffectKind::DropShadow(Shadow {
                    offset: Vec2::new(0.0, 3.0),
                    blur: 0.0,
                    spread: 0.0,
                    color: Color::from_rgba8(0, 0, 0, 120),
                    blend: ondin_core::BlendMode::Normal,
                }));
                shadow.vars[2] = Some(soft);
                d.apply(vec![Operation::SetEffects {
                    id: r,
                    effects: keyed_by_position([shadow]),
                }]);
            }
        }
        // The default mode set inside another: back to the light accent.
        if mode.is_some() {
            let inner = d.add(
                Some(f),
                NodeKind::Artboard {
                    size: Size::new(40.0, 40.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                Affine::IDENTITY,
                "Light again",
            );
            d.apply(vec![
                Operation::SetModes {
                    id: inner,
                    modes: vec![light],
                },
                Operation::SetFills {
                    id: inner,
                    fills: keyed_by_position([Fill {
                        brush: BrushSource::Bound {
                            var: accent,
                            alpha: 1.0,
                        },
                        visible: true,
                    }]),
                },
            ]);
        }
    }
    let (doc, res) = d.done();
    holds("variables", &doc, &res);
    let export = html_of(&doc, &res, doc.get(doc.first_page()).unwrap().children());
    let tokens =
        String::from_utf8(export.file("tokens.css").expect("tokens.css").to_vec()).unwrap();
    assert!(
        tokens.contains(
            "[data-mode=\"dark\"] {\n  --brand-500: #3c78f0;\n  --accent: var(--brand-500);"
        ),
        "the alias is redeclared in the dark block:\n{tokens}"
    );
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    for read in [
        "outline: max(0px, calc(var(--line) * 1px)) solid #14141e",
        "outline-offset: calc(max(0px, calc(var(--line) * 1px)) * -0.5)",
        "color-mix(in srgb, var(--accent) 50%, transparent)",
        "drop-shadow(0 3px calc(max(0px, calc(var(--soft) * 1px)) * 0.5)",
        "opacity: calc(var(--fade) / 100)",
        "width: max(0px, calc(var(--wide) * 1px))",
        "min-width: max(0px, calc(var(--wide) * 1px))",
        "column-gap: calc(var(--gap) * 1px)",
        "min(max(0px, calc(var(--radius) * 1px)), 25px)",
        "min(max(0px, calc(var(--radius) * 1px)), calc(min(max(0px, calc(var(--wide) * 1px)), 50px) / 2))",
    ] {
        assert!(css.contains(read), "styles.css lacks `{read}`:\n{css}");
    }
}

/// **A frame that picks its mode by width draws in the browser as on the
/// canvas at each width**: written alone it fills the window, which the media
/// blocks read as the canvas reads the frame — 600 wide is Mobile, 900 Tablet.
/// A bound gap, colour, visibility and direction each differ between the two,
/// the hidden rect written with a `none`/`initial` token and the direction with
/// a bare `column`/`row` one.
///
/// flip: `Tokens::css` writing the by-width collection's modes as `[data-mode]`
/// blocks → 136.7 at the 900 page, which draws Mobile's colour and gap.
#[test]
fn a_frame_picked_by_width_matches_the_canvas_at_each_width() {
    use ondin_core::BrushSource;
    use ondin_core::variable::{Cell, CollectionId, Mode, VarKind, Variable};
    for (w, name) in [(900.0, "tablet"), (600.0, "mobile")] {
        let mut d = Doc::new();
        let (mobile, tablet) = (d.ids.mint_mode(), d.ids.mint_mode());
        let mode = |id, name: &str, index| Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: Mode {
                id,
                name: name.into(),
            },
            index,
            cells: vec![],
        };
        d.apply(vec![
            mode(mobile, "Mobile", 0),
            mode(tablet, "Tablet", 1),
            Operation::SetBreakpoints {
                id: CollectionId::FIRST,
                breakpoints: Some([(mobile, 0.0), (tablet, 768.0)].into()),
            },
        ]);
        let var = |d: &mut Doc, name: &str, kind, a: Cell, b: Cell| {
            let id = d.ids.mint_var();
            d.apply(vec![Operation::AddVariable {
                var: Variable {
                    id,
                    name: name.into(),
                    kind,
                    cells: [(mobile, a), (tablet, b)].into_iter().collect(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            }]);
            id
        };
        let gap = var(
            &mut d,
            "gap",
            VarKind::Number,
            Cell::Number(8.0),
            Cell::Number(40.0),
        );
        let ink = var(
            &mut d,
            "ink",
            VarKind::Color,
            Cell::Color(Color::from_rgba8(230, 70, 50, 255)),
            Cell::Color(Color::from_rgba8(60, 120, 240, 255)),
        );
        let wide = var(
            &mut d,
            "show-wide",
            VarKind::Boolean,
            Cell::Boolean(false),
            Cell::Boolean(true),
        );
        let flow = var(
            &mut d,
            "flow",
            VarKind::String,
            Cell::String("Column".into()),
            Cell::String("Row".into()),
        );
        let mut v = d.doc.variables().get(flow).unwrap().clone();
        v.scope = Some(ondin_core::variable::Scope::of([
            ondin_core::variable::Field::FlexDirection,
        ]));
        d.apply(vec![Operation::SetVariable { var: v }]);
        let f = d.frame(None, 0.0, 0.0, w, 240.0, "Home");
        d.fill(f, solid(250, 250, 250, 255));
        d.apply(vec![
            Operation::SetDisplay {
                id: f,
                display: Some(Display::Flex(Flex {
                    align_items: AlignItems::Center,
                    padding: [10.0; 4],
                    ..Default::default()
                })),
            },
            Operation::SetSpacingVars {
                id: f,
                vars: [Some(gap), None, None, None, None, None],
            },
        ]);
        for j in 0..3 {
            let r = d.rect(f, 0.0, 0.0, 60.0, 60.0, 0.0);
            d.apply(vec![Operation::SetFills {
                id: r,
                fills: keyed_by_position([Fill {
                    brush: BrushSource::Bound {
                        var: ink,
                        alpha: 1.0,
                    },
                    visible: true,
                }]),
            }]);
            if j == 1 {
                d.apply(vec![Operation::SetVisibleVar {
                    id: r,
                    var: Some(wide),
                }]);
            }
        }
        let bind = ondin_core::build::bind_flow_ops(&d.doc, f, 0, Some(flow));
        d.apply(bind);
        let (doc, res) = d.done();
        holds(&format!("breakpoint-{name}"), &doc, &res);
        let export = html_of(&doc, &res, doc.get(doc.first_page()).unwrap().children());
        let tokens =
            String::from_utf8(export.file("tokens.css").expect("tokens.css").to_vec()).unwrap();
        assert!(
            tokens.contains("@media (min-width: 768px) {\n  :root {\n    --gap: 40;"),
            "{name}: tokens.css lacks the media block:\n{tokens}"
        );
        assert!(
            tokens.contains("[data-mode=\"mobile\"] {"),
            "{name}: a pin's block:\n{tokens}"
        );
        let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
        assert!(
            css.contains("display: var(--show-wide, block)"),
            "{name}: the visibility token:\n{css}"
        );
        assert!(
            css.contains("flex-direction: var(--flow)") && tokens.contains("--flow: column;"),
            "{name}: the direction token, bare:\n{css}\n{tokens}"
        );
    }
}

/// **A frame with a width query is written as a CSS container query**: the
/// frame names its query in `data-query`, `tokens.css` makes it a container of
/// its inline size, gives its children the first mode, each other mode under
/// `@container (min-width: …)`, and a pin on the frame after them. Read from the
/// files alone; no browser runs.
///
/// flip: `Tokens::css` without `query_blocks` → "lacks the container".
#[test]
fn a_width_query_is_written_as_a_container_query() {
    use ondin_core::BrushSource;
    use ondin_core::variable::{Cell, CollectionId, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let (compact, wide) = (d.ids.mint_mode(), d.ids.mint_mode());
    for (index, (id, name)) in [(compact, "Compact"), (wide, "Wide")]
        .into_iter()
        .enumerate()
    {
        d.apply(vec![Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: Mode {
                id,
                name: name.into(),
            },
            index,
            cells: vec![],
        }]);
    }
    let ink = d.ids.mint_var();
    d.apply(vec![Operation::AddVariable {
        var: Variable {
            id: ink,
            name: "ink".into(),
            kind: VarKind::Color,
            cells: [
                (compact, Cell::Color(Color::from_rgba8(230, 70, 50, 255))),
                (wide, Cell::Color(Color::from_rgba8(60, 120, 240, 255))),
            ]
            .into_iter()
            .collect(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    }]);
    let card = d.frame(None, 0.0, 0.0, 400.0, 120.0, "Card");
    let r = d.rect(card, 10.0, 10.0, 60.0, 60.0, 0.0);
    d.apply(vec![
        Operation::SetFills {
            id: r,
            fills: keyed_by_position([Fill {
                brush: BrushSource::Bound {
                    var: ink,
                    alpha: 1.0,
                },
                visible: true,
            }]),
        },
        Operation::SetQueries {
            id: card,
            queries: vec![[(compact, 0.0), (wide, 320.0)].into()],
        },
    ]);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[card]);
    let tokens =
        String::from_utf8(export.file("tokens.css").expect("tokens.css").to_vec()).unwrap();
    assert!(
        tokens.contains("[data-query~=\"card-variables\"] {\n  container-type: inline-size;\n}"),
        "tokens.css lacks the container:\n{tokens}"
    );
    assert!(
        tokens.contains(
            "@container (min-width: 320px) {\n  :where([data-query~=\"card-variables\"]) > * {"
        ),
        "tokens.css lacks the wide block:\n{tokens}"
    );
    assert!(
        tokens.contains(
            "[data-pin~=\"wide\"] > [data-query~=\"card-variables\"] > *, \
             [data-mode=\"wide\"][data-query~=\"card-variables\"] > *"
        ),
        "tokens.css lacks the pin:\n{tokens}"
    );
    let marked = export
        .files
        .iter()
        .any(|f| String::from_utf8_lossy(&f.bytes).contains("data-query=\"card-variables\""));
    assert!(marked, "no file marks the frame with its query");
}

/// A text's bound type numbers write their variables in the unit each literal
/// holds — a size and a paragraph spacing in px, a weight bare, a `%` line height
/// as a multiple and a `%` letter spacing in hundredths of an em — and draw as
/// the canvas does in both modes.
///
/// flip: `Walk::text` writing a bound `%` line height in px → 122.5 in the
/// browser, its lines 140 px apart, and *"styles.css lacks"* its reading.
#[test]
fn bound_type_numbers_match_the_canvas_in_every_mode() {
    use ondin_core::variable::{
        Cell, Mode, TEXT_LETTER_SPACING, TEXT_LINE_HEIGHT, TEXT_PARAGRAPH_SPACING, TEXT_SIZE,
        TEXT_WEIGHT, VarKind, Variable,
    };
    let mut d = Doc::new();
    let (light, dark) = (d.ids.mint_mode(), d.ids.mint_mode());
    for (index, (id, name)) in [(light, "Light"), (dark, "Dark")].into_iter().enumerate() {
        d.apply(vec![Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id,
                name: name.into(),
            },
            index,
            cells: vec![],
        }]);
    }
    let num = |d: &mut Doc, name: &str, a: f64, b: f64| {
        let id = d.ids.mint_var();
        d.apply(vec![Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Number,
                cells: [(light, Cell::Number(a)), (dark, Cell::Number(b))]
                    .into_iter()
                    .collect(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]);
        id
    };
    // Dark's 5% of 20 tracks a whole pixel: a fractional tracking at 22 is placed
    // apart enough by the two renderers to pass the text limit.
    let size = num(&mut d, "size", 16.0, 20.0);
    let weight = num(&mut d, "weight", 400.0, 700.0);
    let leading = num(&mut d, "leading", 140.0, 120.0);
    let tracking = num(&mut d, "tracking", 2.0, 5.0);
    let space = num(&mut d, "space", 6.0, 12.0);
    for (i, mode) in [None, Some(dark)].into_iter().enumerate() {
        let f = d.frame(None, 0.0, i as f64 * 140.0, 300.0, 130.0, "Board");
        d.apply(vec![Operation::SetModes {
            id: f,
            modes: mode.into_iter().collect(),
        }]);
        let kind = text_node(
            "Bound type\nA second paragraph",
            TextSizing::Auto,
            TextStyle {
                line_height: Some(ondin_core::Length::Em(1.0)),
                letter_spacing: ondin_core::Length::Em(0.0),
                ..Default::default()
            },
            |_| {},
        );
        let t = d.add(Some(f), kind, Affine::translate((12.0, 10.0)), "Copy");
        d.fill(t, solid(30, 40, 60, 255));
        let mut vars = ondin_core::TextVars::default();
        vars[TEXT_SIZE] = Some(size);
        vars[TEXT_WEIGHT] = Some(weight);
        vars[TEXT_LINE_HEIGHT] = Some(leading);
        vars[TEXT_LETTER_SPACING] = Some(tracking);
        vars[TEXT_PARAGRAPH_SPACING] = Some(space);
        let ops = ondin_core::bind_text_ops(&d.doc, t, vars);
        d.apply(ops);
    }
    let (doc, res) = d.done();
    holds_within("bound-type", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
    let export = html_of(&doc, &res, doc.get(doc.first_page()).unwrap().children());
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    for read in [
        "font-size: max(1px, calc(var(--size) * 1px))",
        "font-weight: clamp(1, round(var(--weight)), 1000)",
        "line-height: max(0, calc(var(--leading) / 100))",
        "letter-spacing: calc(var(--tracking) * 0.01em)",
        "margin-top: calc(var(--space) * 1px)",
    ] {
        assert!(css.contains(read), "styles.css lacks `{read}`:\n{css}");
    }
}

/// A paragraph with a spacing of its own keeps it under a bound default, even
/// where it equals the variable's value in the mode exported.
///
/// flip: `Walk::text` telling a paragraph's own spacing by its value → fails at
/// *"one paragraph draws the variable"*, the second paragraph bound too.
#[test]
fn a_paragraph_spacing_of_its_own_stays_under_a_bound_default() {
    use ondin_core::variable::{Cell, Mode, TEXT_PARAGRAPH_SPACING, VarKind, Variable};
    let mut d = Doc::new();
    let mode = d.ids.mint_mode();
    d.apply(vec![Operation::AddMode {
        collection: ondin_core::variable::CollectionId::FIRST,
        mode: Mode {
            id: mode,
            name: "Mode 1".into(),
        },
        index: 0,
        cells: vec![],
    }]);
    let space = d.ids.mint_var();
    d.apply(vec![Operation::AddVariable {
        var: Variable {
            id: space,
            name: "space".into(),
            kind: VarKind::Number,
            cells: [(mode, Cell::Number(6.0))].into_iter().collect(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 300.0, 130.0, "Board");
    let kind = text_node("One\nTwo\nThree", TextSizing::Auto, body_style(), |_| {});
    let t = d.add(Some(f), kind, Affine::translate((12.0, 10.0)), "Copy");
    let ops = ondin_core::bind_text_ops(
        &d.doc,
        t,
        ondin_core::variable::text_vars_at(TEXT_PARAGRAPH_SPACING, space),
    );
    d.apply(ops);
    let Some(NodeKind::Text { paragraph, .. }) = d.doc.get(t).map(|n| n.kind().clone()) else {
        panic!("a text");
    };
    // "Two" spaced 6 of its own, the variable's value here.
    let spans = ondin_core::ParaSpans::from_parts(
        vec![ondin_core::Span {
            start: 4,
            end: 8,
            attr: ondin_core::ParaAttr::Spacing(ondin_core::Length::Px(6.0)),
        }],
        &paragraph,
    );
    d.apply(vec![Operation::SetParagraphSpans { id: t, spans }]);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, doc.get(doc.first_page()).unwrap().children());
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert_eq!(
        css.matches("margin-top: calc(var(--space) * 1px)").count(),
        1,
        "one paragraph draws the variable:\n{css}"
    );
    assert!(
        css.contains("margin-top: 6px"),
        "the other its own 6:\n{css}"
    );
}

/// **A content that varies by mode draws each mode's text**: one wrapper per
/// value, shown by a hidden token `initial` in its modes and `none` in the
/// others, so the frame set to Dark draws Dark's two paragraphs and the other
/// Light's one, as the canvas does.
///
/// flip: `Tokens::mode_block` writing every hidden token `initial` → 88.1, each
/// frame drawing both texts.
#[test]
fn a_content_varying_by_mode_draws_each_modes_text() {
    use ondin_core::variable::{Cell, Mode, TEXT_CONTENT, VarKind, Variable};
    let mut d = Doc::new();
    let (light, dark) = (d.ids.mint_mode(), d.ids.mint_mode());
    let mode = |id, name: &str, index| Operation::AddMode {
        collection: ondin_core::variable::CollectionId::FIRST,
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    d.apply(vec![mode(light, "Light", 0), mode(dark, "Dark", 1)]);
    let greeting = d.ids.mint_var();
    d.apply(vec![Operation::AddVariable {
        var: Variable {
            id: greeting,
            name: "greeting".into(),
            kind: VarKind::String,
            cells: [
                (light, Cell::String("Hello".into())),
                (dark, Cell::String("Good evening\nand welcome".into())),
            ]
            .into_iter()
            .collect(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    }]);
    for (i, set) in [None, Some(dark)].into_iter().enumerate() {
        let f = d.frame(None, 0.0, i as f64 * 90.0, 260.0, 80.0, "Board");
        d.apply(vec![Operation::SetModes {
            id: f,
            modes: set.into_iter().collect(),
        }]);
        let kind = text_node("", TextSizing::Auto, body_style(), |_| {});
        let t = d.add(Some(f), kind, Affine::translate((12.0, 10.0)), "Greeting");
        let ops = ondin_core::bind_text_ops(
            &d.doc,
            t,
            ondin_core::variable::text_vars_at(TEXT_CONTENT, greeting),
        );
        d.apply(ops);
    }
    let (doc, res) = d.done();
    holds_within("content_by_mode", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
    let export = html_of(&doc, &res, doc.get(doc.first_page()).unwrap().children());
    let tokens =
        String::from_utf8(export.file("tokens.css").expect("tokens.css").to_vec()).unwrap();
    for block in [
        ":root, [data-mode=\"light\"] {\n  --greeting-light: initial;\n  --greeting-dark: none;\n}",
        "[data-mode=\"dark\"] {\n  --greeting-light: none;\n  --greeting-dark: initial;\n}",
    ] {
        assert!(tokens.contains(block), "lacks {block:?}:\n{tokens}");
    }
    assert!(export.approximated.is_empty(), "{:?}", export.approximated);
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    assert_eq!(html.matches("Hello").count(), 2, "{html}");
    assert_eq!(html.matches("and welcome").count(), 2, "{html}");
}

/// **Frames picking their mode by width beside other layers are pages of their
/// own**, each filling the window: the other layers keep `index.html` (`App`),
/// each frame its own file named by it, and nothing is reported.
///
/// flip: `walk::page` putting every origin on one page → fails at the
/// `home.html` lookup.
#[test]
fn frames_picked_by_width_beside_others_are_pages_of_their_own() {
    use ondin_core::variable::{CollectionId, Mode};
    use ondin_export::code::{react::react_of, vue::vue_of};
    let mut d = Doc::new();
    let (mobile, tablet) = (d.ids.mint_mode(), d.ids.mint_mode());
    let mode = |id, name: &str, index| Operation::AddMode {
        collection: CollectionId::FIRST,
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    d.apply(vec![
        mode(mobile, "Mobile", 0),
        mode(tablet, "Tablet", 1),
        Operation::SetBreakpoints {
            id: CollectionId::FIRST,
            breakpoints: Some([(mobile, 0.0), (tablet, 768.0)].into()),
        },
    ]);
    d.frame(None, 0.0, 0.0, 900.0, 200.0, "Home");
    d.frame(None, 1000.0, 0.0, 600.0, 300.0, "Pricing");
    d.frame(None, 1700.0, 0.0, 300.0, 300.0, "Con");
    d.add(
        None,
        NodeKind::Rect {
            size: Size::new(40.0, 40.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        Affine::translate((0.0, 400.0)),
        "Note",
    );
    let (doc, res) = d.done();
    let origins = doc.get(doc.first_page()).unwrap().children().to_vec();
    let export = html_of(&doc, &res, &origins);
    assert!(export.approximated.is_empty(), "{:?}", export.approximated);
    let file = |name: &str| {
        String::from_utf8(
            export
                .file(name)
                .unwrap_or_else(|| panic!("{name}"))
                .to_vec(),
        )
        .unwrap()
    };
    let (index, home, pricing) = (file("index.html"), file("home.html"), file("pricing.html"));
    assert!(
        index.contains("class=\"note\"") && !index.contains("class=\"home\""),
        "{index}"
    );
    assert!(home.contains("class=\"home\""), "Home's page:\n{home}");
    assert!(pricing.contains("class=\"pricing\""), "{pricing}");
    // A Windows device name is no file name.
    assert!(
        export.file("con-page.html").is_some() && export.file("con.html").is_none(),
        "Con's page"
    );
    let css = file("styles.css");
    assert_eq!(
        css.matches("width: 100%").count(),
        6,
        "each page and frame fills the window:\n{css}"
    );
    let react = react_of(&doc, &res, &origins);
    for path in ["App.tsx", "Home.tsx", "Home.module.css", "Pricing.tsx"] {
        assert!(react.file(path).is_some(), "React lacks {path}");
    }
    let home = String::from_utf8(react.file("Home.tsx").unwrap().to_vec()).unwrap();
    assert!(home.contains("export default function Home() {"), "{home}");
    let vue = vue_of(&doc, &res, &origins);
    assert!(vue.file("Pricing.vue").is_some() && vue.file("App.vue").is_some());
}

/// A layer that is not the window names the mode of the collection picked by
/// width that it draws in, the default too, so a wide window's media block does
/// not reach it; and a direction string aliasing a wider one reads that one's
/// word bare, which `flex-direction` takes.
///
/// flip: the walk leaving out the default of the collection picked by width →
/// fails at "pinned to Mobile"; `Tokens::value` quoting an aliased string →
/// fails at "bare".
#[test]
fn a_layer_off_the_window_pins_its_width_mode_and_an_aliased_direction_is_bare() {
    use ondin_core::variable::{Cell, CollectionId, Field, Mode, Scope, VarKind, Variable};
    let mut d = Doc::new();
    let (mobile, tablet) = (d.ids.mint_mode(), d.ids.mint_mode());
    let mode = |id, name: &str, index| Operation::AddMode {
        collection: CollectionId::FIRST,
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    let (label, dir) = (d.ids.mint_var(), d.ids.mint_var());
    let string = |id, name: &str, a: Cell, b: Cell, scope: &[Field]| Operation::AddVariable {
        var: Variable {
            id,
            name: name.into(),
            kind: VarKind::String,
            cells: [(mobile, a), (tablet, b)].into_iter().collect(),
            scope: Some(Scope::of(scope.iter().copied())),
            origin: None,
            forked: None,
            description: String::new(),
        },
    };
    d.apply(vec![
        mode(mobile, "Mobile", 0),
        mode(tablet, "Tablet", 1),
        Operation::SetBreakpoints {
            id: CollectionId::FIRST,
            breakpoints: Some([(mobile, 0.0), (tablet, 768.0)].into()),
        },
        string(
            label,
            "label",
            Cell::String("Column".into()),
            Cell::String("Row".into()),
            &[Field::FlexDirection, Field::Content],
        ),
        string(
            dir,
            "dir",
            Cell::Alias(label),
            Cell::Alias(label),
            &[Field::FlexDirection],
        ),
    ]);
    let home = d.frame(None, 0.0, 0.0, 375.0, 200.0, "Home");
    let card = d.frame(Some(home), 10.0, 10.0, 200.0, 100.0, "Card");
    d.apply(vec![Operation::SetDisplay {
        id: card,
        display: Some(Display::Flex(Flex::default())),
    }]);
    let ops = ondin_core::build::bind_flow_ops(&d.doc, card, 0, Some(dir));
    d.apply(ops);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[card]);
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    assert!(
        html.contains("data-mode=\"mobile\""),
        "pinned to Mobile:\n{html}"
    );
    let tokens = String::from_utf8(export.file("tokens.css").unwrap().to_vec()).unwrap();
    assert!(tokens.contains("--label: column;"), "bare:\n{tokens}");
    assert!(tokens.contains("--dir: var(--label);"), "{tokens}");
}

/// An export origin setting a mode of its own still names its ancestors' modes
/// of the other collections, the width's pin among them: a *Card* set to Dark
/// inside a 375 *Home* set to Comfy writes all three.
///
/// flip: `Walk::page_root` naming the inherited modes only for a layer setting
/// none → fails at "every collection's".
#[test]
fn an_origin_setting_a_mode_keeps_its_ancestors_others() {
    use ondin_core::variable::{Collection, CollectionId, Mode};
    let mut d = Doc::new();
    let [light, dark, mobile, desktop, compact, comfy] = [(); 6].map(|_| d.ids.mint_mode());
    let (size, density) = (d.ids.mint_collection(), d.ids.mint_collection());
    let mode = |id, name: &str| Mode {
        id,
        name: name.into(),
    };
    let collection = |id, name: &str, modes, index| Operation::AddCollection {
        collection: Collection {
            id,
            name: name.into(),
            modes,
            breakpoints: None,
        },
        index,
    };
    d.apply(vec![
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(light, "Light"),
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(dark, "Dark"),
            index: 1,
            cells: vec![],
        },
        collection(
            size,
            "Size",
            vec![mode(mobile, "Mobile"), mode(desktop, "Desktop")],
            1,
        ),
        Operation::SetBreakpoints {
            id: size,
            breakpoints: Some([(mobile, 0.0), (desktop, 1024.0)].into()),
        },
        collection(
            density,
            "Density",
            vec![mode(compact, "Compact"), mode(comfy, "Comfy")],
            2,
        ),
    ]);
    let home = d.frame(None, 0.0, 0.0, 375.0, 200.0, "Home");
    let card = d.frame(Some(home), 10.0, 10.0, 200.0, 100.0, "Card");
    d.apply(vec![
        Operation::SetModes {
            id: home,
            modes: vec![comfy],
        },
        Operation::SetModes {
            id: card,
            modes: vec![dark],
        },
    ]);
    let (doc, res) = d.done();
    assert_eq!(doc.mode_at(card, size), Some(mobile), "Home picks Mobile");
    let export = html_of(&doc, &res, &[card]);
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    let words: std::collections::BTreeSet<&str> = html
        .split("data-mode=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect(&html)
        .split(' ')
        .collect();
    assert_eq!(
        words,
        ["comfy", "dark", "mobile"].into(),
        "every collection's:\n{html}"
    );
    assert_eq!(html.matches("data-mode=").count(), 1, "{html}");
}

/// A picture inside the window frame bakes the frame's width mode, so one
/// reading a variable another width changes — a fill, a visibility — is
/// reported; one reading none, or pinned to its mode, is not.
///
/// flip: `Walk::picture` never asking `picture_follows_width` → fails at
/// "both pictures".
#[test]
fn a_picture_in_the_window_that_reads_the_width_is_reported() {
    use ondin_core::BrushSource;
    use ondin_core::variable::{Cell, CollectionId, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let (mobile, desktop) = (d.ids.mint_mode(), d.ids.mint_mode());
    let (brand, wide) = (d.ids.mint_var(), d.ids.mint_var());
    let mode = |id, name: &str, index| Operation::AddMode {
        collection: CollectionId::FIRST,
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    let var = |id, name: &str, kind, a, b| Operation::AddVariable {
        var: Variable {
            id,
            name: name.into(),
            kind,
            cells: [(mobile, a), (desktop, b)].into(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    };
    d.apply(vec![
        mode(mobile, "Mobile", 0),
        mode(desktop, "Desktop", 1),
        Operation::SetBreakpoints {
            id: CollectionId::FIRST,
            breakpoints: Some([(mobile, 0.0), (desktop, 1024.0)].into()),
        },
        var(
            brand,
            "brand",
            VarKind::Color,
            Cell::Color(Color::from_rgba8(220, 40, 40, 255)),
            Cell::Color(Color::from_rgba8(40, 90, 220, 255)),
        ),
        var(
            wide,
            "show-desktop",
            VarKind::Boolean,
            Cell::Boolean(false),
            Cell::Boolean(true),
        ),
    ]);
    let home = d.frame(None, 0.0, 0.0, 375.0, 200.0, "Home");
    let pinned = d.frame(Some(home), 200.0, 10.0, 100.0, 100.0, "Pinned");
    d.apply(vec![Operation::SetModes {
        id: pinned,
        modes: vec![mobile],
    }]);
    let star = |d: &mut Doc, parent, x, name: &str| {
        d.add(
            Some(parent),
            NodeKind::Star {
                size: Size::new(20.0, 20.0),
                points: 5,
                inner_ratio: 0.5,
                corner_radius: 0.0,
                corner_smoothing: 0.0,
            },
            Affine::translate((x, 10.0)),
            name,
        )
    };
    let bound = Fill {
        brush: BrushSource::Bound {
            var: brand,
            alpha: 1.0,
        },
        visible: true,
    };
    let icon = star(&mut d, home, 10.0, "Icon");
    let spark = star(&mut d, home, 40.0, "Spark");
    let plain = star(&mut d, home, 70.0, "Plain");
    let kept = star(&mut d, pinned, 10.0, "Kept");
    d.fill(spark, solid(0, 0, 0, 255));
    d.fill(plain, solid(0, 0, 0, 255));
    for id in [icon, kept] {
        d.apply(vec![Operation::SetFills {
            id,
            fills: keyed_by_position([bound.clone()]),
        }]);
    }
    d.apply(vec![Operation::SetVisibleVar {
        id: spark,
        var: Some(wide),
    }]);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[home]);
    let follows = |what: &str| {
        export
            .approximated
            .iter()
            .any(|a| a.contains(what) && a.contains("width mode"))
    };
    assert!(
        follows("“Icon”") && follows("“Spark”"),
        "both pictures: {:?}",
        export.approximated
    );
    assert!(
        !follows("“Plain”") && !follows("“Kept”"),
        "{:?}",
        export.approximated
    );
}

/// An alias into another collection follows that collection's mode a frame
/// sets: Theme's `gap` aliases Density's `space` (4, 12), and a row set to
/// Comfy is 12 apart, as on the canvas. One aliasing by mode, which no block
/// of Density can redeclare, is reported.
///
/// flip: `Tokens::mode_block` declaring only its own collection's variables →
/// fails at "Comfy's block", and at 105 in the browser.
#[test]
fn an_alias_into_another_collection_follows_its_mode() {
    use ondin_core::variable::{Cell, Collection, CollectionId, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let [light, dark, compact, comfy] = [(); 4].map(|_| d.ids.mint_mode());
    let density = d.ids.mint_collection();
    let (space, gap) = (d.ids.mint_var(), d.ids.mint_var());
    let mode = |id, name: &str| Mode {
        id,
        name: name.into(),
    };
    let var = |id, name: &str, cells: Vec<(_, Cell)>| Operation::AddVariable {
        var: Variable {
            id,
            name: name.into(),
            kind: VarKind::Number,
            cells: cells.into_iter().collect(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    };
    d.apply(vec![
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(light, "Light"),
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(dark, "Dark"),
            index: 1,
            cells: vec![],
        },
        Operation::AddCollection {
            collection: Collection {
                id: density,
                name: "Density".into(),
                modes: vec![mode(compact, "Compact"), mode(comfy, "Comfy")],
                breakpoints: None,
            },
            index: 1,
        },
        var(
            space,
            "space",
            vec![(compact, Cell::Number(4.0)), (comfy, Cell::Number(12.0))],
        ),
        var(
            gap,
            "gap",
            vec![(light, Cell::Alias(space)), (dark, Cell::Alias(space))],
        ),
    ]);
    let row = d.frame(None, 0.0, 0.0, 200.0, 60.0, "Row");
    d.fill(row, solid(250, 250, 250, 255));
    d.apply(vec![
        Operation::SetModes {
            id: row,
            modes: vec![comfy],
        },
        Operation::SetDisplay {
            id: row,
            display: Some(Display::Flex(Flex {
                padding: [10.0; 4],
                ..Default::default()
            })),
        },
        Operation::SetSpacingVars {
            id: row,
            vars: [Some(gap), None, None, None, None, None],
        },
    ]);
    for _ in 0..2 {
        let r = d.rect(row, 0.0, 0.0, 40.0, 40.0, 0.0);
        d.fill(r, solid(40, 90, 200, 255));
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[row]);
    let tokens = String::from_utf8(export.file("tokens.css").unwrap().to_vec()).unwrap();
    let block = &tokens[tokens.find("[data-mode~=\"comfy\"] {").expect(&tokens)..];
    assert!(
        block[..block.find('}').unwrap()].contains("--gap: var(--space);"),
        "Comfy's block:\n{tokens}"
    );
    holds("alias-across-collections", &doc, &res);

    let mut doc = doc;
    doc.apply(&Transaction(vec![Operation::SetVariableCell {
        id: gap,
        mode: dark,
        cell: Cell::Number(6.0),
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let export = html_of(&doc, &res, &[row]);
    assert_eq!(
        export.approximated,
        vec![
            "variable “gap”: it aliases another collection's variables by mode, so a frame \
             setting that collection's mode reads the page's value"
        ],
    );
}

/// A 24×16 picture with a hard quadrant in each corner over a smooth ramp, so a
/// mirror, a turn or a misplaced crop all move something visible.
fn picture_png() -> Vec<u8> {
    let (w, h) = (24u32, 16u32);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let px = match (x < 6, y < 4, x >= w - 6, y >= h - 4) {
                (true, true, _, _) => [220, 40, 40, 255],
                (_, true, true, _) => [40, 160, 60, 255],
                (true, _, _, true) => [40, 70, 220, 255],
                (_, _, true, true) => [240, 200, 40, 255],
                _ => [(x * 10) as u8, (y * 15) as u8, 160, 255],
            };
            rgba.extend_from_slice(&px);
        }
    }
    ondin_export::png::encode_rgba8(&rgba, w, h)
}

/// Image fills as backgrounds: each of the four fits, a turned, mirrored picture
/// whose file is baked upright, and a crop under per-side borders.
///
/// flip: Fill written as `contain` → 155 at the first rect; the bake skipping the
/// mirror → 149 at the fifth; `background-origin` left at `padding-box` → 50.8 at
/// the bordered one.
#[test]
fn image_fills_match_the_canvas() {
    use ondin_core::{
        ImageBrush, ImageEntry, ImageFit, ImageFormat, ImageId, ImageOrient, ImageRef, ImageSource,
    };
    let mut d = Doc::new();
    let id = ImageId("photo".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(picture_png().into()),
            format: ImageFormat::Png,
            width: 24,
            height: 16,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 502.0, 100.0, "Board");
    let fills = |r: ImageRef| {
        vec![Brush::Image(ImageBrush {
            image: r,
            sampler: Default::default(),
        })]
    };
    let crop = ImageRef {
        fit: ImageFit::Crop,
        crop: ondin_core::kurbo::Rect::new(0.1, 0.2, 0.6, 0.9),
        ..ImageRef::new(id.clone())
    };
    let base = ImageRef::new(id);
    let cases = [
        ImageRef {
            fit: ImageFit::Fill,
            ..base.clone()
        },
        ImageRef {
            fit: ImageFit::Fit,
            ..base.clone()
        },
        ImageRef {
            fit: ImageFit::Crop,
            crop: ondin_core::kurbo::Rect::new(0.1, 0.2, 0.6, 0.9),
            ..base.clone()
        },
        ImageRef {
            fit: ImageFit::Tile,
            tile_scale: 1.5,
            ..base.clone()
        },
        ImageRef {
            orient: ImageOrient {
                quarters: 1,
                mirrored: true,
            },
            ..base.clone()
        },
    ];
    for (i, r) in cases.into_iter().enumerate() {
        let rect = d.rect(f, 10.0 + i as f64 * 82.0, 10.0, 72.0, 80.0, 0.0);
        d.fills(rect, fills(r));
    }
    // Borders do not move the picture: it is framed on the whole box.
    let bordered = d.rect(f, 420.0, 10.0, 72.0, 80.0, 0.0);
    d.fills(bordered, fills(crop.clone()));
    d.stroke(
        bordered,
        4.0,
        StrokeAlign::Inside,
        StrokeSides::Custom {
            top: 10.0,
            right: 0.0,
            bottom: 2.0,
            left: 6.0,
        },
    );
    let (doc, res) = d.done();
    holds("images", &doc, &res);
    let export = html_of(&doc, &res, &[f]);
    assert!(
        export.positioned.is_empty(),
        "all CSS: {:?}",
        export.positioned
    );
    assert!(export.file("assets/photo.png").is_some());
    assert!(
        export.file("assets/photo-r1m.png").is_some(),
        "the turned file is baked"
    );
}

/// A GIF ships as a PNG of its first frame, the frame the canvas draws, since a
/// browser would animate the GIF; and two pictures sampled differently on one
/// layer go to SVG, `image-rendering` being the element's.
#[test]
fn a_gif_is_baked_and_mixed_sampling_is_svg() {
    use ondin_core::peniko::{ImageQuality, ImageSampler};
    use ondin_core::{ImageBrush, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource};
    let frame = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255]));
    let mut gif = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut gif)
        .encode_frame(image::Frame::new(frame))
        .expect("a gif");
    let mut d = Doc::new();
    let id = ImageId("anim".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(gif.into()),
            format: ImageFormat::Gif,
            width: 4,
            height: 4,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let fill = |quality| {
        Brush::Image(ImageBrush {
            image: ImageRef::new(id.clone()),
            sampler: ImageSampler {
                quality,
                ..Default::default()
            },
        })
    };
    let one = d.rect(f, 10.0, 10.0, 30.0, 30.0, 0.0);
    d.fills(one, vec![fill(ImageQuality::Low)]);
    let two = d.rect(f, 50.0, 10.0, 30.0, 30.0, 0.0);
    d.fills(two, vec![fill(ImageQuality::Low), fill(ImageQuality::High)]);
    d.apply(vec![Operation::SetName {
        id: two,
        name: "Mixed".into(),
    }]);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let png = export
        .file("assets/anim.png")
        .expect("the GIF is written as PNG");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(export.file("assets/anim.gif").is_none());
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(css.contains("image-rendering: pixelated"), "{css}");
    assert_eq!(
        export.positioned,
        vec!["layer “Mixed”: pictures sampled two ways, written as inline SVG"]
    );
}

/// A crop past the picture's edge, which the canvas fills with the edge pixels,
/// has neither a CSS nor an SVG form: it is a PNG, and reported.
///
/// flip: the SVG writer not reporting `crops_past_picture` → `approximated` is
/// empty and the layer goes out as an SVG that repeats the picture.
#[test]
fn a_crop_past_the_picture_is_a_reported_png() {
    use ondin_core::{
        ImageBrush, ImageEntry, ImageFit, ImageFormat, ImageId, ImageRef, ImageSource,
    };
    let mut d = Doc::new();
    let id = ImageId("photo".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(picture_png().into()),
            format: ImageFormat::Png,
            width: 24,
            height: 16,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let r = d.rect(f, 10.0, 10.0, 72.0, 80.0, 0.0);
    d.fills(
        r,
        vec![Brush::Image(ImageBrush {
            image: ImageRef {
                fit: ImageFit::Crop,
                crop: ondin_core::kurbo::Rect::new(-0.2, 0.0, 0.8, 1.0),
                ..ImageRef::new(id)
            },
            sampler: Default::default(),
        })],
    );
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    assert_eq!(
        export.approximated,
        vec![
            "layer “Rect”: a crop past the picture's edge, repeated where the canvas stretches it, so it is a PNG"
        ]
    );
    assert!(export.file("assets/rect.png").is_some());
}

/// A picture fill under 100% goes to SVG, CSS backgrounds having no opacity of
/// their own. Not compared in a browser: the CPU renderer cannot draw an image
/// paint with alpha (`vello_common` leaves it unimplemented).
#[test]
fn a_faint_picture_fill_is_written_as_svg() {
    use ondin_core::{ImageBrush, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource};
    let mut d = Doc::new();
    let id = ImageId("photo".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(picture_png().into()),
            format: ImageFormat::Png,
            width: 24,
            height: 16,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let r = d.rect(f, 10.0, 10.0, 72.0, 80.0, 0.0);
    d.fills(
        r,
        vec![Brush::Image(ImageBrush {
            image: ImageRef::new(id),
            sampler: ondin_core::peniko::ImageSampler {
                alpha: 0.5,
                ..Default::default()
            },
        })],
    );
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    assert_eq!(
        export.positioned,
        vec!["layer “Rect”: a picture fill under 100%, written as inline SVG"]
    );
}

/// Inline SVGs link the pictures they draw, so layers sharing one share its file
/// rather than each embedding it; React imports it, Vue links it from the file.
///
/// flip: `Walk::picture` linking nothing → the page embeds two base64 copies.
#[test]
fn a_picture_in_inline_svg_is_one_linked_file() {
    use ondin_core::{ImageBrush, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource};
    let mut d = Doc::new();
    let id = ImageId("photo".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(picture_png().into()),
            format: ImageFormat::Png,
            width: 24,
            height: 16,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 200.0, 100.0, "Board");
    for x in [10.0, 100.0] {
        let r = d.rect(f, x, 10.0, 72.0, 80.0, 0.0);
        d.fills(
            r,
            vec![Brush::Image(ImageBrush {
                image: ImageRef::new(id.clone()),
                sampler: ondin_core::peniko::ImageSampler {
                    alpha: 0.5,
                    ..Default::default()
                },
            })],
        );
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    assert!(!html.contains("base64"), "{html}");
    assert_eq!(
        html.matches("href=\"assets/photo.png\"").count(),
        2,
        "{html}"
    );
    assert!(export.file("assets/photo.png").is_some());

    let react = ondin_export::code::react::react_of(&doc, &res, &[f]);
    let app = String::from_utf8(react.file("App.tsx").unwrap().to_vec()).unwrap();
    assert!(
        app.contains("import photo_png from \"./assets/photo.png\";"),
        "{app}"
    );
    assert!(app.contains("href={photo_png}"), "{app}");
    let vue = ondin_export::code::vue::vue_of(&doc, &res, &[f]);
    let app = String::from_utf8(vue.file("App.vue").unwrap().to_vec()).unwrap();
    assert!(app.contains("href=\"./assets/photo.png\""), "{app}");
}

/// A text's bound colour and a single side's bound width are `var()` too, not the
/// default mode's numbers.
#[test]
fn bound_text_colour_and_side_width_are_written_as_var() {
    use ondin_core::BrushSource;
    use ondin_core::variable::{Cell, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let mode = d.ids.mint_mode();
    d.apply(vec![Operation::AddMode {
        collection: ondin_core::variable::CollectionId::FIRST,
        mode: Mode {
            id: mode,
            name: "Base".into(),
        },
        index: 0,
        cells: vec![],
    }]);
    let (ink, edge) = (d.ids.mint_var(), d.ids.mint_var());
    for (id, name, kind, cell) in [
        (ink, "ink", VarKind::Color, Cell::Color(Color::BLACK)),
        (edge, "edge", VarKind::Number, Cell::Number(3.0)),
    ] {
        d.apply(vec![Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind,
                cells: [(mode, cell)].into_iter().collect(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]);
    }
    let f = d.frame(None, 0.0, 0.0, 200.0, 100.0, "Board");
    let t = d.add(
        Some(f),
        NodeKind::Text {
            content: "Ink".into(),
            style: Box::default(),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        Affine::translate((10.0, 10.0)),
        "Label",
    );
    d.apply(vec![Operation::SetFills {
        id: t,
        fills: keyed_by_position([Fill {
            brush: BrushSource::Bound {
                var: ink,
                alpha: 1.0,
            },
            visible: true,
        }]),
    }]);
    let r = d.rect(f, 10.0, 50.0, 80.0, 30.0, 0.0);
    d.stroke(r, 0.0, StrokeAlign::Inside, StrokeSides::Top);
    let mut strokes = d.doc.get(r).unwrap().paint().strokes.clone();
    strokes[0].value.width_var = Some(edge);
    d.apply(vec![Operation::SetStrokes { id: r, strokes }]);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(css.contains("color: var(--ink)"), "{css}");
    assert!(
        css.contains("border-top: max(0px, calc(var(--edge) * 1px)) solid"),
        "{css}"
    );
}

/// A background blur on a box is `backdrop-filter` at half its radius, as a
/// layer blur's `blur()` is; beside a shadow, whose `filter` would also apply
/// over the backdrop, the layer is a picture and says why.
#[test]
fn a_background_blur_is_a_backdrop_filter_on_a_box() {
    let render = |shadowed: bool| {
        let mut d = Doc::new();
        let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
        let r = d.rect(f, 20.0, 20.0, 60.0, 60.0, 0.0);
        d.fill(r, solid(255, 255, 255, 64));
        let mut effects = vec![EffectKind::BackgroundBlur { radius: 16.0 }];
        if shadowed {
            effects.push(EffectKind::DropShadow(Shadow::default()));
        }
        d.effects(r, effects);
        let (doc, res) = d.done();
        html_of(&doc, &res, &[f])
    };
    let plain = render(false);
    let css = String::from_utf8(plain.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(css.contains("backdrop-filter: blur(8px)"), "{css}");
    assert!(plain.approximated.is_empty(), "{:?}", plain.approximated);

    let shadowed = render(true);
    assert!(
        shadowed
            .approximated
            .iter()
            .any(|s| s.contains("a background blur")),
        "{:?}",
        shadowed.approximated
    );
}

/// A feature the SVG writer approximates goes to a PNG and is reported, not
/// passed off as exact.
#[test]
fn an_approximated_layer_is_reported() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let r = d.rect(f, 20.0, 20.0, 60.0, 60.0, 0.0);
    d.fill(r, solid(200, 200, 200, 255));
    d.effects(
        r,
        vec![EffectKind::DropShadow(Shadow {
            offset: Vec2::new(2.0, 2.0),
            blur: 4.0,
            spread: 6.0,
            color: Color::BLACK,
            blend: ondin_core::BlendMode::Normal,
        })],
    );
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    assert_eq!(
        export.approximated,
        vec!["layer “Rect”: a shadow's spread, drawn with a square kernel, so it is a PNG"]
    );
    assert!(
        export.file("assets/rect.png").is_some(),
        "the PNG is in the export"
    );
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    assert!(
        html.contains(r#"<img class="rect" src="assets/rect.png""#),
        "{html}"
    );
}

/// Two PNGs of one class in two files are two assets: a component's *Rect* and
/// the page's, each with its own shadow.
/// flip: `Walk::picture` naming a new PNG by its class alone → fails at "two
/// paths".
#[test]
fn pngs_of_one_class_in_two_files_keep_their_own_paths() {
    let spread = |spread: f64| {
        vec![EffectKind::DropShadow(Shadow {
            offset: Vec2::new(2.0, 2.0),
            blur: 4.0,
            spread,
            color: Color::BLACK,
            blend: ondin_core::BlendMode::Normal,
        })]
    };
    let mut d = Doc::new();
    let mains = d.frame(None, 0.0, 300.0, 380.0, 100.0, "Mains");
    let board = d.frame(None, 0.0, 0.0, 380.0, 180.0, "Board");
    let shadowed = |d: &mut Doc, c: [NodeId; 3]| d.effects(c[1], spread(6.0));
    let main = d.card(mains, (10.0, 10.0), None, shadowed);
    d.card(board, (10.0, 100.0), Some(main), shadowed);
    let own = d.rect(board, 300.0, 20.0, 40.0, 40.0, 0.0);
    d.fill(own, solid(30, 120, 200, 255));
    d.effects(own, spread(3.0));
    let (doc, res) = d.done();
    let export = ondin_export::code::Target::React.export(&doc, &res, &[board]);
    assert!(export.inlined.is_empty(), "{:?}", export.inlined);
    let mut pngs: Vec<&str> = export
        .files
        .iter()
        .map(|f| f.path.as_str())
        .filter(|p| p.ends_with(".png"))
        .collect();
    pngs.sort_unstable();
    let len = pngs.len();
    pngs.dedup();
    assert_eq!((len, pngs.len()), (2, 2), "two paths: {pngs:?}");
}

// ---------------------------------------------------------------------------
// Components: the React and Vue walk, drawn as plain HTML
// ---------------------------------------------------------------------------

impl Doc {
    /// A card — a frame holding an icon and a title, `painted` after it is made —
    /// under `parent`, linked to `main`'s layers when it is given.
    fn card(
        &mut self,
        parent: NodeId,
        at: (f64, f64),
        main: Option<[NodeId; 3]>,
        painted: impl Fn(&mut Doc, [NodeId; 3]),
    ) -> [NodeId; 3] {
        let frame = self.frame(Some(parent), at.0, at.1, 160.0, 60.0, "Card");
        let icon = self.rect(frame, 12.0, 18.0, 24.0, 24.0, 4.0);
        self.fill(icon, solid(220, 60, 60, 255));
        let title = text_node(
            "Hello",
            TextSizing::Auto,
            TextStyle {
                font_size: 20.0,
                line_height: Some(ondin_core::Length::Px(24.0)),
                ..Default::default()
            },
            |_| {},
        );
        let title = self.add(Some(frame), title, Affine::translate((48.0, 18.0)), "Title");
        let card = [frame, icon, title];
        painted(self, card);
        match main {
            None => self.apply(vec![Operation::SetComponent {
                id: frame,
                component: true,
            }]),
            Some(m) => {
                // The two trees are alike, `painted`'s layers included.
                let mut links = Vec::new();
                let mut pairs = vec![(frame, m[0])];
                while let Some((copy, source)) = pairs.pop() {
                    links.push(Operation::SetLink {
                        id: copy,
                        link: Some(source),
                    });
                    let children = |id| self.doc.get(id).unwrap().children().to_vec();
                    pairs.extend(children(copy).into_iter().zip(children(source)));
                }
                self.apply(links);
            }
        }
        card
    }

    /// `main` given a property per `(name, kind, bound layer)`.
    fn properties(&mut self, main: NodeId, props: &[(&str, PropKind, NodeId)]) {
        for (name, kind, bound) in props {
            let (tx, _) =
                variant::define(&self.doc, &mut self.ids, main, name, *kind, vec![*bound])
                    .expect("a property");
            self.doc.apply(&tx).expect("the property applies");
        }
    }

    /// The instance at `root` set to `values`, in its properties' order.
    fn set_properties(&mut self, root: NodeId, values: Vec<PropValue>) {
        let props = variant::instance_properties(&self.doc, root);
        for (p, v) in props.iter().zip(values) {
            let ops = variant::set_property(&self.doc, &[root], p, &v);
            self.apply(ops);
        }
    }
}

/// A main card with a boolean and a text property, and two uses of it: one
/// setting both, one setting neither. The control for the component fixtures:
/// both are uses, and the page matches.
///
/// flip: `html_of_components` dropping a use's arguments → 125.3 at the icon
/// the set use hides.
#[test]
fn component_uses_match_the_canvas() {
    let mut d = Doc::new();
    let board = d.frame(None, 0.0, 0.0, 380.0, 180.0, "Board");
    let main = d.card(board, (10.0, 10.0), None, |_, _| {});
    d.properties(
        main[0],
        &[
            ("Show icon", PropKind::Boolean, main[1]),
            ("Title", PropKind::Text, main[2]),
        ],
    );
    let set = d.card(board, (10.0, 100.0), Some(main), |_, _| {});
    d.set_properties(
        set[0],
        vec![PropValue::Boolean(false), PropValue::Text("World".into())],
    );
    d.card(board, (200.0, 100.0), Some(main), |_, _| {});
    let (doc, res) = d.done();
    let export = html_of_components(&doc, &res, &[board]);
    assert!(export.inlined.is_empty(), "{:?}", export.inlined);
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    assert_eq!(html.matches("Card--card").count(), 2, "two uses: {html}");
    holds_components("component-uses", &doc, &res);
}

/// A marked set's state drawn by its CSS: a *Card* set whose Hover recolours
/// the icon, a use at rest and one forced to Hover, both written as buttons.
///
/// flip: `Element::states_css` writing no rules → 123.3 at the forced use's
/// icon, which stays red.
#[test]
fn a_forced_state_matches_the_canvas() {
    use ondin_core::variant::{Interaction, VariantProp, VariantSet};
    let mut d = Doc::new();
    let set = d.frame(None, 0.0, 200.0, 380.0, 180.0, "Chip");
    let rest = d.card(set, (10.0, 10.0), None, |_, _| {});
    d.apply(vec![
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![VariantProp::new(
                    "State",
                    vec!["Default".into(), "Hover".into()],
                )],
                ..Default::default()
            }),
        },
        Operation::SetVariant {
            id: rest[0],
            values: vec!["Default".into()],
        },
    ]);
    let res = Resolved::rebuild(&d.doc);
    let (tx, hover) = variant::add_variant(&d.doc, &res, &mut d.ids, set, Some(rest[0])).unwrap();
    d.doc.apply(&tx).unwrap();
    let tx = variant::set_values(&d.doc, hover, vec!["Hover".into()]).unwrap();
    d.doc.apply(&tx).unwrap();
    let kids = d.doc.get(hover).unwrap().children().to_vec();
    let main = [hover, kids[0], kids[1]];
    let blue = |d: &mut Doc, c: [NodeId; 3]| d.fill(c[1], solid(40, 90, 220, 255));
    blue(&mut d, main);
    let tie = variant::tie_state(&d.doc, set, 0, 1, Some(Interaction::Hover)).unwrap();
    d.doc.apply(&tie).unwrap();
    let board = d.frame(None, 0.0, 0.0, 380.0, 80.0, "Board");
    for (main, x) in [(rest[0], 10.0), (hover, 200.0)] {
        let (tx, made) = ondin_core::insert_subtrees(
            &d.doc,
            &mut d.ids,
            &[ondin_core::Placement {
                nodes: d.doc.capture_subtree(main).unwrap(),
                parent: board,
                index: None,
            }],
            Default::default(),
        );
        d.doc.apply(&tx).unwrap();
        d.apply(vec![Operation::SetTransform {
            id: made[0],
            transform: Affine::translate((x, 10.0)),
        }]);
    }
    let (doc, res) = d.done();
    let export = html_of_components(&doc, &res, &[board]);
    assert!(export.inlined.is_empty(), "{:?}", export.inlined);
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    assert!(html.contains("data-state=\"Hover\""), "{html}");
    holds_components("forced-state", &doc, &res);
}

/// `holds` for `html_of_components`, after three passes of `blurred`: a
/// component's text lands at whatever x its use puts it, and at some offsets
/// glyph antialiasing alone passes the two-pass limit (8.9 on the default use
/// in `component_uses_match_the_canvas`).
fn holds_components(test: &str, doc: &Document, res: &Resolved) {
    holds_as(test, doc, res, 3, MAX_TEXT_CELL_ERROR, html_of_components);
}

/// A use whose visibility varies by mode is shown by an element around it, so
/// its component root keeps its own flex layout: shown in Light, hidden in Dark.
///
/// flip: `Walk::node` writing the visibility token on the use itself → fails
/// at "no display on the use".
#[test]
fn a_use_shown_by_mode_keeps_its_roots_layout() {
    use ondin_core::variable::{Cell, CollectionId, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let (light, dark) = (d.ids.mint_mode(), d.ids.mint_mode());
    for (index, (id, name)) in [(light, "Light"), (dark, "Dark")].into_iter().enumerate() {
        d.apply(vec![Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: Mode {
                id,
                name: name.into(),
            },
            index,
            cells: vec![],
        }]);
    }
    let show = d.ids.mint_var();
    d.apply(vec![Operation::AddVariable {
        var: Variable {
            id: show,
            name: "show-cta".into(),
            kind: VarKind::Boolean,
            cells: [(light, Cell::Boolean(true)), (dark, Cell::Boolean(false))].into(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    }]);
    // A row, whose items a block would stack; centred across only, so the text
    // stays on whole pixels.
    let centred = |d: &mut Doc, c: [NodeId; 3]| {
        d.apply(vec![Operation::SetDisplay {
            id: c[0],
            display: Some(Display::Flex(Flex {
                align_items: AlignItems::Center,
                ..Default::default()
            })),
        }]);
    };
    let mains = d.frame(None, 0.0, 300.0, 380.0, 100.0, "Mains");
    let main = d.card(mains, (10.0, 10.0), None, centred);
    let mut uses = Vec::new();
    for (i, mode) in [light, dark].into_iter().enumerate() {
        let board = d.frame(None, i as f64 * 200.0, 0.0, 180.0, 80.0, "Board");
        d.apply(vec![Operation::SetModes {
            id: board,
            modes: vec![mode],
        }]);
        let used = d.card(board, (10.0, 10.0), Some(main), centred)[0];
        d.apply(vec![Operation::SetVisibleVar {
            id: used,
            var: Some(show),
        }]);
        uses.push(used);
    }
    let (doc, res) = d.done();
    let export = ondin_export::code::Target::React.export(&doc, &res, &[uses[0]]);
    assert!(export.inlined.is_empty(), "{:?}", export.inlined);
    let css = String::from_utf8(export.file("App.module.css").unwrap().to_vec()).unwrap();
    let rule = &css[css.find(".card.card {").expect(&css)..];
    let rule = &rule[..rule.find('}').unwrap()];
    assert!(!rule.contains("display"), "no display on the use: {css}");
    assert!(
        css.contains("display: var(--show-cta, contents)"),
        "the element around it: {css}"
    );
    holds_components("use-shown-by-mode", &doc, &res);
}

/// A boolean property bound to a dot inside a masked group, which is drawn as a
/// picture: a use cannot pass it into the picture, so the instance hiding the
/// dot is its layers.
///
/// flip: `Walk::unwritable` checking only the trees' roots → `inlined` is empty;
/// past that assertion, the use draws the main's dot at 112.0.
#[test]
fn a_property_drawn_as_a_picture_is_not_frozen_at_the_mains_value() {
    let mut d = Doc::new();
    let board = d.frame(None, 0.0, 0.0, 200.0, 100.0, "Board");
    // The card's fourth layer: a group masked by a circle, holding a dot.
    let badge = |d: &mut Doc, [frame, ..]: [NodeId; 3]| {
        let g = d.add(
            Some(frame),
            NodeKind::Group,
            Affine::translate((120.0, 14.0)),
            "Badge",
        );
        let mask = d.add(
            Some(g),
            NodeKind::Ellipse {
                size: Size::new(32.0, 32.0),
                arc: ondin_core::EllipseArc::default(),
            },
            Affine::IDENTITY,
            "Mask",
        );
        d.fill(mask, solid(0, 0, 0, 255));
        d.apply(vec![Operation::SetMask {
            id: mask,
            mask: true,
        }]);
        let dot = d.rect(g, 0.0, 0.0, 32.0, 32.0, 0.0);
        d.fill(dot, solid(40, 120, 220, 255));
    };
    let dot_of = |d: &Doc, frame: NodeId| {
        let g = d.doc.get(frame).unwrap().children()[2];
        d.doc.get(g).unwrap().children()[1]
    };
    // The main past the board's clipping edge: only the instance is compared.
    let main = d.card(board, (500.0, 10.0), None, badge);
    let dot = dot_of(&d, main[0]);
    d.properties(main[0], &[("Show dot", PropKind::Boolean, dot)]);
    let set = d.card(board, (10.0, 10.0), Some(main), badge);
    d.set_properties(set[0], vec![PropValue::Boolean(false)]);
    assert!(!d.doc.get(dot_of(&d, set[0])).unwrap().visible());
    let (doc, res) = d.done();
    let export = html_of_components(&doc, &res, &[board]);
    assert_eq!(
        export.inlined,
        vec!["instance “Card”: its main draws a property's layer as a picture"]
    );
    holds_components("picture-prop", &doc, &res);
}

/// A use under another mode than its main's, whose main draws a picture filled
/// from a colour variable and a stroke whose width variable is 0 in the main's
/// mode: the canvas draws both in the use's mode, so the instance is its layers.
///
/// flip: `Walk::why_inline` ignoring the use's mode → nothing is inlined; past
/// that assertion, the star is red and the stroke missing, at 127.4.
#[test]
fn a_use_in_another_mode_draws_that_modes_values() {
    use ondin_core::BrushSource;
    use ondin_core::variable::{Cell, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let (light, dark) = (d.ids.mint_mode(), d.ids.mint_mode());
    for (i, (mode, name)) in [(light, "Light"), (dark, "Dark")].into_iter().enumerate() {
        d.apply(vec![Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id: mode,
                name: name.into(),
            },
            index: i,
            cells: vec![],
        }]);
    }
    let (ink, line) = (d.ids.mint_var(), d.ids.mint_var());
    for (id, name, kind, cells) in [
        (
            ink,
            "ink",
            VarKind::Color,
            [
                (light, Cell::Color(Color::from_rgba8(220, 40, 40, 255))),
                (dark, Cell::Color(Color::from_rgba8(40, 80, 220, 255))),
            ],
        ),
        (
            line,
            "line",
            VarKind::Number,
            [(light, Cell::Number(0.0)), (dark, Cell::Number(4.0))],
        ),
    ] {
        d.apply(vec![Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind,
                cells: cells.into_iter().collect(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]);
    }
    let board = d.frame(None, 0.0, 0.0, 200.0, 100.0, "Board");
    let painted = |d: &mut Doc, [frame, _, _]: [NodeId; 3]| {
        d.stroke(frame, 0.0, StrokeAlign::Inside, StrokeSides::All);
        let mut strokes = d.doc.get(frame).unwrap().paint().strokes.clone();
        strokes[0].value.width_var = Some(line);
        d.apply(vec![Operation::SetStrokes { id: frame, strokes }]);
        // A picture filled from `ink`.
        let star = d.add(
            Some(frame),
            NodeKind::Star {
                size: Size::new(24.0, 24.0),
                points: 5,
                inner_ratio: 0.5,
                corner_radius: 0.0,
                corner_smoothing: 0.0,
            },
            Affine::translate((120.0, 18.0)),
            "Star",
        );
        d.apply(vec![Operation::SetFills {
            id: star,
            fills: keyed_by_position([Fill {
                brush: BrushSource::Bound {
                    var: ink,
                    alpha: 1.0,
                },
                visible: true,
            }]),
        }]);
    };
    // The main past the board's clipping edge: only the instance is compared.
    let main = d.card(board, (500.0, 10.0), None, painted);
    let used = d.card(board, (10.0, 10.0), Some(main), painted);
    d.apply(vec![Operation::SetModes {
        id: used[0],
        modes: vec![dark],
    }]);
    let (doc, res) = d.done();
    assert_eq!(html_of_components(&doc, &res, &[board]).inlined.len(), 1);
    holds_components("use-mode", &doc, &res);
}

/// A smoothed rectangle whose corners are bound to a variable that reads 0 in this
/// mode still gets `border-radius` for the others, so it is noted all the same.
///
/// flip: the note keyed on this mode's radii → fails at the note.
#[test]
fn a_bound_corner_reading_zero_still_notes_its_smoothing() {
    use ondin_core::variable::{Cell, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let (light, dark, radius) = (d.ids.mint_mode(), d.ids.mint_mode(), d.ids.mint_var());
    let mode = |id, name: &str, index| Operation::AddMode {
        collection: ondin_core::variable::CollectionId::FIRST,
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    d.apply(vec![
        mode(light, "Light", 0),
        mode(dark, "Dark", 1),
        Operation::AddVariable {
            var: Variable {
                id: radius,
                name: "radius".into(),
                kind: VarKind::Number,
                cells: [(light, Cell::Number(0.0)), (dark, Cell::Number(16.0))]
                    .into_iter()
                    .collect(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ]);
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let card = d.add(
        Some(f),
        NodeKind::Rect {
            size: Size::new(60.0, 40.0),
            corner_radii: RoundedRectRadii::from_single_radius(0.0),
            corner_smoothing: 0.6,
        },
        Affine::translate((10.0, 10.0)),
        "Card",
    );
    let ops = ondin_core::bind_corners_ops(&d.doc, card, [Some(radius); 4]);
    d.apply(ops);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(
        css.contains("border-radius"),
        "fixture: rounded in CSS: {css}"
    );
    assert!(
        export
            .approximated
            .iter()
            .any(|n| n.contains("no corner smoothing")),
        "{:?}",
        export.approximated
    );
}
