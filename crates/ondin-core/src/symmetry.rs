//! A mirrored path: a `NodeKind::Path` that says which of its own axes it is
//! symmetric about, so point editing can keep it so.
//!
//! The axes' positions are not stored. A shape symmetric about a vertical line
//! has its points' bounds centred on that line, so the axis is the centre of the
//! path's control box ([`centre`]) and cannot drift from the points. The flags
//! are kept only while they hold ([`kept`]): any write that leaves the path
//! asymmetric drops them, and one that moves or scales it carries them along.

use crate::edit::{PenAnchor, PenSubpath};
use crate::geometry::anchor_runs;
use kurbo::{BezPath, PathSeg, Point, Vec2};
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

// --- a mirrored path under point editing -----------------------------------

/// A handle offset reflected across a vertical or horizontal axis.
fn flip(v: Vec2, vertical: bool) -> Vec2 {
    match vertical {
        true => Vec2::new(-v.x, v.y),
        false => Vec2::new(v.x, -v.y),
    }
}

/// `a`'s image across the axis `x = c` (or `y = c`): `swap` when the run passes
/// its image the other way round, so the image's arriving handle is the image of
/// `a`'s leaving one.
fn image_of(a: &PenAnchor, vertical: bool, c: f64, swap: bool) -> PenAnchor {
    let (out, back) = match swap {
        true => (flip(a.back, vertical), flip(a.out, vertical)),
        false => (flip(a.out, vertical), flip(a.back, vertical)),
    };
    PenAnchor {
        at: reflect(a.at, vertical, c),
        out,
        back,
        radius: a.radius,
    }
}

/// Each anchor's image across one axis among `subs`, by flat index, with
/// whether the run passes it the other way round; `None` where no anchor sits
/// at the image.
///
/// The way round comes from the neighbours, not the handles: a corner has none
/// to tell by, and pulling one out of it must still land on the right side.
pub fn partners(
    subs: &[PenSubpath],
    vertical: bool,
    c: f64,
    tol: f64,
) -> Vec<Option<(usize, bool)>> {
    let flat: Vec<&PenAnchor> = subs.iter().flat_map(|s| &s.anchors).collect();
    // Each anchor's neighbours along its run, by flat index.
    let mut prev = Vec::with_capacity(flat.len());
    let mut next = Vec::with_capacity(flat.len());
    let mut start = 0;
    for s in subs {
        let n = s.anchors.len();
        for i in 0..n {
            let wrap = |j: usize| start + j % n;
            prev.push(match (i, s.closed) {
                (0, false) => None,
                (0, true) => Some(wrap(n - 1)),
                _ => Some(start + i - 1),
            });
            next.push(match (i + 1 == n, s.closed) {
                (true, false) => None,
                _ => Some(wrap(i + 1)),
            });
        }
        start += n;
    }
    let at: Vec<Option<usize>> = flat
        .iter()
        .map(|a| {
            let want = reflect(a.at, vertical, c);
            flat.iter().position(|b| (b.at - want).hypot() <= tol)
        })
        .collect();
    (0..flat.len())
        .map(|a| {
            let p = at[a]?;
            let image = |j: Option<usize>| j.and_then(|j| at[j]);
            let swap = match (next[a], prev[a]) {
                (Some(_), _) => image(next[a]).is_some() && image(next[a]) == prev[p],
                (None, Some(_)) => image(prev[a]).is_some() && image(prev[a]) == next[p],
                (None, None) => false,
            };
            Some((p, swap))
        })
        .collect()
}

/// Whether point editing moved, reshaped or re-rounded anchor `a` into `b`.
fn changed(a: &PenAnchor, b: &PenAnchor, eps: f64) -> bool {
    (a.at - b.at).hypot() > eps
        || (a.out - b.out).hypot() > eps
        || (a.back - b.back).hypot() > eps
        || (a.radius - b.radius).abs() > eps
}

/// `edited`, made symmetric again across `mirror`'s axes through `at` after a
/// point edit that started from the symmetric `base`, both in the path's own
/// space. Where an anchor changed, its image takes the reflection of it; where
/// both did, the first in order wins. An anchor on an axis stays on it, its two
/// handles each other's image. An edit that changed how many anchors there are
/// is left alone: the model drops the axis if it no longer holds.
pub fn symmetric_edit(
    base: &[PenSubpath],
    edited: &mut [PenSubpath],
    mirror: PathMirror,
    at: Point,
) {
    let shape = |s: &[PenSubpath]| s.iter().map(|s| s.anchors.len()).collect::<Vec<_>>();
    if shape(base) != shape(edited) {
        return;
    }
    let size = base
        .iter()
        .flat_map(|s| &s.anchors)
        .map(|a| (a.at - at).hypot())
        .fold(0.0, f64::max);
    let tol = 1e-6 * (1.0 + size);
    let flat_base: Vec<PenAnchor> = base.iter().flat_map(|s| s.anchors.clone()).collect();
    for (vertical, c, on) in [
        (true, at.x, mirror.vertical),
        (false, at.y, mirror.horizontal),
    ] {
        if !on {
            continue;
        }
        let pairs = partners(base, vertical, c, tol);
        let mut flat: Vec<PenAnchor> = edited.iter().flat_map(|s| s.anchors.clone()).collect();
        let mut done = vec![false; flat.len()];
        for a in 0..flat.len() {
            let Some((p, swap)) = pairs[a] else { continue };
            if done[a] {
                continue;
            }
            done[a] = true;
            done[p] = true;
            let (ca, cp) = (
                changed(&flat_base[a], &flat[a], tol),
                changed(&flat_base[p], &flat[p], tol),
            );
            if a == p {
                if ca {
                    flat[a] = on_axis(&flat_base[a], &flat[a], vertical, c, swap, tol);
                }
            } else if ca {
                flat[p] = image_of(&flat[a], vertical, c, swap);
            } else if cp {
                flat[a] = image_of(&flat[p], vertical, c, swap);
            }
        }
        let mut it = flat.into_iter();
        for sub in edited.iter_mut() {
            for a in &mut sub.anchors {
                *a = it.next().expect("one anchor per anchor");
            }
        }
    }
}

