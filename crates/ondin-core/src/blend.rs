//! *Blend shapes*: the steps between two shapes, as plain paths with their outline,
//! paint and opacity interpolated, in a group named *Blend* that takes the two
//! shapes as well, one at each end.
//!
//! Computed once; nothing stays live. Both outlines are taken in world space and
//! cut into the same number of cubic segments, so step `k` of `n` is every control
//! point a fraction `k / (n + 1)` of the way from the first shape to the second.

use crate::build;
use crate::kurbo::{
    Affine, BezPath, CubicBez, Line, ParamCurve, ParamCurveArclen, ParamCurveNearest, PathSeg,
    Point, Rect, Shape, Vec2,
};
use crate::node::NodeKind;
use crate::{Brush, Document, IdSource, NodeId, Operation, Resolved, Transaction};

/// The group the steps land in.
pub const GROUP_NAME: &str = "Blend";

/// The most steps a blend makes between its two shapes.
pub const MAX_STEPS: usize = 200;

/// How the two shapes are blended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blend {
    /// How many shapes go between the two.
    pub steps: usize,
    /// Fills, strokes and opacity move from one to the other; off, every step
    /// wears the first shape's.
    pub paint: bool,
    /// Where on each shape's box the first anchors pair up, as a fraction of the
    /// box: `(0, 0)` its top-left, `(0.5, 0.5)` its middle.
    pub start: Vec2,
    /// Run the second shape's outline the other way round.
    pub reverse: bool,
}

/// Why two layers cannot be blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not exactly two shapes.
    Count,
    Picture,
    Text,
    /// One shape is closed and the other open, or they have different numbers of
    /// parts, so their outlines do not pair.
    Parts,
    /// A shape is locked, or in a locked group.
    Locked,
    /// A shape sits in a boolean, a main or an instance, where a new group would
    /// change what that draws.
    Inside,
}

impl Refusal {
    /// The sentence a dimmed row and a refused commit say.
    pub fn says(self) -> &'static str {
        match self {
            Refusal::Count => "Blend shapes needs exactly two shapes",
            Refusal::Picture => "Blend shapes can't take a picture",
            Refusal::Text => "Blend shapes can't take text",
            Refusal::Parts => {
                "Blend shapes needs two shapes made of the same number of parts, both closed or both open"
            }
            Refusal::Locked => "A layer to blend is locked",
            Refusal::Inside => {
                "Blend shapes can't work inside a boolean, a component or an instance"
            }
        }
    }
}

/// The two shapes `selection` blends, bottom of the stack first, or why not.
pub fn pair(doc: &Document, selection: &[NodeId]) -> Result<(NodeId, NodeId), Refusal> {
    let members = build::in_document_order(doc, selection);
    let [a, b] = members.as_slice() else {
        return Err(Refusal::Count);
    };
    for id in [a, b] {
        let node = doc.get(*id).ok_or(Refusal::Count)?;
        if crate::is_effectively_locked(doc, *id) {
            return Err(Refusal::Locked);
        }
        // The group lands beside the upper shape: in a boolean it would be one
        // more operand, in an instance a layer its main does not have.
        let mut up = node.parent();
        while let Some(p) = up.and_then(|p| doc.get(p)) {
            if matches!(p.kind(), NodeKind::Boolean { .. }) || p.component() || p.link().is_some() {
                return Err(Refusal::Inside);
            }
            up = p.parent();
        }
        if node.link().is_some() {
            return Err(Refusal::Inside);
        }
        if node.paint().image_fill().is_some() {
            return Err(Refusal::Picture);
        }
        match node.kind() {
            NodeKind::Text { .. } => return Err(Refusal::Text),
            NodeKind::Rect { .. }
            | NodeKind::Ellipse { .. }
            | NodeKind::Polygon { .. }
            | NodeKind::Star { .. }
            | NodeKind::Line { .. }
            | NodeKind::Path { .. }
            | NodeKind::Boolean { .. } => {}
            _ => return Err(Refusal::Count),
        }
    }
    Ok((*a, *b))
}

/// One subpath as cubic segments, and whether it closes.
#[derive(Clone, Debug)]
struct Run {
    segs: Vec<CubicBez>,
    closed: bool,
}

fn cubic(seg: PathSeg) -> CubicBez {
    match seg {
        PathSeg::Line(Line { p0, p1 }) => {
            CubicBez::new(p0, p0.lerp(p1, 1.0 / 3.0), p0.lerp(p1, 2.0 / 3.0), p1)
        }
        PathSeg::Quad(q) => q.raise(),
        PathSeg::Cubic(c) => c,
    }
}

