//! *Warp*: layers bent through an envelope and baked into plain paths.
//!
//! An envelope maps the selection's world box onto a new shape: four corners
//! dragged freely (a perspective map, so straight lines stay straight), or one of
//! three presets, *Arc*, *Flag* and *Bulge*, each set by one bend. Every outline
//! under the selection is taken into world space, carried through the map and
//! refitted with cubics that follow it within [`TOLERANCE`]; text is outlined and
//! shapes become paths. Nothing stays live.

use crate::build;
use crate::kurbo::{BezPath, CubicBez, ParamCurve, PathEl, Point, Rect, Vec2};
use crate::node::NodeKind;
use crate::{Document, IdSource, NodeId, Operation, Resolved, Transaction};

/// How far a warped outline may stray from the true warp of the original, in
/// world units.
pub const TOLERANCE: f64 = 0.05;

/// How many times a curve is halved at most while refitting it.
const MAX_DEPTH: u32 = 12;

/// The three envelopes set by one bend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// The box bent round a circle: its middle rises and its sides lean in, as
    /// type set on an arch.
    Arc,
    /// One wave along the box, up at the first quarter and down at the third.
    Flag,
    /// The box swells from its middle; a negative bend pinches it.
    Bulge,
}

/// The shape the box is mapped onto.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Envelope {
    /// Where the box's corners go, top-left first and then clockwise.
    Free([Point; 4]),
    /// A preset at `bend`, `-1..=1`, bent along the box's width, or its height
    /// when `vertical`.
    Preset {
        preset: Preset,
        bend: f64,
        vertical: bool,
    },
}

/// An envelope over the box it maps, both in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    pub bounds: Rect,
    pub envelope: Envelope,
}

impl Warp {
    /// The envelope that changes nothing: the box's own corners.
    pub fn square(bounds: Rect) -> Self {
        Self {
            bounds,
            envelope: Envelope::Free(corners(bounds)),
        }
    }

    /// Where `p`, a world point, goes.
    pub fn map(&self, p: Point) -> Point {
        let b = self.bounds;
        match self.envelope {
            Envelope::Free(q) => {
                let (u, v) = (
                    (p.x - b.x0) / b.width().max(1e-12),
                    (p.y - b.y0) / b.height().max(1e-12),
                );
                Homography::new(q).map(u, v)
            }
            Envelope::Preset {
                preset,
                bend,
                vertical,
            } => {
                let c = b.center();
                let (x, y) = (p.x - c.x, p.y - c.y);
                let (w, h) = (b.width(), b.height());
                let (x, y) = if vertical {
                    let (y, x) = bent(preset, bend, y, x, h, w);
                    (x, y)
                } else {
                    bent(preset, bend, x, y, w, h)
                };
                Point::new(c.x + x, c.y + y)
            }
        }
    }

    /// How far from the origin the box reaches, which sets the rounding error of
    /// every mapped point.
    fn magnitude(&self) -> f64 {
        let b = self.bounds;
        b.x0.abs().max(b.x1.abs()).max(b.y0.abs()).max(b.y1.abs())
    }

    /// [`TOLERANCE`], or the rounding of coordinates this far out where that is
    /// coarser: a fit judged under the noise would halve to `MAX_DEPTH` every time.
    fn tolerance(&self) -> f64 {
        TOLERANCE.max(16.0 * f64::EPSILON * self.magnitude())
    }

    /// The map's derivative at `p`, as the images of a unit step along x and y.
    fn jacobian(&self, p: Point) -> (Vec2, Vec2) {
        // A step the coordinates can still resolve, however far out the box is.
        let e = (1e-5 * self.bounds.width().max(self.bounds.height()).max(1.0))
            .max(1e3 * f64::EPSILON * self.magnitude());
        let dx = (self.map(p + Vec2::new(e, 0.0)) - self.map(p - Vec2::new(e, 0.0))) / (2.0 * e);
        let dy = (self.map(p + Vec2::new(0.0, e)) - self.map(p - Vec2::new(0.0, e))) / (2.0 * e);
        (dx, dy)
    }

    /// Whether straight lines stay straight, so a line segment maps exactly.
    fn keeps_lines(&self) -> bool {
        matches!(self.envelope, Envelope::Free(_))
    }

    /// Whether the envelope turns part of the box over itself: a corner dragged
    /// across another's edge, or an arc bent tighter than the box is tall.
    pub fn folds(&self) -> bool {
        match self.envelope {
            Envelope::Free(q) => !convex(q),
            Envelope::Preset {
                preset: Preset::Arc,
                bend,
                vertical,
            } => bend.abs() > max_bend(self.bounds, Preset::Arc, vertical) + 1e-9,
            Envelope::Preset { .. } => false,
        }
    }

    /// Whether the envelope moves nothing.
    pub fn is_identity(&self) -> bool {
        match self.envelope {
            Envelope::Free(q) => q == corners(self.bounds),
            Envelope::Preset { bend, .. } => bend == 0.0,
        }
    }

