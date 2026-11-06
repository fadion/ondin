//! Two shapes that share an edge must not show a seam between them.
//!
//! Reported as "a line shows between them ... when zoomed out it shows the most":
//! an 800×100 rect above an 800×500 rect on a white 800×600 frame, both the same
//! grey, drew a **lighter** line along `y = 100`. Not a geometry bug — the rects
//! abut exactly. It is vello's conflation artifact (its open issue #49): coverage
//! is computed per path and alpha-blended independently, so where the shared edge
//! falls at fraction `f` across a device pixel, `f(1-f)` of the white behind both
//! leaks between them. That peaks at a quarter of the fill→backdrop contrast —
//! `102` grey on white measured **140** at `f = 0.5`.
//!
//! No antialiasing setting touches it: `Area`, `Msaa8` and `Msaa16` each measured
//! 140 on device, because vello's MSAA sharpens coverage and then collapses it to
//! a scalar before blending, exactly as `Area` does. `scene::snapped_box_world`
//! rounds axis-aligned boxes onto the pixel grid instead, so the two shapes round
//! the shared edge to the same integer and meet on a boundary with no coverage to
//! conflate.
//!
//! The phases below are the ones that were *wrong* before the fix. `f = 0` was
//! always clean — it is here so a "fix" that merely moved the artifact somewhere
//! else cannot pass, and so the fixture is pinned to a grid the seam can be
//! absent from for the right reason.

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Fill, IdSource, NodeKind, Operation, Resolved, Transaction, keyed_by_position,
};
use ondin_render::{ImageStore, VelloCpuRenderer, Viewport};

const GREY: Color = Color::from_rgb8(102, 102, 102);
/// The grey's own channel value, and what every pixel down the seam must read.
const FILL: u8 = 102;
const SPLIT: f64 = 100.0;

/// 800×600 white frame, an 800×100 rect on top of an 800×500 rect, sharing `y = 100`.
fn abutting_pair() -> (Document, Resolved) {
    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut d = Document::new(root);
    let ab = ids.mint();
    let top = ids.mint();
    let bot = ids.mint();
    d.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(800.0, 600.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: ab,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::WHITE).into(),
                visible: true,
            }]),
        },
        Operation::CreateNode {
            id: top,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(800.0, SPLIT),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((0.0, 0.0))),
            name: None,
        },
        Operation::CreateNode {
            id: bot,
            parent: ab,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(800.0, 500.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((0.0, SPLIT))),
            name: None,
        },
    ]))
    .unwrap();
    for id in [top, bot] {
        d.apply(&Transaction(vec![Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(GREY).into(),
                visible: true,
            }]),
        }]))
        .unwrap();
    }
    let res = Resolved::rebuild(&d);
    (d, res)
}

/// Render a 200×200 window centred on the shared edge, with the edge sitting
/// `phase` device pixels off the grid, and return the column through it.
fn column_through_the_seam(zoom: f64, phase: f64) -> Vec<u8> {
    let (d, res) = abutting_pair();
    let (w, h) = (200u32, 200u32);
    let vh = h as f64 / zoom;
    let y0 = SPLIT - vh / 2.0 + phase / zoom;
    let vp = Viewport {
        view: Rect::new(0.0, y0, w as f64 / zoom, y0 + vh),
        pixel_size: (w, h),
    };
    let (rgba, _, _) =
        VelloCpuRenderer::new().render_to_rgba(&d, &res, &vp, &ImageStore::new(), d.first_page());
    // Well inside the 800-wide rects horizontally, so only the shared edge is in play.
    let x = 100usize;
    (0..h as usize)
        .map(|y| rgba[(y * w as usize + x) * 4])
        .collect()
}

