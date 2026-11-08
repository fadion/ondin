//! The shape builder and *Divide*: the selection's outlines cut into the faces of
//! their planar arrangement ([`crate::boolean::regions`]), which a drag merges or
//! removes and *Divide* turns each into a path of its own.
//!
//! An open path or line in the selection cuts the faces it crosses, along the
//! Knife's own surgery ([`crate::knife::pieces`]), and fills none of its own; it is
//! left as it is.
//!
//! Everything is worked in world space, so sources under different parents and
//! transforms cut each other as they are drawn. What lands is plain `Path` layers.
//! A source the edit takes from is rewritten as a path holding what is left of it,
//! keeping its name, place and look; a live boolean is flattened that way, and undo
//! brings it back live.

use crate::boolean::{self, Region};
use crate::build;
use crate::kurbo::{Affine, BezPath, Point};
use crate::node::{BoolOp, NodeKind};
use crate::{Document, IdSource, NodeId, OpError, Operation, Resolved, Transaction};

/// The selection's outlines cut into faces, ready for the shape builder's edits.
#[derive(Clone, Debug)]
pub struct Arrangement {
    /// The layers whose outlines were cut, bottom of the stack first.
    pub sources: Vec<NodeId>,
    /// Each source's outline in world space, in [`Self::sources`]' order.
    outlines: Vec<BezPath>,
    /// Whether each source's fill rule reads its subpaths otherwise than a boolean
    /// operand is read, so what it fills is only the faces it covers.
    expanded: Vec<bool>,
    /// The faces, in world space; [`Region::covers`] indexes [`Self::sources`].
    pub regions: Vec<Region>,
    /// The open paths and lines that cut the faces without filling one of their
    /// own, bottom of the stack first. They are left as they are.
    pub cutters: Vec<NodeId>,
    /// Whether the selection held layers the arrangement leaves out: text,
    /// pictures and frames.
    pub left_out: bool,
}

/// Why there is no [`Arrangement`] to build with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Fewer than two closed shapes in the selection, or one and nothing to cut it.
    TooFew,
    /// The shapes are there but none overlaps another and no line cuts one.
    NoOverlap,
    /// The arithmetic unwound ([`boolean::failures`]).
    Abandoned,
}

/// What a layer is to the arrangement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// A closed outline, whose inside fills faces.
    Shape,
    /// An open path or line, which cuts the faces it crosses.
    Cutter,
    Out,
}

/// What `id` is to the arrangement: a visible (ancestors included), unlocked layer
/// that is no mask, outside any boolean, any instance and any layout, is a
/// [`Part::Shape`] when
/// it is a closed shape, path or boolean, a [`Part::Cutter`] when it is an open
/// path, a line, an arc or a spiral.
fn part(doc: &Document, id: NodeId) -> Part {
    let Some(node) = doc.get(id) else {
        return Part::Out;
    };
    // A mask paints nothing, and a face cut from it would paint its fill and stop
    // clipping the layers above it.
    if crate::is_effectively_locked(doc, id)
        || !doc.shown_through(id)
        || node.mask()
        || node.paint().image_fill().is_some()
    {
        return Part::Out;
    }
    // In a layout every new or reshaped layer would be laid out anew, moving the
    // pieces and their siblings.
    if crate::component::instance_root(doc, id).is_some()
        || crate::container::parent_lays_out(&crate::resolve::DocView(doc), id)
    {
        return Part::Out;
    }
    let mut up = node.parent();
    while let Some(p) = up.and_then(|p| doc.get(p)) {
        if matches!(p.kind(), NodeKind::Boolean { .. }) {
            return Part::Out;
        }
        up = p.parent();
    }
    match node.kind() {
        NodeKind::Rect { .. }
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Boolean { .. } => Part::Shape,
        NodeKind::Path { path, .. } if is_closed(path) => Part::Shape,
        NodeKind::Path { .. }
        | NodeKind::Line { .. }
        | NodeKind::Arc { .. }
        | NodeKind::Spiral { .. } => Part::Cutter,
        _ => Part::Out,
    }
}

/// Whether `id` gives the arrangement a closed outline to fill faces with.
pub(crate) fn takes_part(doc: &Document, id: NodeId) -> bool {
    part(doc, id) == Part::Shape
}

