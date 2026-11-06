//! *Trace image*: a picture's pixels to filled paths.
//!
//! Three steps, each its own function so a card can show the middle one:
//! [`label`] sorts every pixel into a colour of a small palette, or none
//! (transparent, or light in black and white), and merges specks smaller than the
//! setting into the colour around them; [`trace`] follows the edges between
//! pixels of each colour into closed loops, pairs each outer loop with the holes
//! inside it, and fits the loops with curves (`crate::fit`). Paths are in pixel
//! space, `(0, 0)` the raster's top-left corner, one unit a pixel.
//!
//! Outer loops run clockwise on screen and holes the other way, so the paths fill
//! correctly under the non-zero rule.

use crate::kurbo::{BezPath, PathEl, Point};
use crate::peniko::Color;

/// A label meaning "nothing here": transparent, or light in black and white.
pub const EMPTY: u16 = u16::MAX;

/// The most colours a trace keeps.
pub const MAX_COLOURS: usize = 16;

/// How the picture's colours are read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Pixels darker than `threshold` (`0..=1` of the luminance) become black.
    BlackAndWhite { threshold: f64 },
    /// The picture is cut down to `count` colours.
    Colours { count: usize },
}

/// What the card's settings ask of the trace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub mode: Mode,
    /// `0..=1`: how closely the curves follow the pixels.
    pub detail: f64,
    /// `0..=1`: how readily a turn stays a corner rather than a curve.
    pub corners: f64,
    /// Regions narrower than this many pixels, measured as the side of a square of
    /// the same area, are merged into what surrounds them.
    pub speck: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::BlackAndWhite { threshold: 0.5 },
            detail: 0.6,
            corners: 0.5,
            speck: 4.0,
        }
    }
}

impl Settings {
    /// How far a fitted curve may stray from the pixel edge, in pixels.
    fn accuracy(&self) -> f64 {
        let loose = 1.0 - self.detail.clamp(0.0, 1.0);
        0.35 + 2.5 * loose * loose
    }

    /// The turn, in degrees, past which a point stays a corner.
    fn corner_deg(&self) -> f64 {
        120.0 - 100.0 * self.corners.clamp(0.0, 1.0)
    }
}

/// Straight-alpha RGBA8 pixels, row by row.
#[derive(Clone, Copy, Debug)]
pub struct Bitmap<'a> {
    pub rgba: &'a [u8],
    pub width: usize,
    pub height: usize,
}

/// Every pixel's colour, an index into `palette` or [`EMPTY`].
#[derive(Clone, Debug, PartialEq)]
pub struct Labels {
    pub width: usize,
    pub height: usize,
    pub index: Vec<u16>,
    /// Darkest first.
    pub palette: Vec<Color>,
}

/// One colour's paths.
#[derive(Clone, Debug)]
pub struct Layer {
    pub colour: Color,
    /// One per outer loop, its holes inside it.
    pub paths: Vec<BezPath>,
}

/// A whole trace: the colours' layers, darkest first.
#[derive(Clone, Debug, Default)]
pub struct Traced {
    pub layers: Vec<Layer>,
}

impl Traced {
    /// How many paths it makes.
    pub fn paths(&self) -> usize {
        self.layers.iter().map(|l| l.paths.len()).sum()
    }

    /// How many anchors its paths hold.
    pub fn points(&self) -> usize {
        self.layers
            .iter()
            .flat_map(|l| &l.paths)
            .flat_map(|p| p.elements())
            .filter(|e| !matches!(e, PathEl::ClosePath))
            .count()
    }
}

/// A pixel's luminance, `0..=1`, read on the sRGB values.
fn luma(p: [u8; 4]) -> f64 {
    (0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])) / 255.0
}

fn pixel(b: &Bitmap<'_>, i: usize) -> [u8; 4] {
    let p = &b.rgba[i * 4..i * 4 + 4];
    [p[0], p[1], p[2], p[3]]
}

/// Whether a pixel counts at all: more than half opaque.
fn opaque(p: [u8; 4]) -> bool {
    p[3] >= 128
}

