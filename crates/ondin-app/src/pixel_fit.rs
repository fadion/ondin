//! *Pixel-fit*: put a layer's edges on the pixel grid, so a 16–24px icon renders
//! crisp at 1×. A command, not a mode: it moves points once and leaves them.
//!
//! The grid is world units. A corner, and an anchor where the curve turns level or
//! upright, moves to the nearest grid line on each axis that matters; its handles
//! move with it, so a curve keeps its shape. A box shape (a rectangle, an ellipse,
//! a polygon, a star, a spiral) keeps its kind and has its box rounded instead; a
//! line or an arc has its two ends fitted. A layer whose
//! stroke is centred and an odd number of units wide is fitted to the half unit,
//! so both of the stroke's edges land on whole units. No point moves more than half
//! a unit.
//!
//! A layer turned by anything but a quarter turn is skipped, since no move of its
//! points lines it up with the page's pixels; so is text. Both are counted and
//! named on the status line.

use crate::app::OndinApp;
use crate::preview::{PenSubpath, PointRef};
use crate::theme::icon;
use ondin_core::kurbo::{Affine, Point, Rect, Vec2};
use ondin_core::{GeometryPatch, NodeId, NodeKind, Operation, StrokeAlign, Transaction, build};

/// Below this a coordinate has not moved, and a coefficient is zero.
const EPS: f64 = 1e-6;

/// What one fit did: the points it moved and the layers it would not touch.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Fitted {
    pub(crate) moved: usize,
    pub(crate) text: usize,
    pub(crate) turned: usize,
}

impl Fitted {
    /// The status line: what moved, then what was skipped and why.
    pub(crate) fn message(&self) -> String {
        let mut skipped = Vec::new();
        if self.text > 0 {
            skipped.push(crate::ui::count(self.text, "text layer"));
        }
        if self.turned > 0 {
            skipped.push(crate::ui::count(self.turned, "rotated layer"));
        }
        let skipped = skipped.join(" and ");
        match (self.moved, skipped.is_empty()) {
            (0, true) => "Already on the pixel grid".to_owned(),
            (0, false) => format!("Pixel-fit skipped {skipped}"),
            (n, true) => format!("Moved {} onto the pixel grid", crate::ui::count(n, "point")),
            (n, false) => format!(
                "Moved {} onto the pixel grid; skipped {skipped}",
                crate::ui::count(n, "point")
            ),
        }
    }
}

/// `v` on the nearest line of the grid `off` from the whole units (`off` is 0 or
/// 0.5), a tie going down: a whole shape fitted this way moves as one and keeps
/// its size.
fn snap(v: f64, off: f64) -> f64 {
    (v - off - 0.5).ceil() + off
}

/// [`snap`] with a tie going up: a box's far edge, so a tied box grows outward.
fn snap_up(v: f64, off: f64) -> f64 {
    (v - off + 0.5).floor() + off
}

/// Whether `world` lines a layer's axes up with the page's: a scale, a flip or a
/// quarter turn, and no shear.
fn square_to_the_page(world: Affine) -> bool {
    let [a, b, c, d, _, _] = world.as_coeffs();
    (b.abs() < EPS && c.abs() < EPS) || (a.abs() < EPS && d.abs() < EPS)
}

/// Where a layer's edges go so its stroke covers whole pixels: the half unit for a
/// centred stroke an odd number of units wide, the whole unit otherwise. The first
/// visible stroke decides; a stroke inside or outside starts at the edge itself.
fn grid_offset(app: &OndinApp, id: NodeId) -> f64 {
    let Some(node) = app.session.doc.get(id) else {
        return 0.0;
    };
    let paint = app.session.doc.resolved_paint(id, node.paint());
    let Some(s) = paint.strokes.iter().find(|s| s.visible) else {
        return 0.0;
    };
    let w = s.value.width;
    let odd = (w - w.round()).abs() < EPS && (w.round() as i64) % 2 == 1;
    match (s.value.align, odd) {
        (StrokeAlign::Center, true) => 0.5,
        _ => 0.0,
    }
}

/// Fit the anchors of world-space `subpaths` (only those in `only`, when given) to
/// the grid `off` from the whole units. Answers how many moved.
pub(crate) fn fit_anchors(
    subpaths: &mut [PenSubpath],
    only: Option<&[PointRef]>,
    off: f64,
) -> usize {
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
                if fit_x { snap(at.x, off) } else { at.x },
                if fit_y { snap(at.y, off) } else { at.y },
            );
            if (to - at).hypot() > EPS {
                anchor.at = to;
                moved += 1;
            }
        }
    }
    moved
}

/// `r` with each edge on the grid `off` from the whole units, a tied box grown
/// outward, and no side shorter than one unit unless it was already.
fn fit_rect(r: Rect, off: f64) -> Rect {
    let axis = |lo: f64, hi: f64| {
        let (a, mut b) = (snap(lo, off), snap_up(hi, off));
        if b - a < 1.0 && hi - lo >= 1.0 - EPS {
            b = a + 1.0;
        }
        (a, b)
    };
    let (x0, x1) = axis(r.x0, r.x1);
    let (y0, y1) = axis(r.y0, r.y1);
    Rect::new(x0, y0, x1, y1)
}

