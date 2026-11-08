//! The knife: a line drawn across closed shapes cuts each into the pieces it parts.
//!
//! A shape's outline is first read as the boolean union reads it, so its rings are
//! wound one way and its holes the other; where the layer's fill rule reads its
//! subpaths otherwise, each face it fills is cut on its own. The parts of the line between two
//! crossings of the outline that run inside it are its *chords*, and each is cut
//! along exactly, by surgery on the rings rather than by a boolean (flo_curves
//! mishandles operands that share an edge, which every piece and the knife's own
//! side would):
//!
//! - a chord between two points of a piece's outer ring splits it into two loops,
//!   each taking the holes inside it;
//! - a chord from a ring to a hole joins the hole to the ring along it, both ways
//!   (a keyhole), so that the next chord out of that hole parts the piece.
//!
//! A line that stops inside a shape cuts nothing there: only a chord crosses.

use crate::boolean;
use crate::kurbo::{BezPath, Line, ParamCurve, ParamCurveNearest, PathSeg, Point};
use crate::node::BoolOp;
use crate::shape_builder::{placed_path, reshaped, takes_part, world_outline};
use crate::{Document, IdSource, NodeId, OpError, Resolved, Transaction};

/// How far, in world units, a point may be from a ring to count as on it.
const ON_RING: f64 = 1e-3;

/// What a cut did, for the status line and the selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cut {
    /// How many shapes the line parted.
    pub shapes: usize,
    /// How many pieces they became.
    pub pieces: usize,
    /// Whether the line crossed an instance's shape, which is left whole: cutting
    /// it would break the link to its main without saying so.
    pub instances: bool,
    /// Every piece's layer, by its id after the cut.
    pub made: Vec<NodeId>,
}

/// What a cut along `trail` would do, without doing it: where the line crosses the
/// outlines of the shapes it would part, and how many those are.
pub fn preview(
    doc: &Document,
    res: &Resolved,
    page: NodeId,
    selection: &[NodeId],
    trail: &[Point],
) -> (Vec<Point>, usize) {
    let (subjects, _) = subjects(doc, res, page, selection, trail);
    let mut dots = Vec::new();
    let mut shapes = 0;
    for id in subjects {
        if parted(doc, res, id, trail).is_none() {
            continue;
        }
        shapes += 1;
        if let Some(outline) = world_outline(doc, res, id) {
            dots.extend(crossings(&outline, trail).into_iter().map(|(_, p)| p));
        }
    }
    (dots, shapes)
}

/// The pieces the trail `trail` (world space) cuts the closed outline `outline`
/// into, in world space; the outline's own connected parts when it cuts nothing.
/// `None` when the outline cannot be read as a boolean reads it.
pub fn pieces(outline: &BezPath, trail: &[Point]) -> Option<Vec<BezPath>> {
    let canonical = boolean::evaluate(BoolOp::Union, std::slice::from_ref(outline))?;
    let mut parts: Vec<Vec<Ring>> = boolean::pieces(&canonical)
        .iter()
        .map(|p| crate::geometry::subpaths(p).iter().map(ring_of).collect())
        .collect();
    for chord in split_at_crossings(chords(&canonical, trail)) {
        let mid = chord_middle(&chord);
        let Some(k) = parts
            .iter()
            .position(|rings| crate::geometry::winding(&path_of(rings), mid) != 0)
        else {
            continue;
        };
        if let Some(split) = cut_piece(&parts[k], &chord) {
            parts.splice(k..=k, split);
        }
    }
    Some(
        parts
            .iter()
            .map(|rings| path_of(rings))
            .filter(|p| kurbo::Shape::area(p).abs() >= MIN_PIECE_AREA)
            .collect(),
    )
}

/// The least area, in square units, a piece keeps: a chord that runs along the
/// outline leaves a loop of none. `chords` drops such runs too; this also catches
/// one that strays just past `ON_RING` of it.
const MIN_PIECE_AREA: f64 = 1e-3;