    /// `path`, in world space, carried through the map.
    pub fn path(&self, path: &BezPath) -> BezPath {
        let mut out = BezPath::new();
        let (mut start, mut last) = (Point::ZERO, Point::ZERO);
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    out.move_to(self.map(p));
                    (start, last) = (p, p);
                }
                PathEl::LineTo(p) => {
                    self.line(last, p, &mut out);
                    last = p;
                }
                PathEl::QuadTo(a, p) => {
                    let q = crate::kurbo::QuadBez::new(last, a, p).raise();
                    self.cubic(q, &mut out, 0);
                    last = p;
                }
                PathEl::CurveTo(a, b, p) => {
                    self.cubic(CubicBez::new(last, a, b, p), &mut out, 0);
                    last = p;
                }
                PathEl::ClosePath => {
                    // The closing edge bends too, unless lines stay straight.
                    if !self.keeps_lines() && last.distance(start) > 1e-9 {
                        self.line(last, start, &mut out);
                    }
                    out.close_path();
                    last = start;
                }
            }
        }
        out
    }

    fn line(&self, a: Point, b: Point, out: &mut BezPath) {
        if self.keeps_lines() {
            out.line_to(self.map(b));
        } else {
            let c = CubicBez::new(a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b);
            self.cubic(c, out, 0);
        }
    }

    /// One cubic carried through the map: the cubic through the mapped ends whose
    /// handles are the original handles carried by the map's derivative, halved
    /// until it stays within [`Self::tolerance`] of the true image.
    fn cubic(&self, c: CubicBez, out: &mut BezPath, depth: u32) {
        let (p0, p3) = (self.map(c.p0), self.map(c.p3));
        let along = |at: Point, d: Vec2| {
            let (jx, jy) = self.jacobian(at);
            jx * d.x + jy * d.y
        };
        let fit = CubicBez::new(
            p0,
            p0 + along(c.p0, c.p1 - c.p0),
            p3 + along(c.p3, c.p2 - c.p3),
            p3,
        );
        let tolerance = self.tolerance();
        let close = [0.25, 0.5, 0.75]
            .iter()
            .all(|t| fit.eval(*t).distance(self.map(c.eval(*t))) <= tolerance);
        if close || depth >= MAX_DEPTH {
            out.curve_to(fit.p1, fit.p2, fit.p3);
        } else {
            self.cubic(c.subsegment(0.0..0.5), out, depth + 1);
            self.cubic(c.subsegment(0.5..1.0), out, depth + 1);
        }
    }

    /// The envelope's own outline: the box carried through the map.
    pub fn outline(&self) -> BezPath {
        self.path(&crate::kurbo::Shape::to_path(&self.bounds, 0.1))
    }

    /// Where a preset's one handle sits: the warped top of the bend, or of the
    /// wave's first crest; the left edge's when vertical.
    pub fn handle(&self) -> Option<Point> {
        let Envelope::Preset {
            preset, vertical, ..
        } = self.envelope
        else {
            return None;
        };
        Some(self.map(rest_handle(self.bounds, preset, vertical)))
    }

    /// The bend that puts the handle as far out as `pointer` is, along the bend's
    /// direction, held to the range the preset allows without folding. Answers the
    /// bend and whether the pointer asked for more than that.
    pub fn bend_toward(&self, pointer: Point) -> Option<(f64, bool)> {
        let Envelope::Preset {
            preset, vertical, ..
        } = self.envelope
        else {
            return None;
        };
        let rest = rest_handle(self.bounds, preset, vertical);
        // Outward from the box: up for a horizontal bend, left for a vertical one.
        let out = |p: Point| if vertical { rest.x - p.x } else { rest.y - p.y };
        let at = |bend: f64| {
            let w = Warp {
                bounds: self.bounds,
                envelope: Envelope::Preset {
                    preset,
                    bend,
                    vertical,
                },
            };
            out(w.map(rest))
        };
        let want = out(pointer);
        let limit = max_bend(self.bounds, preset, vertical);
        let (mut lo, mut hi) = (-limit, limit);
        if want <= at(lo) {
            return Some((lo, limit < 1.0 && want < at(lo) - 1e-6));
        }
        if want >= at(hi) {
            return Some((hi, limit < 1.0 && want > at(hi) + 1e-6));
        }
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if at(mid) < want {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(((lo + hi) / 2.0, false))
    }
}

/// The box's corners, top-left first and then clockwise.
pub fn corners(b: Rect) -> [Point; 4] {
    [
        Point::new(b.x0, b.y0),
        Point::new(b.x1, b.y0),
        Point::new(b.x1, b.y1),
        Point::new(b.x0, b.y1),
    ]
}

/// The largest bend `preset` takes on `bounds` without folding: 1, or for an arc
/// the bend at which its inside edge would reach the circle's centre.
pub fn max_bend(bounds: Rect, preset: Preset, vertical: bool) -> f64 {
    if preset != Preset::Arc {
        return 1.0;
    }
    let (w, h) = match vertical {
        false => (bounds.width(), bounds.height()),
        true => (bounds.height(), bounds.width()),
    };
    if h <= 1e-12 {
        return 1.0;
    }
    // The centreline's radius is `w / θ` with `θ = bend · π`; the inside edge
    // reaches the centre when that radius falls to `h / 2`. A hair inside it.
    (2.0 * w / (std::f64::consts::PI * h) * 0.999).min(1.0)
}

