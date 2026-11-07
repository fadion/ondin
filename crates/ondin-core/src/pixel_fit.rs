//! *Pixel-fit*: put a layer's edges on the pixel grid, so a 16–24px icon renders
//! crisp at 1×. A command, not a mode: it moves points once and leaves them.
//!
//! The grid is world units. A corner, and an anchor where the curve turns level or
//! upright, moves to the nearest grid line on each axis that matters; its handles
//! move with it, so a curve keeps its shape. A box shape (a rectangle, an ellipse,
//! a polygon, a star, a spiral) keeps its kind and has its box rounded instead; a
//! line or an arc has its two ends fitted. The grid is whole units for every layer,
//! so a box's X, Y, W and H all land on integers; a stroke one unit wide and centred
//! then straddles two pixel rows, and is crisp aligned inside or outside. No point
//! moves more than half a unit.
//!
//! A layer turned by anything but a quarter turn is skipped, since no move of its
//! points lines it up with the page's pixels; so is text. Both are counted.

use crate::build;
use crate::document::Document;
use crate::edit::{PenSubpath, PointRef, path_subpaths, rewrite_points};
use crate::id::NodeId;
use crate::node::NodeKind;
use crate::op::{GeometryPatch, Operation, Transaction};
use crate::resolve::Resolved;
use kurbo::{Affine, Point, Rect, Vec2};

/// Below this a coordinate has not moved, and a coefficient is zero.
const EPS: f64 = 1e-6;

/// What one fit did: the points it moved and the layers it would not touch.
#[derive(Debug, Default, PartialEq)]
pub struct Fitted {
    pub moved: usize,
    pub text: usize,
    pub turned: usize,
}

/// `v` on the nearest whole unit, a tie going down: a whole shape fitted this way
/// moves as one and keeps its size.
fn snap(v: f64) -> f64 {
    (v - 0.5).ceil()
}