/// An anchor on the axis after an edit: pulled back onto it, so it slides along
/// it, with the handle the edit moved deciding the other.
fn on_axis(
    was: &PenAnchor,
    now: &PenAnchor,
    vertical: bool,
    c: f64,
    swap: bool,
    tol: f64,
) -> PenAnchor {
    let mut a = *now;
    match vertical {
        true => a.at.x = c,
        false => a.at.y = c,
    }
    // A run crossing the axis meets its image here: each handle is the other's.
    // One running along the axis keeps both handles on it.
    match swap {
        true if (now.back - was.back).hypot() > tol && (now.out - was.out).hypot() <= tol => {
            a.out = flip(a.back, vertical);
        }
        true => a.back = flip(a.out, vertical),
        false => {
            let along = |v: Vec2| match vertical {
                true => Vec2::new(0.0, v.y),
                false => Vec2::new(v.x, 0.0),
            };
            a.out = along(a.out);
            a.back = along(a.back);
        }
    }
    a
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

    const VERTICAL: PathMirror = PathMirror {
        vertical: true,
        horizontal: false,
    };

    /// An open run of corners through `points`.
    fn run(points: &[(f64, f64)]) -> PenSubpath {
        PenSubpath {
            anchors: points
                .iter()
                .map(|&(x, y)| PenAnchor::corner(Point::new(x, y)))
                .collect(),
            closed: false,
        }
    }

    /// A closed diamond about x = 0, clockwise from the top: top and bottom on the
    /// axis, right and left each other's image.
    fn diamond() -> Vec<PenSubpath> {
        let mut d = run(&[(0.0, -10.0), (10.0, 0.0), (0.0, 10.0), (-10.0, 0.0)]);
        d.closed = true;
        vec![d]
    }

    /// Whether the model would keep the vertical axis on `subs`.
    fn holds(subs: &[PenSubpath]) -> bool {
        let path = crate::edit::pen_path(subs);
        kept(&path, &crate::edit::pen_radii(subs), VERTICAL).vertical
    }

    /// A dragged anchor takes its image with it, mirrored.
    /// flip: `symmetric_edit` returning early → fails at "the image followed".
    #[test]
    fn a_moved_anchor_moves_its_image() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[1].at = Point::new(14.0, 2.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(
            edited[0].anchors[3].at,
            Point::new(-14.0, 2.0),
            "the image followed"
        );
        assert!(holds(&edited));
    }

    /// The image's own edit wins when only it moved.
    #[test]
    fn an_edit_on_the_image_side_carries_back() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[3].radius = 4.0;
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(edited[0].anchors[1].radius, 4.0);
        assert!(holds(&edited));
    }

    /// An anchor on the axis is locked to it: dragged off, it slides along it.
    /// flip: `on_axis` keeping `now.at` → fails at "still on the axis".
    #[test]
    fn an_axis_anchor_slides_along_the_axis() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[0].at = Point::new(3.0, -12.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(
            edited[0].anchors[0].at,
            Point::new(0.0, -12.0),
            "still on the axis"
        );
        assert!(holds(&edited));
    }

    /// A handle pulled out of a corner lands on its image's matching side, which
    /// the neighbours decide since a corner has no handle to tell by; on the axis,
    /// the other handle becomes its image.
    /// flip: `partners` answering `swap` false always → fails at the handle assertion.
    #[test]
    fn a_handle_pulled_from_a_corner_mirrors_onto_the_right_side() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[1].out = Vec2::new(0.0, 5.0);
        edited[0].anchors[0].out = Vec2::new(6.0, 0.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(edited[0].anchors[3].back, Vec2::new(0.0, 5.0));
        assert_eq!(edited[0].anchors[3].out, Vec2::ZERO);
        assert_eq!(edited[0].anchors[0].back, Vec2::new(-6.0, 0.0));
        assert!(holds(&edited));
    }

    /// Both axes: one moved anchor takes all three images with it.
    #[test]
    fn both_axes_carry_an_edit_to_all_four_quarters() {
        let mut sq = run(&[(10.0, 10.0), (-10.0, 10.0), (-10.0, -10.0), (10.0, -10.0)]);
        sq.closed = true;
        let base = vec![sq];
        let mut edited = base.clone();
        edited[0].anchors[0].at = Point::new(12.0, 15.0);
        symmetric_edit(&base, &mut edited, BOTH, Point::ZERO);
        let ats: Vec<Point> = edited[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(
            ats,
            [(12.0, 15.0), (-12.0, 15.0), (-12.0, -15.0), (12.0, -15.0)]
                .map(|(x, y)| Point::new(x, y))
        );
    }
}
