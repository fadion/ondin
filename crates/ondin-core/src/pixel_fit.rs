//! *Pixel-fit*: put a layer's edges on the pixel grid, so a 16–24px icon renders
//! crisp at 1×. A command, not a mode: it moves points once and leaves them.
//!
//! The grid is world units, or the pixels of one icon size under [`fit_on`]. A corner, and an anchor where the curve turns level or
//! upright, moves to the nearest grid line on each axis that matters; its handles
//! move with it, so a curve keeps its shape. A box shape (a rectangle, an ellipse,
//! a polygon, a star, a spiral) keeps its kind and has its box rounded instead; a
//! line or an arc has its two ends fitted. The grid is whole units for every layer,
//! so a box's X, Y, W and H all land on integers; a stroke one unit wide and centred
//! then straddles two pixel rows, and is crisp aligned inside or outside. No point
//! moves more than half a unit.
//!
//! A layer turned by anything but a quarter turn is skipped, since no move of its
//! points lines it up with the page's pixels; so are text and a locked layer. All
//! three are counted. A slice draws nothing and is passed over uncounted.

use crate::build;
use crate::document::Document;
use crate::edit::{PenSubpath, PointRef, map_subpaths, path_subpaths, pen_path, rewrite_path};
use crate::id::NodeId;
use crate::node::NodeKind;
use crate::op::{GeometryPatch, Operation, Transaction};
use crate::resolve::Resolved;
use crate::symmetry::PathMirror;
use kurbo::{Affine, Point, Rect, Vec2};

/// Below this a coordinate has not moved, and a coefficient is zero.
const EPS: f64 = 1e-6;

/// What one fit did: the points it moved and the layers it would not touch.
#[derive(Debug, Default, PartialEq)]
pub struct Fitted {
    pub moved: usize,
    pub text: usize,
    pub turned: usize,
    pub locked: usize,
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
/// whole units. Answers how many moved. A subpath that fitting would collapse (a
/// closed one losing its area, an open one shrinking to a point) is left as it was.
pub fn fit_anchors(subpaths: &mut [PenSubpath], only: Option<&[PointRef]>) -> usize {
    let mut moved = 0;
    for (s, sub) in subpaths.iter_mut().enumerate() {
        let was = sub.clone();
        let mut here = 0;
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
                here += 1;
            }
        }
        if here > 0 && collapsed(&was, sub) {
            *sub = was;
        } else {
            moved += here;
        }
    }
    moved
}

/// Whether fitting took `was` to nothing: a closed run with area to none, or an
/// open one with length to a point.
fn collapsed(was: &PenSubpath, now: &PenSubpath) -> bool {
    let path = |s: &PenSubpath| crate::edit::pen_path(std::slice::from_ref(s));
    match was.closed {
        true => {
            use kurbo::Shape;
            path(was).area().abs() > EPS && path(now).area().abs() <= EPS
        }
        false => {
            let extent = |s: &PenSubpath| {
                let b = path(s).control_box();
                b.width().max(b.height())
            };
            extent(was) > EPS && extent(now) <= EPS
        }
    }
}

/// `r` with each edge on a whole unit, a tied box grown outward, and no side
/// shorter than one unit unless it had no length at all: a sub-unit side takes
/// the unit its middle is in, so a thin bar never rounds away.
fn fit_rect(r: Rect) -> Rect {
    let axis = |lo: f64, hi: f64| {
        let (a, b) = (snap(lo), snap_up(hi));
        match b - a < 1.0 && hi - lo > EPS {
            true if hi - lo >= 1.0 - EPS => (a, a + 1.0),
            true => {
                let a = ((lo + hi) / 2.0).floor();
                (a, a + 1.0)
            }
            false => (a, b),
        }
    };
    let (x0, x1) = axis(r.x0, r.x1);
    let (y0, y1) = axis(r.y0, r.y1);
    Rect::new(x0, y0, x1, y1)
}

/// *Pixel-fit* on the outermost layers of `ids`: every visible path, shape and line
/// in them, through groups, frames and booleans, as one transaction.
pub fn fit(doc: &Document, res: &Resolved, ids: &[NodeId]) -> (Transaction, Fitted) {
    fit_on(doc, res, ids, Affine::IDENTITY)
}