/// Sort every pixel into a palette colour, then merge the specks.
pub fn label(b: Bitmap<'_>, s: &Settings) -> Labels {
    let n = b.width * b.height;
    let mut labels = match s.mode {
        Mode::BlackAndWhite { threshold } => Labels {
            width: b.width,
            height: b.height,
            index: (0..n)
                .map(|i| {
                    let p = pixel(&b, i);
                    if opaque(p) && luma(p) < threshold {
                        0
                    } else {
                        EMPTY
                    }
                })
                .collect(),
            palette: vec![Color::BLACK],
        },
        Mode::Colours { count } => quantized(b, count.clamp(2, MAX_COLOURS)),
    };
    despeckle(&mut labels, s.speck);
    labels
}

/// The picture cut down to `count` colours: a median cut over the opaque pixels,
/// refined by a few rounds of nearest-mean, darkest first.
fn quantized(b: Bitmap<'_>, count: usize) -> Labels {
    let n = b.width * b.height;
    let opaque_px: Vec<[f64; 3]> = (0..n)
        .map(|i| pixel(&b, i))
        .filter(|p| opaque(*p))
        .map(|p| [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])])
        .collect();
    let mut means = median_cut(&opaque_px, count);
    let nearest = |c: [f64; 3], means: &[[f64; 3]]| {
        means
            .iter()
            .enumerate()
            .map(|(k, m)| (k, (0..3).map(|j| (c[j] - m[j]).powi(2)).sum::<f64>()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(0, |(k, _)| k)
    };
    for _ in 0..4 {
        let mut sums = vec![[0.0; 4]; means.len()];
        for c in &opaque_px {
            let k = nearest(*c, &means);
            for j in 0..3 {
                sums[k][j] += c[j];
            }
            sums[k][3] += 1.0;
        }
        for (m, s) in means.iter_mut().zip(&sums) {
            if s[3] > 0.0 {
                *m = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            }
        }
    }
    let lum = |m: &[f64; 3]| 0.2126 * m[0] + 0.7152 * m[1] + 0.0722 * m[2];
    means.sort_by(|a, b| lum(a).total_cmp(&lum(b)));
    means.dedup_by(|a, b| (0..3).all(|j| (a[j] - b[j]).abs() < 0.5));
    let index = (0..n)
        .map(|i| {
            let p = pixel(&b, i);
            if !opaque(p) {
                return EMPTY;
            }
            nearest([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])], &means) as u16
        })
        .collect();
    let palette = means
        .iter()
        .map(|m| {
            let c = |v: f64| v.round().clamp(0.0, 255.0) as u8;
            Color::from_rgb8(c(m[0]), c(m[1]), c(m[2]))
        })
        .collect();
    Labels {
        width: b.width,
        height: b.height,
        index,
        palette,
    }
}