/// Whether every subpath of `path` is closed.
fn is_closed(path: &BezPath) -> bool {
    let subs = crate::geometry::subpaths(path);
    !subs.is_empty()
        && subs
            .iter()
            .all(|s| matches!(s.elements().last(), Some(kurbo::PathEl::ClosePath)))
}

/// The layers of `selection` that take part, groups opened into their contents,
/// bottom of the stack first: the closed shapes and the cutters. And whether any
/// selected layer was left out.
fn sources_of(doc: &Document, selection: &[NodeId]) -> (Vec<NodeId>, Vec<NodeId>, bool) {
    let (mut shapes, mut cutters) = (Vec::new(), Vec::new());
    let mut left_out = false;
    let mut stack: Vec<NodeId> = build::outermost(doc, selection);
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if matches!(node.kind(), NodeKind::Group) {
            stack.extend(node.children().iter().copied());
            continue;
        }
        match part(doc, id) {
            Part::Shape => shapes.push(id),
            Part::Cutter => cutters.push(id),
            Part::Out if doc.shown_through(id) => left_out = true,
            Part::Out => {}
        }
    }
    (
        build::all_in_document_order(doc, &shapes),
        build::all_in_document_order(doc, &cutters),
        left_out,
    )
}

/// Whether there is enough to arrange: two closed shapes, or one and a cutter.
fn enough(shapes: usize, cutters: usize) -> bool {
    shapes >= 2 || (shapes == 1 && cutters > 0)
}

/// How finely a cutter's curves are followed, in world units: well under what
/// a pointer or a pixel can tell apart.
const CUT_TOLERANCE: f64 = 0.01;

/// The world-space polylines `id` cuts along inside `reach`, the faces' box.
fn trails(doc: &Document, res: &Resolved, id: NodeId, reach: kurbo::Rect) -> Vec<Vec<Point>> {
    world_outline(doc, res, id).map_or_else(Vec::new, |outline| trail_points(&outline, reach))
}

/// `outline` as polylines, followed only inside `reach`: a run that misses it can
/// cross no face, so it is dropped and the polyline broken there. Clipping before
/// flattening keeps the work bounded by the faces' size, not the cutter's: a cutter
/// reaching 10¹² units would otherwise be followed at `CUT_TOLERANCE` all the way.
pub(crate) fn trail_points(outline: &BezPath, reach: kurbo::Rect) -> Vec<Vec<Point>> {
    let extent = reach.width().max(reach.height());
    let tolerance = CUT_TOLERANCE.max(extent * 1e-7);
    let mut out: Vec<Vec<Point>> = Vec::new();
    for sub in crate::geometry::subpaths(outline) {
        let mut open = false;
        for seg in sub.segments() {
            clip_segment(seg, reach, extent, tolerance, 0, &mut out, &mut open);
        }
    }
    out.retain(|t| t.len() >= 2);
    out
}

/// [`trail_points`] for one segment: halved until each half either misses
/// `reach` or is no larger than a few times it, then flattened onto the trail in
/// hand (`open`) or a new one.
fn clip_segment(
    seg: kurbo::PathSeg,
    reach: kurbo::Rect,
    extent: f64,
    tolerance: f64,
    depth: u32,
    out: &mut Vec<Vec<Point>>,
    open: &mut bool,
) {
    use kurbo::{ParamCurve, ParamCurveExtrema};
    let b = seg.bounding_box();
    let meets = b.x0 <= reach.x1 && b.x1 >= reach.x0 && b.y0 <= reach.y1 && b.y1 >= reach.y0;
    // `meets` is false for a non-finite box too, so it is never followed.
    if !meets {
        *open = false;
        return;
    }
    if depth < 64 && b.width().max(b.height()) > 4.0 * extent {
        let (a, c) = seg.subdivide();
        clip_segment(a, reach, extent, tolerance, depth + 1, out, open);
        clip_segment(c, reach, extent, tolerance, depth + 1, out, open);
        return;
    }
    if !*open {
        out.push(vec![seg.start()]);
        *open = true;
    }
    let Some(trail) = out.last_mut() else { return };
    let mut piece = BezPath::new();
    piece.move_to(seg.start());
    piece.push(seg.as_path_el());
    kurbo::flatten(piece, tolerance, |el| {
        if let kurbo::PathEl::LineTo(p) = el {
            trail.push(p);
        }
    });
}