#[test]
fn abutting_shapes_leave_no_seam_at_any_sub_pixel_phase() {
    // A quarter, a half and a third of a pixel off the grid, at zooms either side
    // of 1:1. Half is the worst case — it is the phase that measured 140.
    for &(zoom, phase) in &[
        (1.0, 0.5),
        (1.0, 0.25),
        (2.0, 0.25),
        (4.0, 0.13),
        (0.37, 0.5),
        (0.5, 0.33),
        // Already clean before the fix; snapping must not break it.
        (1.0, 0.0),
    ] {
        let col = column_through_the_seam(zoom, phase);
        // A band about the shared edge, not the whole column: the view is centred
        // on the edge, so at the low zooms the window is tall enough to contain the
        // *frame's* own top edge as well, and that one is a real boundary against
        // the page. 20 rows either side is inside both rects at every zoom here.
        //
        // Every value in the band being the fill is what makes this non-vacuous —
        // a window that missed the artwork would read 0 or 255, not 102.
        let mid = col.len() / 2;
        let band = &col[mid - 20..mid + 20];
        if let Some((i, &v)) = band.iter().enumerate().find(|&(_, &v)| v != FILL) {
            let y = mid - 20 + i;
            panic!(
                "zoom {zoom} phase {phase}: seam at y={y} reads {v}, expected {FILL} \
                 (the reported symptom measured 140 against a {FILL} fill)"
            );
        }
    }
}

/// The rounding must not delete a shape that is thinner than a device pixel.
///
/// At 25% a 2-unit bar is half a pixel, and rounding both its edges to the nearest
/// integer sends them to the same place — so the naive spelling of this fix erases
/// hairlines at low zoom instead of snapping them.
#[test]
fn a_sub_pixel_shape_survives_snapping() {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut d = Document::new(root);
    let bar = ids.mint();
    d.apply(&Transaction(vec![Operation::CreateNode {
        id: bar,
        parent: root,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(400.0, 2.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((0.0, 50.0))),
        name: None,
    }]))
    .unwrap();
    d.apply(&Transaction(vec![Operation::SetFills {
        id: bar,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::BLACK).into(),
            visible: true,
        }]),
    }]))
    .unwrap();
    let res = Resolved::rebuild(&d);

    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 400.0, 400.0),
        pixel_size: (100, 100), // 0.25x: the 2-unit bar is half a device pixel
    };
    let (rgba, _, _) =
        VelloCpuRenderer::new().render_to_rgba(&d, &res, &vp, &ImageStore::new(), d.first_page());
    let ink = (0..100usize)
        .filter(|y| rgba[(y * 100 + 50) * 4 + 3] > 0)
        .count();
    assert!(ink > 0, "the bar rounded away to nothing at 25% zoom");
}

/// A white 800×300 frame holding a black 1-unit horizontal line, centred on
/// `y = SPLIT - 0.5`, as a path, and, when `covered`, a white rect over it from
/// the frame's top down to `y = SPLIT`, which ends exactly where the line does.
fn line_under_a_box(covered: bool) -> (Document, Resolved) {
    line_by_a_box(SPLIT - 0.5, covered)
}

/// `line_under_a_box` with the line centred on `y = centre` instead.
fn line_by_a_box(centre: f64, covered: bool) -> (Document, Resolved) {
    use ondin_core::kurbo::BezPath;
    let mut ids = IdSource::new(21);
    let root = ids.mint();
    let mut d = Document::new(root);
    let (ab, line, cover) = (ids.mint(), ids.mint(), ids.mint());
    let mut path = BezPath::new();
    path.move_to((0.0, centre));
    path.line_to((800.0, centre));
    let white = || {
        keyed_by_position([Fill {
            brush: Brush::Solid(Color::WHITE).into(),
            visible: true,
        }])
    };
    let mut ops = vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(800.0, 300.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: ab,
            fills: white(),
        },
        Operation::CreateNode {
            id: line,
            parent: ab,
            index: 0,
            kind: NodeKind::Path {
                path,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: ondin_core::symmetry::PathMirror::NONE,
            },
            transform: None,
            name: None,
        },
        Operation::SetStrokes {
            id: line,
            strokes: keyed_by_position([ondin_core::Stroke::default()]),
        },
    ];
    if covered {
        ops.extend([
            Operation::CreateNode {
                id: cover,
                parent: ab,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(800.0, SPLIT),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: cover,
                fills: white(),
            },
        ]);
    }
    d.apply(&Transaction(ops)).unwrap();
    let res = Resolved::rebuild(&d);
    (d, res)
}