/// The handle's place on the unbent box.
fn rest_handle(b: Rect, preset: Preset, vertical: bool) -> Point {
    let c = b.center();
    let along = match preset {
        Preset::Flag => -0.25,
        Preset::Arc | Preset::Bulge => 0.0,
    };
    match vertical {
        false => Point::new(c.x + along * b.width(), b.y0),
        true => Point::new(b.x0, c.y + along * b.height()),
    }
}

/// A preset bent along the width, about the box's centre: `(x, y)` from the
/// centre of a `w` by `h` box.
fn bent(preset: Preset, bend: f64, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
    let u = x / w.max(1e-12);
    match preset {
        Preset::Arc => {
            let theta = bend * std::f64::consts::PI;
            if theta.abs() < 1e-9 || w <= 1e-12 {
                return (x, y);
            }
            // The centreline keeps its length on a circle of radius `r0`, centred
            // below the box for an upward bend; the whole is lifted so the
            // centreline's ends stay level with where they were.
            let r0 = w / theta;
            let r = r0 - y;
            let phi = u * theta;
            let lift = r0 * (1.0 - (theta / 2.0).cos());
            (r * phi.sin(), r0 - r * phi.cos() - lift)
        }
        Preset::Flag => {
            let wave = (std::f64::consts::TAU * (u + 0.5)).sin();
            (x, y - bend * h / 2.0 * wave)
        }
        Preset::Bulge => (x, y * (1.0 + bend * (1.0 - 4.0 * u * u))),
    }
}

/// The perspective map of the unit square onto a quadrilateral.
struct Homography([f64; 8]);

impl Homography {
    fn new(q: [Point; 4]) -> Self {
        let [q0, q1, q2, q3] = q;
        let sx = q0.x - q1.x + q2.x - q3.x;
        let sy = q0.y - q1.y + q2.y - q3.y;
        let (dx1, dx2) = (q1.x - q2.x, q3.x - q2.x);
        let (dy1, dy2) = (q1.y - q2.y, q3.y - q2.y);
        let den = dx1 * dy2 - dx2 * dy1;
        let (g, h) = if den.abs() < 1e-12 {
            (0.0, 0.0)
        } else {
            ((sx * dy2 - dx2 * sy) / den, (dx1 * sy - sx * dy1) / den)
        };
        Self([
            q1.x - q0.x + g * q1.x,
            q3.x - q0.x + h * q3.x,
            q0.x,
            q1.y - q0.y + g * q1.y,
            q3.y - q0.y + h * q3.y,
            q0.y,
            g,
            h,
        ])
    }

    fn map(&self, u: f64, v: f64) -> Point {
        let [a, b, c, d, e, f, g, h] = self.0;
        let w = g * u + h * v + 1.0;
        Point::new((a * u + b * v + c) / w, (d * u + e * v + f) / w)
    }
}

/// Whether the four corners make a convex quadrilateral that keeps the box's
/// turning, so no edge crosses another and the map does not fold.
fn convex(q: [Point; 4]) -> bool {
    (0..4).all(|i| {
        let (a, b, c) = (q[i], q[(i + 1) % 4], q[(i + 2) % 4]);
        (b - a).cross(c - b) > 1e-9
    })
}

/// Why the selection cannot be warped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing selected, or nothing under it with an outline.
    Nothing,
    Picture,
    Frame,
    Locked,
    /// A layer sits in a main or an instance, whose layers a path cannot replace.
    Inside,
}

impl Refusal {
    /// The sentence a dimmed row and a refused commit say.
    pub fn says(self) -> &'static str {
        match self {
            Refusal::Nothing => "Select layers to warp",
            Refusal::Picture => "Pictures can't be warped; trace it first",
            Refusal::Frame => "Frames can't be warped; warp the layers inside",
            Refusal::Locked => "A layer to warp is locked",
            Refusal::Inside => "Warp can't work inside a component or an instance",
        }
    }
}

/// The layers `selection` warps: every leaf under it with an outline, groups and
/// booleans opened, or why not.
pub fn subjects(doc: &Document, selection: &[NodeId]) -> Result<Vec<NodeId>, Refusal> {
    let members = build::outermost(doc, selection);
    if members.is_empty() {
        return Err(Refusal::Nothing);
    }
    let mut leaves = Vec::new();
    for id in members {
        if crate::is_effectively_locked(doc, id) {
            return Err(Refusal::Locked);
        }
        let mut up = doc.get(id).and_then(|n| n.parent());
        while let Some(p) = up.and_then(|p| doc.get(p)) {
            if p.component() || p.link().is_some() {
                return Err(Refusal::Inside);
            }
            up = p.parent();
        }
        collect(doc, id, &mut leaves)?;
    }
    if leaves.is_empty() {
        return Err(Refusal::Nothing);
    }
    Ok(leaves)
}