/// `regions` with each face cut along every trail that crosses it, each piece
/// covered as its face was. A face the cut cannot read is kept whole.
fn cut_regions(regions: Vec<Region>, trails: &[Vec<Point>]) -> Vec<Region> {
    let mut out = regions;
    for trail in trails {
        out = out
            .into_iter()
            .flat_map(|r| match crate::knife::pieces(&r.path, trail) {
                Some(pieces) if pieces.len() > 1 => pieces
                    .into_iter()
                    .map(|path| Region {
                        path,
                        covers: r.covers.clone(),
                    })
                    .collect(),
                _ => vec![r],
            })
            .collect();
    }
    out
}

/// `id`'s outline in world space, as drawn.
pub(crate) fn world_outline(doc: &Document, res: &Resolved, id: NodeId) -> Option<BezPath> {
    Some(res.world_transform(id)? * res.local_path(doc, id)?)
}

/// Whether `selection` holds two or more closed shapes the arrangement would cut,
/// or one and an open path or line to cut it — the cheap test a menu row dims on,
/// with no arithmetic.
pub fn can_arrange(doc: &Document, selection: &[NodeId]) -> bool {
    let (shapes, cutters, _) = sources_of(doc, selection);
    enough(shapes.len(), cutters.len())
}

/// Cut `selection`'s closed shapes into their faces, and each face along the
/// open paths and lines that cross it.
pub fn arrange(
    doc: &Document,
    res: &Resolved,
    selection: &[NodeId],
) -> Result<Arrangement, Refusal> {
    let (candidates, cutters, left_out) = sources_of(doc, selection);
    let (sources, outlines): (Vec<NodeId>, Vec<BezPath>) = candidates
        .into_iter()
        .filter_map(|id| Some((id, world_outline(doc, res, id)?)))
        .filter(|(id, _)| {
            res.world_transform(*id)
                .is_some_and(|w| w.determinant() != 0.0)
        })
        .unzip();
    let reach = outlines
        .iter()
        .map(kurbo::Shape::bounding_box)
        .reduce(|a, b| a.union(b))
        .map(|r| r.inflate(1.0 + r.width() * 0.01, 1.0 + r.height() * 0.01));
    let lines: Vec<Vec<Point>> = match reach {
        Some(reach) => cutters
            .iter()
            .flat_map(|id| trails(doc, res, *id, reach))
            .collect(),
        None => Vec::new(),
    };
    // Counted by layer, not trail: a cutter that misses every face is still one,
    // and the answer is then `NoOverlap`.
    if !enough(sources.len(), cutters.len()) {
        return Err(Refusal::TooFew);
    }
    let (whole, expanded) = filled_regions(doc, &sources, &outlines).ok_or(Refusal::Abandoned)?;
    // The union reads each outline as an operand, which an expanded source is not.
    if !expanded.iter().any(|e| *e) && overlapping(&whole, &outlines) {
        return Err(Refusal::Abandoned);
    }
    let before = whole.len();
    let regions = cut_regions(whole, &lines);
    if regions.len() == before && !regions.iter().any(|r| r.covers.len() > 1) {
        return Err(Refusal::NoOverlap);
    }
    Ok(Arrangement {
        sources,
        outlines,
        expanded,
        regions,
        cutters,
        left_out,
    })
}

