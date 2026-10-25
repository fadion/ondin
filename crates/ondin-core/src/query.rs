//! Read-only queries over `Resolved` + `Document` (§5.10).
//!
//! `hit_test` narrows to R-tree candidates, then runs an EXACT geometric test
//! (point-in-path for fills, distance-to-stroke for lines, box test for text) in
//! each node's local space — never bounds-only — and returns matches
//! topmost-first, skipping nodes that are effectively hidden or locked.

use crate::document::Document;
use crate::geometry;
use crate::id::NodeId;
use crate::resolve::Resolved;
use kurbo::{Point, Rect, Shape};

/// Indexed nodes on `page` whose world bounds intersect `view`, for culling.
/// Unordered.
pub fn nodes_in_view(res: &Resolved, page: NodeId, view: Rect) -> Vec<NodeId> {
    res.candidates_intersecting(page, view)
}

/// Topmost-first nodes on `page` whose painted area contains `p` (world space),
/// skipping nodes that are hidden or locked (directly or via an ancestor).
///
/// `slop` is a picking allowance in **world** units, and only the kinds with nothing but
/// a stroke to aim at spend it (see [`geometry::contains_local`]). It is the caller's
/// number because only the caller knows the zoom: the canvas passes a screen distance
/// divided by it, so a line is equally grabbable at 25% and at 400%, which no constant in
/// world units can be. Pass `0.0` for an exact test.
pub fn hit_test(res: &Resolved, doc: &Document, page: NodeId, p: Point, slop: f64) -> Vec<NodeId> {
    // The allowance widens the candidate query too: a horizontal line's cached bounds
    // are zero-height, so a point slightly off it would never reach the exact filter.
    // Every added candidate still has to pass its own geometry.
    let point_area = Rect::new(p.x - slop, p.y - slop, p.x + slop, p.y + slop);
    let mut hits: Vec<NodeId> = res
        .candidates_intersecting(page, point_area)
        .into_iter()
        .filter(|id| is_effectively_interactable(doc, *id))
        .filter(|id| !masked_away(doc, res, *id, p))
        .filter(|id| {
            let Some(world) = res.world_transform(*id) else {
                return false;
            };
            let Some(node) = doc.get(*id) else {
                return false;
            };
            let local_p = world.inverse() * p;
            // A boolean is tested against its derived outline, which only the
            // `Resolved` cache holds (`contains_local` cannot build it), and under
            // the node's own fill rule (§15 D239): an `Exclude` is even-odd, so a
            // winding test would hit its holes.
            if let Some(path) = res.boolean_path(*id) {
                return node.fill_rule().contains(path, local_p);
            }
            // With no outline, the failure placeholder is the target (§15 D298):
            // `boolean_placeholder` is `None` for a merely empty boolean, so a
            // target exists exactly where something is drawn.
            if let Some(b) = boolean_placeholder(doc, res, *id) {
                return b.inflate(slop, slop).contains(local_p);
            }
            let Some(kind) = res.used_kind(doc, *id) else {
                return false;
            };
            geometry::contains_local(
                node,
                &doc.resolved_paint(*id, node.paint()),
                kind,
                local_p,
                res.text_layout(*id),
                slop,
            )
        })
        .collect();

    // Topmost first: later paint order = larger child-index path.
    hits.sort_by_key(|id| std::cmp::Reverse(paint_path(doc, *id)));
    hits
}

/// World bounds of a node, if known.
pub fn bounds(res: &Resolved, id: NodeId) -> Option<Rect> {
    res.world_bounds(id)
}