impl OndinApp {
    /// *Pixel-fit* on the selection: every path, shape and line in it, through
    /// groups, frames and booleans. One undo step, *Pixel-fit*.
    pub(crate) fn pixel_fit(&mut self) {
        let roots = build::outermost(&self.session.doc, self.session.selection.ids());
        let mut ops = Vec::new();
        let mut fitted = Fitted::default();
        for id in roots {
            self.fit_layer(id, &mut ops, &mut fitted);
        }
        self.commit_fit(ops, &fitted);
    }

    /// *Pixel-fit* from the path bar: the edited path's selected points, or the
    /// whole path with none selected.
    pub(crate) fn pixel_fit_points(&mut self) {
        let Some(id) = self.edited_path() else { return };
        let points = self.subject_points(id);
        let only = (!points.is_empty()).then_some(points);
        let mut fitted = Fitted::default();
        let ops = match self.fit_path(id, only.as_deref(), &mut fitted) {
            Some(tx) => tx.0,
            None => Vec::new(),
        };
        self.commit_fit(ops, &fitted);
    }

    fn commit_fit(&mut self, ops: Vec<Operation>, fitted: &Fitted) {
        if !ops.is_empty() {
            self.session.name_step("Pixel-fit", icon::PIXEL_FIT);
            if !self.commit_edit(Transaction(ops)) {
                return;
            }
        }
        self.session.info(fitted.message());
    }