/// The faces of `outlines` (`sources`' world outlines), each covered by the sources
/// that paint it under their own fill rule. A source whose rule reads its subpaths
/// otherwise than [`boolean::regions`] does ([`boolean::filled_faces`]) enters as
/// its subpaths, and a face keeps it only where it fills; a face nothing fills goes.
/// Also which sources entered so.
fn filled_regions(
    doc: &Document,
    sources: &[NodeId],
    outlines: &[BezPath],
) -> Option<(Vec<Region>, Vec<bool>)> {
    let rule = |j: usize| {
        doc.get(sources[j])
            .map(crate::Node::fill_rule)
            .unwrap_or_default()
    };
    let mut entries: Vec<BezPath> = Vec::new();
    let mut owner: Vec<usize> = Vec::new();
    let mut expanded = vec![false; outlines.len()];
    for (j, outline) in outlines.iter().enumerate() {
        if boolean::filled_faces(outline, rule(j)).is_some() {
            expanded[j] = true;
            for sub in crate::geometry::subpaths(outline) {
                entries.push(sub);
                owner.push(j);
            }
        } else {
            entries.push(outline.clone());
            owner.push(j);
        }
    }
    let faces = boolean::regions(&entries)?;
    let faces = faces
        .into_iter()
        .filter_map(|r| {
            // Entries are in source order, so owners come out ascending.
            let mut covers: Vec<usize> = r.covers.iter().map(|e| owner[*e]).collect();
            covers.dedup();
            if covers.iter().any(|j| expanded[*j]) {
                let p = boolean::point_inside(&r.path)?;
                covers.retain(|j| !expanded[*j] || boolean::fills(&outlines[*j], rule(*j), p));
            }
            (!covers.is_empty()).then_some(Region {
                path: r.path,
                covers,
            })
        })
        .collect();
    Some((faces, expanded))
}

/// Whether the faces cover more than the outlines' union, so some overlap: the
/// arithmetic can hand back a whole operand as the outside of a nearly coincident
/// one, and a face offered twice would be painted twice by *Divide*.
fn overlapping(faces: &[Region], outlines: &[BezPath]) -> bool {
    use kurbo::Shape;
    let Some(union) = boolean::evaluate(BoolOp::Union, outlines) else {
        return false;
    };
    let covered = union.area().abs();
    let total: f64 = faces.iter().map(|f| f.path.area().abs()).sum();
    total > covered * (1.0 + 1e-3) + 1e-3
}

/// What a shape-builder edit made: the merged path, if it merged, and every source
/// that is still there, by its id after the edit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Built {
    pub merged: Option<NodeId>,
    pub kept: Vec<NodeId>,
}

impl Arrangement {
    /// The face under `world`, topmost-covered first if faces touch there.
    pub fn region_at(&self, world: Point) -> Option<usize> {
        self.regions
            .iter()
            .position(|r| crate::geometry::winding(&r.path, world) != 0)
    }

    /// The topmost source over face `region`.
    pub fn top_source(&self, region: usize) -> Option<NodeId> {
        let top = *self.regions.get(region)?.covers.last()?;
        self.sources.get(top).copied()
    }

    /// Merge the faces `picked` into one path, cut out of every source that
    /// covered them. The path takes the look of the topmost source over the first
    /// face and lands just above the topmost source it took from.
    pub fn merge(
        &self,
        doc: &Document,
        res: &Resolved,
        ids: &mut IdSource,
        picked: &[usize],
    ) -> Result<(Transaction, Built), OpError> {
        self.carve(doc, res, ids, picked, true)
    }

    /// Cut the faces `picked` out of every source that covered them.
    pub fn remove(
        &self,
        doc: &Document,
        res: &Resolved,
        ids: &mut IdSource,
        picked: &[usize],
    ) -> Result<(Transaction, Built), OpError> {
        self.carve(doc, res, ids, picked, false)
    }