/// `chords`, each cut where it crosses an earlier one. A freehand line can cross
/// itself inside a shape; once the earlier chord has parted the piece, a later
/// one runs from a ring to that cut and on, so it is applied part by part, each
/// between points on the rings as they then stand.
fn split_at_crossings(chords: Vec<Vec<Point>>) -> Vec<Vec<Point>> {
    let mut out: Vec<Vec<Point>> = Vec::new();
    for (n, chord) in chords.iter().enumerate() {
        let mut cuts: Vec<(f64, Point)> = Vec::new();
        for (k, w) in chord.windows(2).enumerate() {
            for earlier in &chords[..n] {
                for v in earlier.windows(2) {
                    if let Some((t, p)) = crossing(w[0], w[1], v[0], v[1]) {
                        cuts.push((k as f64 + t, p));
                    }
                }
            }
        }
        cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
        cuts.dedup_by(|b, a| a.1.distance(b.1) < ON_RING);
        let mut part = vec![chord[0]];
        let mut next = cuts.iter().peekable();
        for (k, w) in chord.windows(2).enumerate() {
            while let Some((_, p)) = next.next_if(|(s, _)| *s < (k + 1) as f64) {
                part.push(*p);
                out.push(std::mem::replace(&mut part, vec![*p]));
            }
            part.push(w[1]);
        }
        out.push(part);
    }
    out.retain(|c| c.len() >= 2 && c.windows(2).any(|w| w[0] != w[1]));
    out
}

/// Where segment `a0–a1` crosses segment `b0–b1` strictly inside both: the
/// parameter along `a` and the point.
fn crossing(a0: Point, a1: Point, b0: Point, b1: Point) -> Option<(f64, Point)> {
    let (r, s) = (a1 - a0, b1 - b0);
    let den = r.cross(s);
    if den.abs() < 1e-12 {
        return None;
    }
    let t = (b0 - a0).cross(s) / den;
    let u = (b0 - a0).cross(r) / den;
    let eps = 1e-9;
    (t > eps && t < 1.0 - eps && u > eps && u < 1.0 - eps).then(|| (t, a0 + r * t))
}

/// One closed ring, as the segments it runs through in order.
type Ring = Vec<PathSeg>;

fn ring_of(sub: &BezPath) -> Ring {
    sub.segments()
        .filter(|s| s.start().distance(s.end()) > 0.0 || !matches!(s, PathSeg::Line(_)))
        .collect()
}

/// `rings` as one path, each ring closed.
fn path_of(rings: &[Ring]) -> BezPath {
    let mut out = BezPath::new();
    for ring in rings {
        let Some(first) = ring.first() else { continue };
        out.move_to(first.start());
        for seg in ring {
            match seg {
                PathSeg::Line(l) => out.line_to(l.p1),
                PathSeg::Quad(q) => out.quad_to(q.p1, q.p2),
                PathSeg::Cubic(c) => out.curve_to(c.p1, c.p2, c.p3),
            }
        }
        out.close_path();
    }
    out
}

/// Where `trail` crosses `outline`, in order along it: each as its distance along
/// the trail in segments (`k + t` on the `k`th) and the point.
fn crossings(outline: &BezPath, trail: &[Point]) -> Vec<(f64, Point)> {
    let mut crossed: Vec<(f64, Point)> = Vec::new();
    for (k, w) in trail.windows(2).enumerate() {
        let line = Line::new(w[0], w[1]);
        if line.p0 == line.p1 {
            continue;
        }
        for seg in outline.segments() {
            for hit in seg.intersect_line(line) {
                crossed.push((k as f64 + hit.line_t, line.eval(hit.line_t)));
            }
        }
    }
    crossed.sort_by(|a, b| a.0.total_cmp(&b.0));
    crossed.dedup_by(|b, a| a.1.distance(b.1) < ON_RING);
    crossed
}

/// The parts of `trail` that run inside `outline` from one crossing of it to the
/// next, each as its points from crossing to crossing.
fn chords(outline: &BezPath, trail: &[Point]) -> Vec<Vec<Point>> {
    let crossed = crossings(outline, trail);
    let along = |s: f64| -> Point {
        let k = (s.floor() as usize).min(trail.len().saturating_sub(2));
        Line::new(trail[k], trail[k + 1]).eval(s - k as f64)
    };
    // A run along the outline itself (a line drawn down a shape's edge) is no
    // chord, whichever way the winding test falls on the boundary.
    let on_outline = |p: Point| {
        outline
            .segments()
            .any(|seg| seg.nearest(p, 1e-9).distance_sq <= ON_RING * ON_RING)
    };
    crossed
        .windows(2)
        .filter(|w| {
            let mid = along((w[0].0 + w[1].0) / 2.0);
            crate::geometry::winding(outline, mid) != 0 && !on_outline(mid)
        })
        .map(|w| {
            let mut pts = vec![w[0].1];
            let (from, to) = (w[0].0.floor() as usize + 1, w[1].0.ceil() as usize);
            pts.extend((from..to).map(|k| trail[k]));
            pts.push(w[1].1);
            pts.dedup();
            pts
        })
        .collect()
}