/// [`fit`] onto the grid `grid` maps whole units of: world space to the grid's
/// space, where each whole unit is a pixel. An icon export fits a copy of its
/// layer to each size's pixels this way; `grid` must be a scale and a translation.
pub fn fit_on(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    grid: Affine,
) -> (Transaction, Fitted) {
    fit_with(
        Place {
            doc,
            res,
            grid,
            locks: true,
        },
        ids,
    )
}

/// [`fit_on`] for a throwaway copy, as an icon export fits: a lock keeps the
/// person's own edits off a layer, and the copy is no one's edit, so locked
/// layers are fitted too.
pub fn fit_copy_on(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    grid: Affine,
) -> (Transaction, Fitted) {
    fit_with(
        Place {
            doc,
            res,
            grid,
            locks: false,
        },
        ids,
    )
}

fn fit_with(place: Place, ids: &[NodeId]) -> (Transaction, Fitted) {
    let mut ops = Vec::new();
    let mut fitted = Fitted::default();
    for id in build::outermost(place.doc, ids) {
        fit_layer(&place, id, &mut ops, &mut fitted);
    }
    (Transaction(ops), fitted)
}

/// What every step of a fit reads: the tree, the grid it fits onto, and whether
/// a locked layer is left alone.
struct Place<'a> {
    doc: &'a Document,
    res: &'a Resolved,
    grid: Affine,
    locks: bool,
}

impl Place<'_> {
    /// `id`'s transform from its local space into the grid's.
    fn world(&self, id: NodeId) -> Option<Affine> {
        Some(self.grid * self.res.world_transform(id)?)
    }

    /// The local transform that puts `id` at `placed`, a transform into the grid.
    fn local(&self, id: NodeId, placed: Affine) -> Affine {
        build::local_for_world(self.doc, self.res, id, self.grid.inverse() * placed)
    }
}

/// *Pixel-fit* on the path `id`'s points: those in `only`, or every one.
pub fn fit_points(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    only: Option<&[PointRef]>,
) -> (Transaction, Fitted) {
    let mut fitted = Fitted::default();
    let place = Place {
        doc,
        res,
        grid: Affine::IDENTITY,
        locks: true,
    };
    let tx = fit_path(&place, id, only, &mut fitted).unwrap_or_default();
    (tx, fitted)
}

/// The ops that fit `id`, or each layer inside it, gathered into `ops`.
fn fit_layer(place: &Place, id: NodeId, ops: &mut Vec<Operation>, fitted: &mut Fitted) {
    let (doc, res) = (place.doc, place.res);
    let Some(node) = doc.get(id) else { return };
    if !node.visible() {
        return;
    }
    let kind = res.used_kind_of(node);
    // A slice draws nothing, so it is neither fitted nor counted.
    if matches!(kind, NodeKind::Slice { .. }) {
        return;
    }
    // A selected group still opens onto a locked layer inside it.
    if place.locks && crate::query::is_effectively_locked(doc, id) {
        fitted.locked += 1;
        return;
    }
    match kind {
        NodeKind::Group | NodeKind::Artboard { .. } | NodeKind::Boolean { .. } => {
            for child in node.children().to_vec() {
                fit_layer(place, child, ops, fitted);
            }
            return;
        }
        NodeKind::Text { .. } => {
            fitted.text += 1;
            return;
        }
        _ => {}
    }
    let Some(world) = place.world(id) else {
        return;
    };
    if !square_to_the_page(world) {
        fitted.turned += 1;
        return;
    }
    match kind {
        NodeKind::Path { .. } => {
            if let Some(tx) = fit_path(place, id, None, fitted) {
                ops.extend(tx.0);
            }
        }
        // An arc's ends are a line's: the origin and its one point.
        NodeKind::Line { end } | NodeKind::Arc { end, .. } => {
            let (from, to) = (world * Point::ZERO, world * *end);
            let (a, mut b) = (
                Point::new(snap(from.x), snap(from.y)),
                Point::new(snap(to.x), snap(to.y)),
            );
            // A sub-unit line that snaps to one point keeps a unit along its
            // longer side, as `fit_rect` keeps a thin bar, rather than vanishing.
            let along = to - from;
            if a == b && along.hypot() > EPS {
                b += match along.x.abs() >= along.y.abs() {
                    true => kurbo::Vec2::new(along.x.signum(), 0.0),
                    false => kurbo::Vec2::new(0.0, along.y.signum()),
                };
            }
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
                transform: place.local(id, placed),
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
            ops.extend(box_ops(place, id, world, local, now));
        }
        _ => {}
    }
}