/// `path`'s subpaths as runs, dropping segments with no length.
fn runs(path: &BezPath) -> Vec<Run> {
    crate::geometry::subpaths(path)
        .into_iter()
        .filter_map(|sub| {
            let closed = matches!(sub.elements().last(), Some(kurbo::PathEl::ClosePath));
            let segs: Vec<CubicBez> = sub
                .segments()
                .map(cubic)
                .filter(|c| c.p0.distance(c.p3) > 1e-9 || c.p0.distance(c.p1) > 1e-9)
                .collect();
            (!segs.is_empty()).then_some(Run { segs, closed })
        })
        .collect()
}

/// The signed area a closed run encloses, curves included: its sign is which
/// way it turns, even for a lens of two segments whose anchors enclose nothing.
fn turning(run: &Run) -> f64 {
    let mut path = BezPath::new();
    path.move_to(run.segs[0].p0);
    for c in &run.segs {
        path.curve_to(c.p1, c.p2, c.p3);
    }
    path.close_path();
    path.area()
}

fn reversed(run: &Run) -> Run {
    Run {
        segs: run
            .segs
            .iter()
            .rev()
            .map(|c| CubicBez::new(c.p3, c.p2, c.p1, c.p0))
            .collect(),
        closed: run.closed,
    }
}

/// How finely an arc length is measured.
const ACCURACY: f64 = 1e-3;

/// A closed run started at the point of its outline nearest `at`, the segment
/// there cut in two.
fn started_near(run: &Run, at: Point) -> Run {
    let (i, t) = run
        .segs
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.nearest(at, ACCURACY)))
        .min_by(|(_, a), (_, b)| a.distance_sq.total_cmp(&b.distance_sq))
        .map_or((0, 0.0), |(i, n)| (i, n.t));
    let c = run.segs[i];
    let mut segs = Vec::with_capacity(run.segs.len() + 1);
    if t < 1.0 - 1e-9 {
        segs.push(c.subsegment(t..1.0));
    }
    segs.extend_from_slice(&run.segs[i + 1..]);
    segs.extend_from_slice(&run.segs[..i]);
    if t > 1e-9 {
        segs.push(c.subsegment(0.0..t));
    }
    Run {
        segs,
        closed: run.closed,
    }
}

/// Where each anchor after the first falls along `run`, as a fraction of its
/// length; the last is 1.
fn fractions(run: &Run) -> Vec<f64> {
    let lengths: Vec<f64> = run.segs.iter().map(|c| c.arclen(ACCURACY)).collect();
    let total: f64 = lengths.iter().sum::<f64>().max(1e-9);
    let mut at = 0.0;
    lengths
        .iter()
        .map(|l| {
            at += l;
            at / total
        })
        .collect()
}

/// `run` cut wherever `cuts` (fractions of its length, ascending) fall inside a
/// segment, so every fraction becomes an anchor and its own anchors stay.
fn cut_at(run: &Run, cuts: &[f64]) -> Run {
    let lengths: Vec<f64> = run.segs.iter().map(|c| c.arclen(ACCURACY)).collect();
    let total: f64 = lengths.iter().sum::<f64>().max(1e-9);
    let mut segs = Vec::new();
    let mut from = 0.0;
    for (c, l) in run.segs.iter().zip(&lengths) {
        let to = from + l / total;
        let mut t0 = 0.0;
        for &f in cuts.iter().filter(|f| **f > from + 1e-9 && **f < to - 1e-9) {
            let t = c.inv_arclen((f - from) * total, ACCURACY);
            segs.push(c.subsegment(t0..t));
            t0 = t;
        }
        segs.push(c.subsegment(t0..1.0));
        from = to;
    }
    Run {
        segs,
        closed: run.closed,
    }
}

/// `x` and `y` cut at each other's anchors, by fraction of length: the same
/// number of segments, and a corner of one meets the point as far round the other.
fn paired(x: &Run, y: &Run) -> (Run, Run) {
    let mut cuts: Vec<f64> = fractions(x).into_iter().chain(fractions(y)).collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let (x, y) = (cut_at(x, &cuts), cut_at(y, &cuts));
    // Rounding at a near-coincident pair can leave one a segment short; pad it
    // with a point segment at its end rather than mismatch.
    let pad = |mut r: Run, n: usize| {
        while r.segs.len() < n {
            let p = r.segs.last().map_or(Point::ZERO, |c| c.p3);
            r.segs.push(CubicBez::new(p, p, p, p));
        }
        r
    };
    let n = x.segs.len().max(y.segs.len());
    (pad(x, n), pad(y, n))
}