/// Up to `count` mean colours of `px`, by splitting the box with the widest
/// channel at its median until there are enough boxes.
fn median_cut(px: &[[f64; 3]], count: usize) -> Vec<[f64; 3]> {
    if px.is_empty() {
        return vec![[0.0; 3]];
    }
    let mut boxes: Vec<Vec<[f64; 3]>> = vec![px.to_vec()];
    while boxes.len() < count {
        let spread = |b: &Vec<[f64; 3]>| {
            (0..3)
                .map(|j| {
                    let (lo, hi) = b.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| {
                        (lo.min(c[j]), hi.max(c[j]))
                    });
                    (j, hi - lo)
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap_or((0, 0.0))
        };
        let Some((i, (axis, _))) = boxes
            .iter()
            .enumerate()
            .map(|(i, b)| (i, spread(b)))
            .filter(|(_, b)| b.1 > 0.0)
            .max_by(|a, b| a.1.1.total_cmp(&b.1.1))
        else {
            break;
        };
        let mut b = boxes.swap_remove(i);
        b.sort_by(|p, q| p[axis].total_cmp(&q[axis]));
        let upper = b.split_off(b.len() / 2);
        boxes.push(b);
        boxes.push(upper);
    }
    boxes
        .iter()
        .filter(|b| !b.is_empty())
        .map(|b| {
            let n = b.len() as f64;
            let s = b
                .iter()
                .fold([0.0; 3], |s, c| [s[0] + c[0], s[1] + c[1], s[2] + c[2]]);
            [s[0] / n, s[1] / n, s[2] / n]
        })
        .collect()
}

/// Merge every region of fewer than `speck²` pixels into the label it shares the
/// most edge with, the smallest first so a speck in a speck goes with its host.
/// An empty region inside a colour counts too: small holes close.
fn despeckle(l: &mut Labels, speck: f64) {
    let min = (speck.max(0.0) * speck.max(0.0)).ceil() as usize;
    if min <= 1 {
        return;
    }
    let (w, h) = (l.width, l.height);
    let mut region = vec![usize::MAX; w * h];
    let mut regions: Vec<Vec<usize>> = Vec::new();
    for start in 0..w * h {
        if region[start] != usize::MAX {
            continue;
        }
        let r = regions.len();
        let mut members = vec![start];
        region[start] = r;
        let mut k = 0;
        while k < members.len() {
            let i = members[k];
            k += 1;
            for j in neighbours(i, w, h) {
                if region[j] == usize::MAX && l.index[j] == l.index[start] {
                    region[j] = r;
                    members.push(j);
                }
            }
        }
        regions.push(members);
    }
    let mut order: Vec<usize> = (0..regions.len())
        .filter(|r| regions[*r].len() < min)
        .collect();
    order.sort_by_key(|r| regions[*r].len());
    for r in order {
        let own = l.index[regions[r][0]];
        let mut votes: Vec<(u16, usize)> = Vec::new();
        for &i in &regions[r] {
            for j in neighbours(i, w, h) {
                let v = l.index[j];
                if v != own {
                    match votes.iter_mut().find(|(x, _)| *x == v) {
                        Some((_, n)) => *n += 1,
                        None => votes.push((v, 1)),
                    }
                }
            }
        }
        if let Some((v, _)) = votes.into_iter().max_by_key(|(_, n)| *n) {
            for &i in &regions[r] {
                l.index[i] = v;
            }
        }
    }
}

/// The four pixels beside pixel `i`, inside the raster.
fn neighbours(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (i % w, i / w);
    [
        (x > 0).then(|| i - 1),
        (x + 1 < w).then(|| i + 1),
        (y > 0).then(|| i - w),
        (y + 1 < h).then(|| i + w),
    ]
    .into_iter()
    .flatten()
}

/// Every colour's loops, fitted and paired into paths.
pub fn trace(l: &Labels, s: &Settings) -> Traced {
    let layers = (0..l.palette.len())
        .map(|k| Layer {
            colour: l.palette[k],
            paths: paths_of(&loops(l, k as u16), s),
        })
        .filter(|layer| !layer.paths.is_empty())
        .collect();
    Traced { layers }
}

/// One closed loop's lattice points, in order.
type Loop = Vec<(i32, i32)>;

/// The closed loops round the pixels labelled `k`, on the pixel-corner lattice,
/// clockwise on screen round the colour and anticlockwise round its holes. Where
/// two pixels of the colour touch only at a corner, the loop turns right and
/// keeps them apart.
fn loops(l: &Labels, k: u16) -> Vec<Loop> {
    let (w, h) = (l.width as i32, l.height as i32);
    let inside =
        |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && l.index[(y * w + x) as usize] == k;
    // The edges leaving each lattice point, one bit a direction: +x, +y, −x, −y.
    let stride = (w + 1) as usize;
    let mut out = vec![0u8; stride * (h + 1) as usize];
    let at = |x: i32, y: i32| y as usize * stride + x as usize;
    for y in 0..h {
        for x in 0..w {
            if !inside(x, y) {
                continue;
            }
            if !inside(x, y - 1) {
                out[at(x, y)] |= 1;
            }
            if !inside(x + 1, y) {
                out[at(x + 1, y)] |= 2;
            }
            if !inside(x, y + 1) {
                out[at(x + 1, y + 1)] |= 4;
            }
            if !inside(x - 1, y) {
                out[at(x, y + 1)] |= 8;
            }
        }
    }
    const STEP: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    let mut loops = Vec::new();
    for sy in 0..=h {
        for sx in 0..=w {
            while out[at(sx, sy)] != 0 {
                let mut d = out[at(sx, sy)].trailing_zeros() as usize;
                let (mut x, mut y) = (sx, sy);
                let mut pts = Vec::new();
                loop {
                    pts.push((x, y));
                    out[at(x, y)] &= !(1 << d);
                    x += STEP[d].0;
                    y += STEP[d].1;
                    if (x, y) == (sx, sy) {
                        break;
                    }
                    let bits = out[at(x, y)];
                    let Some(next) = [(d + 1) % 4, d, (d + 3) % 4]
                        .into_iter()
                        .find(|n| bits & (1 << n) != 0)
                    else {
                        break;
                    };
                    d = next;
                }
                loops.push(pts);
            }
        }
    }
    loops
}

/// Twice the signed area a loop encloses; positive for clockwise on screen.
fn area2(pts: &[(i32, i32)]) -> i64 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            i64::from(a.0) * i64::from(b.1) - i64::from(b.0) * i64::from(a.1)
        })
        .sum()
}