/// A point halfway along `chord`, by length: inside the piece it crosses.
fn chord_middle(chord: &[Point]) -> Point {
    let total: f64 = chord.windows(2).map(|w| w[0].distance(w[1])).sum();
    let mut left = total / 2.0;
    for w in chord.windows(2) {
        let d = w[0].distance(w[1]);
        if d >= left && d > 0.0 {
            return w[0].lerp(w[1], left / d);
        }
        left -= d;
    }
    chord[0]
}

/// Where `p` lies on `ring`: the segment and the parameter on it, if it is on it.
fn locate(ring: &Ring, p: Point) -> Option<(usize, f64)> {
    ring.iter()
        .enumerate()
        .map(|(i, seg)| {
            let near = seg.nearest(p, 1e-12);
            (near.distance_sq, i, near.t)
        })
        .filter(|(d, _, _)| *d <= ON_RING * ON_RING)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i, t)| (i, t))
}

/// The run of `ring` forward from `from` to `to`; all the way round when they are
/// the same place.
fn arc(ring: &Ring, from: (usize, f64), to: (usize, f64)) -> Ring {
    let n = ring.len();
    let mut out = Ring::new();
    let part = |seg: PathSeg, a: f64, b: f64| (b - a > 1e-12).then(|| seg.subsegment(a..b));
    let ((i, t0), (j, t1)) = (from, to);
    if i == j && t1 > t0 + 1e-12 {
        out.extend(part(ring[i], t0, t1));
        return out;
    }
    out.extend(part(ring[i], t0, 1.0));
    let mut k = (i + 1) % n;
    while k != j {
        out.push(ring[k]);
        k = (k + 1) % n;
    }
    out.extend(part(ring[j], 0.0, t1));
    out
}

/// `points` as straight segments, from `from` to `to` in place of its ends, so the
/// chord meets the ring's runs exactly.
fn chord_segs(points: &[Point], from: Point, to: Point) -> Ring {
    let mut pts = points.to_vec();
    if let Some(p) = pts.first_mut() {
        *p = from;
    }
    if let Some(p) = pts.last_mut() {
        *p = to;
    }
    pts.windows(2)
        .filter(|w| w[0] != w[1])
        .map(|w| PathSeg::Line(Line::new(w[0], w[1])))
        .collect()
}

/// A point on `ring` itself, to tell which loop a ring that the chord does not
/// touch lies inside.
fn on(ring: &Ring) -> Option<Point> {
    ring.first().map(|s| s.eval(0.5))
}