/// The column at `x = 100` through `d`, with `y = SPLIT` sitting `phase` device
/// pixels into a row: the red channel, row by row.
fn column_at(d: &Document, res: &Resolved, zoom: f64, phase: f64) -> Vec<u8> {
    let (w, h) = (200u32, 200u32);
    let vh = h as f64 / zoom;
    let y0 = SPLIT - (100.0 + phase) / zoom;
    let vp = Viewport {
        view: Rect::new(0.0, y0, w as f64 / zoom, y0 + vh),
        pixel_size: (w, h),
    };
    let (rgba, _, _) =
        VelloCpuRenderer::new().render_to_rgba(d, res, &vp, &ImageStore::new(), d.first_page());
    (0..h as usize)
        .map(|y| rgba[(y * w as usize + 100) * 4])
        .collect()
}

/// A path's stroke ending where a box over it ends stays covered: the box's
/// snapped edge and the stroke's edge round alike. Unsnapped, a quarter-pixel
/// phase rounded the box up and left a sliver of the line under it reading 181.
///
/// flip: `scene::strokes_snap` answering false → fails at zoom 1, phase 0.25;
/// `snapped_stroke` rounding the near edge and the width instead of both edges →
/// fails at zoom 1.5, phase 0.4.
#[test]
fn a_stroke_ending_where_a_box_over_it_ends_stays_covered() {
    let (d, res) = line_under_a_box(true);
    for &(zoom, phase) in &[
        (1.0, 0.25),
        (1.0, 0.1),
        (1.0, 0.4),
        (1.33, 0.3),
        // 1.5 px wide, its edges rounding to a 1 px row: a width rounded on its
        // own (to 2) would reach past the box.
        (1.5, 0.4),
        (2.0, 0.25),
        (3.0, 0.4),
    ] {
        let col = column_at(&d, &res, zoom, phase);
        if let Some((y, &v)) = col.iter().enumerate().find(|&(_, &v)| v != 255) {
            panic!("zoom {zoom} phase {phase}: row {y} reads {v} under a white box");
        }
    }
    // The fixture draws a line where the box is absent, or the check is vacuous.
    let (bare, bare_res) = line_under_a_box(false);
    assert!(
        column_at(&bare, &bare_res, 1.0, 0.25)
            .iter()
            .any(|&v| v < 128),
        "the line is drawn when nothing covers it"
    );
}

/// A one-unit line above a pixel wide comes out on whole rows: every row the
/// column crosses is the line's full ink or none of it.
///
/// flip: `scene::strokes_snap` answering false → fails at zoom 1, phase 0.5.
#[test]
fn a_snapped_stroke_lands_on_whole_rows() {
    let (d, res) = line_under_a_box(false);
    for &(zoom, phase) in &[(1.0, 0.5), (1.33, 0.3), (2.5, 0.7)] {
        let col = column_at(&d, &res, zoom, phase);
        let partial: Vec<(usize, u8)> = col
            .iter()
            .copied()
            .enumerate()
            .filter(|&(_, v)| v != 0 && v != 255)
            .collect();
        assert!(
            partial.is_empty(),
            "zoom {zoom} phase {phase}: partial rows {partial:?}"
        );
        assert!(col.contains(&0), "zoom {zoom}: the line is drawn");
    }
}

/// A line under a pixel wide keeps its ink when snapped: rounding its width up
/// to a whole row would turn a zoomed-out grid of hairlines bold.
///
/// flip: `snapped_stroke`'s sub-pixel arm taking the whole row it rounds to →
/// fails at zoom 0.3 with a whole row of ink. Both 0.5 phases round the line's
/// edges together and keep it where it lies, so they do not see that arm.
#[test]
fn a_sub_pixel_stroke_keeps_its_ink() {
    let (d, res) = line_under_a_box(false);
    for &(zoom, phase) in &[(0.5, 0.3), (0.5, 0.0), (0.3, 0.6)] {
        // Rows about the line only: at low zoom the window reaches past the
        // frame, where nothing is drawn and the red channel reads 0.
        let col = column_at(&d, &res, zoom, phase);
        let ink: f64 = col[90..110]
            .iter()
            .map(|&v| f64::from(255 - v) / 255.0)
            .sum();
        assert!(
            (ink - zoom).abs() < 0.03,
            "zoom {zoom} phase {phase}: ink {ink}, the line's own is {zoom}"
        );
    }
}