/// A node's box in its **own local space**, including containers.
///
/// `geometry::local_bounds` answers this for the drawable kinds and `None` for
/// `Group`/`Root`, whose extent is their children's. This unions those children
/// *through their own transforms*, so the result is a box that rotates with the
/// group instead of the axis-aligned world union `Resolved` caches.
///
/// A selection outline drawn from the world AABB of a rotated group would be a
/// rectangle the group does not have. No stroke expansion: this is the geometry box
/// the inspector's W/H reports and `resize_to_handle` edits.
///
/// Deliberately uncached (§15 D778): the chrome asks it a handful of times a frame,
/// but `session::preview_local_box` re-walks the children itself whenever an
/// override is live, so a cache would help only the resting frame, not a drag. A
/// cache would also be a seventh derived map for `update` to keep equal to
/// `rebuild` (§5.9 says the count of six is load-bearing). Revisit with a real
/// document of ~10k nodes under one selection, or a way to cache through the
/// preview door.
pub fn local_box(doc: &Document, res: &Resolved, id: NodeId) -> Option<Rect> {
    let node = doc.get(id)?;
    // The **used** kind and each child's **used** local (§15 D868): the box that is
    // drawn, not the one the user typed. The resize tools edit from it too (§15
    // D874), and `build::keep_insets` turns the result into insets for a pinned layer.
    if let Some(b) = geometry::local_bounds(res.used_kind(doc, id)?, res.text_layout(id)) {
        return Some(b);
    }
    // A group with a layout is the box its layout made (§15 D869), padding included.
    if let Some(s) = res.used_frame(id) {
        return Some(Rect::from_origin_size(kurbo::Point::ZERO, s));
    }
    // A boolean's box is its result's, not its consumed operands'. This sits ahead
    // of the child union rather than in `local_bounds`, which is a pure function of
    // the kind.
    if let Some(path) = res.boolean_path(id) {
        return (!path.is_empty()).then(|| path.bounding_box());
    }
    // Masked the same way `Resolved`'s world box is, or handles and snap would
    // disagree about where a masked container ends.
    let mut mask_box: Option<Rect> = None;
    node.children()
        .iter()
        .filter_map(|c| {
            let child = doc.get(*c)?;
            let local = res.used_local(doc, *c)?;
            let b = local_box(doc, res, *c).map(|b| geometry::transform_rect(local, b));
            if child.mask() {
                // Not `b` (§15 D460): a mask group narrows its own box with its
                // inner mask but clips with the un-narrowed union that
                // `Resolved::mask_path` builds. Local rather than shared because
                // `Resolved`'s helper is in world space.
                mask_box =
                    mask_extent_local(doc, res, *c).map(|m| geometry::transform_rect(local, m));
                // And nothing is contributed (§15 D494): a mask is a clip, not an
                // area.
                return None;
            }
            match mask_box {
                Some(m) => b
                    .map(|b| b.intersect(m))
                    .filter(|i| i.x1 >= i.x0 && i.y1 >= i.y0),
                None => b,
            }
        })
        .reduce(|a, b| a.union(b))
}

/// The box `id` clips with when it is a mask, in its own local space:
/// `resolve::mask_parts` folded over [`local_box`]. Separate from the world-space
/// fold to avoid an inverse per child.
fn mask_extent_local(doc: &Document, res: &Resolved, id: NodeId) -> Option<Rect> {
    match crate::resolve::mask_parts(doc, id)? {
        crate::resolve::MaskParts::Members(cs) => cs
            .into_iter()
            .filter_map(|c| {
                let local = res.used_local(doc, c)?;
                mask_extent_local(doc, res, c).map(|b| geometry::transform_rect(local, b))
            })
            .reduce(|a, b| a.union(b)),
        crate::resolve::MaskParts::Own => local_box(doc, res, id),
    }
}

/// The box a boolean draws its failure placeholder in, in its own local space, or
/// `None` for a working boolean and every other kind.
///
/// `Some` exactly when [`Resolved::boolean_failed`] is (§15 D298): operands that
/// genuinely cancel draw nothing correctly and get no cross. The box is the
/// operands' union, [`local_box`]'s fallthrough when there is no cached outline.
/// The scene walk, hit test, `Resolved`'s bounds and the SVG writer all ask it, so
/// drawing, click target and cull box are one rectangle.
pub fn boolean_placeholder(doc: &Document, res: &Resolved, id: NodeId) -> Option<Rect> {
    res.boolean_failed(id)
        .then(|| local_box(doc, res, id))
        .flatten()
}

