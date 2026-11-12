//! A layer's visual centre: the centre of mass of the ink it paints, which
//! *Align by visual centre* puts where a box centre would go, so a play triangle
//! sits centred in its circle.
//!
//! Ink is weighed by area in world space: fills with their fill rule, strokes by
//! the ink they lay ([`crate::build::stroke_ink`]), overlaps counted once and a
//! masked run cut to its mask. Opacity does not weigh, and hidden layers and
//! slices do not count.

use crate::build::{Flatten, polyline};
use crate::{Document, NodeId, NodeKind, Resolved};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::float::single::SingleFloatOverlay;
use kurbo::{BezPath, Point, Rect};

/// Where a layer's weight sits, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VisualCentre {
    /// The centroid of its shown ink.
    Ink(Point),
    /// Its box's centre: a frame, which is a box, or a layer with no ink of area.
    Box(Point),
    /// Its box's centre, because it is text: aligned by its box everywhere else in
    /// the app, so a caller says it fell back.
    Text(Point),
}

impl VisualCentre {
    /// The point, whichever way it was measured.
    pub fn at(self) -> Point {
        match self {
            Self::Ink(p) | Self::Box(p) | Self::Text(p) => p,
        }
    }
}

/// How far the polygons the ink is weighed over may stray from its curves, in
/// world units: far under anything an alignment can show.
const FLATTEN: f64 = 0.01;

/// The visual centre of `id`, or `None` when it has no world box.
pub fn visual_centre(doc: &Document, res: &Resolved, id: NodeId) -> Option<VisualCentre> {
    let centre = res.world_bounds(id)?.center();
    let node = doc.get(id)?;
    match res.used_kind(doc, id).unwrap_or(node.kind()) {
        NodeKind::Artboard { .. } => return Some(VisualCentre::Box(centre)),
        NodeKind::Text { .. } => return Some(VisualCentre::Text(centre)),
        _ => {}
    }
    let mut ink = Vec::new();
    collect(doc, res, id, &mut ink);
    let at = std::panic::catch_unwind(|| centroid(&ink.simplify_shape(FillRule::NonZero)))
        .ok()
        .flatten();
    Some(at.map_or(VisualCentre::Box(centre), VisualCentre::Ink))
}

/// Put the visual centre of each of `ids` on `at` along `axis` — *Align centres*
/// or *Align middles* by weight; the caller picks `at` as it picks a box target.
/// Also answers how many of the members moved were text, measured by their box.
pub fn align(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    at: f64,
    axis: crate::build::Axis,
) -> (crate::op::Transaction, usize) {
    let (mut ops, mut text) = (Vec::new(), 0);
    for id in crate::build::outermost(doc, ids) {
        let Some(centre) = visual_centre(doc, res, id) else {
            continue;
        };
        text += usize::from(matches!(centre, VisualCentre::Text(_)));
        let p = centre.at();
        let delta = match axis {
            crate::build::Axis::X => kurbo::Vec2::new(at - p.x, 0.0),
            crate::build::Axis::Y => kurbo::Vec2::new(0.0, at - p.y),
        };
        crate::build::push_offset(doc, res, id, delta, &mut ops);
    }
    (crate::op::Transaction(ops), text)
}