/// [`snap`] with a tie going up: a box's far edge, so a tied box grows outward.
fn snap_up(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// Whether `world` lines a layer's axes up with the page's: a scale, a flip or a
/// quarter turn, and no shear.
fn square_to_the_page(world: Affine) -> bool {
    let [a, b, c, d, _, _] = world.as_coeffs();
    (b.abs() < EPS && c.abs() < EPS) || (a.abs() < EPS && d.abs() < EPS)
}

/// Fit the anchors of world-space `subpaths` (only those in `only`, when given) to
/// whole units. Answers how many moved.
pub fn fit_anchors(subpaths: &mut [PenSubpath], only: Option<&[PointRef]>) -> usize {
    let mut moved = 0;
    for (s, sub) in subpaths.iter_mut().enumerate() {
        for (a, anchor) in sub.anchors.iter_mut().enumerate() {
            if only.is_some_and(|only| !only.contains(&(s, a))) {
                continue;
            }
            let (out, back) = (anchor.out, anchor.back);
            let smooth = out.hypot() > EPS
                && back.hypot() > EPS
                && out.cross(back).abs() < EPS * out.hypot() * back.hypot()
                && out.dot(back) < 0.0;
            // A corner fits on both axes; a smooth anchor only across its tangent,
            // and only where the tangent is level or upright.
            let (fit_x, fit_y) = match smooth {
                false => (true, true),
                true => {
                    let t = out.normalize();
                    (t.x.abs() < EPS, t.y.abs() < EPS)
                }
            };
            let at = anchor.at;
            let to = Point::new(
                if fit_x { snap(at.x) } else { at.x },
                if fit_y { snap(at.y) } else { at.y },
            );
            if (to - at).hypot() > EPS {
                anchor.at = to;
                moved += 1;
            }
        }
    }
    moved
}

/// `r` with each edge on a whole unit, a tied box grown outward, and no side
/// shorter than one unit unless it was already.
fn fit_rect(r: Rect) -> Rect {
    let axis = |lo: f64, hi: f64| {
        let (a, mut b) = (snap(lo), snap_up(hi));
        if b - a < 1.0 && hi - lo >= 1.0 - EPS {
            b = a + 1.0;
        }
        (a, b)
    };
    let (x0, x1) = axis(r.x0, r.x1);
    let (y0, y1) = axis(r.y0, r.y1);
    Rect::new(x0, y0, x1, y1)
}

/// *Pixel-fit* on the outermost layers of `ids`: every visible path, shape and line
/// in them, through groups, frames and booleans, as one transaction.
pub fn fit(doc: &Document, res: &Resolved, ids: &[NodeId]) -> (Transaction, Fitted) {
    let mut ops = Vec::new();
    let mut fitted = Fitted::default();
    for id in build::outermost(doc, ids) {
        fit_layer(doc, res, id, &mut ops, &mut fitted);
    }
    (Transaction(ops), fitted)
}

/// *Pixel-fit* on the path `id`'s points: those in `only`, or every one.
pub fn fit_points(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    only: Option<&[PointRef]>,
) -> (Transaction, Fitted) {
    let mut fitted = Fitted::default();
    let tx = fit_path(doc, res, id, only, &mut fitted).unwrap_or_default();
    (tx, fitted)
}

/// The ops that fit `id`, or each layer inside it, gathered into `ops`.
fn fit_layer(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    ops: &mut Vec<Operation>,
    fitted: &mut Fitted,
) {
    let Some(node) = doc.get(id) else { return };
    if !node.visible() {
        return;
    }
    let kind = res.used_kind_of(node);
    match kind {
        NodeKind::Group | NodeKind::Artboard { .. } | NodeKind::Boolean { .. } => {
            for child in node.children().to_vec() {
                fit_layer(doc, res, child, ops, fitted);
            }
            return;
        }
        NodeKind::Text { .. } => {
            fitted.text += 1;
            return;
        }
        _ => {}
    }
    let Some(world) = res.world_transform(id) else {
        return;
    };
    if !square_to_the_page(world) {
        fitted.turned += 1;
        return;
    }
    match kind {
        NodeKind::Path { .. } => {
            if let Some(tx) = fit_path(doc, res, id, None, fitted) {
                ops.extend(tx.0);
            }
        }
        // An arc's ends are a line's: the origin and its one point.
        NodeKind::Line { end } | NodeKind::Arc { end, .. } => {
            let (from, to) = (world * Point::ZERO, world * *end);
            let (a, b) = (
                Point::new(snap(from.x), snap(from.y)),
                Point::new(snap(to.x), snap(to.y)),
            );
            let moved = [(from, a), (to, b)]
                .iter()
                .filter(|(p, q)| (*q - *p).hypot() > EPS)
                .count();
            if moved == 0 {
                return;
            }
            fitted.moved += moved;
            let placed = Affine::translate(a - from) * world;
            ops.push(Operation::SetTransform {
                id,
                transform: build::local_for_world(doc, res, id, placed),
            });
            ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::LineEnd(placed.inverse() * b),
            });
        }
        NodeKind::Rect { .. }
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Spiral { .. } => {
            let Some(local) = crate::local_box(doc, res, id) else {
                return;
            };
            let was = world.transform_rect_bbox(local);
            let now = fit_rect(was);
            // Each corner of the box that moved counts as a point.
            let corners =
                |r: Rect| [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)].map(Point::from);
            let moved = corners(was)
                .iter()
                .zip(corners(now))
                .filter(|(p, q)| (*q - **p).hypot() > EPS)
                .count();
            if moved == 0 {
                return;
            }
            fitted.moved += moved;
            ops.extend(box_ops(doc, res, id, world, local, now));
        }
        _ => {}
    }
}

/// The path `id` with its anchors (those in `only`, when given) fitted, or `None`
/// when nothing moved.
fn fit_path(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    only: Option<&[PointRef]>,
    fitted: &mut Fitted,
) -> Option<Transaction> {
    let (world, base) = path_subpaths(doc, res, id)?;
    if !square_to_the_page(world) {
        fitted.turned += 1;
        return None;
    }
    let mut subpaths = base.clone();
    let moved = fit_anchors(&mut subpaths, only);
    if moved == 0 {
        return None;
    }
    fitted.moved += moved;
    Some(rewrite_points(doc, id, world, &base, subpaths))
}