/// A line under a pixel wide, ending where a box over it ends, stays covered when
/// both its edges round to the box's edge: it is laid on the box's side of it.
/// Every phase here rounds the line's two edges together.
///
/// flip: `snapped_stroke` laying such a line where it lies → fails at zoom 0.5,
/// phase 0.3.
#[test]
fn a_sub_pixel_stroke_ending_where_a_box_over_it_ends_stays_covered() {
    let (d, res) = line_under_a_box(true);
    for &(zoom, phase) in &[(0.5, 0.3), (0.37, 0.2), (0.6, 0.4), (0.25, 0.1)] {
        let col = column_at(&d, &res, zoom, phase);
        if let Some((y, &v)) = col[90..110].iter().enumerate().find(|&(_, &v)| v != 255) {
            panic!(
                "zoom {zoom} phase {phase}: row {} reads {v} under a white box",
                y + 90
            );
        }
    }
    let (bare, bare_res) = line_under_a_box(false);
    assert!(
        column_at(&bare, &bare_res, 0.5, 0.3)[90..110]
            .iter()
            .any(|&v| v < 255),
        "the line is drawn when nothing covers it"
    );
}

/// Each fill's outline on the device, as the walk lays it.
#[derive(Default)]
struct Fills(Vec<ondin_core::kurbo::BezPath>);

impl Fills {
    /// Each fill's box.
    fn boxes(&self) -> Vec<Rect> {
        use ondin_core::kurbo::Shape;
        self.0.iter().map(|p| p.bounding_box()).collect()
    }

    /// The box of each subpath of every fill, in the order laid.
    fn pieces(&self) -> Vec<Rect> {
        use ondin_core::kurbo::Shape;
        self.0
            .iter()
            .flat_map(ondin_core::geometry::subpaths)
            .map(|p| p.bounding_box())
            .collect()
    }
}

/// `d`'s first page through a 200×100 view at 1:1, culled to `cull` when given.
fn fills_of(d: &Document, res: &Resolved, cull: Option<Rect>) -> Fills {
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 200.0, 100.0),
        pixel_size: (200, 100),
    };
    let ov = ondin_render::RenderOverrides::default();
    let page = d.first_page();
    let mut fills = Fills::default();
    match cull {
        None => ondin_render::scene::build(d, res, &vp, &ov, page, &mut fills),
        Some(c) => ondin_render::scene::build_within(d, res, &vp, &ov, page, &[c], &mut fills),
    }
    fills
}