fn collect(doc: &Document, id: NodeId, leaves: &mut Vec<NodeId>) -> Result<(), Refusal> {
    let node = doc.get(id).ok_or(Refusal::Nothing)?;
    if node.component() || node.link().is_some() {
        return Err(Refusal::Inside);
    }
    // A locked layer inside a selected group is still rewritten, so it refuses too.
    if node.locked() {
        return Err(Refusal::Locked);
    }
    if node.paint().image_fill().is_some() {
        return Err(Refusal::Picture);
    }
    match node.kind() {
        NodeKind::Group | NodeKind::Boolean { .. } => {
            for c in node.children() {
                collect(doc, *c, leaves)?;
            }
        }
        NodeKind::Artboard { .. } => return Err(Refusal::Frame),
        NodeKind::Root => return Err(Refusal::Nothing),
        // A slice draws nothing and is laid over the art, so it is left as it is.
        NodeKind::Slice { .. } => {}
        NodeKind::Rect { .. }
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Line { .. }
        | NodeKind::Spiral { .. }
        | NodeKind::Arc { .. }
        | NodeKind::Path { .. }
        | NodeKind::Text { .. } => leaves.push(id),
    }
    Ok(())
}

/// The world box the envelope wraps: the union of the outlines it carries, which is
/// where the map is defined. Not the layers' bounds: a boolean's box is its
/// result's, smaller than the operands that are warped, and a point outside the box
/// is extrapolated (across a perspective map's horizon, or round an arc's centre).
pub fn bounds(doc: &Document, res: &Resolved, selection: &[NodeId]) -> Option<Rect> {
    subjects(doc, selection)
        .ok()?
        .into_iter()
        .filter_map(|leaf| leaf_outline(doc, res, leaf))
        .map(|(world, local)| crate::kurbo::Shape::bounding_box(&(world * local)))
        .reduce(|a, b| a.union(b))
        .filter(|b| b.width() > 1e-9 || b.height() > 1e-9)
}

/// `leaf`'s world transform and its outline in its own space (a text's glyphs), or
/// `None` with nothing to warp.
fn leaf_outline(
    doc: &Document,
    res: &Resolved,
    leaf: NodeId,
) -> Option<(crate::kurbo::Affine, BezPath)> {
    let node = doc.get(leaf)?;
    let local = match node.kind() {
        NodeKind::Text { .. } => res.text_layout(leaf).map(crate::text::outline),
        _ => res.local_path(doc, leaf),
    };
    let local = local.filter(|p| !p.elements().is_empty())?;
    let world = res
        .world_transform(leaf)
        .filter(|w| w.determinant().abs() > 1e-12)?;
    Some((world, local))
}

/// One leaf's outline carried through the warp, in the leaf's own space.
struct Warped {
    leaf: NodeId,
    path: BezPath,
}

fn warped(
    doc: &Document,
    res: &Resolved,
    selection: &[NodeId],
    warp: &Warp,
) -> Result<Vec<Warped>, Refusal> {
    let mut out = Vec::new();
    for leaf in subjects(doc, selection)? {
        let Some((world, local)) = leaf_outline(doc, res, leaf) else {
            continue;
        };
        let path = world.inverse() * warp.path(&(world * local));
        out.push(Warped { leaf, path });
    }
    if out.is_empty() {
        return Err(Refusal::Nothing);
    }
    Ok(out)
}

/// The edit that bakes `warp` into the selection: a path keeps its node and takes
/// the warped outline; any other leaf is replaced by a path of its warped outline
/// that keeps its look. Answers the edit and the selection afterwards, a replaced
/// selected layer standing in for its old self.
pub fn warp(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    selection: &[NodeId],
    warp: &Warp,
) -> Result<(Transaction, Vec<NodeId>), Refusal> {
    let mut ops = Vec::new();
    let mut picked = selection.to_vec();
    for Warped { leaf, path } in warped(doc, res, selection, warp)? {
        let node = doc.get(leaf).ok_or(Refusal::Nothing)?;
        if matches!(node.kind(), NodeKind::Path { .. }) {
            ops.push(path_patch(leaf, path));
            continue;
        }
        let (tx, new) =
            build::replace_with_path(doc, ids, leaf, path).map_err(|_| Refusal::Nothing)?;
        ops.extend(tx.0);
        for p in &mut picked {
            if *p == leaf {
                *p = new;
            }
        }
    }
    Ok((Transaction(ops), picked))
}