/// The outline between `a` and `b` at `t`, segment by segment.
fn between(a: &[Run], b: &[Run], t: f64) -> BezPath {
    let mut out = BezPath::new();
    for (ra, rb) in a.iter().zip(b) {
        let lerp = |p: Point, q: Point| p.lerp(q, t);
        let first = (ra.segs[0], rb.segs[0]);
        out.move_to(lerp(first.0.p0, first.1.p0));
        for (ca, cb) in ra.segs.iter().zip(&rb.segs) {
            out.curve_to(lerp(ca.p1, cb.p1), lerp(ca.p2, cb.p2), lerp(ca.p3, cb.p3));
        }
        if ra.closed {
            out.close_path();
        }
    }
    out
}

/// The two outlines, in world space, paired run for run and cut to the same
/// segments, ready to be stepped between.
fn matched(
    doc: &Document,
    res: &Resolved,
    a: NodeId,
    b: NodeId,
    blend: Blend,
) -> Result<(Vec<Run>, Vec<Run>), Refusal> {
    let outline = |id: NodeId| -> Option<(Vec<Run>, Rect)> {
        let world = res.world_transform(id)? * res.local_path(doc, id)?;
        let bounds = world.bounding_box();
        Some((runs(&world), bounds))
    };
    let (ra, box_a) = outline(a).ok_or(Refusal::Count)?;
    let (rb, box_b) = outline(b).ok_or(Refusal::Count)?;
    if ra.is_empty()
        || ra.len() != rb.len()
        || ra.iter().zip(&rb).any(|(x, y)| x.closed != y.closed)
    {
        return Err(Refusal::Parts);
    }
    let corner = |r: Rect| {
        Point::new(
            r.x0 + r.width() * blend.start.x,
            r.y0 + r.height() * blend.start.y,
        )
    };
    let (mut out_a, mut out_b) = (Vec::new(), Vec::new());
    for (x, y) in ra.iter().zip(&rb) {
        let (mut x, mut y) = (x.clone(), y.clone());
        if x.closed {
            // Turned the same way as the first, so the steps do not pass through
            // themselves; *Reverse* still turns it back by hand.
            if turning(&x).signum() != turning(&y).signum() {
                y = reversed(&y);
            }
            if blend.reverse {
                y = reversed(&y);
            }
            x = started_near(&x, corner(box_a));
            y = started_near(&y, corner(box_b));
        } else if blend.reverse {
            y = reversed(&y);
        }
        let (x, y) = paired(&x, &y);
        out_a.push(x);
        out_b.push(y);
    }
    Ok((out_a, out_b))
}

/// `a` and `b` mixed `t` of the way, channel by channel, alpha included.
fn mix(a: crate::peniko::Color, b: crate::peniko::Color, t: f32) -> crate::peniko::Color {
    let (p, q) = (a.components, b.components);
    crate::peniko::Color::new(std::array::from_fn(|i| p[i] + (q[i] - p[i]) * t))
}

/// The look of step `t`: each fill and stroke the two share by position mixed
/// where both are solid colours, else the nearer shape's; widths and opacity
/// mixed. Literal throughout, as the steps are unbound. `scale` maps each shape's
/// stroke widths into the steps' own units, so a stroke draws as wide as it did.
fn look_between(
    doc: &Document,
    (a, b): (NodeId, NodeId),
    t: f64,
    paint: bool,
    scale: (f64, f64),
) -> (crate::Paint, f32) {
    let (na, nb) = (doc.get(a), doc.get(b));
    let resolved = |n: Option<&crate::Node>, id, k: f64| {
        let mut p = n
            .map(|n| doc.resolved_paint(id, n.paint()).into_owned())
            .unwrap_or_default();
        for s in &mut p.strokes {
            s.value.width *= k;
            s.value.width_var = None;
        }
        p
    };
    let (pa, pb) = (resolved(na, a, scale.0), resolved(nb, b, scale.1));
    let (oa, ob) = (doc.drawn_opacity(a), doc.drawn_opacity(b));
    if !paint {
        return (pa, oa);
    }
    let t32 = t as f32;
    let near = if t < 0.5 { &pa } else { &pb };
    let mut out = near.clone();
    let solid = |s: &crate::variable::BrushSource| match s.literal() {
        Some(Brush::Solid(c)) => Some(*c),
        _ => None,
    };
    if pa.fills.len() == pb.fills.len() {
        for (i, f) in out.fills.iter_mut().enumerate() {
            if let (Some(x), Some(y)) = (solid(&pa.fills[i].brush), solid(&pb.fills[i].brush)) {
                f.value.brush = crate::variable::BrushSource::Literal(Brush::Solid(mix(x, y, t32)));
            }
        }
    }
    if pa.strokes.len() == pb.strokes.len() {
        for (i, s) in out.strokes.iter_mut().enumerate() {
            let (x, y) = (&pa.strokes[i], &pb.strokes[i]);
            s.value.width = x.width + (y.width - x.width) * t;
            if let (Some(cx), Some(cy)) = (solid(&x.brush), solid(&y.brush)) {
                s.value.brush =
                    crate::variable::BrushSource::Literal(Brush::Solid(mix(cx, cy, t32)));
            }
        }
    }
    (out, oa + (ob - oa) * t32)
}