/// Where `id`'s transforms pivot, in world space.
///
/// The composition of [`local_box`], the node's own [`crate::Pivot`] and its world
/// transform, so core's gestures ([`crate::build::flip`]) and the app ask the same
/// question of the committed tree. The app's canvas reads the same three factors
/// from the preview layer to track a gesture in flight.
pub fn pivot_world(doc: &Document, res: &Resolved, id: NodeId) -> Option<Point> {
    let node = doc.get(id)?;
    let local = local_box(doc, res, id)?;
    Some(res.world_transform(id)? * geometry::pivot_point(node.pivot(), local))
}

/// The unbroken run of group-like ancestors above `id` — groups, booleans and
/// the frames between them — **outermost first**, stopping below `inside` when
/// given.
///
/// This is what "clicking a grouped layer selects the group" needs: the hit test
/// finds a leaf and the picker decides how far up to hand the selection. The run
/// stops at the root, and at a frame with no group above it, since a group is the
/// only container the user assembled by hand.
///
/// A frame inside a group is a rung, not a stop (§15 D876): a card frame in a row
/// group is part of the row, so a click on its text selects the row and each
/// double-click steps one level in. Only frames outside the outermost group are
/// dropped, which keeps a top-level frame's contents directly clickable.
///
/// An instance root is a rung of its own whatever its kind (§15 D981 (b)), an
/// exception to §15 D22 only for instance roots.
///
/// `inside` is the group the user has stepped into: it and everything above are
/// not candidates. An `inside` not on the chain leaves the result unchanged, which
/// tells the caller the click landed outside and the isolation should end.
pub fn group_chain(doc: &Document, id: NodeId, inside: Option<NodeId>) -> Vec<NodeId> {
    // Asked of the container the user stepped into, nothing above it is a
    // candidate; a stepped-into frame is clicked on its background (§15 D876), and
    // the walk from its parent would otherwise hand back the row it sits in.
    if Some(id) == inside {
        return Vec::new();
    }
    let mut chain = Vec::new();
    let mut cursor = doc.get(id).and_then(|n| n.parent());
    let mut reached_inside = false;
    while let Some(c) = cursor {
        if Some(c) == inside {
            reached_inside = true;
            break;
        }
        let Some(node) = doc.get(c) else { break };
        if !steps_into(node.kind()) && !is_frame(node.kind()) {
            break;
        }
        chain.push(c);
        cursor = node.parent();
    }
    // Frames above the outermost group are not rungs, so the chain ends where the
    // last group does, unless the walk stopped at `inside` (everything below it is
    // inside a group). An instance root picks like a group (§15 D981 (b)) and ends
    // the trimming.
    let nodes = doc.node_map();
    while !reached_inside
        && chain
            .last()
            .and_then(|c| doc.get(*c))
            .is_some_and(|n| is_frame(n.kind()) && !crate::component::is_instance_root(nodes, n))
    {
        chain.pop();
    }
    chain.reverse();
    chain
}

fn is_frame(kind: &crate::node::NodeKind) -> bool {
    matches!(kind, crate::node::NodeKind::Artboard { .. })
}

/// The kinds [`group_chain`] walks through: the containers a click resolves upward
/// past.
///
/// A boolean behaves as a group: its operands are not drawn (`scene::paint_node`),
/// so a click on one means the boolean; double-clicking still steps inside to edit
/// an operand. A named function so `node`'s matrix can assert the sets it divides
/// `NodeKind` into (§15 D668).
pub(crate) fn steps_into(kind: &crate::node::NodeKind) -> bool {
    matches!(
        kind,
        crate::node::NodeKind::Group | crate::node::NodeKind::Boolean { .. }
    )
}