/// The path `id` with its anchors (those in `only`, when given) fitted, or `None`
/// when nothing moved.
///
/// A mirrored path is fitted about its axis: fitting the whole path first moves the
/// axis to the nearest half unit, then each anchor's image is mirrored from it, so
/// images land on whole units too and a second fit moves nothing. Points are counted
/// from what is written, the mirroring included.
fn fit_path(
    place: &Place,
    id: NodeId,
    only: Option<&[PointRef]>,
    fitted: &mut Fitted,
) -> Option<Transaction> {
    let (doc, res) = (place.doc, place.res);
    let (world, mut base) = path_subpaths(doc, res, id)?;
    // Into the grid: the anchors, and the transform `rewrite_path` maps them back by.
    map_subpaths(&mut base, place.grid);
    let world = place.grid * world;
    if !square_to_the_page(world) {
        fitted.turned += 1;
        return None;
    }
    let mirror = match doc.get(id)?.kind() {
        NodeKind::Path { mirror, .. } => *mirror,
        _ => PathMirror::NONE,
    };
    let mut start = base.clone();
    let mut subpaths = base.clone();
    if !mirror.is_none() {
        // The path's own axes in world terms: a quarter turn swaps them.
        let quarter = world.as_coeffs()[1].abs() > EPS;
        let mirror = PathMirror {
            vertical: if quarter {
                mirror.horizontal
            } else {
                mirror.vertical
            },
            horizontal: if quarter {
                mirror.vertical
            } else {
                mirror.horizontal
            },
        };
        let at = crate::symmetry::centre(&pen_path(&base));
        let half = |v: f64| (v * 2.0).round() / 2.0;
        let to = match only {
            Some(_) => at,
            None => Point::new(
                if mirror.vertical { half(at.x) } else { at.x },
                if mirror.horizontal { half(at.y) } else { at.y },
            ),
        };
        map_subpaths(&mut start, Affine::translate(to - at));
        subpaths = start.clone();
        fit_anchors(&mut subpaths, only);
        crate::symmetry::remirror(&start, &mut subpaths, mirror, to);
    } else {
        fit_anchors(&mut subpaths, only);
    }
    let moved = base
        .iter()
        .flat_map(|s| &s.anchors)
        .zip(subpaths.iter().flat_map(|s| &s.anchors))
        .filter(|(a, b)| (a.at - b.at).hypot() > EPS)
        .count();
    if moved == 0 {
        return None;
    }
    fitted.moved += moved;
    let NodeKind::Path {
        path, corner_radii, ..
    } = res.used_kind(doc, id)?
    else {
        return None;
    };
    // Already mirrored above: `rewrite_points` would mirror again about the old axis.
    Some(rewrite_path(
        id,
        world,
        &subpaths,
        Some((path, corner_radii)),
    ))
}