/// The piece `rings` (outer ring first) cut along `chord`: two pieces when the
/// chord runs from the outer ring back to it, the same piece with a hole joined to
/// a ring when it runs between the two, and `None` when it cannot be placed.
fn cut_piece(rings: &[Ring], chord: &[Point]) -> Option<Vec<Vec<Ring>>> {
    let (a, b) = (*chord.first()?, *chord.last()?);
    let find = |p: Point| {
        rings
            .iter()
            .enumerate()
            .find_map(|(r, ring)| locate(ring, p).map(|at| (r, at)))
    };
    let ((ra, pa), (rb, pb)) = (find(a)?, find(b)?);
    let at = |r: usize, (i, t): (usize, f64)| rings[r][i].eval(t);
    let (a, b) = (at(ra, pa), at(rb, pb));
    if ra == rb {
        // Across one hole and back: not a cut through the piece.
        if ra != 0 {
            return None;
        }
        let ring = &rings[0];
        let mut one = chord_segs(chord, a, b);
        one.extend(arc(ring, pb, pa));
        let mut back: Vec<Point> = chord.to_vec();
        back.reverse();
        let mut two = chord_segs(&back, b, a);
        two.extend(arc(ring, pa, pb));
        let mut first = vec![one];
        let mut second = vec![two];
        let outline = path_of(&first);
        for hole in &rings[1..] {
            let inside = on(hole).is_some_and(|p| crate::geometry::winding(&outline, p) != 0);
            match inside {
                true => first.push(hole.clone()),
                false => second.push(hole.clone()),
            }
        }
        return Some(vec![first, second]);
    }
    // A keyhole: round ring `ra` from the chord's start, along the chord, round
    // ring `rb` from its end, and back along the chord.
    let mut joined = arc(&rings[ra], pa, pa);
    joined.extend(chord_segs(chord, a, b));
    joined.extend(arc(&rings[rb], pb, pb));
    let mut back: Vec<Point> = chord.to_vec();
    back.reverse();
    joined.extend(chord_segs(&back, b, a));
    let keep = ra.min(rb);
    let mut out: Vec<Ring> = Vec::with_capacity(rings.len() - 1);
    for (r, ring) in rings.iter().enumerate() {
        if r == keep {
            out.push(joined.clone());
        } else if r != ra && r != rb {
            out.push(ring.clone());
        }
    }
    Some(vec![out])
}

/// The layers a knife drawn along `trail` acts on: `selection`'s closed shapes,
/// groups opened, or with nothing selected every one on `page` under the trail;
/// and apart, the visible shapes inside instances among them, which are not cut.
fn subjects(
    doc: &Document,
    res: &Resolved,
    page: NodeId,
    selection: &[NodeId],
    trail: &[Point],
) -> (Vec<NodeId>, Vec<NodeId>) {
    let (mut found, mut instanced) = (Vec::new(), Vec::new());
    let mut stack: Vec<NodeId> = match selection.is_empty() {
        true => {
            let Some(first) = trail.first() else {
                return (found, instanced);
            };
            let reach = trail
                .iter()
                .fold(kurbo::Rect::from_points(*first, *first), |r, p| {
                    r.union_pt(*p)
                });
            crate::nodes_in_view(res, page, reach)
        }
        false => crate::build::outermost(doc, selection),
    };
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        let group = matches!(node.kind(), crate::NodeKind::Group);
        // The view's list holds a group's children already.
        if group && !selection.is_empty() {
            stack.extend(node.children().iter().copied());
        } else if group {
        } else if crate::component::instance_root(doc, id).is_some() {
            if doc.shown_through(id) {
                instanced.push(id);
            }
        } else if takes_part(doc, id) {
            found.push(id);
        }
    }
    (crate::build::all_in_document_order(doc, &found), instanced)
}

/// Whether `trail` would part `id`'s outline: more pieces than it has parts.
/// Also the pieces, in world space.
///
/// The outline is read by the layer's fill rule: where that differs from how a
/// boolean reads it (a same-way inner ring under non-zero is solid, a crossing
/// under even-odd a hole), the union of its filled faces is cut instead, so the
/// cut follows only the edges it draws.
fn parted(doc: &Document, res: &Resolved, id: NodeId, trail: &[Point]) -> Option<Vec<BezPath>> {
    let mut outline = world_outline(doc, res, id)?;
    let rule = doc.get(id)?.fill_rule();
    if let Some(faces) = boolean::filled_faces(&outline, rule) {
        outline = boolean::evaluate(BoolOp::Union, &faces)?;
    }
    let whole = boolean::pieces(&boolean::evaluate(
        BoolOp::Union,
        std::slice::from_ref(&outline),
    )?)
    .len();
    let parts = pieces(&outline, trail)?;
    (parts.len() > whole).then_some(parts)
}