/// Whether `id` is locked, or sits anywhere inside something that is.
///
/// A locked group locks its contents. Every refusal that protects a layer from
/// being changed asks this, not [`crate::Node::locked`], which answers only about
/// the node's own flag. Locked alone, deliberately: a hidden layer is still
/// editable, since hiding says "do not draw" and locking says "do not change".
///
/// The lock control and its label must read [`crate::Node::locked`] instead:
/// `ToggleLocked` writes the node's own flag, so a row reading this would offer
/// *Unlock* on a layer whose own flag is clear and appear to do nothing.
pub fn is_effectively_locked(doc: &Document, id: NodeId) -> bool {
    let mut cursor = Some(id);
    while let Some(c) = cursor {
        let Some(node) = doc.get(c) else {
            // A node not in the document cannot be edited either; `false` would
            // be a licence, not a refusal.
            return true;
        };
        if node.locked() {
            return true;
        }
        cursor = node.parent();
    }
    false
}

/// Whether `ancestor` is on `id`'s parent chain (or is `id` itself).
pub fn is_within(doc: &Document, id: NodeId, ancestor: NodeId) -> bool {
    let mut cursor = Some(id);
    while let Some(c) = cursor {
        if c == ancestor {
            return true;
        }
        cursor = doc.get(c).and_then(|n| n.parent());
    }
    false
}

/// Whether a click at `p` (world) falls outside what masking leaves of `id`.
///
/// Two readings of one fact: a mask is never hit (it paints nothing; the layers
/// panel and a marquee still reach it through bounds), and masked content is hit
/// only inside its mask. Every ancestor is asked, since a mask two levels up clips
/// this layer just as surely.
fn masked_away(doc: &Document, res: &Resolved, id: NodeId, p: Point) -> bool {
    let mut cursor = id;
    loop {
        let Some(node) = doc.get(cursor) else {
            return false;
        };
        if node.mask() {
            return true;
        }
        let Some(parent) = node.parent() else {
            return false;
        };
        // The mask's outline is in its parent's space: invert the parent's world
        // transform, never the mask's, which would apply its local twice. A mask
        // with no geometry falls through to the next ancestor, since `None` means
        // "masks nothing" and must not stop the runs above being asked.
        if let Some(mask) = doc.governing_mask(cursor)
            && let Some(pw) = res.world_transform(parent)
            && let Some(path) = res.mask_path(doc, mask)
            && !path.contains(pw.inverse() * p)
        {
            return true;
        }
        cursor = parent;
    }
}