/// The ops that put a box shape's world box at `now`, its axes kept: a new size
/// along each local axis and the translation that lands the box's corner.
fn box_ops(place: &Place, id: NodeId, world: Affine, local: Rect, now: Rect) -> Vec<Operation> {
    let doc = place.doc;
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
            transform: place.local(id, placed),
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

    /// A side under one unit keeps a unit wherever it sits, so a thin bar never
    /// rounds to nothing; a side with no length stays so.
    /// flip: `fit_rect` without its sub-unit arm → fails at "a thin bar keeps a unit".
    #[test]
    fn a_sub_unit_side_never_rounds_to_nothing() {
        let thin = fit_rect(Rect::new(10.1, 0.0, 10.4, 10.0));
        assert_eq!((thin.x0, thin.x1), (10.0, 11.0), "a thin bar keeps a unit");
        let flat = fit_rect(Rect::new(10.2, 0.0, 10.2, 10.0));
        assert_eq!(flat.width(), 0.0, "{flat:?}");
    }

    /// A sub-unit closed run whose corners would all land on one point is left as
    /// it was, and counted as unmoved.
    /// flip: `fit_anchors` without its `collapsed` check → fails at the count.
    #[test]
    fn a_speck_is_not_fitted_to_nothing() {
        let mut subs = vec![PenSubpath {
            anchors: [(10.1, 10.1), (10.4, 10.1), (10.25, 10.4)]
                .map(|(x, y)| PenAnchor::corner(Point::new(x, y)))
                .to_vec(),
            closed: true,
        }];
        let was = subs.clone();
        assert_eq!(fit_anchors(&mut subs, None), 0);
        assert_eq!(subs, was, "the speck survives");
    }

    use crate::id::IdSource;
    use crate::op::Operation;
    use kurbo::{BezPath, Size};

    /// `n` fresh ids after the root, and the document holding what `build` makes
    /// of them under the root.
    fn doc_of<const N: usize>(
        build: impl FnOnce(NodeId, [NodeId; N]) -> Vec<Operation>,
    ) -> (Document, [NodeId; N]) {
        let mut ids = IdSource::new(0xF17);
        let root = ids.mint();
        let made = [(); N].map(|_| ids.mint());
        let mut doc = Document::new(root);
        doc.apply(&Transaction(build(root, made)))
            .expect("the fixture applies");
        (doc, made)
    }

    fn rect_at(id: NodeId, parent: NodeId, index: usize, at: Affine, size: Size) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size,
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(at),
            name: None,
        }
    }

    fn group(id: NodeId, parent: NodeId, at: Affine) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(at),
            name: None,
        }
    }

    /// The ops of `tx` that name `id`.
    fn naming(tx: &Transaction, id: NodeId) -> usize {
        tx.0.iter().filter(|op| op.overwrites() == Some(id)).count()
    }

    /// A locked layer inside a selected group is neither moved nor resized, and is
    /// counted; its unlocked sibling still fits.
    /// flip: `fit_layer` without its lock check → fails at "the locked rect is left alone".
    #[test]
    fn a_locked_layer_in_a_selected_group_is_skipped() {
        let (doc, [g, locked, free]) = doc_of(|root, [g, locked, free]| {
            let odd = Size::new(20.4, 9.7);
            vec![
                group(g, root, Affine::IDENTITY),
                rect_at(locked, g, 0, Affine::translate((10.3, 4.6)), odd),
                rect_at(free, g, 1, Affine::translate((50.3, 4.6)), odd),
                Operation::SetLocked {
                    id: locked,
                    locked: true,
                },
            ]
        });
        let res = Resolved::rebuild(&doc);
        let (tx, fitted) = fit(&doc, &res, &[g]);
        assert_eq!(naming(&tx, locked), 0, "the locked rect is left alone");
        assert!(naming(&tx, free) > 0, "its sibling fits");
        assert_eq!(fitted.locked, 1);
    }

    /// A slice under a turned frame is neither fitted nor counted as turned.
    /// flip: `fit_layer` without its slice arm → fails at "a slice is not counted".
    #[test]
    fn a_slice_is_left_alone_uncounted() {
        let (doc, [g, slice]) = doc_of(|root, [g, slice]| {
            vec![
                group(g, root, Affine::rotate(0.17)),
                Operation::CreateNode {
                    id: slice,
                    parent: g,
                    index: 0,
                    kind: NodeKind::Slice {
                        size: Size::new(10.3, 10.3),
                    },
                    transform: Some(Affine::translate((0.4, 0.4))),
                    name: None,
                },
            ]
        });
        let res = Resolved::rebuild(&doc);
        let (tx, fitted) = fit(&doc, &res, &[g, slice]);
        assert!(tx.0.is_empty());
        assert_eq!(fitted, Fitted::default(), "a slice is not counted");
    }

    /// On a grid of 24 world units to 16 pixels, a rect and a path inside a group
    /// land on whole pixels of that grid, not on whole world units.
    /// flip: `Place::local` without the grid's inverse → fails at "the rect's box
    /// lands on the grid".
    #[test]
    fn a_fit_onto_a_finer_grid_lands_on_its_pixels() {
        let (doc, [g, rect, path]) = doc_of(|root, [g, rect, path]| {
            let mut line = kurbo::BezPath::new();
            line.move_to((2.2, 2.2));
            line.line_to((20.1, 2.2));
            line.line_to((20.1, 13.0));
            vec![
                group(g, root, Affine::translate((0.6, 0.0))),
                rect_at(
                    rect,
                    g,
                    0,
                    Affine::translate((1.2, 0.3)),
                    Size::new(10.0, 7.3),
                ),
                Operation::CreateNode {
                    id: path,
                    parent: g,
                    index: 1,
                    kind: NodeKind::Path {
                        path: line,
                        corner_radii: Vec::new(),
                        corner_smoothing: 0.0,
                        mirror: PathMirror::NONE,
                    },
                    transform: None,
                    name: None,
                },
            ]
        });
        let res = Resolved::rebuild(&doc);
        let grid = Affine::scale(16.0 / 24.0);
        let (tx, fitted) = fit_on(&doc, &res, &[g], grid);
        assert!(fitted.moved > 0);
        let mut after = doc.clone();
        after.apply(&tx).expect("the fit applies");
        let ra = Resolved::rebuild(&after);
        let on_grid = |v: f64| ((v * 16.0 / 24.0) - (v * 16.0 / 24.0).round()).abs() < 1e-6;
        let b = ra.world_bounds(rect).unwrap();
        assert!(
            [b.x0, b.y0, b.x1, b.y1].into_iter().all(on_grid),
            "the rect's box lands on the grid: {b:?}"
        );
        let (_, subs) = path_subpaths(&after, &ra, path).unwrap();
        for a in subs.iter().flat_map(|s| &s.anchors) {
            assert!(on_grid(a.at.x) && on_grid(a.at.y), "{:?}", a.at);
        }
    }

    /// On the same finer grid, a line in a scaled group, a quarter-turned rect and
    /// a mirrored path land on that grid's pixels (the mirror's anchors on its
    /// half pixels), not on world units.
    /// flip: the line arm writing `build::local_for_world` without `Place::local`'s
    /// grid inverse → fails at "the line's ends".
    #[test]
    fn lines_turns_and_mirrors_fit_onto_a_finer_grid() {
        let (doc, [g, line, turned, mirrored]) = doc_of(|root, [g, line, turned, mirrored]| {
            let mut p = BezPath::new();
            p.move_to((40.3, -10.0));
            p.line_to((50.0, 0.0));
            p.line_to((40.3, 10.0));
            p.line_to((30.6, 0.0));
            p.close_path();
            vec![
                group(g, root, Affine::translate((3.3, 1.2)) * Affine::scale(2.0)),
                Operation::CreateNode {
                    id: line,
                    parent: g,
                    index: 0,
                    kind: NodeKind::Line {
                        end: Point::new(10.2, 3.1),
                    },
                    transform: Some(Affine::translate((1.1, 2.2))),
                    name: None,
                },
                rect_at(
                    turned,
                    root,
                    1,
                    Affine::translate((80.3, 4.6)) * Affine::rotate(std::f64::consts::FRAC_PI_2),
                    Size::new(20.4, 9.7),
                ),
                Operation::CreateNode {
                    id: mirrored,
                    parent: root,
                    index: 2,
                    kind: NodeKind::Path {
                        path: p,
                        corner_radii: Vec::new(),
                        corner_smoothing: 0.0,
                        mirror: PathMirror {
                            vertical: true,
                            horizontal: false,
                        },
                    },
                    transform: None,
                    name: None,
                },
            ]
        });
        let res = Resolved::rebuild(&doc);
        let k = 16.0 / 24.0;
        let (tx, _) = fit_on(&doc, &res, &[g, turned, mirrored], Affine::scale(k));
        let mut after = doc.clone();
        after.apply(&tx).expect("the fit applies");
        let ra = Resolved::rebuild(&after);
        let near = |v: f64, step: f64| ((v * k / step) - (v * k / step).round()).abs() < 1e-6;
        let world = ra.world_transform(line).unwrap();
        let NodeKind::Line { end } = after.get(line).unwrap().kind() else {
            panic!("a line");
        };
        for p in [world * Point::ZERO, world * *end] {
            assert!(near(p.x, 1.0) && near(p.y, 1.0), "the line's ends: {p:?}");
        }
        let b = ra.world_bounds(turned).unwrap();
        assert!(
            [b.x0, b.y0, b.x1, b.y1].into_iter().all(|v| near(v, 1.0)),
            "the quarter-turned box: {b:?}"
        );
        let (_, subs) = path_subpaths(&after, &ra, mirrored).unwrap();
        for a in &subs[0].anchors {
            assert!(
                near(a.at.x, 0.5) && near(a.at.y, 0.5),
                "the mirror's anchors: {subs:?}"
            );
        }
    }

    /// A line in a scaled group lands both world ends on whole units; a rect under
    /// a flip and a quarter turn lands its world box on whole units and a second fit
    /// does nothing; a sheared rect is counted as turned; a hidden rect is passed over.
    /// flip: the line arm's end written as `world.inverse() * b` → fails at "the far
    /// end lands whole"; `box_ops` reading the quarter turn as unturned → fails at
    /// "the quarter-turned box lands whole".
    #[test]
    fn lines_quarter_turns_shears_and_hidden_layers() {
        let (doc, [g, line, turned, sheared, hidden]) =
            doc_of(|root, [g, line, turned, sheared, hidden]| {
                let flip_turn = Affine::translate((40.3, 4.6))
                    * Affine::FLIP_X
                    * Affine::rotate(std::f64::consts::FRAC_PI_2);
                vec![
                    group(g, root, Affine::translate((3.3, 1.2)) * Affine::scale(2.0)),
                    Operation::CreateNode {
                        id: line,
                        parent: g,
                        index: 0,
                        kind: NodeKind::Line {
                            end: Point::new(10.2, 3.1),
                        },
                        transform: Some(Affine::translate((1.1, 2.2))),
                        name: None,
                    },
                    rect_at(turned, root, 1, flip_turn, Size::new(20.4, 9.7)),
                    rect_at(
                        sheared,
                        root,
                        2,
                        Affine::translate((80.3, 0.0)) * Affine::skew(0.3, 0.0),
                        Size::new(10.0, 10.0),
                    ),
                    rect_at(
                        hidden,
                        root,
                        3,
                        Affine::translate((120.3, 0.4)),
                        Size::new(10.0, 10.0),
                    ),
                    Operation::SetVisible {
                        id: hidden,
                        visible: false,
                    },
                ]
            });
        let res = Resolved::rebuild(&doc);
        let (tx, fitted) = fit(&doc, &res, &[g, turned, sheared, hidden]);
        assert_eq!(naming(&tx, sheared), 0);
        assert_eq!(naming(&tx, hidden), 0);
        assert_eq!(fitted.turned, 1, "the sheared rect is counted");
        let mut after = doc.clone();
        after.apply(&tx).expect("the fit applies");
        let ra = Resolved::rebuild(&after);
        let whole = |p: Point| p.x.fract() == 0.0 && p.y.fract() == 0.0;
        let world = ra.world_transform(line).unwrap();
        let NodeKind::Line { end } = after.get(line).unwrap().kind() else {
            panic!("a line");
        };
        assert!(whole(world * Point::ZERO), "{:?}", world * Point::ZERO);
        let far = world * *end;
        assert!(
            (far.x - far.x.round()).abs() < 1e-9 && (far.y - far.y.round()).abs() < 1e-9,
            "the far end lands whole: {far:?}"
        );
        // Drawn at x 40.3–50, y 4.6–25.
        let b = ra.world_bounds(turned).unwrap();
        assert!(
            [b.x0 - 40.0, b.y0 - 5.0, b.x1 - 50.0, b.y1 - 25.0]
                .iter()
                .all(|v| v.abs() < 1e-9),
            "the quarter-turned box lands whole: {b:?}"
        );
        let (again, _) = fit(&after, &ra, &[turned]);
        assert!(again.0.is_empty(), "a second fit does nothing: {again:?}");
    }

    /// A line or arc shorter than a unit keeps a unit's length along its longer
    /// side rather than collapsing to a point.
    /// flip: the line arm without its collapse test → fails at "keeps a length".
    #[test]
    fn a_sub_unit_line_or_arc_keeps_a_unit() {
        let (doc, [line, arc]) = doc_of(|root, [line, arc]| {
            vec![
                Operation::CreateNode {
                    id: line,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Line {
                        end: Point::new(0.3, 0.0),
                    },
                    transform: Some(Affine::translate((10.1, 10.1))),
                    name: None,
                },
                Operation::CreateNode {
                    id: arc,
                    parent: root,
                    index: 1,
                    kind: NodeKind::Arc {
                        end: Point::new(-0.1, -0.4),
                        sweep: 90.0,
                    },
                    transform: Some(Affine::translate((20.2, 20.3))),
                    name: None,
                },
            ]
        });
        let res = Resolved::rebuild(&doc);
        let (tx, _) = fit(&doc, &res, &[line, arc]);
        let mut after = doc.clone();
        after.apply(&tx).expect("the fit applies");
        let ra = Resolved::rebuild(&after);
        for (id, want) in [
            (line, (Point::new(10.0, 10.0), Point::new(11.0, 10.0))),
            (arc, (Point::new(20.0, 20.0), Point::new(20.0, 19.0))),
        ] {
            let world = ra.world_transform(id).unwrap();
            let (NodeKind::Line { end } | NodeKind::Arc { end, .. }) =
                after.get(id).unwrap().kind()
            else {
                panic!("a line or an arc");
            };
            let got = (world * Point::ZERO, world * *end);
            assert!(
                (got.0 - want.0).hypot() < 1e-9 && (got.1 - want.1).hypot() < 1e-9,
                "keeps a length: {got:?} vs {want:?}"
            );
        }
    }

    /// A path mirrored about an axis off the half-unit grid has its axis moved to
    /// the nearest half unit and every anchor on whole units, the axis kept, and a
    /// second fit moves nothing.
    /// flip: `fit_path` leaving the axis where it is → fails at "every anchor is
    /// whole or half".
    #[test]
    fn a_mirrored_path_fits_about_its_axis() {
        let (doc, [made]) = doc_of(|root, [made]| {
            let mut p = BezPath::new();
            p.move_to((10.3, -10.0));
            p.line_to((20.0, 0.0));
            p.line_to((10.3, 10.0));
            p.line_to((0.6, 0.0));
            p.close_path();
            vec![Operation::CreateNode {
                id: made,
                parent: root,
                index: 0,
                kind: NodeKind::Path {
                    path: p,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: PathMirror {
                        vertical: true,
                        horizontal: false,
                    },
                },
                transform: None,
                name: None,
            }]
        });
        let res = Resolved::rebuild(&doc);
        let (tx, fitted) = fit(&doc, &res, &[made]);
        let mut after = doc.clone();
        after.apply(&tx).expect("the fit applies");
        let ra = Resolved::rebuild(&after);
        let (_, subs) = path_subpaths(&after, &ra, made).unwrap();
        for a in &subs[0].anchors {
            for v in [a.at.x, a.at.y] {
                assert!(
                    (v * 2.0).fract() == 0.0,
                    "every anchor is whole or half: {subs:?}"
                );
            }
        }
        let NodeKind::Path { mirror, .. } = after.get(made).unwrap().kind() else {
            panic!("a path");
        };
        assert!(mirror.vertical, "the axis is kept");
        assert_eq!(fitted.moved, 3, "the right anchor was already whole");
        let (again, twice) = fit(&after, &ra, &[made]);
        assert!(again.0.is_empty(), "a second fit moves nothing: {again:?}");
        assert_eq!(twice.moved, 0);
    }
}