/// Whether `p` is inside the loop, by the even-odd crossing rule.
fn contains(pts: &[(i32, i32)], p: (f64, f64)) -> bool {
    let n = pts.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let (ax, ay, bx, by) = (
            f64::from(a.0),
            f64::from(a.1),
            f64::from(b.0),
            f64::from(b.1),
        );
        if (ay > p.1) != (by > p.1) && p.0 < ax + (p.1 - ay) * (bx - ax) / (by - ay) {
            inside = !inside;
        }
    }
    inside
}

/// The loops as paths: each outer loop with the holes it holds, fitted.
fn paths_of(loops: &[Loop], s: &Settings) -> Vec<BezPath> {
    let (outer, holes): (Vec<&Loop>, Vec<&Loop>) = loops.iter().partition(|l| area2(l) > 0);
    let mut owned: Vec<Vec<&Loop>> = vec![Vec::new(); outer.len()];
    for hole in holes {
        // The middle of its first edge is on the hole, inside its outer loop.
        let (a, b) = (hole[0], hole[1 % hole.len()]);
        let p = (f64::from(a.0 + b.0) / 2.0, f64::from(a.1 + b.1) / 2.0);
        let host = outer
            .iter()
            .enumerate()
            .filter(|(_, o)| contains(o, p))
            .min_by_key(|(_, o)| area2(o))
            .map(|(i, _)| i);
        if let Some(i) = host {
            owned[i].push(hole);
        }
    }
    outer
        .iter()
        .zip(owned)
        .map(|(o, holes)| {
            let mut path = BezPath::new();
            for l in std::iter::once(*o).chain(holes) {
                for el in fitted(l, s).elements() {
                    path.push(*el);
                }
            }
            path
        })
        .filter(|p| !p.elements().is_empty())
        .collect()
}

/// One loop fitted with curves. The polygon through the middles of its pixel
/// edges stands in for the staircase, so a diagonal of pixels reads as a line.
fn fitted(l: &[(i32, i32)], s: &Settings) -> BezPath {
    let n = l.len();
    let mids: Vec<Point> = (0..n)
        .map(|i| {
            let (a, b) = (l[i], l[(i + 1) % n]);
            Point::new(f64::from(a.0 + b.0) / 2.0, f64::from(a.1 + b.1) / 2.0)
        })
        .collect();
    crate::fit::fit_closed(&mids, s.accuracy(), s.corner_deg())
}

/// The group a trace lands in.
pub const GROUP_NAME: &str = "Trace";

/// Why a picture cannot be traced where it is.
pub const INSIDE: &str = "Can't trace inside a boolean, a component or an instance";

/// Why a trace of `picture` cannot be placed where it stands: in a boolean, where
/// the group would be one more operand, or in a main or an instance.
pub fn refusal(doc: &crate::Document, picture: crate::NodeId) -> Option<&'static str> {
    let mut up = Some(picture);
    while let Some(n) = up.and_then(|id| doc.get(id)) {
        let boolean = matches!(n.kind(), crate::NodeKind::Boolean { .. }) && n.id() != picture;
        if boolean || n.component() || n.link().is_some() {
            return Some(INSIDE);
        }
        up = n.parent();
    }
    None
}