/// The node on `page` whose outline passes within `slop` of `p`, nearest first:
/// the query behind the Text tool's *type along this path* affordance (§15 D408).
///
/// [`hit_test`]'s complement: that asks what the pointer is over, this asks what it
/// is on the edge of, the only way a rail can be picked. At a shape's middle
/// `hit_test` reports the shape and this reports nothing. Nearest wins, not
/// topmost, since two outlines can both be within tolerance and the one being
/// traced is meant.
///
/// Excluded: text (its outline must not shadow *edit that text*), frames (a frame's
/// edge is a boundary, not a curve), booleans (a limitation: their outline lives in
/// `Resolved`, not [`geometry::local_path`], which `build::text_on_new_path` also
/// reads; this is the only door onto a rail, §15 D409), and a boolean's operands
/// (§15 D453; a `Subtract`'s subtrahend has an edge on the result's silhouette),
/// enforced by ancestry in the loop below. What governs is a node's role, not its
/// kind (§15 D456): a mask is never picked and masked content only inside its
/// mask, [`masked_away`]'s question as in [`hit_test`]. The two doors deliberately
/// differ on a boolean's operands, which `hit_test` resolves through `group_chain`.
///
/// `slop` is in world units, the caller's number for [`hit_test`]'s reason.
pub fn outline_at(
    res: &Resolved,
    doc: &Document,
    page: NodeId,
    p: Point,
    slop: f64,
) -> Option<NodeId> {
    use crate::node::NodeKind;
    use kurbo::ParamCurveNearest;

    /// How exactly the nearest point on a segment is solved. Loose on purpose: the
    /// answer is thresholded against a band several units wide, and this runs every
    /// frame the tool is in hand.
    const ACCURACY: f64 = 0.1;

    let area = Rect::new(p.x - slop, p.y - slop, p.x + slop, p.y + slop);
    let mut best: Option<(NodeId, f64)> = None;
    for id in res.candidates_intersecting(page, area) {
        if !is_effectively_interactable(doc, id) {
            continue;
        }
        let Some(node) = doc.get(id) else { continue };
        // Text has no outline to type along; the roles that may not become a rail
        // are `text_on_new_path`'s question, not [`is_effectively_interactable`]'s,
        // which must let a boolean's operand through to `group_chain`.
        if matches!(node.kind(), NodeKind::Text { .. }) || !crate::build::is_rail_artwork(doc, id) {
            continue;
        }
        // Nor is ink a mask has clipped away (§15 D456). Behind the cheap
        // rejections on purpose: `mask_path` is the most expensive question here.
        if masked_away(doc, res, id, p) {
            continue;
        }
        let (Some(path), Some(world)) = (
            res.used_kind(doc, id).and_then(geometry::local_path),
            res.world_transform(id),
        ) else {
            continue;
        };
        let mut in_world = path;
        in_world.apply_affine(world);
        // A segment is rejected by its own box before it is solved (§15 D507):
        // `PathSeg::nearest` is a quintic root-find per cubic, and the work scales
        // with segments, not the nodes the candidate query bounds. The box is the
        // control hull, which contains the curve, so only segments that were going
        // to lose are rejected. The benchmark is on
        // `a_many_segment_outline_answers_the_same_with_the_segment_reject`.
        let mut d2 = f64::INFINITY;
        // The squared distance a segment has to beat: the band to begin with,
        // then the nearest found so far.
        let mut cap = slop * slop;
        for seg in in_world.segments() {
            // The control hull, not `PathSeg::bounding_box()`, whose per-axis
            // root-find gives back half of what the reject is worth.
            let b = seg_hull(&seg);
            // Squared distance from `p` to the box, zero inside it. Written out
            // because `inflate(…).contains(p)` is half-open and would reject a
            // segment lying exactly on the tolerance.
            let dx = (b.x0 - p.x).max(p.x - b.x1).max(0.0);
            let dy = (b.y0 - p.y).max(p.y - b.y1).max(0.0);
            if dx * dx + dy * dy > cap {
                continue;
            }
            let n = seg.nearest(p, ACCURACY).distance_sq;
            if n < d2 {
                d2 = n;
                cap = n;
            }
        }
        if d2 <= slop * slop && best.is_none_or(|(_, b)| d2 < b) {
            best = Some((id, d2));
        }
    }
    best.map(|(id, _)| id)
}

/// The box of a segment's control points, a conservative bound on the curve.
///
/// A Bézier lies inside its control points' hull, so a reject on this box never
/// changes the answer; too generous only costs a wasted solve. It is four min/max
/// pairs, where `PathSeg::bounding_box()` solves the derivative's roots per axis.
fn seg_hull(seg: &kurbo::PathSeg) -> Rect {
    use kurbo::PathSeg;
    let pts: &[Point] = match seg {
        PathSeg::Line(l) => &[l.p0, l.p1],
        PathSeg::Quad(q) => &[q.p0, q.p1, q.p2],
        PathSeg::Cubic(c) => &[c.p0, c.p1, c.p2, c.p3],
    };
    let (mut x0, mut y0) = (f64::INFINITY, f64::INFINITY);
    let (mut x1, mut y1) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for q in pts {
        x0 = x0.min(q.x);
        y0 = y0.min(q.y);
        x1 = x1.max(q.x);
        y1 = y1.max(q.y);
    }
    Rect::new(x0, y0, x1, y1)
}