    fn carve(
        &self,
        doc: &Document,
        res: &Resolved,
        ids: &mut IdSource,
        picked: &[usize],
        merge: bool,
    ) -> Result<(Transaction, Built), OpError> {
        let faces: Vec<&Region> = picked.iter().filter_map(|i| self.regions.get(*i)).collect();
        let Some(first) = faces.first() else {
            return Err(OpError::EmptyGeometry);
        };
        let paths: Vec<BezPath> = faces.iter().map(|r| r.path.clone()).collect();
        let set = evaluated(BoolOp::Union, &paths)?;
        let mut touched: Vec<usize> = faces
            .iter()
            .flat_map(|r| r.covers.iter().copied())
            .collect();
        touched.sort_unstable();
        touched.dedup();
        let painter = self.sources[*first.covers.last().ok_or(OpError::EmptyGeometry)?];
        let anchor = self.sources[*touched.last().ok_or(OpError::EmptyGeometry)?];

        let mut advanced = doc.clone();
        let mut ops = Vec::new();
        let mut step = |tx: Transaction, advanced: &mut Document| -> Result<(), OpError> {
            advanced.apply(&tx)?;
            ops.extend(tx.0);
            Ok(())
        };
        let mut built = Built::default();
        if merge {
            let (tx, made) = placed_path(doc, res, ids, anchor, painter, &set, None)?;
            step(tx, &mut advanced)?;
            built.merged = Some(made);
        }
        for (j, &source) in self.sources.iter().enumerate() {
            if touched.binary_search(&j).is_err() {
                built.kept.push(source);
                continue;
            }
            // Nothing left deletes the source; an unwind refuses the whole edit. A
            // source whose rule a boolean would misread keeps the faces it still
            // fills, so a hole its rule makes stays one.
            let left = match self.expanded[j] {
                true => {
                    let rest: Vec<BezPath> = self
                        .regions
                        .iter()
                        .enumerate()
                        .filter(|(i, r)| r.covers.contains(&j) && !picked.contains(i))
                        .map(|(_, r)| r.path.clone())
                        .collect();
                    match rest.is_empty() {
                        true => Err(OpError::EmptyGeometry),
                        false => evaluated(BoolOp::Union, &rest),
                    }
                }
                false => evaluated(BoolOp::Subtract, &[self.outlines[j].clone(), set.clone()]),
            };
            let left = match left {
                Ok(left) => left,
                Err(OpError::EmptyGeometry) => {
                    step(
                        Transaction(vec![Operation::DeleteNode { id: source }]),
                        &mut advanced,
                    )?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let world = res
                .world_transform(source)
                .ok_or(OpError::NoSuchNode(source))?;
            let local = world.inverse() * left;
            let (tx, now) = reshaped(&advanced, ids, source, local)?;
            step(tx, &mut advanced)?;
            built.kept.push(now);
        }
        Ok((Transaction(ops), built))
    }
}

/// `operands` combined by `op`, an unwind told apart from an empty answer.
fn evaluated(op: BoolOp, operands: &[BezPath]) -> Result<BezPath, OpError> {
    let before = boolean::failures();
    match boolean::evaluate(op, operands) {
        Some(path) => Ok(path),
        None if boolean::failures() != before => Err(OpError::BooleanAbandoned),
        None => Err(OpError::EmptyGeometry),
    }
}

/// Source `id` holding `local` in its own space: a path's geometry rewritten in
/// place, any other kind replaced by a path in its slot.
pub(crate) fn reshaped(
    doc: &Document,
    ids: &mut IdSource,
    id: NodeId,
    local: BezPath,
) -> Result<(Transaction, NodeId), OpError> {
    let node = doc.get(id).ok_or(OpError::NoSuchNode(id))?;
    if matches!(node.kind(), NodeKind::Path { .. }) {
        let tx = Transaction(vec![Operation::SetGeometry {
            id,
            geometry: crate::GeometryPatch::Path {
                path: local,
                corner_radii: Vec::new(),
                mirror: None,
            },
        }]);
        return Ok((tx, id));
    }
    build::replace_with_path(doc, ids, id, local)
}

/// The space `anchor`'s parent draws its children in, as a world transform.
fn parent_world(doc: &Document, res: &Resolved, anchor: NodeId) -> Result<Affine, OpError> {
    let world = res
        .world_transform(anchor)
        .ok_or(OpError::NoSuchNode(anchor))?;
    let local = res
        .used_local(doc, anchor)
        .ok_or(OpError::NoSuchNode(anchor))?;
    Ok(world * local.inverse())
}

/// A new path holding the world outline `world`, looking like `painter`, just above
/// `anchor` in its parent, named `name` or else a free *Path* name.
pub(crate) fn placed_path(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    anchor: NodeId,
    painter: NodeId,
    world: &BezPath,
    name: Option<String>,
) -> Result<(Transaction, NodeId), OpError> {
    let node = doc.get(anchor).ok_or(OpError::NoSuchNode(anchor))?;
    let parent = node.parent().ok_or(OpError::CannotModifyRoot)?;
    let siblings = doc
        .get(parent)
        .ok_or(OpError::NoSuchNode(parent))?
        .children();
    let at = siblings
        .iter()
        .position(|c| *c == anchor)
        .ok_or(OpError::MalformedSubtree)?;
    let path = parent_world(doc, res, anchor)?.inverse() * world.clone();
    let kind = NodeKind::Path {
        path,
        corner_radii: Vec::new(),
        corner_smoothing: 0.0,
        mirror: crate::symmetry::PathMirror::NONE,
    };
    let name = name.unwrap_or_else(|| {
        crate::naming::free_name(&doc.child_names(parent, &[]), kind.default_name(), 1)
    });
    let new = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: new,
        parent,
        index: siblings.len(),
        kind,
        transform: None,
        name: Some(name),
    }];
    ops.extend(build::carried_look(doc, painter, new));
    ops.push(Operation::Reorder {
        id: new,
        index: at + 1,
    });
    Ok((Transaction(ops), new))
}

