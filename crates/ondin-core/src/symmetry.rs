//! A mirrored path: a `NodeKind::Path` that says which of its own axes it is
//! symmetric about, so point editing can keep it so.
//!
//! The axes' positions are not stored. A shape symmetric about a vertical line
//! has its points' bounds centred on that line, so the axis is the centre of the
//! path's control box ([`centre`]) and cannot drift from the points. The flags
//! are kept only while they hold ([`kept`]): any write that leaves the path
//! asymmetric drops them, and one that moves or scales it carries them along.

use crate::geometry::anchor_runs;
use kurbo::{BezPath, PathSeg, Point};
use serde::{Deserialize, Serialize};

/// Which of a path's own axes it is mirrored across: the vertical line through
/// its centre, the horizontal one, or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PathMirror {
    #[serde(default, skip_serializing_if = "is_false")]
    pub vertical: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub horizontal: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl PathMirror {
    /// No axis: a plain path.
    pub const NONE: Self = Self {
        vertical: false,
        horizontal: false,
    };

    /// Whether the path is mirrored across neither axis.
    pub fn is_none(&self) -> bool {
        !self.vertical && !self.horizontal
    }
}

/// Where a path's axes cross, in its own space: the centre of its control box.
pub fn centre(path: &BezPath) -> Point {
    path.control_box().center()
}

/// `mirror` less any axis `path` is no longer symmetric about, with `radii`
/// (indexed as `NodeKind::Path::corner_radii`) mirrored too.
pub fn kept(path: &BezPath, radii: &[f64], mirror: PathMirror) -> PathMirror {
    if mirror.is_none() {
        return mirror;
    }
    let c = centre(path);
    PathMirror {
        vertical: mirror.vertical && holds(path, radii, true, c.x),
        horizontal: mirror.horizontal && holds(path, radii, false, c.y),
    }
}

/// The reflection of `p` across the vertical line `x = c` or the horizontal `y = c`.
pub fn reflect(p: Point, vertical: bool, c: f64) -> Point {
    match vertical {
        true => Point::new(2.0 * c - p.x, p.y),
        false => Point::new(p.x, 2.0 * c - p.y),
    }
}

/// Whether every segment of `path` has its reflection in the path, either way
/// round, and every anchor's radius is its image's.
fn holds(path: &BezPath, radii: &[f64], vertical: bool, c: f64) -> bool {
    let b = path.control_box();
    let tol = 1e-6 * (1.0 + b.width().max(b.height()));
    let runs = anchor_runs(path);
    let segs: Vec<Vec<Point>> = runs
        .iter()
        .flat_map(|r| r.segs.iter().map(seg_points))
        .collect();
    let images: Vec<Vec<Point>> = segs
        .iter()
        .map(|s| s.iter().map(|p| reflect(*p, vertical, c)).collect())
        .collect();
    if !matches_all(&segs, &images, tol, |a, b| same_seg(a, b, tol)) {
        return false;
    }
    // A lone anchor has no segment to carry it.
    let mut anchors = Vec::new();
    let mut k = 0;
    for run in &runs {
        for p in &run.points {
            anchors.push(vec![
                *p,
                Point::new(radii.get(k).copied().unwrap_or(0.0), 0.0),
            ]);
            k += 1;
        }
    }
    let mirrored: Vec<Vec<Point>> = anchors
        .iter()
        .map(|a| vec![reflect(a[0], vertical, c), a[1]])
        .collect();
    matches_all(&anchors, &mirrored, tol, |a, b| {
        (a[0] - b[0]).hypot() <= tol && (a[1].x - b[1].x).abs() <= tol
    })
}

/// A segment's points, anchor to anchor.
fn seg_points(s: &PathSeg) -> Vec<Point> {
    match *s {
        PathSeg::Line(l) => vec![l.p0, l.p1],
        PathSeg::Quad(q) => vec![q.p0, q.p1, q.p2],
        PathSeg::Cubic(c) => vec![c.p0, c.p1, c.p2, c.p3],
    }
}

/// Whether two segments are one curve, either way round.
fn same_seg(a: &[Point], b: &[Point], tol: f64) -> bool {
    let near = |p: &Point, q: &Point| (*p - *q).hypot() <= tol;
    a.len() == b.len()
        && (a.iter().zip(b).all(|(p, q)| near(p, q))
            || a.iter().zip(b.iter().rev()).all(|(p, q)| near(p, q)))
}

/// Whether `items` and `images` pair off one to one under `same`, matched by
/// their centroids (which neither reversal nor order changes) within `tol`.
fn matches_all(
    items: &[Vec<Point>],
    images: &[Vec<Point>],
    tol: f64,
    same: impl Fn(&[Point], &[Point]) -> bool,
) -> bool {
    if items.len() != images.len() {
        return false;
    }
    let centroid = |s: &[Point]| {
        let n = s.len().max(1) as f64;
        let sum = s.iter().fold(Point::ZERO, |acc, p| acc + p.to_vec2());
        Point::new(sum.x / n, sum.y / n)
    };
    let mut pool: Vec<(Point, usize)> = images
        .iter()
        .enumerate()
        .map(|(i, s)| (centroid(s), i))
        .collect();
    pool.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    let mut used = vec![false; pool.len()];
    for item in items {
        let at = centroid(item);
        let lo = pool.partition_point(|(p, _)| p.x < at.x - tol);
        let found = (lo..pool.len())
            .take_while(|&j| pool[j].0.x <= at.x + tol)
            .find(|&j| {
                !used[j] && (pool[j].0.y - at.y).abs() <= tol && same(item, &images[pool[j].1])
            });
        match found {
            Some(j) => used[j] = true,
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Affine;

    const BOTH: PathMirror = PathMirror {
        vertical: true,
        horizontal: true,
    };

    /// A heart-ish closed run, symmetric about x = 50 only.
    fn heart() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((50.0, 20.0));
        p.curve_to((60.0, 0.0), (90.0, 10.0), (80.0, 40.0));
        p.line_to((50.0, 90.0));
        p.line_to((20.0, 40.0));
        p.curve_to((10.0, 10.0), (40.0, 0.0), (50.0, 20.0));
        p.close_path();
        p
    }

    /// The heart keeps its vertical axis and loses the horizontal one it never had.
    #[test]
    fn a_symmetric_path_keeps_the_axis_it_is_symmetric_about() {
        assert_eq!(
            kept(&heart(), &[], BOTH),
            PathMirror {
                vertical: true,
                horizontal: false
            }
        );
    }

    /// Moving one anchor off its image's place drops the axis.
    #[test]
    fn an_asymmetric_edit_drops_the_axis() {
        let mut p = heart();
        if let Some(kurbo::PathEl::LineTo(q)) = p.elements_mut().get_mut(2) {
            q.x += 4.0;
        }
        assert!(kept(&p, &[], BOTH).is_none());
    }

    /// A scale baked into the points keeps the axis: it is wherever the centre
    /// went.
    #[test]
    fn a_baked_scale_keeps_the_axis() {
        let mut p = heart();
        p.apply_affine(Affine::scale_non_uniform(2.5, 0.5) * Affine::translate((7.0, 3.0)));
        assert!(kept(&p, &[], BOTH).vertical);
    }

    /// One rounded corner without its image's radius breaks the symmetry.
    #[test]
    fn radii_must_mirror_too() {
        // Anchors: (50,20) (80,40) (50,90) (20,40).
        assert!(kept(&heart(), &[0.0, 6.0, 0.0, 6.0], BOTH).vertical);
        assert!(!kept(&heart(), &[0.0, 6.0, 0.0, 0.0], BOTH).vertical);
    }
}