    /// The ops that fit `id`, or each layer inside it, gathered into `ops`.
    fn fit_layer(&self, id: NodeId, ops: &mut Vec<Operation>, fitted: &mut Fitted) {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let Some(node) = doc.get(id) else { return };
        if !node.visible() {
            return;
        }
        let kind = res.used_kind_of(node);
        match kind {
            NodeKind::Group | NodeKind::Artboard { .. } | NodeKind::Boolean { .. } => {
                for child in node.children().to_vec() {
                    self.fit_layer(child, ops, fitted);
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
                if let Some(tx) = self.fit_path(id, None, fitted) {
                    ops.extend(tx.0);
                }
            }
            // An arc's ends are a line's: the origin and its one point.
            NodeKind::Line { end } | NodeKind::Arc { end, .. } => {
                let off = grid_offset(self, id);
                let (from, to) = (world * Point::ZERO, world * *end);
                let (a, b) = (
                    Point::new(snap(from.x, off), snap(from.y, off)),
                    Point::new(snap(to.x, off), snap(to.y, off)),
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
                let Some(local) = ondin_core::local_box(doc, res, id) else {
                    return;
                };
                let off = grid_offset(self, id);
                let was = world.transform_rect_bbox(local);
                let now = fit_rect(was, off);
                // Each corner of the box that moved counts as a point.
                let corners = |r: Rect| {
                    [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)].map(Point::from)
                };
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

    /// The path `id` with its anchors (those in `only`, when given) fitted, or
    /// `None` when nothing moved.
    fn fit_path(
        &self,
        id: NodeId,
        only: Option<&[PointRef]>,
        fitted: &mut Fitted,
    ) -> Option<Transaction> {
        let (world, mut subpaths) = self.edited_subpaths(id)?;
        if !square_to_the_page(world) {
            fitted.turned += 1;
            return None;
        }
        let moved = fit_anchors(&mut subpaths, only, grid_offset(self, id));
        if moved == 0 {
            return None;
        }
        fitted.moved += moved;
        Some(self.rewrite_edit(id, world, subpaths))
    }
}

/// The ops that put a box shape's world box at `now`, its axes kept: a new size
/// along each local axis and the translation that lands the box's corner.
fn box_ops(
    doc: &ondin_core::Document,
    res: &ondin_core::Resolved,
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
            geometry: GeometryPatch::Size(ondin_core::kurbo::Size::new(w, h)),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::PenAnchor;

    /// A corner moves on both axes to the nearest whole unit; with an odd centred
    /// stroke, to the half unit.
    #[test]
    fn a_corner_lands_on_the_grid() {
        let mut subs = vec![PenSubpath {
            anchors: vec![PenAnchor::corner(Point::new(3.3, 4.6))],
            closed: false,
        }];
        assert_eq!(fit_anchors(&mut subs, None, 0.0), 1);
        assert_eq!(subs[0].anchors[0].at, Point::new(3.0, 5.0));
        // Both coordinates tie at the half unit, and both go down.
        assert_eq!(fit_anchors(&mut subs, None, 0.5), 1);
        assert_eq!(subs[0].anchors[0].at, Point::new(2.5, 4.5));
        assert_eq!(fit_anchors(&mut subs, None, 0.5), 0, "already there");
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
        assert_eq!(fit_anchors(&mut subs, None, 0.0), 1);
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
        assert_eq!(fit_anchors(&mut subs, Some(&[(0, 1)]), 0.0), 1);
        assert_eq!(subs[0].anchors[0].at, Point::new(0.4, 0.4));
        assert_eq!(subs[0].anchors[1].at, Point::new(5.0, 0.0));
    }

    /// A box's edges round, never more than half a unit, and a box at least one
    /// unit wide stays at least one unit wide.
    #[test]
    fn a_box_rounds_its_edges_and_keeps_a_unit() {
        let r = fit_rect(Rect::new(3.2, 4.0, 20.7, 4.9), 0.0);
        assert_eq!(r, Rect::new(3.0, 4.0, 21.0, 5.0));
        let thin = fit_rect(Rect::new(3.4, 0.0, 4.45, 1.0), 0.0);
        assert_eq!(thin.width(), 1.0, "{thin:?}");
        // A 1-wide centred stroke on whole edges: each edge ties and goes outward,
        // so the stroke's two edges land on 3 and 4, and on 20 and 21.
        let half = fit_rect(Rect::new(4.0, 4.0, 20.0, 20.0), 0.5);
        assert_eq!(half, Rect::new(3.5, 3.5, 20.5, 20.5));
    }

    /// Over a selection of a stroked rect, a path, a turned rect and a text layer,
    /// one *Pixel-fit* step fits the rect's box to the half unit (its stroke is 1
    /// wide and centred), the path's corners to whole units, and skips and names
    /// the other two.
    /// flip: `fit_layer` without its `square_to_the_page` gate → fails at "the
    /// turned rect is left alone".
    #[test]
    fn pixel_fit_fits_the_selection_in_one_step() {
        use ondin_core::kurbo::{BezPath, Size};
        use ondin_core::{Brush, Stroke, keyed_by_position};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let page = app.session.doc.first_page();
        let [rect, path, turned, text] = [(); 4].map(|_| app.session.ids.mint());
        let shape = |id, index, at: (f64, f64), rotate: f64| Operation::CreateNode {
            id,
            parent: page,
            index,
            kind: NodeKind::Rect {
                size: Size::new(10.4, 9.7),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate(at) * Affine::rotate(rotate)),
            name: None,
        };
        let mut line = BezPath::new();
        line.move_to((0.4, 0.4));
        line.line_to((6.6, 0.4));
        line.line_to((6.6, 5.2));
        let tx = Transaction(vec![
            shape(rect, 0, (3.3, 4.6), 0.0),
            Operation::SetStrokes {
                id: rect,
                strokes: keyed_by_position([Stroke {
                    brush: Brush::Solid(ondin_core::peniko::Color::BLACK).into(),
                    width: 1.0,
                    ..Default::default()
                }]),
            },
            Operation::CreateNode {
                id: path,
                parent: page,
                index: 1,
                kind: NodeKind::Path {
                    path: line,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: ondin_core::symmetry::PathMirror::NONE,
                },
                transform: Some(Affine::translate((40.0, 40.0))),
                name: None,
            },
            shape(turned, 2, (80.3, 80.3), 0.3),
            Operation::CreateNode {
                id: text,
                parent: page,
                index: 3,
                kind: NodeKind::Text {
                    content: "Hi".into(),
                    style: Box::default(),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing: ondin_core::TextSizing::Auto,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: Some(Affine::translate((0.3, 90.2))),
                name: None,
            },
        ]);
        assert!(app.session.commit(tx));
        let turned_before = app.session.resolved.world_transform(turned);
        app.session.selection.set(vec![rect, path, turned, text]);
        let depth = app.session.history.undo_depth();
        app.pixel_fit();

        let res = &app.session.resolved;
        let world = |id| {
            let local = ondin_core::local_box(&app.session.doc, res, id).expect("a box");
            res.world_transform(id)
                .expect("placed")
                .transform_rect_bbox(local)
        };
        assert_eq!(
            world(rect),
            Rect::new(3.5, 4.5, 13.5, 14.5),
            "the half unit"
        );
        let (_, subs) = app.edited_subpaths(path).expect("a path");
        let at: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(
            at,
            [
                Point::new(40.0, 40.0),
                Point::new(47.0, 40.0),
                Point::new(47.0, 45.0)
            ]
        );
        assert_eq!(
            res.world_transform(turned),
            turned_before,
            "the turned rect is left alone"
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        assert_eq!(
            app.session.status().text,
            "Moved 7 points onto the pixel grid; skipped 1 text layer and 1 rotated layer"
        );
    }

    /// The status line names what moved and what was skipped.
    #[test]
    fn the_status_line_says_what_happened() {
        let say = |moved, text, turned| {
            Fitted {
                moved,
                text,
                turned,
            }
            .message()
        };
        assert_eq!(say(0, 0, 0), "Already on the pixel grid");
        assert_eq!(say(14, 0, 0), "Moved 14 points onto the pixel grid");
        assert_eq!(say(0, 2, 0), "Pixel-fit skipped 2 text layers");
        assert_eq!(say(0, 0, 1), "Pixel-fit skipped 1 rotated layer");
        assert_eq!(
            say(4, 1, 1),
            "Moved 4 points onto the pixel grid; skipped 1 text layer and 1 rotated layer"
        );
    }
}
