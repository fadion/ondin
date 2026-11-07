//! The shape builder and *Divide*: the selection's outlines cut into the faces of
//! their planar arrangement ([`crate::boolean::regions`]), which a drag merges or
//! removes and *Divide* turns each into a path of its own.
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
    /// The faces, in world space; [`Region::covers`] indexes [`Self::sources`].
    pub regions: Vec<Region>,
    /// Whether the selection held layers the arrangement leaves out: text,
    /// pictures, frames, open paths and lines.
    pub left_out: bool,
}

/// Why there is no [`Arrangement`] to build with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Fewer than two closed shapes in the selection.
    TooFew,
    /// The shapes are there but none overlaps another.
    NoOverlap,
    /// The arithmetic unwound ([`boolean::failures`]).
    Abandoned,
}

/// Whether `id` gives the arrangement an outline: a visible, unlocked closed shape,
/// path or boolean outside any boolean and any instance.
pub(crate) fn takes_part(doc: &Document, id: NodeId) -> bool {
    let Some(node) = doc.get(id) else {
        return false;
    };
    if crate::is_effectively_locked(doc, id)
        || !doc.shown(id)
        || node.paint().image_fill().is_some()
    {
        return false;
    }
    if crate::component::instance_root(doc, id).is_some() {
        return false;
    }
    let mut up = node.parent();
    while let Some(p) = up.and_then(|p| doc.get(p)) {
        if matches!(p.kind(), NodeKind::Boolean { .. }) {
            return false;
        }
        up = p.parent();
    }
    match node.kind() {
        NodeKind::Rect { .. }
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Boolean { .. } => true,
        NodeKind::Path { path, .. } => is_closed(path),
        _ => false,
    }
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
/// bottom of the stack first; and whether any selected layer was left out.
fn sources_of(doc: &Document, selection: &[NodeId]) -> (Vec<NodeId>, bool) {
    let mut found = Vec::new();
    let mut left_out = false;
    let mut stack: Vec<NodeId> = build::outermost(doc, selection);
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if matches!(node.kind(), NodeKind::Group) {
            stack.extend(node.children().iter().copied());
        } else if takes_part(doc, id) {
            found.push(id);
        } else if doc.shown(id) {
            left_out = true;
        }
    }
    (build::all_in_document_order(doc, &found), left_out)
}

/// `id`'s outline in world space, as drawn.
pub(crate) fn world_outline(doc: &Document, res: &Resolved, id: NodeId) -> Option<BezPath> {
    Some(res.world_transform(id)? * res.local_path(doc, id)?)
}

/// Whether `selection` holds two or more closed shapes the arrangement would cut —
/// the cheap test a menu row dims on, with no arithmetic.
pub fn can_arrange(doc: &Document, selection: &[NodeId]) -> bool {
    sources_of(doc, selection).0.len() >= 2
}

/// Cut `selection`'s closed shapes into their faces.
pub fn arrange(
    doc: &Document,
    res: &Resolved,
    selection: &[NodeId],
) -> Result<Arrangement, Refusal> {
    let (candidates, left_out) = sources_of(doc, selection);
    let (sources, outlines): (Vec<NodeId>, Vec<BezPath>) = candidates
        .into_iter()
        .filter_map(|id| Some((id, world_outline(doc, res, id)?)))
        .filter(|(id, _)| {
            res.world_transform(*id)
                .is_some_and(|w| w.determinant() != 0.0)
        })
        .unzip();
    if sources.len() < 2 {
        return Err(Refusal::TooFew);
    }
    let regions = boolean::regions(&outlines).ok_or(Refusal::Abandoned)?;
    if !regions.iter().any(|r| r.covers.len() > 1) {
        return Err(Refusal::NoOverlap);
    }
    Ok(Arrangement {
        sources,
        outlines,
        regions,
        left_out,
    })
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
            // Nothing left deletes the source; an unwind refuses the whole edit.
            let left = match evaluated(BoolOp::Subtract, &[self.outlines[j].clone(), set.clone()]) {
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
    }
    // A selected group left empty goes whole; any other source goes alone.
    let consumed: rustc_hash::FxHashSet<NodeId> = arr.sources.iter().copied().collect();
    let emptied: Vec<NodeId> = outer
        .iter()
        .copied()
        .filter(|o| only_sources_in(doc, *o, &consumed))
        .collect();
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