/// The ops that put a box shape's world box at `now`, its axes kept: a new size
/// along each local axis and the translation that lands the box's corner.
fn box_ops(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    world: Affine,
    local: Rect,
    now: Rect,
) -> Vec<Operation> {
    let [a, b, c, d, _, _] = world.as_coeffs();
    // A quarter turn sends local width to world height: read each local side off
    // the world axis it lies along.
    let (w, h) = match b.abs() < EPS {
        true => (now.width() / a.abs(), now.height() / d.abs()),
        false => (now.height() / b.abs(), now.width() / c.abs()),
    };
    let (Some(node), true) = (doc.get(id), w.is_finite() && h.is_finite()) else {
        return Vec::new();
    };
    let sized = Rect::from_origin_size(local.origin(), (w, h));
    let lands = Affine::new([a, b, c, d, 0.0, 0.0]).transform_rect_bbox(sized);
    let placed = Affine::translate(Vec2::new(now.x0 - lands.x0, now.y0 - lands.y0))
        * Affine::new([a, b, c, d, 0.0, 0.0]);
    vec![
        Operation::SetTransform {
            id: node.id(),
            transform: build::local_for_world(doc, res, id, placed),
        },
        Operation::SetGeometry {
            id,
            geometry: GeometryPatch::Size(kurbo::Size::new(w, h)),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::PenAnchor;

    /// A corner moves on both axes to the nearest whole unit, a tie going down.
    #[test]
    fn a_corner_lands_on_the_grid() {
        let mut subs = vec![PenSubpath {
            anchors: vec![PenAnchor::corner(Point::new(3.3, 4.6))],
            closed: false,
        }];
        assert_eq!(fit_anchors(&mut subs, None), 1);
        assert_eq!(subs[0].anchors[0].at, Point::new(3.0, 5.0));
        assert_eq!(fit_anchors(&mut subs, None), 0, "already there");
        subs[0].anchors[0].at = Point::new(2.5, 4.5);
        assert_eq!(fit_anchors(&mut subs, None), 1);
        assert_eq!(subs[0].anchors[0].at, Point::new(2.0, 4.0));
    }

    /// A smooth anchor whose tangent is level moves only up or down, a diagonal one
    /// not at all, and the handles ride along.
    #[test]
    fn a_smooth_anchor_moves_only_across_a_level_or_upright_tangent() {
        let smooth = |at: Point, dir: Vec2| PenAnchor {
            at,
            out: dir,
            back: -dir,
            radius: 0.0,
        };
        let mut subs = vec![PenSubpath {
            anchors: vec![
                smooth(Point::new(8.3, 2.2), Vec2::new(3.0, 0.0)),
                smooth(Point::new(8.3, 2.2), Vec2::new(2.0, 2.0)),
            ],
            closed: false,
        }];
        assert_eq!(fit_anchors(&mut subs, None), 1);
        assert_eq!(subs[0].anchors[0].at, Point::new(8.3, 2.0), "level: y only");
        assert_eq!(subs[0].anchors[0].out, Vec2::new(3.0, 0.0), "handle kept");
        assert_eq!(
            subs[0].anchors[1].at,
            Point::new(8.3, 2.2),
            "diagonal stays"
        );
    }

    /// Only the points asked for move.
    #[test]
    fn only_the_chosen_points_move() {
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.4, 0.4)),
                PenAnchor::corner(Point::new(5.4, 0.4)),
            ],
            closed: false,
        }];
        assert_eq!(fit_anchors(&mut subs, Some(&[(0, 1)])), 1);
        assert_eq!(subs[0].anchors[0].at, Point::new(0.4, 0.4));
        assert_eq!(subs[0].anchors[1].at, Point::new(5.0, 0.0));
    }

    /// A box's edges round, never more than half a unit, a tie growing the box
    /// outward, and a box at least one unit wide stays at least one unit wide.
    #[test]
    fn a_box_rounds_its_edges_and_keeps_a_unit() {
        let r = fit_rect(Rect::new(3.2, 4.0, 20.7, 4.9));
        assert_eq!(r, Rect::new(3.0, 4.0, 21.0, 5.0));
        let thin = fit_rect(Rect::new(3.4, 0.0, 4.45, 1.0));
        assert_eq!(thin.width(), 1.0, "{thin:?}");
        let tied = fit_rect(Rect::new(3.5, 3.5, 20.5, 20.5));
        assert_eq!(tied, Rect::new(3.0, 3.0, 21.0, 21.0));
    }
}