/// The blend of `selection`'s two shapes: the [`Operation::InsertSubtree`] that
/// adds the *Blend* group where the upper of the two stands in its parent, then
/// the two moved into it around the steps, first shape lowest. Answers the edit
/// and the group.
pub fn blend(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    selection: &[NodeId],
    blend: Blend,
) -> Result<(Transaction, NodeId), Refusal> {
    let (a, b) = pair(doc, selection)?;
    if blend.steps == 0 || blend.steps > MAX_STEPS {
        return Err(Refusal::Count);
    }
    let (ra, rb) = matched(doc, res, a, b, blend)?;
    let upper = doc.get(b).ok_or(Refusal::Count)?;
    let parent = upper.parent().ok_or(Refusal::Count)?;
    let at = doc
        .get(parent)
        .and_then(|p| p.children().iter().position(|c| *c == b))
        .ok_or(Refusal::Count)?;
    let parent_world = res.world_transform(parent).unwrap_or(Affine::IDENTITY);
    let to_parent = parent_world.inverse();
    // How much larger a unit of each shape draws than a unit of the steps.
    let size = |w: Affine| w.determinant().abs().sqrt();
    let unit = |id: NodeId| {
        res.world_transform(id)
            .map_or(1.0, |w| size(w) / size(parent_world).max(1e-12))
    };
    let scale = (unit(a), unit(b));

    // Built on a scratch document, so each node takes every default a created
    // one has, then carried over whole.
    let root = ids.mint();
    let mut scratch = Document::new(root);
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
    for k in 1..=blend.steps {
        let t = k as f64 / (blend.steps + 1) as f64;
        let step = ids.mint();
        let path = to_parent * between(&ra, &rb, t);
        ops.push(Operation::CreateNode {
            id: step,
            parent: group,
            index: k - 1,
            kind: NodeKind::Path {
                path,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: crate::symmetry::PathMirror::NONE,
            },
            transform: None,
            name: Some(format!("Step {k}")),
        });
        let (paint, opacity) = look_between(doc, (a, b), t, blend.paint, scale);
        // The nearer shape's rule, so a shape with holes keeps them.
        let near = if t < 0.5 || !blend.paint { a } else { b };
        if let Some(rule) = doc.get(near).map(|n| n.fill_rule()) {
            ops.push(Operation::SetFillRule { id: step, rule });
        }
        ops.push(Operation::SetFills {
            id: step,
            fills: paint.fills,
        });
        ops.push(Operation::SetStrokes {
            id: step,
            strokes: paint.strokes,
        });
        if opacity != 1.0 {
            ops.push(Operation::SetOpacity { id: step, opacity });
        }
    }
    scratch
        .apply(&Transaction(ops))
        .map_err(|_| Refusal::Parts)?;
    let nodes = scratch.capture_subtree(group).ok_or(Refusal::Parts)?;
    let mut ops = vec![Operation::InsertSubtree {
        nodes,
        parent,
        index: at,
    }];
    // The two shapes join their steps, the first under them and the second over,
    // each where it stood: the group sits in the upper shape's parent with no
    // transform of its own.
    for (id, index) in [(a, 0), (b, blend.steps + 1)] {
        let world = res.world_transform(id).ok_or(Refusal::Count)?;
        ops.push(Operation::Reparent {
            id,
            new_parent: group,
            index,
        });
        ops.push(Operation::SetTransform {
            id,
            transform: to_parent * world,
        });
    }
    Ok((Transaction(ops), group))
}