/// A node is interactable only if it and every ancestor are visible and none is
/// locked (Figma: hidden/locked layers don't receive canvas clicks).
///
/// The lock half is [`is_effectively_locked`] rather than a second walk: two
/// spellings of one rule is how §15 D266 happened, and a weaker lock reading
/// always looks like it works.
fn is_effectively_interactable(doc: &Document, id: NodeId) -> bool {
    if is_effectively_locked(doc, id) {
        return false;
    }
    let mut cursor = Some(id);
    while let Some(c) = cursor {
        let Some(node) = doc.get(c) else {
            return false;
        };
        if !doc.shown(c) {
            return false;
        }
        cursor = node.parent();
    }
    true
}

/// Child-index path from the root to `id`. Lexicographic ordering of these
/// paths matches paint order (later = on top).
fn paint_path(doc: &Document, id: NodeId) -> Vec<usize> {
    let mut path = Vec::new();
    let mut current = id;
    while let Some(node) = doc.get(current) {
        let Some(parent_id) = node.parent() else {
            break;
        };
        let parent = doc.get(parent_id).expect("parent exists");
        let index = parent
            .children()
            .iter()
            .position(|c| *c == current)
            .expect("node listed in parent");
        path.push(index);
        current = parent_id;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    //! `group_chain` tests; `hit_test` and `local_box` are exercised through other
    //! suites.

    use super::*;
    use crate::IdSource;
    use crate::node::{BoolOp, NodeKind};
    use crate::op::{Operation, Transaction};
    use kurbo::{Affine, Size};

    fn create(doc: &mut Document, id: NodeId, parent: NodeId, kind: NodeKind) {
        let index = doc.get(parent).map_or(0, |n| n.children().len());
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::IDENTITY),
            name: None,
        }]))
        .expect("the fixture builds");
    }

    fn leaf() -> NodeKind {
        NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
        }
    }

    fn frame() -> NodeKind {
        NodeKind::Artboard {
            size: Size::new(800.0, 600.0),
        }
    }

    /// Root → Artboard → outer `Group` → inner `Group` → a rect, plus a second
    /// artboard's rect well away from all of it.
    struct Nest {
        doc: Document,
        board: NodeId,
        outer: NodeId,
        inner: NodeId,
        rect: NodeId,
        elsewhere: NodeId,
    }

    fn nest() -> Nest {
        let mut ids = IdSource::new(0xC0FFEE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let board = ids.mint();
        let outer = ids.mint();
        let inner = ids.mint();
        let rect = ids.mint();
        let other_board = ids.mint();
        let elsewhere = ids.mint();
        create(&mut doc, board, root, frame());
        create(&mut doc, outer, board, NodeKind::Group);
        create(&mut doc, inner, outer, NodeKind::Group);
        create(&mut doc, rect, inner, leaf());
        create(&mut doc, other_board, root, frame());
        create(&mut doc, elsewhere, other_board, leaf());
        Nest {
            doc,
            board,
            outer,
            inner,
            rect,
            elsewhere,
        }
    }

    /// The chain is outermost first and stops below the frame. The order matters
    /// because `canvas::pick_from_chain` reads the slice positionally. The stop is
    /// asserted by the chain's whole value, since "the frame is absent" would pass a
    /// chain that stopped one group early.
    #[test]
    fn a_group_chain_is_outermost_first_and_stops_below_the_frame() {
        let n = nest();
        assert_eq!(
            group_chain(&n.doc, n.rect, None),
            vec![n.outer, n.inner],
            "outermost first, and the artboard is not a candidate"
        );
        assert_eq!(
            group_chain(&n.doc, n.inner, None),
            vec![n.outer],
            "asked of the inner group, the run is just what is above it"
        );
        assert!(
            group_chain(&n.doc, n.elsewhere, None).is_empty(),
            "a layer sitting straight on a frame has no chain at all"
        );
        assert!(
            group_chain(&n.doc, n.board, None).is_empty(),
            "and neither does the frame"
        );
    }

    /// A frame inside a group is a rung of the chain and a frame outside every
    /// group is not (§15 D876): Root → frame → row `Group` → card frame → text.
    ///
    /// The first assertion pins the plain click, the second stepping into the row
    /// (trimming there would pop the card and skip a level).
    ///
    /// flip: the frame arm dropped from the walk's continue condition → fails at
    /// the first assertion (`[]` against `[row, card]`); the trailing-frame trim
    /// removed → fails there too (`[board, row, card]`); the `id == inside` early
    /// return flipped → fails at the last assertion (`[row]`).
    #[test]
    fn a_frame_inside_a_group_is_a_rung_and_a_page_is_not() {
        let mut ids = IdSource::new(0xCA4D);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let board = ids.mint();
        let row = ids.mint();
        let card = ids.mint();
        let text = ids.mint();
        create(&mut doc, board, root, frame());
        create(&mut doc, row, board, NodeKind::Group);
        create(&mut doc, card, row, frame());
        create(&mut doc, text, card, leaf());

        assert_eq!(
            group_chain(&doc, text, None),
            vec![row, card],
            "the row, then the card; the page is not a rung"
        );
        assert_eq!(
            group_chain(&doc, text, Some(row)),
            vec![card],
            "stepped into the row, the card is the next level"
        );
        assert!(
            group_chain(&doc, text, Some(card)).is_empty(),
            "stepped into the card, the click picks the text itself"
        );
        assert_eq!(
            group_chain(&doc, card, None),
            vec![row],
            "and a click on the card's own body selects the row"
        );
        assert!(
            group_chain(&doc, card, Some(card)).is_empty(),
            "stepped into the card, a click on its body is the card"
        );
    }

    /// A boolean behaves as a group for a click: otherwise the click selects an
    /// operand, which `scene::paint_node` never draws.
    ///
    /// flip: `| NodeKind::Boolean { .. }` deleted from `steps_into` → fails at "a
    /// click on an operand means the boolean" (`[]` against the boolean's id).
    #[test]
    fn a_boolean_is_a_group_as_far_as_a_click_is_concerned() {
        let mut ids = IdSource::new(0xB001);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let board = ids.mint();
        let boolean = ids.mint();
        let operand = ids.mint();
        let other = ids.mint();
        create(&mut doc, board, root, frame());
        create(
            &mut doc,
            boolean,
            board,
            NodeKind::Boolean {
                op: BoolOp::Subtract,
            },
        );
        create(&mut doc, operand, boolean, leaf());
        create(&mut doc, other, boolean, leaf());

        assert_eq!(
            group_chain(&doc, operand, None),
            vec![boolean],
            "a click on an operand means the boolean"
        );
        assert_eq!(
            group_chain(&doc, other, None),
            vec![boolean],
            "and so does a click on the one being subtracted"
        );
    }

    /// Isolation excludes the group stepped into, and leaves the chain unchanged
    /// when the click landed elsewhere, which `canvas::pick_for_click` reads to end
    /// isolation. The two cases fall to different mutations.
    ///
    /// flip: the `inside` break moved below the push → fails at "the group you
    /// stepped into is not a candidate" (`[outer, inner]` against `[inner]`); the
    /// unrelated-`inside` assertion stays green.
    #[test]
    fn stepping_into_a_group_takes_it_and_everything_above_it_off_the_chain() {
        let n = nest();
        assert_eq!(
            group_chain(&n.doc, n.rect, Some(n.outer)),
            vec![n.inner],
            "the group you stepped into is not a candidate, nor is anything above it"
        );
        assert!(
            group_chain(&n.doc, n.rect, Some(n.inner)).is_empty(),
            "stepped all the way in, the click picks the leaf itself"
        );
        assert_eq!(
            group_chain(&n.doc, n.rect, Some(n.elsewhere)),
            group_chain(&n.doc, n.rect, None),
            "an `inside` that is not on the chain leaves it unchanged, which is \
             how the caller knows the click landed outside the isolation"
        );
    }
}