/// The polygons `id` paints, in world space, each set already read with its own
/// fill rule so that the caller can union them non-zero.
fn collect(doc: &Document, res: &Resolved, id: NodeId, out: &mut Vec<Vec<[f64; 2]>>) {
    let Some(node) = doc.get(id) else { return };
    if !doc.shown(id) {
        return;
    }
    let Some(world) = res.world_transform(id) else {
        return;
    };
    let kind = res.used_kind(doc, id).unwrap_or(node.kind());
    match kind {
        NodeKind::Root | NodeKind::Slice { .. } => return,
        // Weighed by its box inside a group too: no outline to weigh.
        NodeKind::Text { .. } => {
            if let Some(b) = res.world_bounds(id) {
                out.push(rect_ring(b));
            }
            return;
        }
        _ => {}
    }
    if let Some(shape) = res.local_path(doc, id) {
        let paint = doc.resolved_paint(id, node.paint());
        if node.paint().fills.iter().any(|f| f.visible) {
            let rule = match node.fill_rule() {
                crate::node::FillRule::EvenOdd => FillRule::EvenOdd,
                crate::node::FillRule::NonZero => FillRule::NonZero,
            };
            push_read(&rings(&(world * shape.clone())), rule, out);
        }
        for (s, d) in node.paint().strokes.iter().zip(&paint.strokes) {
            if let Some(laid) =
                crate::build::stroke_ink(kind, &shape, &d.value).filter(|_| s.visible)
            {
                push_read(&rings(&(world * laid)), FillRule::NonZero, out);
            }
        }
    }
    // A boolean's operands are consumed into its own outline, drawn above.
    if matches!(kind, NodeKind::Boolean { .. }) {
        return;
    }
    let mut mask: Option<Vec<Vec<[f64; 2]>>> = None;
    for c in node.children() {
        if !crate::resolve::lends_bounds(doc, *c) {
            continue;
        }
        let Some(child) = doc.get(*c) else { continue };
        if child.mask() {
            // A mask paints nothing and clips the run above it, until the next.
            mask = res
                .mask_path(doc, *c)
                .map(|p| rings(&(world * p)))
                .and_then(|r| {
                    std::panic::catch_unwind(|| flat(r.simplify_shape(FillRule::NonZero))).ok()
                });
            continue;
        }
        let mut own = Vec::new();
        collect(doc, res, *c, &mut own);
        match &mask {
            Some(m) => {
                let cut = std::panic::catch_unwind(|| {
                    flat(own.overlay(m, OverlayRule::Intersect, FillRule::NonZero))
                });
                out.extend(cut.unwrap_or_default());
            }
            None => out.extend(own),
        }
    }
}

/// `path`'s subpaths as closed rings, a fill closing an open one.
fn rings(path: &BezPath) -> Vec<Vec<[f64; 2]>> {
    crate::geometry::subpaths(path)
        .iter()
        .filter_map(|s| polyline(s, Flatten::chords(FLATTEN), 3))
        .collect()
}

/// `rings` read under `rule` into rings that union non-zero, appended to `out`.
fn push_read(rings: &[Vec<[f64; 2]>], rule: FillRule, out: &mut Vec<Vec<[f64; 2]>>) {
    if rings.is_empty() {
        return;
    }
    // i_overlay asserts on bounds it cannot scale, so a panic weighs nothing.
    if let Ok(read) = std::panic::catch_unwind(|| rings.simplify_shape(rule)) {
        out.extend(flat(read));
    }
}

/// i_overlay's shapes, each an outer ring and its holes, as one list of rings.
fn flat(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> Vec<Vec<[f64; 2]>> {
    shapes.into_iter().flatten().collect()
}

/// A box as a ring.
fn rect_ring(b: Rect) -> Vec<[f64; 2]> {
    vec![[b.x0, b.y0], [b.x1, b.y0], [b.x1, b.y1], [b.x0, b.y1]]
}

/// The centre of mass of i_overlay's shapes, whose holes wind against their
/// outer rings, so signed areas subtract them. `None` when they have no area.
fn centroid(shapes: &[Vec<Vec<[f64; 2]>>]) -> Option<Point> {
    let (mut area, mut mx, mut my) = (0.0, 0.0, 0.0);
    for ring in shapes.iter().flatten() {
        // About the ring's first point, so a layer far from the origin keeps its digits.
        let Some(&[ox, oy]) = ring.first() else {
            continue;
        };
        for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
            let (ax, ay, bx, by) = (a[0] - ox, a[1] - oy, b[0] - ox, b[1] - oy);
            let cross = ax * by - bx * ay;
            area += cross / 2.0;
            mx += (ax + bx + 3.0 * ox) * cross / 6.0;
            my += (ay + by + 3.0 * oy) * cross / 6.0;
        }
    }
    let at = Point::new(mx / area, my / area);
    (area.abs() > 1e-9 && at.is_finite()).then_some(at)
}