/// The name *Divide*'s group takes.
pub const DIVIDED: &str = "Divided";

/// *Divide*: every face of `selection`'s arrangement becomes a path of its own,
/// looking like and named after the topmost source over it, so nothing moves on
/// screen. The paths go into a group named [`DIVIDED`] in the slot of the
/// outermost selected layer holding the topmost source; the sources go, and so does
/// a selected group nothing is left in.
///
/// `arr` is [`arrange`]'s answer for the same `selection`; one path per region.
pub fn divide(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    selection: &[NodeId],
    arr: &Arrangement,
) -> Result<(Transaction, NodeId), OpError> {
    let outer = build::outermost(doc, selection);
    let top = *arr.sources.last().ok_or(OpError::EmptyGeometry)?;
    // The outermost selected layer the topmost source sits in, or is.
    let holder = outer
        .iter()
        .copied()
        .find(|o| within(doc, *o, top))
        .ok_or(OpError::MalformedSubtree)?;
    let parent = doc
        .get(holder)
        .and_then(|n| n.parent())
        .ok_or(OpError::CannotModifyRoot)?;
    let siblings = doc
        .get(parent)
        .ok_or(OpError::NoSuchNode(parent))?
        .children();
    let at = siblings
        .iter()
        .position(|c| *c == holder)
        .ok_or(OpError::MalformedSubtree)?;
    let to_parent = parent_world(doc, res, holder)?.inverse();

    let group = ids.mint();
    let mut ops = vec![
        Operation::CreateNode {
            id: group,
            parent,
            index: siblings.len(),
            kind: NodeKind::Group,
            transform: None,
            name: Some(DIVIDED.to_string()),
        },
        Operation::Reorder {
            id: group,
            index: at + 1,
        },
    ];
    // A selected group left empty goes whole; any other source goes alone.
    let consumed: rustc_hash::FxHashSet<NodeId> = arr.sources.iter().copied().collect();
    let emptied: Vec<NodeId> = outer
        .iter()
        .copied()
        .filter(|o| only_sources_in(doc, *o, &consumed))
        .collect();
    // One selected group that goes and held every source: *Divided* stands in for
    // it and wears its opacity, blend, effects, mask and place in a layout.
    let stand_in = (emptied == [holder] && arr.sources.iter().all(|s| within(doc, holder, *s)))
        .then_some(holder);
    if let Some(g) = stand_in.and_then(|g| doc.get(g)) {
        ops.extend(build::carried_look(doc, g.id(), group));
        ops.extend(build::carried_mask(g, group));
        if g.insets().is_authored() {
            ops.push(Operation::SetInsets {
                id: group,
                insets: *g.insets(),
            });
        }
        if *g.item() != crate::container::LayoutItem::default() {
            ops.push(Operation::SetLayoutItem {
                id: group,
                item: *g.item(),
            });
        }
    }
    // What the parents a face leaves composited over it, less what its new ones
    // do: opacity folds into the face. A clip or an effect on such a parent is
    // not carried.
    let chain = |from: Option<NodeId>, skip: Option<NodeId>| -> f64 {
        let mut product = 1.0;
        let mut at = from;
        while let Some(id) = at {
            if Some(id) != skip {
                product *= f64::from(doc.drawn_opacity(id));
            }
            at = doc.get(id).and_then(|n| n.parent());
        }
        product
    };
    let over_group = chain(Some(parent), None);
    // Bottom first, by the topmost source over each face, so the stack reads as
    // the sources did.
    let mut order: Vec<usize> = (0..arr.regions.len()).collect();
    order.sort_by_key(|&i| arr.regions[i].covers.last().copied());
    for (k, i) in order.into_iter().enumerate() {
        let painter = arr.top_source(i).ok_or(OpError::EmptyGeometry)?;
        let new = ids.mint();
        let name = doc.get(painter).map(|n| n.name().to_string());
        ops.push(Operation::CreateNode {
            id: new,
            parent: group,
            index: k,
            kind: NodeKind::Path {
                path: to_parent * arr.regions[i].path.clone(),
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: crate::symmetry::PathMirror::NONE,
            },
            transform: None,
            name,
        });
        ops.extend(build::carried_look(doc, painter, new));
        let parents = doc.get(painter).and_then(|n| n.parent());
        if over_group > 0.0 {
            let own = f64::from(doc.drawn_opacity(painter));
            let folded = (own * chain(parents, stand_in) / over_group).clamp(0.0, 1.0);
            if (folded - own).abs() > 1e-6 {
                ops.push(Operation::SetOpacity {
                    id: new,
                    opacity: folded as f32,
                });
            }
        }
        let over = arr.regions[i].covers.iter().rev();
        if let Some(credit) = face_credit(doc, over.filter_map(|s| arr.sources.get(*s)), &emptied) {
            ops.push(Operation::SetCredit {
                id: new,
                credit: Some(credit),
            });
        }
    }
    for gone in &emptied {
        ops.push(Operation::DeleteNode { id: *gone });
    }
    for s in &arr.sources {
        if !emptied.iter().any(|g| within(doc, *g, *s)) {
            ops.push(Operation::DeleteNode { id: *s });
        }
    }
    Ok((Transaction(ops), group))
}