/// What the canvas shows while the envelope is open: a path's own outline
/// warped, and every other leaf hidden under a stand-in path that wears its look.
/// A preview cannot delete, so it draws what [`warp`] will leave rather than
/// performing it.
pub fn preview(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    selection: &[NodeId],
    warp: &Warp,
) -> Result<Transaction, Refusal> {
    let mut ops = Vec::new();
    // Each stand-in placed so far, by its parent and the index it was put at in
    // `doc`: every one at or below a leaf's index has moved that leaf up by one.
    let mut placed: Vec<(NodeId, usize)> = Vec::new();
    for Warped { leaf, path } in warped(doc, res, selection, warp)? {
        let node = doc.get(leaf).ok_or(Refusal::Nothing)?;
        if matches!(node.kind(), NodeKind::Path { .. }) {
            ops.push(path_patch(leaf, path));
            continue;
        }
        // A hidden layer stays hidden, and so does its stand-in.
        if !doc.shown(leaf) {
            continue;
        }
        let parent = node.parent().ok_or(Refusal::Nothing)?;
        let at = doc
            .get(parent)
            .and_then(|p| p.children().iter().position(|c| *c == leaf))
            .ok_or(Refusal::Nothing)?;
        let index = at
            + placed
                .iter()
                .filter(|(p, j)| *p == parent && *j <= at)
                .count();
        placed.push((parent, at));
        let ghost = ids.mint();
        ops.push(Operation::CreateNode {
            id: ghost,
            parent,
            index,
            kind: NodeKind::Path {
                path,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: crate::symmetry::PathMirror::NONE,
            },
            transform: Some(node.transform()),
            name: None,
        });
        ops.extend(build::carried_look(doc, leaf, ghost));
        // A mask stays a mask, or the run above it would draw unclipped meanwhile.
        ops.extend(build::carried_mask(node, ghost));
        // Its place in a layout, so in a flex or grid parent the stand-in takes the
        // hidden layer's slot rather than flowing as a new child.
        if node.insets().is_authored() {
            ops.push(Operation::SetInsets {
                id: ghost,
                insets: *node.insets(),
            });
        }
        if *node.item() != crate::container::LayoutItem::default() {
            ops.push(Operation::SetLayoutItem {
                id: ghost,
                item: *node.item(),
            });
        }
        ops.push(Operation::SetVisible {
            id: leaf,
            visible: false,
        });
    }
    Ok(Transaction(ops))
}