impl ondin_render::scene::ScenePainter for Fills {
    fn has_image(&self, _: &ondin_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        t: Affine,
        path: &ondin_core::kurbo::BezPath,
        _: &Brush,
        _: Option<ondin_core::Framing>,
        _: ondin_render::scene::ClipRule,
    ) {
        self.0.push(t * path.clone());
    }
    fn stroke_path(
        &mut self,
        _: Affine,
        _: &ondin_core::kurbo::BezPath,
        _: &ondin_render::scene::StrokePaint<'_>,
    ) {
    }
    fn draw_text(&mut self, _: Affine, _: &ondin_render::scene::TextRun<'_>) {}
    fn push_layer(
        &mut self,
        _: Affine,
        _: Option<&ondin_core::kurbo::BezPath>,
        _: ondin_render::scene::ClipRule,
        _: f32,
    ) {
    }
    fn push_mask_layer(&mut self) {}
    fn pop_layer(&mut self) {}
}

/// A hairline whose edges both round onto a box's edge is laid on the same row
/// by a walk culled to a strip clear of the box, and by a view panned off the
/// box, as by the full walk: the side vote reads the boxes along the line's
/// whole span.
///
/// flip: `walk` building `Edges` from its cull instead of the view → fails at
/// "the strip"; from the view instead of its strips → fails at "panned".
#[test]
fn a_hairline_lies_on_one_row_whatever_the_walk_is_culled_to() {
    let mut ids = IdSource::new(31);
    let root = ids.mint();
    let mut d = Document::new(root);
    let (bar, line) = (ids.mint(), ids.mint());
    d.apply(&Transaction(vec![
        Operation::CreateNode {
            id: bar,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(50.0, 10.3),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: bar,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(GREY).into(),
                visible: true,
            }]),
        },
        Operation::CreateNode {
            id: line,
            parent: root,
            index: 1,
            kind: NodeKind::Line {
                end: ondin_core::kurbo::Point::new(100.0, 0.0),
            },
            transform: Some(Affine::translate((0.0, 10.0))),
            name: None,
        },
        Operation::SetStrokes {
            id: line,
            strokes: keyed_by_position([ondin_core::Stroke {
                width: 0.6,
                ..ondin_core::Stroke::default()
            }]),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&d);
    let laid = |cull: Option<Rect>| {
        fills_of(&d, &res, cull)
            .boxes()
            .into_iter()
            .find(|r| r.width() > 90.0)
            .expect("the line")
    };
    let full = laid(None);
    assert!(
        (full.y1 - 10.0).abs() < 1e-6,
        "fixture: the full walk lays the line above the box's edge, {full:?}"
    );
    assert_eq!(
        laid(Some(Rect::new(80.0, 0.0, 90.0, 20.0))),
        full,
        "the strip"
    );
    // Panned so the box lies off the view while the line runs on into it.
    let vp = Viewport {
        view: Rect::new(60.0, 0.0, 260.0, 100.0),
        pixel_size: (200, 100),
    };
    let mut panned = Fills::default();
    let ov = ondin_render::RenderOverrides::default();
    ondin_render::scene::build(&d, &res, &vp, &ov, d.first_page(), &mut panned);
    let moved = panned
        .boxes()
        .into_iter()
        .find(|r| r.width() > 30.0)
        .expect("the line");
    assert_eq!(
        (moved.y0, moved.y1),
        (full.y0, full.y1),
        "panned: the box off the view still votes"
    );
}

/// A snapped stroke is one whole-pixel rectangle per segment: across, each edge
/// rounded; along, reaching over a join by half the width and past an end by its
/// cap. Covers the vertical, square-cap, corner and closing arms.
///
/// flip: `snapped_stroke` leaving out a closed subpath's closing segment → fails
/// at "closing"; ignoring the square cap's reach → fails at "square cap".
#[test]
fn a_snapped_stroke_is_one_whole_pixel_rect_per_segment() {
    use ondin_core::kurbo::BezPath;
    let laid = |path: BezPath, cap: ondin_core::kurbo::Cap| {
        let mut ids = IdSource::new(41);
        let root = ids.mint();
        let mut d = Document::new(root);
        let line = ids.mint();
        d.apply(&Transaction(vec![
            Operation::CreateNode {
                id: line,
                parent: root,
                index: 0,
                kind: NodeKind::Path {
                    path,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: ondin_core::symmetry::PathMirror::NONE,
                },
                transform: None,
                name: None,
            },
            Operation::SetStrokes {
                id: line,
                strokes: keyed_by_position([ondin_core::Stroke {
                    width: 2.0,
                    cap,
                    ..ondin_core::Stroke::default()
                }]),
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&d);
        fills_of(&d, &res, None).pieces()
    };
    let poly = |pts: &[(f64, f64)], closed: bool| {
        let mut p = BezPath::new();
        p.move_to(pts[0]);
        for q in &pts[1..] {
            p.line_to(*q);
        }
        if closed {
            p.close_path();
        }
        p
    };
    use ondin_core::kurbo::Cap;
    assert_eq!(
        laid(poly(&[(10.3, 20.0), (10.3, 60.0)], false), Cap::Butt),
        [Rect::new(9.0, 20.0, 11.0, 60.0)],
        "vertical"
    );
    assert_eq!(
        laid(poly(&[(10.2, 20.2), (50.2, 20.2)], false), Cap::Square),
        [Rect::new(9.0, 19.0, 51.0, 21.0)],
        "square cap"
    );
    assert_eq!(
        laid(
            poly(&[(10.3, 50.3), (40.3, 50.3), (40.3, 80.3)], false),
            Cap::Butt
        ),
        [
            Rect::new(10.0, 49.0, 41.0, 51.0),
            Rect::new(39.0, 49.0, 41.0, 80.0)
        ],
        "corner"
    );
    assert_eq!(
        laid(
            poly(
                &[(10.3, 10.3), (40.3, 10.3), (40.3, 30.3), (10.3, 30.3)],
                true
            ),
            Cap::Butt
        ),
        [
            Rect::new(9.0, 9.0, 41.0, 11.0),
            Rect::new(39.0, 9.0, 41.0, 31.0),
            Rect::new(9.0, 29.0, 41.0, 31.0),
            Rect::new(9.0, 9.0, 11.0, 31.0)
        ],
        "closing"
    );
}

/// The app's camera: the view's corner moved so the world's origin lands on a
/// whole device pixel (`Camera::viewport`).
fn camera(center: (f64, f64), zoom: f64, px: (u32, u32)) -> Viewport {
    let (w, h) = (f64::from(px.0) / zoom, f64::from(px.1) / zoom);
    let corner = |c: f64, span: f64| ((c - span / 2.0) * zoom).round() / zoom;
    let (x0, y0) = (corner(center.0, w), corner(center.1, h));
    Viewport {
        view: Rect::new(x0, y0, x0 + w, y0 + h),
        pixel_size: px,
    }
}

/// A whole-pixel pan moves every snapped edge by exactly the pan, so the frame
/// kept and moved is the frame drawn fresh: at 1/1.44 every edge here falls on a
/// rounding tie, which the view's origin must not decide.
///
/// flip: `snapped_box_world` rounding a tie with `f64::round` → fails at the
/// edge comparison.
#[test]
fn a_whole_pixel_pan_moves_every_snapped_edge_by_the_pan() {
    let mut ids = IdSource::new(51);
    let root = ids.mint();
    let mut d = Document::new(root);
    let mut ops = Vec::new();
    for k in -12i32..12 {
        let id = ids.mint();
        ops.push(Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(36.0, 36.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            // Every edge on a multiple of 18, which is 12.5 device pixels.
            transform: Some(Affine::translate((
                f64::from(k) * 54.0,
                f64::from(k.rem_euclid(4)) * 54.0 - 90.0,
            ))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::BLACK).into(),
                visible: true,
            }]),
        });
    }
    d.apply(&Transaction(ops)).unwrap();
    let res = Resolved::rebuild(&d);
    let (zoom, px) = (1.0 / 1.44, (400u32, 300u32));
    let render = |vp: &Viewport| {
        VelloCpuRenderer::new()
            .render_to_rgba(&d, &res, vp, &ImageStore::new(), d.first_page())
            .0
    };
    let mut checked = 0;
    for (start, pan) in [
        ((3.3, 7.9), (-12i32, -9i32)),
        ((-41.7, 12.2), (7, 13)),
        ((88.8, -5.5), (-31, 4)),
    ] {
        let a = render(&camera(start, zoom, px));
        let moved = (
            start.0 - f64::from(pan.0) / zoom,
            start.1 - f64::from(pan.1) / zoom,
        );
        let b = render(&camera(moved, zoom, px));
        for y in 0..px.1 as i32 {
            for x in 0..px.0 as i32 {
                let (bx, by) = (x + pan.0, y + pan.1);
                if bx < 0 || by < 0 || bx >= px.0 as i32 || by >= px.1 as i32 {
                    continue;
                }
                let i = ((y * px.0 as i32 + x) * 4) as usize;
                let j = ((by * px.0 as i32 + bx) * 4) as usize;
                assert_eq!(
                    a[i..i + 4],
                    b[j..j + 4],
                    "from {start:?} by {pan:?}: ({x}, {y}) kept is not ({bx}, {by}) fresh"
                );
                checked += usize::from(a[i + 3] > 0);
            }
        }
    }
    assert!(checked > 0, "fixture: the rects are in view");
}

/// A line under a pixel wide starting where a box above it ends keeps its ink
/// when both its edges round to the box's edge: it is laid on its own side.
///
/// flip: `snapped_stroke` laying such a line where it lies → fails at zoom 0.3,
/// phase 0.6, with the ink under the box.
#[test]
fn a_sub_pixel_stroke_starting_where_a_box_ends_keeps_its_ink() {
    let (d, res) = line_by_a_box(SPLIT + 0.5, true);
    for &(zoom, phase) in &[(0.3, 0.6), (0.5, 0.55)] {
        let col = column_at(&d, &res, zoom, phase);
        let ink: f64 = col[90..110]
            .iter()
            .map(|&v| f64::from(255 - v) / 255.0)
            .sum();
        assert!(
            (ink - zoom).abs() < 0.03,
            "zoom {zoom} phase {phase}: ink {ink}, the line's own is {zoom}"
        );
    }
}