/// The credit a face *Divide* makes owes, since it is still the credited asset:
/// the nearest one on a source over it, topmost first, or on a group that goes with
/// that source (in `emptied`).
fn face_credit<'a>(
    doc: &Document,
    over: impl Iterator<Item = &'a NodeId>,
    emptied: &[NodeId],
) -> Option<Box<crate::credit::Credit>> {
    for &source in over {
        let gone_with = emptied.iter().find(|g| within(doc, **g, source));
        let mut at = Some(source);
        while let Some(id) = at {
            let node = doc.get(id)?;
            if let Some(credit) = &node.credit {
                return Some(credit.clone());
            }
            if Some(&id) == gone_with || gone_with.is_none() {
                break;
            }
            at = node.parent();
        }
    }
    None
}

/// Whether `id` is `outer` or sits somewhere inside it.
fn within(doc: &Document, outer: NodeId, id: NodeId) -> bool {
    let mut at = Some(id);
    while let Some(a) = at {
        if a == outer {
            return true;
        }
        at = doc.get(a).and_then(|n| n.parent());
    }
    false
}

/// Whether `id` is a group every leaf of which is in `sources`, so nothing would be
/// left in it.
fn only_sources_in(doc: &Document, id: NodeId, sources: &rustc_hash::FxHashSet<NodeId>) -> bool {
    let Some(node) = doc.get(id) else {
        return false;
    };
    if !matches!(node.kind(), NodeKind::Group) {
        return false;
    }
    node.children()
        .iter()
        .all(|c| sources.contains(c) || only_sources_in(doc, *c, sources))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cutter reaching 10¹² units is followed only near the faces, in a bounded
    /// number of points, and still crosses them where it is drawn.
    ///
    /// flip: `clip_segment` flattening without halving to `reach` → fails at the
    /// point count.
    #[test]
    fn a_huge_cutter_is_followed_only_near_the_faces() {
        let s = 1e12;
        let mut cutter = BezPath::new();
        cutter.move_to((-s, 0.0));
        cutter.curve_to((-s, s), (s, -s), (s, 0.0));
        let reach = kurbo::Rect::new(-100.0, -100.0, 100.0, 100.0);
        let trails = trail_points(&cutter, reach);
        let points: usize = trails.iter().map(Vec::len).sum();
        assert!(points < 10_000, "{points} points");
        assert!(!trails.is_empty(), "the cutter crosses the box");
        let near = trails
            .iter()
            .flatten()
            .any(|p| p.x.abs() < 100.0 && p.y.abs() < 100.0);
        assert!(near, "and is followed inside it");
    }
}