/// Cut the closed shapes the knife acts on ([`subjects`]) along `trail`, in world
/// space. Each shape the line parts keeps its id, look, name and slot for its first
/// piece, as a path (a primitive becomes one); the other pieces land just above it,
/// looking like it and named as it is. One transaction; `Cut` says what it did.
pub fn cut(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    page: NodeId,
    selection: &[NodeId],
    trail: &[Point],
) -> Result<(Transaction, Cut), OpError> {
    let (subjects, instanced) = subjects(doc, res, page, selection, trail);
    let mut report = Cut {
        instances: instanced
            .iter()
            .any(|id| parted(doc, res, *id, trail).is_some()),
        ..Cut::default()
    };
    let mut advanced = doc.clone();
    let mut ops = Vec::new();
    for id in subjects {
        let Some(world) = res.world_transform(id).filter(|w| w.determinant() != 0.0) else {
            continue;
        };
        let Some(parts) = parted(doc, res, id, trail) else {
            continue;
        };
        report.shapes += 1;
        report.pieces += parts.len();
        let name = doc.get(id).map(|n| n.name().to_string());
        // Placed above the source first, while the source still has the transform
        // `res` knows; reshaping a primitive replaces it with a new id.
        for part in parts[1..].iter().rev() {
            let (tx, made) = placed_path(&advanced, res, ids, id, id, part, name.clone())?;
            advanced.apply(&tx)?;
            ops.extend(tx.0);
            report.made.push(made);
        }
        let (tx, now) = reshaped(&advanced, ids, id, world.inverse() * parts[0].clone())?;
        advanced.apply(&tx)?;
        ops.extend(tx.0);
        report.made.push(now);
    }
    Ok((Transaction(ops), report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Rect, Shape};

    fn area(p: &BezPath) -> f64 {
        p.area().abs()
    }

    /// A line across a square parts it into two rectangles whose areas add up to
    /// the square's, along exactly the line.
    #[test]
    fn a_line_across_a_square_parts_it_in_two() {
        let square = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1);
        let cut =
            pieces(&square, &[Point::new(30.0, -20.0), Point::new(30.0, 120.0)]).expect("readable");
        assert_eq!(cut.len(), 2);
        let mut areas: Vec<f64> = cut.iter().map(area).collect();
        areas.sort_by(f64::total_cmp);
        assert!((areas[0] - 3000.0).abs() < 1e-6, "{areas:?}");
        assert!((areas[1] - 7000.0).abs() < 1e-6, "{areas:?}");
    }

    /// A line that stops inside a shape cuts nothing; a bent line across cuts along
    /// its bend.
    #[test]
    fn only_a_line_that_crosses_cuts() {
        let square = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1);
        let half = pieces(&square, &[Point::new(50.0, -20.0), Point::new(50.0, 60.0)]).unwrap();
        assert_eq!(half.len(), 1, "a line that stops inside cuts nothing");
        let bent = [
            Point::new(-10.0, 20.0),
            Point::new(50.0, 80.0),
            Point::new(110.0, 20.0),
        ];
        let cut = pieces(&square, &bent).unwrap();
        assert_eq!(cut.len(), 2);
        let total: f64 = cut.iter().map(area).sum();
        assert!((total - 10_000.0).abs() < 1e-6);
    }

    /// A ring cut straight through both sides comes apart in two halves: the chord
    /// into the hole joins it to the outline, and the chord out of it parts them.
    ///
    /// flip: `cut_piece` refusing a chord between two rings → fails at "two halves".
    #[test]
    fn a_ring_cut_through_comes_apart_in_two() {
        let mut ring = kurbo::Circle::new((0.0, 0.0), 50.0).to_path(1e-3);
        let hole = kurbo::Circle::new((0.0, 0.0), 30.0).to_path(1e-3);
        let mut reversed = BezPath::new();
        for seg in hole.segments().collect::<Vec<_>>().into_iter().rev() {
            let r = seg.reverse();
            if reversed.elements().is_empty() {
                reversed.move_to(r.start());
            }
            match r {
                PathSeg::Line(l) => reversed.line_to(l.p1),
                PathSeg::Quad(q) => reversed.quad_to(q.p1, q.p2),
                PathSeg::Cubic(c) => reversed.curve_to(c.p1, c.p2, c.p3),
            }
        }
        reversed.close_path();
        ring.extend(reversed.iter());
        let full = area(&ring);
        let cut = pieces(&ring, &[Point::new(-70.0, 0.5), Point::new(70.0, 0.5)]).unwrap();
        assert_eq!(cut.len(), 2, "two halves");
        let total: f64 = cut.iter().map(area).sum();
        assert!((total - full).abs() < full * 1e-6, "{total} against {full}");
        for half in &cut {
            assert!(
                (area(half) - full / 2.0).abs() < full * 0.01,
                "{}",
                area(half)
            );
        }
    }
}