fn path_patch(id: NodeId, path: BezPath) -> Operation {
    Operation::SetGeometry {
        id,
        geometry: crate::GeometryPatch::Path {
            path,
            corner_radii: Vec::new(),
            mirror: Some(crate::symmetry::PathMirror::NONE),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kurbo::{ParamCurveNearest, Shape};

    fn b() -> Rect {
        Rect::new(100.0, 50.0, 300.0, 130.0)
    }

    fn preset(preset: Preset, bend: f64, vertical: bool) -> Warp {
        Warp {
            bounds: b(),
            envelope: Envelope::Preset {
                preset,
                bend,
                vertical,
            },
        }
    }

    /// The square envelope and every preset at zero bend leave every point where it is.
    #[test]
    fn nothing_bent_moves_nothing() {
        let mut warps = vec![Warp::square(b())];
        for p in [Preset::Arc, Preset::Flag, Preset::Bulge] {
            warps.push(preset(p, 0.0, false));
            warps.push(preset(p, 0.0, true));
        }
        for w in warps {
            assert!(w.is_identity());
            for p in [
                Point::new(100.0, 50.0),
                Point::new(213.0, 77.0),
                Point::new(300.0, 130.0),
            ] {
                assert!(w.map(p).distance(p) < 1e-9, "{w:?} moved {p:?}");
            }
        }
    }

    /// Free distort sends each corner where it was dragged and keeps lines
    /// straight: the box's outline stays four line segments.
    #[test]
    fn free_distort_moves_the_corners_and_keeps_lines() {
        let q = [
            Point::new(80.0, 40.0),
            Point::new(320.0, 60.0),
            Point::new(290.0, 160.0),
            Point::new(110.0, 120.0),
        ];
        let w = Warp {
            bounds: b(),
            envelope: Envelope::Free(q),
        };
        for (c, want) in corners(b()).into_iter().zip(q) {
            assert!(w.map(c).distance(want) < 1e-9);
        }
        assert!(!w.folds());
        let outline = w.outline();
        assert!(
            outline
                .elements()
                .iter()
                .all(|e| matches!(e, PathEl::MoveTo(_) | PathEl::LineTo(_) | PathEl::ClosePath))
        );
        // A corner dragged across the far edge folds.
        let mut crossed = q;
        crossed[0] = Point::new(330.0, 150.0);
        let w = Warp {
            bounds: b(),
            envelope: Envelope::Free(crossed),
        };
        assert!(w.folds());
    }

    /// A warped curve stays within the tolerance of the true warp of every point
    /// along the original, for every preset both ways and a free distort.
    #[test]
    fn a_warped_outline_follows_the_true_warp() {
        let mut shape = crate::kurbo::Ellipse::new((180.0, 90.0), (60.0, 30.0), 0.3).to_path(0.01);
        shape.move_to((110.0, 60.0));
        shape.line_to((290.0, 60.0));
        shape.line_to((290.0, 125.0));
        shape.close_path();
        let mut warps = vec![Warp {
            bounds: b(),
            envelope: Envelope::Free([
                Point::new(80.0, 40.0),
                Point::new(320.0, 60.0),
                Point::new(290.0, 160.0),
                Point::new(110.0, 120.0),
            ]),
        }];
        for p in [Preset::Arc, Preset::Flag, Preset::Bulge] {
            for vertical in [false, true] {
                let max = max_bend(b(), p, vertical);
                warps.push(preset(p, 0.8 * max, vertical));
                warps.push(preset(p, -0.8 * max, vertical));
            }
        }
        for w in warps {
            let out = w.path(&shape);
            let src: Vec<_> = shape.segments().collect();
            // Sample the original densely; each image must lie near the output.
            let flat = {
                // A closed run's last edge is drawn by its close, so its start is
                // pushed again there.
                let (mut pts, mut start) = (Vec::new(), Point::ZERO);
                crate::kurbo::flatten(out.iter(), 0.005, |el| match el {
                    PathEl::MoveTo(p) => {
                        start = p;
                        pts.push(p);
                    }
                    PathEl::LineTo(p) => pts.push(p),
                    PathEl::ClosePath => pts.push(start),
                    _ => {}
                });
                pts
            };
            for seg in &src {
                for i in 0..=20 {
                    let target = w.map(seg.eval(f64::from(i) / 20.0));
                    let near = flat
                        .windows(2)
                        .map(|ab| {
                            crate::kurbo::Line::new(ab[0], ab[1])
                                .nearest(target, 1e-9)
                                .distance_sq
                        })
                        .fold(f64::INFINITY, f64::min)
                        .sqrt();
                    assert!(near < 2.0 * TOLERANCE + 0.01, "{w:?}: {near}");
                }
            }
        }
    }

    /// An arc lifts the middle of its top edge and keeps the centreline's ends
    /// level; past the bend the box's height allows, it folds, and dragging the
    /// handle past it stops there and says so.
    #[test]
    fn an_arc_lifts_its_middle_and_stops_at_the_fold() {
        let w = preset(Preset::Arc, 0.4, false);
        let top = w.handle().unwrap();
        assert!(top.y < b().y0 - 1.0, "the top of the bend rises: {top:?}");
        let left = w.map(Point::new(b().x0, b().center().y));
        assert!(
            (left.y - b().center().y).abs() < 1e-9,
            "ends level: {left:?}"
        );

        let (bend, stopped) = w.bend_toward(top).unwrap();
        assert!((bend - 0.4).abs() < 1e-6 && !stopped, "{bend}");
        assert_eq!(
            max_bend(b(), Preset::Arc, false),
            1.0,
            "a wide box takes a half circle"
        );

        let tall = Rect::new(0.0, 0.0, 200.0, 200.0);
        let max = max_bend(tall, Preset::Arc, false);
        assert!(max < 1.0, "a box this tall cannot take a full arc");
        let arc = |bend| Warp {
            bounds: tall,
            envelope: Envelope::Preset {
                preset: Preset::Arc,
                bend,
                vertical: false,
            },
        };
        assert!(arc(max * 1.01).folds());
        assert!(!arc(max).folds());
        let (bend, stopped) = arc(0.2).bend_toward(Point::new(100.0, -10_000.0)).unwrap();
        assert!((bend - max).abs() < 1e-9 && stopped);
    }

    /// Each preset's handle follows its bend both ways, so a drag finds the bend
    /// back from where the handle went.
    #[test]
    fn every_handle_finds_its_bend_back() {
        for p in [Preset::Arc, Preset::Flag, Preset::Bulge] {
            for vertical in [false, true] {
                for bend in [-0.5, -0.1, 0.2, 0.55] {
                    let bend = bend * max_bend(b(), p, vertical);
                    let w = preset(p, bend, vertical);
                    let (back, _) = w.bend_toward(w.handle().unwrap()).unwrap();
                    assert!(
                        (back - bend).abs() < 1e-6,
                        "{p:?} {vertical}: {bend} → {back}"
                    );
                }
            }
        }
    }

    /// A bulge swells the middle and a negative bend pinches it; a flag lifts its
    /// first quarter and drops its third.
    #[test]
    fn bulge_and_flag_bend_the_right_way() {
        let c = b().center();
        let top = Point::new(c.x, b().y0);
        assert!(preset(Preset::Bulge, 0.5, false).map(top).y < top.y - 1.0);
        assert!(preset(Preset::Bulge, -0.5, false).map(top).y > top.y + 1.0);
        let flag = preset(Preset::Flag, 0.5, false);
        let q1 = Point::new(b().x0 + b().width() / 4.0, c.y);
        let q3 = Point::new(b().x0 + 3.0 * b().width() / 4.0, c.y);
        assert!(flag.map(q1).y < c.y - 1.0 && flag.map(q3).y > c.y + 1.0);
    }

    /// A group of a rect and a path: the rect is replaced by a path that keeps
    /// its fill, the path keeps its node, the group stays selected, and the
    /// preview hides the rect under a stand-in rather than deleting it. A locked
    /// member and a picture refuse.
    #[test]
    fn a_warp_bakes_a_group_into_paths() {
        use crate::kurbo::{Affine, Size};
        use crate::{Brush, Fill, keyed_by_position};
        let mut ids = IdSource::new(3);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let (group, rect, path) = (ids.mint(), ids.mint(), ids.mint());
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((80.0, 0.0));
        let red = crate::peniko::Color::from_rgb8(200, 0, 0);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: page,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 40.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((10.0, 10.0))),
                name: Some("Box".into()),
            },
            Operation::SetFills {
                id: rect,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(red).into(),
                    visible: true,
                }]),
            },
            Operation::CreateNode {
                id: path,
                parent: group,
                index: 1,
                kind: NodeKind::Path {
                    path: line,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: crate::symmetry::PathMirror::NONE,
                },
                transform: Some(Affine::translate((20.0, 60.0))),
                name: None,
            },
        ]))
        .expect("the fixture");
        let res = Resolved::rebuild(&doc);
        let bounds = bounds(&doc, &res, &[group]).expect("a box");
        let w = Warp {
            bounds,
            envelope: Envelope::Preset {
                preset: Preset::Bulge,
                bend: 0.5,
                vertical: false,
            },
        };

        let mut minter = IdSource::new(9);
        let preview = preview(&doc, &res, &mut minter, &[group], &w).expect("previewed");
        assert!(preview.0.iter().any(|op| matches!(op,
            Operation::SetVisible { id, visible: false } if *id == rect)));
        assert!(
            !preview
                .0
                .iter()
                .any(|op| matches!(op, Operation::DeleteNode { .. }))
        );

        let (tx, picked) = warp(&doc, &res, &mut ids, &[group], &w).expect("warped");
        assert_eq!(picked, vec![group]);
        let mut after = doc.clone();
        after.apply(&tx).expect("the warp applies");
        assert!(after.get(rect).is_none(), "the rect is replaced");
        let kids = after.get(group).unwrap().children().to_vec();
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[1], path, "the path keeps its node and place");
        let new = after.get(kids[0]).unwrap();
        assert!(matches!(new.kind(), NodeKind::Path { .. }));
        assert_eq!(new.name(), "Box");
        assert_eq!(new.paint().fills.len(), 1, "the fill comes along");
        let NodeKind::Path { path: bent, .. } = after.get(path).unwrap().kind() else {
            panic!("still a path");
        };
        // The bottom edge's line swells downward in its middle.
        let mid = bent.segments().next().unwrap().eval(0.5);
        assert!(mid.y > 1.0, "a straight line bulged: {mid:?}");

        // Selecting the rect alone hands back its stand-in.
        let (_, picked) = warp(&doc, &res, &mut ids, &[rect], &w).expect("warped");
        assert_ne!(picked, vec![rect]);

        let mut locked = doc.clone();
        locked
            .apply(&Transaction(vec![Operation::SetLocked {
                id: path,
                locked: true,
            }]))
            .unwrap();
        assert_eq!(subjects(&locked, &[group]), Err(Refusal::Locked));
        let mut picture = doc.clone();
        picture
            .apply(&Transaction(vec![Operation::SetFills {
                id: rect,
                fills: keyed_by_position([Fill {
                    brush: crate::image::image_brush(crate::ImageId("abc".into())).into(),
                    visible: true,
                }]),
            }]))
            .unwrap();
        assert_eq!(subjects(&picture, &[group]), Err(Refusal::Picture));
        assert_eq!(subjects(&doc, &[]), Err(Refusal::Nothing));
    }

    /// A page of `kinds`, each created with its transform; answers the document and
    /// the ids in order. A kind's parent is the page, or the id at the index given.
    fn fixture(
        nodes: &[(NodeKind, crate::kurbo::Affine, Option<usize>)],
    ) -> (Document, Vec<NodeId>) {
        let mut ids = IdSource::new(5);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let mut made: Vec<NodeId> = Vec::new();
        for (kind, at, parent) in nodes {
            let id = ids.mint();
            let parent = parent.map_or(page, |i| made[i]);
            let index = doc.get(parent).unwrap().children().len();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index,
                kind: kind.clone(),
                transform: Some(*at),
                name: None,
            }]))
            .expect("the fixture");
            made.push(id);
        }
        (doc, made)
    }

    fn rect(w: f64, h: f64) -> NodeKind {
        NodeKind::Rect {
            size: crate::kurbo::Size::new(w, h),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        }
    }

    /// The envelope wraps the operands of an Intersect, not its small result, so a
    /// perspective map carries both and the result is the warped lens.
    ///
    /// flip: `bounds` from `res.world_bounds` of the selection → fails at "the
    /// operands' box".
    #[test]
    fn a_boolean_is_wrapped_by_its_operands() {
        use crate::kurbo::{Affine, Size};
        let circle = NodeKind::Ellipse {
            size: Size::new(400.0, 400.0),
            arc: Default::default(),
        };
        let (doc, ids) = fixture(&[
            (
                NodeKind::Boolean {
                    op: crate::node::BoolOp::Intersect,
                },
                Affine::IDENTITY,
                None,
            ),
            (circle.clone(), Affine::IDENTITY, Some(0)),
            (circle, Affine::translate((380.0, 0.0)), Some(0)),
        ]);
        let res = Resolved::rebuild(&doc);
        let lens = res.world_bounds(ids[0]).expect("a lens");
        let b = bounds(&doc, &res, &[ids[0]]).expect("a box");
        assert!(b.x0 < 1.0 && b.x1 > 779.0, "the operands' box: {b:?}");
        let mut q = corners(b);
        q[1].x += b.width();
        q[2].x += b.width();
        let w = Warp {
            bounds: b,
            envelope: Envelope::Free(q),
        };
        assert!(!w.folds());
        let (tx, _) = warp(&doc, &res, &mut IdSource::new(77), &[ids[0]], &w).expect("warped");
        let mut after = doc.clone();
        after.apply(&tx).unwrap();
        let res = Resolved::rebuild(&after);
        let got = res.world_bounds(ids[0]).expect("the result survives");
        let want = Shape::bounding_box(&w.path(&lens.to_path(0.1)));
        assert!(
            got.x0 > want.x0 - 5.0 && got.x1 < want.x1 + 5.0,
            "{got:?} within {want:?}"
        );
    }

    /// While the envelope is open, a mask's stand-in is a mask with its mode, and a
    /// later stand-in sits directly under its own layer, over the path below it.
    ///
    /// flip: `preview` without `carried_mask` → fails at "a mask"; reading the
    /// index from `doc` alone → fails at "above the path".
    #[test]
    fn a_preview_keeps_masks_and_the_stack() {
        use crate::kurbo::Affine;
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((80.0, 30.0));
        let (mut doc, ids) = fixture(&[
            (NodeKind::Group, Affine::IDENTITY, None),
            (rect(100.0, 100.0), Affine::IDENTITY, Some(0)),
            (
                NodeKind::Path {
                    path: line,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: crate::symmetry::PathMirror::NONE,
                },
                Affine::IDENTITY,
                Some(0),
            ),
            (rect(50.0, 50.0), Affine::translate((10.0, 10.0)), Some(0)),
        ]);
        let (group, mask, path, top) = (ids[0], ids[1], ids[2], ids[3]);
        doc.apply(&Transaction(vec![
            Operation::SetMask {
                id: mask,
                mask: true,
            },
            Operation::SetMaskMode {
                id: mask,
                mode: crate::node::MaskMode::Alpha,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let w = Warp {
            bounds: bounds(&doc, &res, &[group]).unwrap(),
            envelope: Envelope::Preset {
                preset: Preset::Bulge,
                bend: 0.3,
                vertical: false,
            },
        };
        let tx = preview(&doc, &res, &mut IdSource::new(90), &[group], &w).unwrap();
        let mut shown = doc.clone();
        shown.apply(&tx).unwrap();
        let kids = shown.get(group).unwrap().children().to_vec();
        let ghost_of = |leaf: NodeId| kids[kids.iter().position(|k| *k == leaf).unwrap() - 1];
        let ghost = shown.get(ghost_of(mask)).unwrap();
        assert!(ghost.mask(), "a mask");
        assert_eq!(ghost.mask_mode(), crate::node::MaskMode::Alpha);
        let (p, g) = (
            kids.iter().position(|k| *k == path).unwrap(),
            kids.iter().position(|k| *k == ghost_of(top)).unwrap(),
        );
        assert!(g > p, "above the path: {kids:?}");
    }

    /// A slice under the selection is left alone, not a refusal.
    #[test]
    fn a_slice_in_a_group_is_left_out() {
        use crate::kurbo::{Affine, Size};
        let (doc, ids) = fixture(&[
            (NodeKind::Group, Affine::IDENTITY, None),
            (rect(100.0, 40.0), Affine::IDENTITY, Some(0)),
            (
                NodeKind::Slice {
                    size: Size::new(30.0, 30.0),
                },
                Affine::IDENTITY,
                Some(0),
            ),
        ]);
        assert_eq!(subjects(&doc, &[ids[0]]), Ok(vec![ids[1]]));
        assert_eq!(subjects(&doc, &[ids[2]]), Err(Refusal::Nothing));
    }

    /// Far from the origin, where the coordinates' own rounding passes the
    /// tolerance, a warp still fits each curve in a few cubics.
    ///
    /// flip: `tolerance` returning `TOLERANCE` → fails at the element count.
    #[test]
    fn a_far_off_outline_warps_in_bounded_work() {
        let far = 1e15;
        let mut shape = BezPath::new();
        for i in 0..20 {
            let c = crate::kurbo::Circle::new((far + f64::from(i) * 50.0, far), 20.0);
            shape.extend(c.path_elements(0.1));
        }
        let b = Shape::bounding_box(&shape);
        let w = Warp {
            bounds: b,
            envelope: Envelope::Preset {
                preset: Preset::Arc,
                bend: 0.5,
                vertical: false,
            },
        };
        let input = shape.elements().len();
        let out = w.path(&shape).elements().len();
        assert!(out < input * 64, "{input} elements became {out}");
    }
}
