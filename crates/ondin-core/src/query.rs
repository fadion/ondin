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

/// Indexed nodes whose world bounds intersect `view`, for culling. Unordered.
pub fn nodes_in_view(res: &Resolved, view: Rect) -> Vec<NodeId> {
    res.candidates_intersecting(view)
}

/// Topmost-first nodes whose painted area contains `p` (world space), skipping
/// nodes that are hidden or locked (directly or via an ancestor).
///
/// `slop` is a picking allowance in **world** units, and only the kinds with nothing but
/// a stroke to aim at spend it (see [`geometry::contains_local`]). It is the caller's
/// number because only the caller knows the zoom: the canvas passes a screen distance
/// divided by it, so a line is equally grabbable at 25% and at 400%, which no constant in
/// world units can be. Pass `0.0` for an exact test.
pub fn hit_test(res: &Resolved, doc: &Document, p: Point, slop: f64) -> Vec<NodeId> {
    // **The allowance widens the candidate query as well as the exact test**, and it has
    // to: a horizontal line's cached bounds are a zero-height rectangle, so a point even
    // slightly off it does not intersect them and the line never reaches the filter below
    // that would have accepted it. Widening here only ever *adds* candidates — every one
    // still has to pass its own geometry — so the cost is a few more point-in-shape tests
    // and the exactness is unchanged.
    let point_area = Rect::new(p.x - slop, p.y - slop, p.x + slop, p.y + slop);
    let mut hits: Vec<NodeId> = res
        .candidates_intersecting(point_area)
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
            // A boolean is hit-tested against its derived outline, which
            // `contains_local` cannot build — it is a function of the subtree, not
            // of the kind, so the cache is the only place it exists. Without this a
            // boolean is unclickable on canvas and can only be selected from the
            // layers tree.
            // **Under the node's own rule** (§15 D239). An `Exclude` is even-odd and
            // its outline is its operands concatenated, so a winding test would
            // report every hole in it as a hit — the shape would be clickable
            // precisely where it draws nothing.
            if let Some(path) = res.boolean_path(*id) {
                return node.fill_rule().contains(path, local_p);
            }
            // **And against its placeholder when there is no outline to test.** The
            // grey cross is ink like any other, so it has to be clickable — a drawing
            // you can see and cannot select is worse than the blank it replaced
            // (§15 D298). `boolean_placeholder` is `None` for a boolean that is
            // merely empty, so this adds a target exactly where one is drawn.
            if let Some(b) = boolean_placeholder(doc, res, *id) {
                return b.inflate(slop, slop).contains(local_p);
            }
            geometry::contains_local(node, local_p, res.text_layout(*id), slop)
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
/// That distinction is the whole reason this exists: a selection outline drawn
/// from the world AABB of a rotated group is a rectangle the group does not
/// have, and resize handles on it move axes the group does not own.
///
/// No stroke expansion. This is the *geometry* box — the one the inspector's
/// W/H reports and the one `resize_to_handle` edits — so handles drawn from it
/// land exactly where a drag will take them.
///
/// 🚨 **Uncached, and the chrome asks it fourteen times a frame — measured, not
/// grepped** (§15 D741, `[S4.1-L4-08]`). Counting calls through one real
/// `eframe::App::ui` frame on a headless app, with a 200-child group selected:
/// **2,814 calls for 201 nodes, i.e. exactly 14.00 top-level asks**, each of which
/// re-walks the whole subtree; **0 with nothing selected**. The asks are fourteen
/// different functions across `canvas.rs`, `rulers.rs`, `inspector.rs` and
/// `picker.rs`, so they are not a hoist away from being one.
///
/// ⚠️ **That multiplies the cost by fourteen and the finding's own ranking did not
/// account for it.** `[S4.1-L4-08]` measured *one* call — 0.349 ms at 16,000
/// children — and reasoned *"a fraction of a 16 ms frame, so this is ranked Low
/// deliberately"*. Timed in `--release`, 20 reps, fourteen walks per frame:
///
/// ```text
/// children   per frame
///  1,000      0.169 ms
///  4,000      0.789 ms
/// 16,000      5.400 ms     <- a third of a 16 ms budget
/// ```
///
/// **The finding predicted this correction and asked for it**: its confidence line
/// reads *"medium on the per-frame total — the call sites were enumerated by grep,
/// not driven through a headless frame… That is what would raise it."* It is
/// raised. The fix it names is a new map on [`Resolved`], filled by the same
/// children-before-parents pass as `world_bounds` — **not** derivable from that map,
/// because a world AABB is not a local one for anything rotated, which is the
/// distinction three paragraphs up.
///
/// 🚨 **Ruled on and declined** (§15 D778). The map is **not** built, and the two
/// reasons are what a later reader needs before proposing it again. First, the
/// number above is stale in the direction that matters: §15 D764 found
/// `selection_handles` asking `preview_local_box` for its own box and then asking
/// `preview_pivot`, which walks for the *same* box again — **eight of the fourteen
/// asks were one repair**, so what is left is six, about 0.07 ms at 1,000
/// children. Second, and this is the one that decides it, **the map answers the
/// resting frame only**: `session::preview_local_box` re-walks the children itself
/// whenever an override is live, so a cache buys nothing during a drag, which is
/// the frame a user can feel. Against that it would add a seventh derived map that
/// `update` has to keep equal to `rebuild` — §5.9 holds six and says the count is
/// load-bearing — for a saving that is under the noise on any document this app
/// has drawn.
///
/// ⚠️ **The trigger to revisit is written down rather than left to taste**: a real
/// document with ~10k nodes under one selection, or a way to cache through the
/// preview door. A number of milliseconds is not the trigger; the preview door is.
///
/// 🚨 **And the map would answer the *resting* frame only, which is not the frame
/// that matters.** `session::preview_local_box` forwards here **only** when
/// `overrides.is_empty()`; with a gesture live it re-walks the children itself, so
/// every frame of a drag — exactly when the handles are moving and a budget is felt
/// — would keep paying the walk. `arch-scribe` found that reading the brief against
/// the code; the count above was taken **at rest** and the gesture figure is
/// unmeasured. **So the fix is two doors, not one**, and a repair that does only the
/// cached one would measure well on a still canvas and change nothing under the hand.
pub fn local_box(doc: &Document, res: &Resolved, id: NodeId) -> Option<Rect> {
    let node = doc.get(id)?;
    if let Some(b) = geometry::local_bounds(node.kind(), res.text_layout(id)) {
        return Some(b);
    }
    // **A boolean's box is its result's, not its operands'.** Falling through to the
    // child union below would measure the shapes that were *consumed* — so a
    // subtraction reported a box reaching out to wherever the subtracted circle
    // happened to end, and the selection outline, the dimensions badge and every
    // resize handle were placed against a rectangle the artwork does not have.
    //
    // Ahead of the child union rather than inside `local_bounds`, because this is
    // the derived answer and `local_bounds` is a pure function of the kind.
    if let Some(path) = res.boolean_path(id) {
        return (!path.is_empty()).then(|| path.bounding_box());
    }
    // **Masked the same way `Resolved`'s world box is**, and it has to be: this
    // is the box the selection outline and the resize handles are drawn from, so
    // a container whose world box stopped at its mask and whose local box did not
    // would draw handles in one place and snap to another.
    let mut mask_box: Option<Rect> = None;
    node.children()
        .iter()
        .filter_map(|c| {
            let child = doc.get(*c)?;
            let b = local_box(doc, res, *c).map(|b| geometry::transform_rect(child.transform(), b));
            if child.mask() {
                // **Not `b`** — the same correction `Resolved`'s two bounds
                // passes take (§15 D460): a mask *group* narrows its own box with
                // its own inner mask, and clips with the un-narrowed union that
                // `Resolved::mask_path` builds. Reading its own box here put the
                // handles and the snap at a tenth of the artwork's size on the
                // fixture `[S4.1-L2-01]` measured. Local rather than shared
                // because this whole function is in the node's own space and
                // `Resolved`'s helper is in world.
                mask_box = mask_extent_local(doc, res, *c)
                    .map(|m| geometry::transform_rect(child.transform(), m));
                // **And nothing is contributed** (§15 D494), the third arm of the
                // same correction: a mask is a clip, not an area. Returning `b`
                // put a mask drawn larger than the artwork it passes into the
                // container's own box, so the handles were drawn round emptiness.
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

/// The box `id` **clips with** when it is a mask, in its own local space
/// (§15 D460).
///
/// [`local_box`]'s twin of `resolve::mask_extent`, arm for arm with
/// [`Resolved::mask_path`]: a `Group` skips the masks *inside* it and unions the
/// rest un-narrowed, an invisible node clips with nothing, and every other kind
/// clips with its own box. Separate from the world-space helper because
/// `local_box` works in the node's own space throughout and composing the two
/// would mean an inverse per child.
fn mask_extent_local(doc: &Document, res: &Resolved, id: NodeId) -> Option<Rect> {
    use crate::node::NodeKind;
    let node = doc.get(id)?;
    if !node.visible() {
        return None;
    }
    match node.kind() {
        NodeKind::Group | NodeKind::Root => node
            .children()
            .iter()
            .filter(|c| !doc.get(**c).is_some_and(|n| n.mask()))
            .filter_map(|c| {
                let child = doc.get(*c)?;
                mask_extent_local(doc, res, *c)
                    .map(|b| geometry::transform_rect(child.transform(), b))
            })
            .reduce(|a, b| a.union(b)),
        _ => local_box(doc, res, id),
    }
}

/// The box a boolean draws its **failure placeholder** in, in its own local
/// space — or `None` for every boolean that is working, and every other kind.
///
/// `Some` exactly when [`Resolved::boolean_failed`] is, which is the distinction
/// the whole placeholder rests on (§15 D298): a boolean whose operands genuinely
/// cancel out draws nothing *correctly*, and putting a grey cross over that would
/// mark a shape the user asked for as broken.
///
/// **The operands' union, because it is the only extent the node has left.** Its
/// own outline is what failed, and unioning the operands to get a better one means
/// running the arithmetic that just gave up. [`local_box`] already answers this —
/// its boolean arm falls through to the child union when there is no cached
/// outline — so this is that box with the failure test in front of it, and there
/// is one construction of the box rather than one per renderer.
///
/// Asked by the scene walk, the hit test, `Resolved`'s own bounds and the SVG
/// writer, so the drawing, the click target and the box the canvas culls against
/// are the same rectangle by construction.
pub fn boolean_placeholder(doc: &Document, res: &Resolved, id: NodeId) -> Option<Rect> {
    res.boolean_failed(id)
        .then(|| local_box(doc, res, id))
        .flatten()
}

/// Where `id`'s transforms pivot, in world space.
///
/// The composition of [`local_box`], the node's own [`crate::Pivot`] and its
/// world transform — spelled once so that core's own gestures ([`crate::build::flip`])
/// and the app's ask the same question of the committed tree. The app's canvas
/// answers it from the *preview* layer instead, so that dragging the marker and
/// rotating about it both track a gesture in flight; that is the same three
/// factors read from `EditorSession` rather than a second rule.
pub fn pivot_world(doc: &Document, res: &Resolved, id: NodeId) -> Option<Point> {
    let node = doc.get(id)?;
    let local = local_box(doc, res, id)?;
    Some(res.world_transform(id)? * geometry::pivot_point(node.pivot(), local))
}

/// The unbroken run of `Group` ancestors above `id`, **outermost first**,
/// stopping below `inside` when given.
///
/// This is what "clicking a grouped layer selects the group" needs: the hit test
/// finds a leaf, and the picker has to decide how far back up to hand the
/// selection. The run stops at the first non-group — an artboard or the root —
/// because a group is the only container the user assembled by hand, and it is
/// the only one they expect to move as a unit.
///
/// `inside` is the group the user has stepped into (Figma's double-click
/// isolation): groups at or above it are not candidates, so a click inside picks
/// something *within* it rather than re-selecting the whole thing. An `inside`
/// that is not on the chain leaves the result unchanged, which is exactly the
/// signal the caller needs to know the click landed outside and the isolation
/// should end.
pub fn group_chain(doc: &Document, id: NodeId, inside: Option<NodeId>) -> Vec<NodeId> {
    let mut chain = Vec::new();
    let mut cursor = doc.get(id).and_then(|n| n.parent());
    while let Some(c) = cursor {
        if Some(c) == inside {
            break;
        }
        let Some(node) = doc.get(c) else { break };
        if !steps_into(node.kind()) {
            break;
        }
        chain.push(c);
        cursor = node.parent();
    }
    chain.reverse();
    chain
}

/// The kinds [`group_chain`] walks *through* — the containers a click resolves
/// upward past.
///
/// A boolean behaves as a group here, and has to: its operands are not artwork on
/// the page — nothing draws them (`scene::paint_node`) — so a click landing on one
/// means the boolean, exactly as a click on a grouped layer means the group.
/// Double-clicking still steps inside, which is how an operand gets edited.
///
/// A named function rather than the `matches!` that was inline in the loop (§15
/// D668): it is one of six kind predicates in this crate that answer `false` for a
/// variant nobody remembered to add, and an expression buried in a `while` was the
/// one of the six no test could name. `node`'s matrix asserts the pair of sets it
/// divides `NodeKind` into.
pub(crate) fn steps_into(kind: &crate::node::NodeKind) -> bool {
    matches!(
        kind,
        crate::node::NodeKind::Group | crate::node::NodeKind::Boolean { .. }
    )
}

/// Whether `id` is locked, **or sits anywhere inside something that is**.
///
/// **A locked group locks its contents.** That is what a lock on a container
/// means rather than a convenience laid on top of it: a group is one thing to the
/// hand that locked it, so nothing inside it is editable while it holds. Every
/// refusal that protects a layer from being *changed* asks this, not
/// [`crate::Node::locked`], which answers only about the node's own flag.
///
/// **Locked alone, deliberately — a hidden layer is still editable.** The two
/// flags travel together through [`is_effectively_interactable`] because a click
/// needs both, and they are not the same question anywhere else: hiding a layer
/// says "do not draw this", where locking it says "do not change this". A text
/// layer switched off is still a text layer whose words can be fixed.
///
/// The two callers that must **not** use this are the lock control itself and its
/// label: `ToggleLocked` writes the node's own flag, so a row reading this would
/// offer *Unlock* on a layer whose own flag is already clear and then appear to do
/// nothing. Those read [`crate::Node::locked`]; everything that refuses an *edit* reads
/// this.
pub fn is_effectively_locked(doc: &Document, id: NodeId) -> bool {
    let mut cursor = Some(id);
    while let Some(c) = cursor {
        let Some(node) = doc.get(c) else {
            // A node that is not in the document cannot be edited either, and
            // answering `false` here would hand a caller a licence rather than a
            // refusal. The walk above stops at the root, whose parent is `None`.
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
/// Two things at once, and they are the same fact read from either end:
///
/// - **A mask is never hit.** It paints nothing, so there is nothing on the
///   canvas to aim at; a click where a mask sits belongs to whatever the mask is
///   letting through. It stays selectable from the layers panel, and a marquee
///   still catches it — both of those go through bounds rather than through here.
/// - **Masked content is hit only inside its mask.** Without this the clipped-away
///   half of a photograph is still clickable, which is the exact complaint that
///   makes an invisible layer feel like a bug: a click on empty canvas selects a
///   picture that is not there.
///
/// Every ancestor is asked, not just the node's own parent, because a mask two
/// levels up clips this layer just as surely — the run it governs is a run of
/// *groups*, and what is inside them travels with them.
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
        // The mask's outline is in its parent's space, so the point has to be
        // too — one inverse of the *parent's* world transform, never the mask's,
        // which would apply the mask's own local a second time.
        //
        // A mask with no geometry, or a parent with no transform, falls through
        // to the next ancestor rather than out of the walk: `mask_path`
        // answering `None` means "masks nothing" everywhere else, and a run that
        // clips nothing must not also stop the runs above it being asked.
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

/// The node whose **outline** passes within `slop` of `p`, nearest first — the
/// query behind the Text tool's *type along this path* affordance (§15 D408).
///
/// **[`hit_test`]'s complement, not a variant of it.** That one asks what the
/// pointer is *over*; this asks what it is *on the edge of*, which is the only
/// question a rail can be picked by — a curve is its edge. The difference shows at
/// a shape's **middle**: `hit_test` reports the shape and this reports nothing, so
/// the Text tool plants an ordinary node in the middle of a circle and writes
/// along it at the rim.
///
/// ⚠️ **Not because an unfilled shape has no interior to hit** — that was the first
/// reading of why this was needed and it is wrong here. This app's hit test does
/// not consult the fill, so an unfilled ellipse is clickable anywhere inside it.
/// The reason is the one above: an edge is a different place from an inside, and
/// only one of them names a curve.
///
/// **Nearest wins, not topmost**, where `hit_test` is deliberately the other way:
/// two outlines can both be inside the tolerance — concentric circles, one shape
/// crossing another — and the one the pointer is actually tracing is the one meant,
/// where paint order would answer with whichever happened to be drawn last.
///
/// ⚠️ **Text and frames are excluded, each for its own reason.** A text node's
/// outline must not shadow *edit that text*, which is the same tool's other answer
/// to the same click. A frame is a page: its edge is a boundary rather than a
/// drawn curve, and consuming a page to make a rail of it is not something a click
/// should be able to do. A **boolean** is excluded too and that one is a
/// limitation rather than a decision — its outline is derived into `Resolved`
/// rather than coming out of [`geometry::local_path`], which is also what
/// `build::text_on_new_path` reads. **This is the only door onto a rail now**
/// (§15 D409), so widening it is one change in one place rather than two that
/// have to agree.
///
/// ⚠️ **And a boolean's *operands* are excluded, which is a fourth exclusion and
/// not part of the third** (§15 D453). The boolean itself is out for the
/// limitation just described; its children are out because they are operands
/// rather than artwork, and a `Subtract`'s subtrahend has an edge lying exactly
/// on the result's silhouette. The rule is enforced by ancestry inside the loop
/// below, and the reasoning for where it sits is there — this list is the summary
/// a reader trusts, so it is stated in both places rather than only the one.
///
/// ⚠️ **The list above is about *kinds*, and the property that actually governs
/// is a node's ROLE** (§15 D456). An operand, a **mask**, and ink a mask has
/// clipped away are all ordinary kinds that are not artwork on the page, and
/// D408's exclusion list — written when only the kinds were known — could not say
/// so. The last two are now [`masked_away`]'s question, asked here as it is in
/// [`hit_test`]: **a mask is never picked, and masked content is picked only
/// inside its mask.** `[S4.1-L1-02]` is what that omission cost.
///
/// ⚠️ **D408's *"both doors agree"* was never true and is still not.** The two
/// doors agree about masks and about clipped-away ink from D456 on; they
/// deliberately **disagree** about a boolean's operands, because `hit_test` is
/// supposed to return one and resolve it through `group_chain` (D453). Read the
/// two claims separately.
///
/// `slop` is in **world** units, the caller's number for [`hit_test`]'s reason.
pub fn outline_at(res: &Resolved, doc: &Document, p: Point, slop: f64) -> Option<NodeId> {
    use crate::node::NodeKind;
    use kurbo::ParamCurveNearest;

    /// How exactly the nearest point on a segment is solved. Loose on purpose: the
    /// answer is thresholded against a band several units wide, so a tenth of a
    /// unit of slack cannot change which side of it a point falls on — and this
    /// runs over every candidate outline on every frame the tool is in hand.
    const ACCURACY: f64 = 0.1;

    let area = Rect::new(p.x - slop, p.y - slop, p.x + slop, p.y + slop);
    let mut best: Option<(NodeId, f64)> = None;
    for id in res.candidates_intersecting(area) {
        if !is_effectively_interactable(doc, id) {
            continue;
        }
        let Some(node) = doc.get(id) else { continue };
        if matches!(
            node.kind(),
            NodeKind::Artboard { .. } | NodeKind::Text { .. }
        ) {
            continue;
        }
        // **A boolean's operands are not outlines anybody may aim at** (§15 D453).
        // They are in the pick index — `build_index` filters on `is_indexed(kind)`
        // alone, which knows nothing about ancestry — and a `Subtract`'s
        // subtrahend has an edge lying *exactly* on the result's silhouette,
        // because that is what a boolean is. So the Text tool's rail affordance
        // lit up on the operand and `text_on_new_path` then consumed it.
        //
        // ⚠️ **Here and not in [`is_effectively_interactable`]**, which walks the
        // same chain and would be the tidier home: `hit_test` shares that
        // function and is *supposed* to return operands, resolving them to their
        // container through `group_chain` afterwards. Folding this in there would
        // make a boolean unclickable.
        //
        // Before the `local_path` build and the nearest-point solve, so on a
        // rejected candidate it saves that work rather than adding to it.
        if has_boolean_ancestor(doc, id) {
            continue;
        }
        // **A mask is not a rail, and neither is ink the mask has clipped away**
        // (§15 D456). `[S4.1-L1-02]`: this asked two of [`hit_test`]'s three
        // questions and not the third, so the *same* two clicks answered
        // differently through the two doors.
        //
        // Measured on the suite's own `mask_fixture` — a group holding a 50×50
        // rect marked *Use as mask* under a 500×400 rect. Clicking the mask's
        // edge: `hit_test` answered the artboard and this answered **the mask**,
        // so the rail affordance lit up on a layer that paints nothing;
        // `text_on_new_path` then deleted it and the group's bounds went
        // `50×50` → `-11,-11..500,400`, the artwork silently unmasked with the
        // status line saying *"the shape became the rail"*. Clicking the masked
        // rect's edge out in the clipped-away region: this answered the rect where
        // the canvas shows nothing at all.
        //
        // ⚠️ **Behind [`is_effectively_interactable`] and behind the two skips
        // above, on purpose.** This walks the ancestor chain and asks
        // `mask_path` — the most expensive question in the loop — and the loop
        // runs over every candidate outline on every frame the Text or Pen tool is
        // in hand. The cheap rejections come first.
        if masked_away(doc, res, id, p) {
            continue;
        }
        let (Some(path), Some(world)) =
            (geometry::local_path(node.kind()), res.world_transform(id))
        else {
            continue;
        };
        let mut in_world = path;
        in_world.apply_affine(world);
        // **A segment is rejected by its own box before it is solved**
        // (§15 D507, `[S4.1-L4-05]`). `PathSeg::nearest` is a quintic root-find
        // per cubic, and the candidate query above bounds this loop in **nodes**
        // while the work is denominated in **segments** — one imported path is
        // enough. Linear with no reject, and `rail_at` asks this for the hover
        // affordance and again for the click, so a frame can pay it twice. On a
        // 500-unit ring at `slop = 4`, **2 of 10,000** segments are within
        // tolerance and the other 9,998 each paid a solve to be discarded.
        //
        // ⚠️ **The before-and-after table lives on
        // `a_many_segment_outline_answers_the_same_with_the_segment_reject`**,
        // in one place rather than two: `[S4.1-L4-05]`'s own figures and the
        // re-run beside this fix differ in the fourth decimal, and three copies
        // of a benchmark is three things to keep true. **0.160 → 0.041 ms/call
        // at 2,000 segments** is the number to carry.
        //
        // The threshold starts at `slop²` and narrows to the nearest distance
        // found so far, so the box test is against the band until something is
        // in hand and against that afterwards.
        //
        // **Exact where it matters**: a segment whose box is within the threshold
        // is still solved, so the answer is the same for every segment that could
        // have produced it. The box is the segment's control hull, which contains
        // the curve — so this can only reject segments that were going to lose.
        let mut d2 = f64::INFINITY;
        // The squared distance a segment has to beat: the band to begin with,
        // then the nearest thing found so far.
        let mut cap = slop * slop;
        for seg in in_world.segments() {
            // ⚠️ **The control hull, not `PathSeg::bounding_box()`.** The tight
            // box is a root-find of its own — `ParamCurveExtrema` per axis — and
            // measured it gave back only half of what the reject was worth. The
            // hull is four min/max pairs over points already in hand, and it
            // *contains* the curve, so it is sound as a reject: a segment it
            // admits is still solved exactly.
            let b = seg_hull(&seg);
            // Squared distance from `p` to the box, zero inside it. Written out
            // rather than `inflate(…).contains(p)`, which is half-open and would
            // reject a segment lying exactly on the tolerance.
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

/// The box of a segment's **control points** — a conservative bound on the curve
/// (§15 D507).
///
/// A Bézier lies inside the convex hull of its control points, so this contains
/// the segment and is never tighter than it. That is the property a reject needs:
/// too generous costs a solve that was going to be discarded, and too tight would
/// change the answer.
///
/// `PathSeg::bounding_box()` is the tight box and would be the obvious call. It
/// is not free — it solves the derivative's roots per axis — and measured on the
/// fixture `[S4.1-L4-05]` names it gave back about **half** of what the reject was
/// worth. This is four min/max pairs over points the segment already holds.
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

/// Whether any ancestor of `id` is a [`crate::node::NodeKind::Boolean`].
///
/// Its own kind is not consulted: a boolean *is* an outline and clicking its edge
/// is exactly the gesture — what must not be aimed at is a node **inside** one.
fn has_boolean_ancestor(doc: &Document, id: NodeId) -> bool {
    let mut cursor = doc.get(id).and_then(|n| n.parent());
    while let Some(c) = cursor {
        let Some(node) = doc.get(c) else { return false };
        if matches!(node.kind(), crate::node::NodeKind::Boolean { .. }) {
            return true;
        }
        cursor = node.parent();
    }
    false
}

/// A node is interactable only if it and every ancestor are visible and none is
/// locked (Figma: hidden/locked layers don't receive canvas clicks).
///
/// **The lock half is [`is_effectively_locked`] rather than a second walk**, which
/// is a deliberate correction: this function held the only ancestor-aware reading
/// of the lock, privately, so every refusal outside the canvas asked
/// [`crate::Node::locked`] instead and stopped at the node's own flag. Two
/// spellings of one rule is how §15 D266 happened — three sites asking the same
/// question and only one asking the shared answer — and a lock is the worst place
/// to repeat it, because the weaker reading always looks like it works.
///
/// ⚠️ **This comment was sitting on [`outline_at`] until 2026-09-07**, two hundred
/// lines away and above a function it says nothing about, while this one had no
/// doc at all. Found by `CLAUDE.md`'s doc-run length ranking — not because the
/// theft was new but because `outline_at`'s own doc grew past the head of the
/// list (§15 D453, D456) and dragged the merged block into view. **The ranking
/// finds accumulated damage, and what makes it visible is a later edit nearby.**
fn is_effectively_interactable(doc: &Document, id: NodeId) -> bool {
    if is_effectively_locked(doc, id) {
        return false;
    }
    let mut cursor = Some(id);
    while let Some(c) = cursor {
        let Some(node) = doc.get(c) else {
            return false;
        };
        if !node.visible() {
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
    //! **This module did not exist** (`[A5-L6-06]`, §15 D503).
    //!
    //! `group_chain` is the basis of every click-to-select decision in the
    //! editor, and `grep -rn "group_chain" crates/ --include=*.rs` returned five
    //! lines: the `pub use` in `lib.rs`, the definition, and three production
    //! call sites in `canvas.rs`. No test in the workspace named it, and
    //! `query.rs` carried no test module at all — its coverage figure comes from
    //! `hit_test` and `local_box` being exercised through other suites.
    //!
    //! No gate could have helped: it is a `pub` item in `ondin-core`, and
    //! `dead_code` never fires on one however many callers it has.

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

    /// **Outermost first, and the run stops at the frame** — the two halves of
    /// what *"clicking a grouped layer selects the group"* means.
    ///
    /// The order is not cosmetic: `canvas::pick_from_chain` reads the slice
    /// positionally (`[_outermost, next, ..]`), so a chain built the other way
    /// round would make a plain click select the innermost group and Ctrl+Alt the
    /// outermost — both gestures still doing *something*, which is why nothing
    /// downstream would have noticed.
    ///
    /// The frame stopping the run is asserted by the chain's **whole value**
    /// rather than by the artboard's absence from it: an assertion that the frame
    /// is not in the list would also pass a chain that stopped one group early.
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

    /// **A boolean behaves as a group here, and has to** — the arm `CLAUDE.md`
    /// names as a `NodeKind`-sweep hazard, and the one member of that list with
    /// no test at all until now.
    ///
    /// Deleting `| NodeKind::Boolean { .. }` makes a click select the **operand**,
    /// which `scene::paint_node` never draws: the user selects something
    /// invisible, and the boolean stops being draggable as a unit. The function's
    /// own comment is the specification — *"its operands are not artwork on the
    /// page … so a click landing on one means the boolean"*.
    ///
    /// **Flip run**, that alternative deleted: fails on *"a click on an operand
    /// means the boolean"* at `[]` against the boolean's id — the predicted site.
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

    /// **Isolation excludes the group you stepped into, and says nothing when the
    /// click landed elsewhere** — two boundaries, and the second is the signal
    /// `canvas::pick_for_click` reads to *end* isolation.
    ///
    /// ⚠️ **Both cases are needed and they fall to different mutations.** Moving
    /// the `if Some(c) == inside { break; }` below the `chain.push(c)` re-selects
    /// the group you stepped into on the very next click — Figma's isolation mode
    /// stops working — and that is invisible to the third assertion, where
    /// `inside` is never on the chain at all.
    ///
    /// **Flip run**, the break moved below the push: fails on *"the group you
    /// stepped into is not a candidate"* at `[outer, inner]` against `[inner]`,
    /// the predicted site. The unrelated-`inside` assertion stays green under it,
    /// which is what says the two are independent rather than one written twice.
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