/// The edit that puts `traced` above the picture `picture`: a *Trace* group in
/// the picture's parent, just over it, holding one path per traced region (in a
/// group per colour when there are several colours), filled with its colour and
/// unstroked. `frame` is the world box the raster covered, at `scale` pixels a
/// unit. Without `keep`, the picture is deleted. Answers the edit and the group,
/// or `None` with nothing to place.
#[allow(clippy::too_many_arguments)]
pub fn place(
    doc: &crate::Document,
    res: &crate::Resolved,
    ids: &mut crate::IdSource,
    picture: crate::NodeId,
    traced: &Traced,
    frame: crate::kurbo::Rect,
    scale: f64,
    keep: bool,
) -> Result<(crate::Transaction, crate::NodeId), &'static str> {
    use crate::kurbo::{Affine, Vec2};
    use crate::node::NodeKind;
    use crate::{Brush, Fill, Operation, keyed_by_position};
    let node = doc.get(picture).ok_or("Nothing to trace")?;
    let parent = node.parent().ok_or("Nothing to trace")?;
    if let Some(why) = refusal(doc, picture) {
        return Err(why);
    }
    if traced.paths() == 0 {
        return Err("Nothing to trace: try another threshold or fewer specks ignored");
    }
    let at = doc
        .get(parent)
        .and_then(|p| p.children().iter().position(|c| *c == picture))
        .ok_or("Nothing to trace")?;
    let parent_world = res.world_transform(parent).unwrap_or(Affine::IDENTITY);
    let to_parent = parent_world.inverse()
        * Affine::translate(Vec2::new(frame.x0, frame.y0))
        * Affine::scale(1.0 / scale);

    let root = ids.mint();
    let mut scratch = crate::Document::new(root);
    let page = scratch.first_page();
    let group = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: group,
        parent: page,
        index: 0,
        kind: NodeKind::Group,
        transform: None,
        name: Some(GROUP_NAME.into()),
    }];
    let nested = traced.layers.len() > 1;
    for (k, layer) in traced.layers.iter().enumerate() {
        let host = if nested {
            let sub = ids.mint();
            let [r, g, b, _] = layer.colour.to_rgba8().to_u8_array();
            ops.push(Operation::CreateNode {
                id: sub,
                parent: group,
                index: k,
                kind: NodeKind::Group,
                transform: None,
                name: Some(format!("#{r:02X}{g:02X}{b:02X}")),
            });
            sub
        } else {
            group
        };
        for (i, path) in layer.paths.iter().enumerate() {
            let id = ids.mint();
            ops.push(Operation::CreateNode {
                id,
                parent: host,
                index: i,
                kind: NodeKind::Path {
                    path: to_parent * path.clone(),
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: crate::symmetry::PathMirror::NONE,
                },
                transform: None,
                name: None,
            });
            ops.push(Operation::SetFills {
                id,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(layer.colour).into(),
                    visible: true,
                }]),
            });
        }
    }
    scratch
        .apply(&crate::Transaction(ops))
        .map_err(|_| "Nothing to trace")?;
    let nodes = scratch.capture_subtree(group).ok_or("Nothing to trace")?;
    let mut ops = vec![Operation::InsertSubtree {
        nodes,
        parent,
        index: at + 1,
    }];
    if !keep {
        ops.push(Operation::DeleteNode { id: picture });
    }
    Ok((crate::Transaction(ops), group))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kurbo::Shape;

    /// A `w` by `h` raster, each pixel the colour `at` gives it.
    fn raster(w: usize, h: usize, at: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| at(x, y))
            .collect()
    }

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const RED: [u8; 4] = [220, 30, 30, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    fn bw(px: &[u8], w: usize, h: usize, s: &Settings) -> Traced {
        let l = label(
            Bitmap {
                rgba: px,
                width: w,
                height: h,
            },
            s,
        );
        trace(&l, s)
    }

    /// A black ring on white traces to one path, its outer edge and its hole, whose
    /// filled area is the ring's pixels; a ring on transparency traces the same.
    #[test]
    fn a_ring_is_one_path_with_its_hole() {
        for ground in [WHITE, CLEAR] {
            let px = raster(40, 40, |x, y| {
                let inner = (12..28).contains(&x) && (12..28).contains(&y);
                let outer = (6..34).contains(&x) && (6..34).contains(&y);
                if outer && !inner { BLACK } else { ground }
            });
            let t = bw(&px, 40, 40, &Settings::default());
            assert_eq!(t.paths(), 1, "{ground:?}");
            let path = &t.layers[0].paths[0];
            let subpaths = path
                .elements()
                .iter()
                .filter(|e| matches!(e, PathEl::MoveTo(_)))
                .count();
            assert_eq!(subpaths, 2, "the outer edge and the hole");
            let area = path.area().abs();
            let want = (28 * 28 - 16 * 16) as f64;
            assert!((area - want).abs() < 0.04 * want, "{area} against {want}");
            let b = path.bounding_box();
            assert!(
                (b.x0 - 6.0).abs() < 1.0 && (b.x1 - 34.0).abs() < 1.0,
                "{b:?}"
            );
        }
    }

    /// A square keeps its four corners at the default settings: the fit has few
    /// anchors, and the outline reaches each corner closely.
    #[test]
    fn a_square_keeps_its_corners() {
        let px = raster(30, 30, |x, y| {
            if (5..25).contains(&x) && (5..25).contains(&y) {
                BLACK
            } else {
                WHITE
            }
        });
        let t = bw(&px, 30, 30, &Settings::default());
        assert_eq!(t.paths(), 1);
        assert!(t.points() <= 10, "{} points", t.points());
        let path = &t.layers[0].paths[0];
        assert!(
            path.contains(Point::new(5.6, 5.6)),
            "the corner stays square"
        );
    }

    /// A speck under the setting vanishes, and so does a pinhole in a shape; with
    /// the setting at zero both stay.
    #[test]
    fn specks_are_merged_into_their_surroundings() {
        let px = raster(40, 20, |x, y| {
            let speck = (30..32).contains(&x) && (5..7).contains(&y);
            let block = (2..20).contains(&x) && (2..18).contains(&y);
            let pin = (10..11).contains(&x) && (9..10).contains(&y);
            if speck || (block && !pin) {
                BLACK
            } else {
                WHITE
            }
        });
        let t = bw(&px, 40, 20, &Settings::default());
        assert_eq!(t.paths(), 1, "the speck is gone");
        let holes = t.layers[0].paths[0]
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::MoveTo(_)))
            .count();
        assert_eq!(holes, 1, "and the pinhole is filled");
        let keep = Settings {
            speck: 0.0,
            ..Settings::default()
        };
        let t = bw(&px, 40, 20, &keep);
        assert_eq!(t.paths(), 2);
    }

    /// Two black pixels touching only at a corner are two paths: the loop turns
    /// to keep them apart.
    #[test]
    fn corner_touching_pixels_stay_apart() {
        let px = raster(4, 4, |x, y| {
            if (x, y) == (1, 1) || (x, y) == (2, 2) {
                BLACK
            } else {
                WHITE
            }
        });
        let s = Settings {
            speck: 0.0,
            ..Settings::default()
        };
        assert_eq!(bw(&px, 4, 4, &s).paths(), 2);
    }

    /// The threshold decides what is dark: a mid grey is traced under a high
    /// threshold and left out under a low one.
    #[test]
    fn the_threshold_decides_what_is_dark() {
        let grey = [128, 128, 128, 255];
        let px = raster(20, 20, |x, _| if x < 10 { grey } else { WHITE });
        let high = Settings {
            mode: Mode::BlackAndWhite { threshold: 0.7 },
            ..Settings::default()
        };
        let low = Settings {
            mode: Mode::BlackAndWhite { threshold: 0.3 },
            ..Settings::default()
        };
        assert_eq!(bw(&px, 20, 20, &high).paths(), 1);
        assert_eq!(bw(&px, 20, 20, &low).paths(), 0);
    }

    /// Colours mode finds the picture's colours, darkest first, each with its own
    /// paths; transparency is left out.
    #[test]
    fn colours_are_layered_darkest_first() {
        let px = raster(30, 30, |x, y| match (x / 10, y / 10) {
            (0, _) => BLACK,
            (1, 1) => CLEAR,
            (1, _) => RED,
            _ => WHITE,
        });
        let s = Settings {
            mode: Mode::Colours { count: 3 },
            ..Settings::default()
        };
        let t = bw(&px, 30, 30, &s);
        assert_eq!(t.layers.len(), 3);
        let lum = |c: Color| {
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
        };
        assert!(
            t.layers
                .windows(2)
                .all(|w| lum(w[0].colour) <= lum(w[1].colour)),
            "darkest first"
        );
        assert_eq!(t.layers[0].colour.to_rgba8().to_u8_array(), BLACK);
        // The red column is two pieces, split by the transparent cell.
        let red = t
            .layers
            .iter()
            .find(|l| l.colour.to_rgba8().to_u8_array() == RED)
            .expect("red found");
        assert_eq!(red.paths.len(), 2);
    }
}
