//! Semantic SVG export (§7.1).
//!
//! A pure function of `Document` + `Resolved`. The output mirrors the document:
//! groups and artboards become nested `<g>` elements carrying their *local*
//! transform and opacity, and shapes inside them are emitted in local
//! coordinates. That is what makes the result editable in another tool rather
//! than a flat pile of pre-transformed primitives — and it is also the only way
//! group opacity is correct, since `opacity` on a `<g>` composites the group as
//! a unit instead of fading each child separately.
//!
//! Artboards clip their contents with a `clipPath`, matching the canvas — and
//! omit it when the layer's `clip` flag is off, for the same reason.
//!
//! Doubles as a model-completeness gate: every `NodeKind` and every paint the
//! model can express — including gradients, which get real
//! `<linearGradient>`/`<radialGradient>` defs — round-trips to valid SVG.
//! Anything the model can say that this cannot write is a gap worth failing on.
//!
//! Text is emitted as `<text>` elements — one per **line**, each holding one
//! `<tspan>` per style run (§15 D81) — so an SVG viewer re-shapes the characters
//! with its own fonts. That is deliberate (it keeps the output editable) and means
//! SVG text metrics can differ from the canvas, which shapes with parley. What is
//! *not* left to the viewer is where the lines fall: line breaking and each
//! baseline are ours, taken from `text::export_lines`, because a viewer given one
//! long string would break it at its own width and place nothing where the canvas
//! did.

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, BezPath, Insets, Rect, Shape};
use ondin_core::peniko::{Color, Extend, GradientKind};
use ondin_core::{
    Decoration, Document, Effect, EffectKind, Fill, Framing, LineStyle, MaskMode, Node, NodeId,
    NodeKind, Resolved, Shadow, Stroke, StrokeAlign, geometry,
};
use std::fmt::Write;

/// An export and what it could not say exactly — the writer's side of
/// `svg_in::Import`'s `skipped`/`approximated` (§15 D780, `[S8.1-L7-05]`).
///
/// **The reader has had this since it was written and the writer has not**, which
/// is the whole finding: `svg_in` carries two lists precisely so a lossy *read*
/// can tell the user, and `svg_of` returned a bare `String`, so a lossy *write*
/// could not. The four losses the writer knew about were each written down in a
/// doc comment only a developer would ever read, and the export UI had nothing to
/// show because the function it called could not tell it anything.
///
/// 🚨 **Two lists, because the finding's flat list of four is two different
/// things and telling a user the wrong one sends them looking for the wrong bug.**
/// That is `Import`'s own argument for splitting `skipped` from `approximated`,
/// one step along:
///
/// - [`Self::approximated`] — **the picture differs.** A viewer showing this file
///   draws something that is not quite what the canvas draws.
/// - [`Self::round_trip`] — **the picture is right and the document is not.** The
///   file draws correctly anywhere; what is lost is that reading it back in gives
///   a different document from the one exported.
///
/// ⚠️ **Two of the finding's four are the second kind, by this writer's own
/// argument, and it says so at both sites.** A flipped rail is written as a
/// *reversed path*, and `text::PathWarp::flip` **is** traversing the rail
/// backwards — so the reversed path is the identical drawing rather than a stand-in
/// for it, and the only loss is that `svg_in` reads it back with the flag clear.
/// An image fill's `<pattern>` was measured in Chrome by `[S8.1-L7-05]` itself and
/// lands exactly where the document puts it; the loss is that no reader this
/// project ships reads a pattern back (`[A5-L6-01]`). **Filing either under
/// "approximated" would tell a user their drawing is wrong when it is not.**
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Export {
    /// The markup.
    pub svg: String,
    /// Features written **not exactly**, so a viewer draws something slightly
    /// different from the canvas. Deduplicated, in the order first met — the same
    /// shape as `Import::approximated`, so the two ends of a round trip read alike.
    pub approximated: Vec<String>,
    /// Features written **exactly**, whose *document* does not survive a return
    /// trip through this project's own reader. The file is right; opening it again
    /// gives a document spelled differently.
    pub round_trip: Vec<String>,
}

/// The sink [`emit_node`] pushes into, so the lists are built from what was
/// actually **written**.
///
/// 🚨 **Not from `collect_defs`, which is the pass it is tempting to build it in**
/// — that one already walks every origin, already carries a `&mut` and already
/// sees all five features. It has **no visibility guard**: it recurses into hidden
/// children on purpose, because an unreferenced def is harmless and skipping one
/// would mean a second visibility rule to keep in step. A report built there would
/// announce a sweep gradient on a layer the file does not contain.
#[derive(Default)]
struct Report {
    approximated: Vec<String>,
    round_trip: Vec<String>,
}

impl Report {
    /// The picture differs. Deduplicated by wording, like the reader's.
    fn approximate(&mut self, what: &str) {
        if !self.approximated.iter().any(|s| s == what) {
            self.approximated.push(what.to_string());
        }
    }

    /// The picture is right; the document will not come back.
    fn round_trip(&mut self, what: &str) {
        if !self.round_trip.iter().any(|s| s == what) {
            self.round_trip.push(what.to_string());
        }
    }
}

/// Render the whole document, or a single artboard subtree, to an SVG string.
pub fn svg(doc: &Document, res: &Resolved, artboard: Option<NodeId>) -> String {
    svg_of(doc, res, &[artboard.unwrap_or_else(|| doc.root())])
}

/// Render **these subtrees** to one SVG, in the order given.
///
/// The general form [`svg`] is one call of: a whole-document export is the root
/// on its own, and a single artboard is that artboard on its own. What a set adds
/// is a selection — a viewBox over the union of their world bounds, and each
/// subtree emitted under its own parent's transform so several layers from
/// different frames still land where they sit relative to one another.
///
/// **Order is the caller's and is kept**, because the output has to be
/// byte-stable (invariant 9) and because SVG has no z-index: the last element
/// painted is on top, so a caller handing over a selection owes it in document
/// order or the copy comes back with its stacking rearranged.
///
/// Nothing is deduplicated here. A caller passing both a group and something
/// inside it would emit that child twice; `build::outermost` is the filter that
/// answers it, and it is the caller's because this function has no way to know
/// whether the repetition was meant.
pub fn svg_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> String {
    svg_of_reported(doc, res, origins).svg
}

/// [`svg_of`], and what it could not say exactly — see [`Export`].
///
/// **This is the function; [`svg_of`] is the shim** (§15 D780). It is that way
/// round rather than the other because the report is the *new* thing and every
/// existing caller wants the markup: `plan::bytes`, the CLI, the round-trip suites
/// and thirty-odd tests take a `String`, and changing all of them to say `.svg`
/// would have been churn in the one direction the finding did not ask for. The
/// two callers that face a user — the export panel and *Copy as SVG* — ask for
/// the report.
pub fn svg_of_reported(doc: &Document, res: &Resolved, origins: &[NodeId]) -> Export {
    // `crate::extent`, not a second union: the PNG writer frames from the same
    // function, and an SVG's `viewBox` describing a different box from the PNG's
    // pixel grid is a difference nobody would see until the two were laid over
    // each other. The fallback stays here rather than in the helper — see its docs.
    let view = crate::extent(res, origins).unwrap_or_else(|| Rect::new(0.0, 0.0, 100.0, 100.0));

    // Collect the defs first so the header can carry them; ids are assigned in
    // document order, making output deterministic.
    let mut defs = Defs::default();
    for id in origins {
        collect_defs(doc, res, *id, origin_outer(doc, res, *id), &mut defs);
    }

    let mut out = String::new();
    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}">"#,
        x = fmt(view.min_x()),
        y = fmt(view.min_y()),
        w = fmt(view.width()),
        h = fmt(view.height()),
    );
    out.push('\n');
    defs.emit(&mut out);

    // **The origin's *parent's* world transform, which is what `emit_node`'s
    // `outer` means** — it composes as `outer * node.transform()`, so handing it
    // the node's own world applies the node's local twice.
    //
    // It did, until 2026-08-20, and the symptom was exactly one local transform of
    // slippage: the fixture's board sits at `translate(10, 10)`, the viewBox was
    // written from `world_bounds` as `10 10 400 300`, and the group came out at
    // `translate(20, 20)` — so a tenth of the artboard hung off the right and
    // bottom edges with a blank strip at the top left. Invisible to the test that
    // covered this path, which asserted that a `<rect>` was *present* and never
    // read a number (§15 D258).
    //
    // One expression rather than a `match` on which caller this is, because the
    // root case is the same statement: the root has no parent, so it answers
    // identity.
    let mut report = Report::default();
    for id in origins {
        emit_node(
            &mut out,
            doc,
            res,
            *id,
            origin_outer(doc, res, *id),
            &defs,
            1,
            &mut report,
        );
    }

    out.push_str("</svg>\n");
    Export {
        svg: out,
        approximated: report.approximated,
        round_trip: report.round_trip,
    }
}

/// The `outer` an origin is emitted under — **its parent's world transform**,
/// because `emit_node` composes `outer * node.transform()` and handing it the
/// node's own world would apply the local one twice.
///
/// Lifted out of `svg_of` when the defs pass started needing it too: the filter
/// region for an effect has to be written in the same user space as the element
/// carrying the `filter` attribute, so the two walks have to agree about what
/// that space is. They were one expression duplicated once, which is how they
/// stop agreeing. See the note this replaces, kept below in [`emit_node`]'s
/// caller: the last time these drifted, a tenth of the artboard hung off the
/// edge and the test that covered the path never read a number (§15 D258).
fn origin_outer(doc: &Document, res: &Resolved, id: NodeId) -> Affine {
    doc.get(id)
        .and_then(|n| n.parent())
        .and_then(|p| res.world_transform(p))
        .unwrap_or(Affine::IDENTITY)
}

/// Gradient definitions, keyed by the node they came from so element emission
/// can look them up again without re-deriving names.
#[derive(Default)]
struct Defs {
    /// **Insertion-ordered**, which [`Defs::emit`] depends on: a `<defs>` block
    /// whose contents reshuffled between two exports of one document would break
    /// invariant 9's byte-stability and every golden in §11.
    entries: Vec<(String, String)>,
    /// The same ids again as a set, so [`Defs::add`] and [`Defs::has`] are O(1)
    /// (§15 D593, `[A4-L4-02]`).
    ///
    /// 🚨 **Both were a linear scan of `entries`, and `collect_defs` calls `add`
    /// once per gradient fill, per gradient stroke, per image (three ids), per
    /// effect stack and per text rail — so D defs cost Θ(D²) string
    /// comparisons.** Measured with the node count held **fixed** at 16,000 so
    /// only the def count moved: 21.3 ms at D=1,000, 23.5 at 2,000, 32.5 at
    /// 4,000, 63.4 at 8,000, **166.9 at 16,000** — the excess multiplying by ≈3.4
    /// per doubling, against 4.0 for a pure square. At D=16,000 the quadratic term
    /// is about **87% of export time**; the emitted bytes grow exactly linearly
    /// over the same row (169 KB → 5.49 MB for ×32 nodes), which is what says the
    /// time is not the extra markup. 16,000 gradient fills is ordinary for a
    /// detailed illustration and `svg_in` accepts up to a `nodes_limit` of
    /// 500,000.
    ///
    /// The ids are `grad-{node}-{slot}`, sharing a long common prefix, so every
    /// one of those comparisons ran past it before differing.
    ///
    /// **`std::collections::HashSet` rather than `FxHashSet`**: this crate does
    /// not depend on `rustc_hash` and §3's dependency rules are not worth a hash
    /// function here — the win is O(D) against O(D²), not a constant factor.
    seen: std::collections::HashSet<String>,
}

impl Defs {
    fn add(&mut self, id: String, body: String) {
        if self.seen.insert(id.clone()) {
            self.entries.push((id, body));
        }
    }

    /// Whether a def of this name was collected.
    ///
    /// Asked by [`paint_attr`] before it writes a `url(#…)` for an image. A
    /// gradient always has a def, so it never needs asking; an image only has one
    /// when the document's table can answer for it, and a reference to a paint
    /// server that is not in the file is an SVG error rather than a blank.
    ///
    /// Called **per image paint during emission**, so on the old linear scan this
    /// added an O(nodes × D) term of its own on top of `add`'s square.
    fn has(&self, id: &str) -> bool {
        self.seen.contains(id)
    }

    fn emit(&self, out: &mut String) {
        if self.entries.is_empty() {
            return;
        }
        out.push_str("  <defs>\n");
        for (_, body) in &self.entries {
            out.push_str(body);
        }
        out.push_str("  </defs>\n");
    }
}

/// Stable per-node, per-slot gradient id (`grad-<node>-fill-0`).
///
/// **The slot carries the paint's index**, including when there is only one. A
/// node can hold several fills and several strokes, so the slot has to name
/// *which* — and spelling the first one `fill` while the second is `fill-1`
/// would leave two ways to write the same thing, one of which only appears in
/// documents nobody has stacked paints on yet.
fn gradient_id(node: NodeId, slot: &str) -> String {
    format!("grad-{}-{slot}", node.to_wire().replace(':', "_"))
}

/// Stable per-node, per-slot pattern id (`img-<node>-fill-0`), spelled like
/// [`gradient_id`] because it names the same thing: the paint server one slot's
/// brush resolves to.
fn image_id(node: NodeId, slot: &str) -> String {
    format!("img-{}-{slot}", node.to_wire().replace(':', "_"))
}

/// The letterbox clip's id, for the one framing that needs one.
fn image_clip_id(node: NodeId, slot: &str) -> String {
    format!("ic-{}-{slot}", node.to_wire().replace(':', "_"))
}

/// The adjustment filter's id, for a picture that carries one.
fn adjust_id(node: NodeId, slot: &str) -> String {
    format!("adj-{}-{slot}", node.to_wire().replace(':', "_"))
}

/// The **payload's** id — keyed by the picture rather than by the node that shows
/// it (§15 D595, `[S9.1-L4-03]`).
///
/// 🚨 **Every def was keyed per node per slot, so N shapes sharing one photograph
/// wrote N full base64 copies of it.** Measured in release on one embedded 200 KB
/// image: at one shape the export is 0.27 MB against a 0.27 MB document; at ten,
/// 2.67 MB against 0.28; at **thirty, 8.01 MB against 0.30 — 26.6×**. The
/// document side is the control and it is flat, because `ImageId` is a content
/// hash and the table holds the blob once. The export threw that away exactly.
/// A moodboard, a photo grid and a repeated logo are ordinary documents.
///
/// **The per-node `<pattern>` is not the duplication and stays**: its
/// `patternTransform` carries that node's own `ImageRef::framing`, which really
/// does differ per node. What is hoisted is the payload inside it, which does
/// not.
///
/// ⚠️ **Sanitised, because an `ImageId` is a hash whose spelling this file does
/// not own.** An id that is not an XML name would produce a document no reader
/// can resolve, and the failure would be a picture that silently does not draw.
fn payload_id(image: &ondin_core::ImageId) -> String {
    let safe: String = image
        .0
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("pix-{safe}")
}

/// The effect stack's filter id (§5.3a). One per node, not one per effect: the
/// whole stack composes into a single filter graph.
fn effect_id(node: NodeId) -> String {
    format!("fx-{}", node.to_wire().replace(':', "_"))
}

/// Stable per-node id for a text node's rail (`rail-<node>`), spelled like the
/// three above.
///
/// One per node and no slot, because a text node has exactly one rail
/// (`NodeKind::Text::on_path`).
fn rail_id(node: NodeId) -> String {
    format!("rail-{}", node.to_wire().replace(':', "_"))
}

/// The box a node's effect filter is allowed to paint in, in the user space of
/// the element that will carry the `filter` attribute.
///
/// **A filter region is not optional and its default is wrong for us.** SVG
/// defaults to `-10%, -10%, 120%, 120%` of the object's bounding box, which
/// clips any shadow reaching further than a tenth of the layer's size — a 24px
/// blur on a 40px icon loses most of itself. So the region is written explicitly,
/// from [`Resolved::ink_bounds`], which is the same number the canvas culls
/// against and the raster export frames from — **and then padded by
/// [`inner_working_pad`]**, because `ink_bounds` answers where the ink *lands*
/// and a region has to hold where the primitives are *evaluated*. Those are the
/// same box for every effect except an inner shadow (§15 D742, `[S10.2-L1-01]`);
/// architecture.md §7 held the unpadded sentence and is amended.
///
/// The three spaces are the fiddly part. `ink_bounds` is world; the filter has to
/// be described where the wrapper element sits, which is the node's *parent's*
/// space (or the viewBox's, for a top-level origin). `transform` maps between
/// them, and composing it with the inverse world is the one expression that is
/// right for both cases — see [`origin_outer`].
fn effect_region(
    res: &Resolved,
    id: NodeId,
    transform: Affine,
    effects: &[Effect],
) -> Option<Rect> {
    let ink = res.ink_bounds(id)?;
    let world = res.world_transform(id)?;
    // **A flattened node exports unfiltered rather than not at all** (§15 D508,
    // `[S8.1-L1-03]`). `world.inverse()` divides by the determinant, so a node
    // scaled flat — `Affine::scale_non_uniform(1.0, 0.0)`, which is a resize
    // handle dragged onto the opposite edge — gave a region of `NaN`s, and
    // `transform_rect_bbox` of a `NaN` affine is a `Rect` of `NaN`s rather than
    // `None`, so the def was collected and the `<g filter>` wrapper written. Per
    // the spec an unparseable filter region means the **referencing element is
    // not rendered**, so one layer went silently missing from a file that was
    // otherwise correct — the worse of this finding's two outcomes, because
    // nothing about it looks wrong.
    //
    // `None` here drops the filter and keeps the artwork, which is the fallback
    // `ScenePainter::push_effect_layer`'s own doc argues for: *"it draws the
    // artwork unfiltered rather than dropping it, which is the failure mode worth
    // having"*. `fmt`'s floor would have made the region `0` instead, which is a
    // filter region of no area — the same silent drop with a parseable spelling.
    // 🚨 **`affine_is_invertible`, and the threshold it replaces was the least
    // defensible of the four** (§15 D762). This read
    // `world.determinant().abs() < f64::EPSILON`. `f64::EPSILON` is ≈2.22e-16 and
    // is a **relative** quantity — the gap between 1.0 and the next representable
    // double — where a determinant is in **squared world units**. On a node at
    // millimetre scale the test is effectively `== 0.0`; on one at `1e8` units it
    // can never fire, so the guard silently stopped guarding at large coordinates.
    // It was a number chosen because it is spelled like a small number.
    if !ondin_core::build::affine_is_invertible(world) {
        return None;
    }
    // Two AABBs rather than one, because a rotated node's ink box is axis-aligned
    // in world space and has to be re-boxed after coming back. Conservative in
    // the direction a filter region must be: too large costs a little memory,
    // too small silently crops the effect.
    let region = (transform * world.inverse()).transform_rect_bbox(ink);
    Some(region + Insets::uniform(inner_working_pad(effects)))
}

/// How far outside the layer this stack's **inner** shadows have to be able to
/// look, in the region's own units.
///
/// 🚨 **A filter's working region is not the same quantity as the layer's ink
/// bounds, and an inner shadow is where the difference bites** (§15 D742,
/// `[S10.2-L1-01]`). `EffectKind::escape` answers `Insets::ZERO` for one, quite
/// correctly — §5.3a: however far its blur reaches, *"none of it lands outside"* —
/// but `escape` is about where the ink **lands** and this is about where the
/// primitives have to be **evaluated**. An inner shadow is cast from everything
/// *outside* the layer, and outside the filter region SVG defines every result as
/// transparent black; so with the region set to the ink bounds the
/// `feComponentTransfer tableValues="1 0"` that inverts the alpha had nothing
/// outside the shape to invert.
///
/// On a shape whose ink is its own bounding box — a rect, a frame, an image, a
/// text node — that is every pixel of the region, and the export drew **nothing
/// at all**. Measured in a real browser rather than reasoned from the spec, on a
/// hand-written pair differing only in this rectangle: with the region at the
/// shape, `(255, 255, 255, 255)` at every sample down from the edge; padded, the
/// profile reads 165, 187, 207, 222, 234, 242, 248, 251, 253, 254 — **the same
/// ten bytes `ondin-render`'s `an_inner_shadow_on_a_rect_reaches_all_four_edges`
/// reads off our own renderer**, which is a stronger agreement than that test can
/// ask for on its own.
///
/// ⚠️ **Eleven samples, and the first one disagrees**: the boundary row itself is
/// 172 in the browser against 140 here. That is the shape's own antialiased edge
/// being rasterized by two different rasterizers, not the filter — the ten rows
/// behind it are identical — but the claim is *"the falloff agrees"* and not
/// *"the picture is the same"*, and the difference is exactly where somebody
/// comparing an export against the canvas would look first.
///
/// **Uniform and generous on purpose.** The exact answer is per-side and depends
/// on the offset's direction the way `escape`'s does, but too large costs a
/// little memory where too small silently crops — and the final
/// `feComposite operator="in"` against the layer's own alpha confines the result,
/// so no extra region area can draw. `0.0` for a stack with no inner shadow in
/// it, which is every case that worked before.
fn inner_working_pad(effects: &[Effect]) -> f64 {
    effects
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match &e.kind {
            EffectKind::InnerShadow(s) => {
                Some(ondin_core::effect::reach(s.blur) + s.spread.abs() + s.offset.hypot())
            }
            _ => None,
        })
        .fold(0.0, f64::max)
}

/// The `<filter>` for one node's effect stack.
///
/// **Two stages, not one fold.** `Filters` and `LayerBlur` transform the layer's
/// own appearance, in list order; every shadow is then cast from *that*, and the
/// shadows merge behind and in front of it. The one-fold version — where each
/// effect reads whatever the previous one produced — is the obvious spelling and
/// it is wrong in a way the markup shows plainly: an inner shadow following a
/// drop shadow takes its silhouette from the layer **plus the drop shadow**, so
/// it draws a second inner edge along the outside of the shadow. Casting every
/// shadow from the layer is also what Figma does, and what anyone stacking two
/// shadows at different distances expects.
///
/// **`color-interpolation-filters="sRGB"` is mandatory, not decoration.** The
/// spec's default is `linearRGB`, so leaving it off makes every browser blur and
/// desaturate in a different colour space from our own canvas. `adjust_def` above
/// already pins it for image adjustments, and `ondin_render::effects` pins the
/// same choice for the two backends.
fn effect_def(name: &str, effects: &[Effect], region: Rect) -> String {
    let mut out = format!(
        "    <filter id=\"{name}\" filterUnits=\"userSpaceOnUse\" \
         color-interpolation-filters=\"sRGB\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\">\n",
        fmt(region.x0),
        fmt(region.y0),
        fmt(region.width()),
        fmt(region.height()),
    );
    // Stage one: what the layer itself looks like.
    let mut graphic = "SourceGraphic".to_string();
    for (n, e) in effects.iter().enumerate() {
        if !e.visible {
            continue;
        }
        let r = format!("fx{n}");
        match &e.kind {
            EffectKind::Filters(f) if !f.is_neutral() => {
                let values = ondin_render::effects::filter_matrix(f)
                    .iter()
                    .map(|v| fmt(*v as f64))
                    .collect::<Vec<_>>()
                    .join(" ");
                let _ = writeln!(
                    out,
                    "      <feColorMatrix in=\"{graphic}\" type=\"matrix\" values=\"{values}\" result=\"{r}\"/>"
                );
                graphic = r;
            }
            EffectKind::LayerBlur { radius } if *radius > 0.0 => {
                let _ = writeln!(
                    out,
                    "      <feGaussianBlur in=\"{graphic}\" stdDeviation=\"{}\" result=\"{r}\"/>",
                    fmt(ondin_core::effect::deviation(*radius)),
                );
                graphic = r;
            }
            _ => {}
        }
    }

    // Stage two: the shadows, every one of them cast from `graphic`.
    let mut behind: Vec<String> = Vec::new();
    let mut over: Vec<String> = Vec::new();
    for (n, e) in effects.iter().enumerate() {
        if !e.visible {
            continue;
        }
        let (s, inner) = match &e.kind {
            EffectKind::DropShadow(s) => (s, false),
            EffectKind::InnerShadow(s) => (s, true),
            _ => continue,
        };
        if s.color.components[3] <= 0.0 {
            continue;
        }
        let r = shadow_graph(&mut out, &graphic, s, n, inner);
        if inner { &mut over } else { &mut behind }.push(r);
    }

    if behind.is_empty() && over.is_empty() {
        // **A filter with no primitives is not a no-op in SVG — it renders the
        // element as nothing at all** — so a stack whose every entry was skipped
        // would erase the layer it was meant to decorate rather than leave it
        // alone. `feOffset` with no attributes is the shortest way to say
        // "unchanged".
        if graphic == "SourceGraphic" {
            out.push_str("      <feOffset/>\n");
        }
    } else {
        // Bottom to top: drop shadows, the layer, inner shadows — each group in
        // the order the panel lists them, so two shadows at different distances
        // stack the way the rows are arranged.
        out.push_str("      <feMerge>\n");
        for r in behind.iter().chain(std::iter::once(&graphic)).chain(&over) {
            let _ = writeln!(out, "        <feMergeNode in=\"{r}\"/>");
        }
        out.push_str("      </feMerge>\n");
    }
    out.push_str("    </filter>\n");
    out
}

/// The primitives for one shadow, drop or inner. Returns the name of its result,
/// for the caller's merge.
///
/// The two differ in two places and share the rest, which is why they are one
/// function: an inner shadow's silhouette is inverted, and its result is clipped
/// back into the layer's own alpha instead of standing on its own. Where each
/// one lands in the stack is the caller's business, not this function's.
fn shadow_graph(out: &mut String, graphic: &str, s: &Shadow, n: usize, inner: bool) -> String {
    let a = format!("fxa{n}");
    let t = format!("fxt{n}");
    // The layer's alpha as an opaque black silhouette — SVG's `SourceAlpha`, but
    // taken from `current` rather than the source, since the stack may already
    // have changed what the layer looks like.
    let _ = writeln!(
        out,
        "      <feColorMatrix in=\"{graphic}\" type=\"matrix\" \
         values=\"0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0\" result=\"{a}\"/>"
    );
    let mut src = a.clone();
    if inner {
        // Everything *outside* the layer, which is what an inner shadow casts
        // from. `tableValues="1 0"` is alpha inverted.
        let _ = writeln!(
            out,
            "      <feComponentTransfer in=\"{src}\" result=\"{t}\">\n        \
             <feFuncA type=\"table\" tableValues=\"1 0\"/>\n      </feComponentTransfer>"
        );
        src = t.clone();
    }
    if s.spread != 0.0 {
        // ⚠️ **`feMorphology` is a square kernel**, so a large spread on a curved
        // shape comes out slightly boxier here than on the canvas, which dilates
        // the silhouette properly. It is the only spread primitive SVG has; the
        // alternative is dropping spread from the export entirely, which is worse
        // — a shadow of visibly the wrong size rather than one of very slightly
        // the wrong shape.
        let op = if (s.spread > 0.0) != inner {
            "dilate"
        } else {
            "erode"
        };
        let m = format!("fxm{n}");
        let _ = writeln!(
            out,
            "      <feMorphology in=\"{src}\" operator=\"{op}\" radius=\"{}\" result=\"{m}\"/>",
            fmt(s.spread.abs()),
        );
        src = m;
    }
    let o = format!("fxo{n}");
    let _ = writeln!(
        out,
        "      <feOffset in=\"{src}\" dx=\"{}\" dy=\"{}\" result=\"{o}\"/>",
        fmt(s.offset.x),
        fmt(s.offset.y),
    );
    let mut shape = o;
    if s.blur > 0.0 {
        let b = format!("fxb{n}");
        let _ = writeln!(
            out,
            "      <feGaussianBlur in=\"{shape}\" stdDeviation=\"{}\" result=\"{b}\"/>",
            fmt(ondin_core::effect::deviation(s.blur)),
        );
        shape = b;
    }
    // Colour it. The flood fills the whole region and the composite cuts it to
    // the silhouette — `feDropShadow` would do all of this in one primitive but
    // has no spread and no way to read from `current`.
    let f = format!("fxf{n}");
    let c = format!("fxc{n}");
    let _ = writeln!(
        out,
        "      <feFlood flood-color=\"{}\" flood-opacity=\"{}\" result=\"{f}\"/>",
        hex(s.color),
        fmt(s.color.components[3] as f64),
    );
    let _ = writeln!(
        out,
        "      <feComposite in=\"{f}\" in2=\"{shape}\" operator=\"in\" result=\"{c}\"/>"
    );
    if inner {
        // Confine it to the layer: an inner shadow is drawn *inside* the alpha it
        // came from, and without this the inverted silhouette paints the whole
        // region outside the layer instead.
        let k = format!("fxk{n}");
        let _ = writeln!(
            out,
            "      <feComposite in=\"{c}\" in2=\"{a}\" operator=\"in\" result=\"{k}\"/>"
        );
        k
    } else {
        c
    }
}

/// The paint list's visible entries, paired with their index in it — the index
/// the gradient slot and the alignment clip are named after, so a hidden paint
/// does not renumber the ones after it.
fn visible_fills(node: &Node) -> impl Iterator<Item = (usize, &Fill)> {
    node.paint()
        .fills
        .iter()
        .enumerate()
        .filter(|(_, f)| f.visible)
}

fn visible_strokes(node: &Node) -> impl Iterator<Item = (usize, &Stroke)> {
    node.paint()
        .strokes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.visible)
}

fn collect_defs(doc: &Document, res: &Resolved, id: NodeId, outer: Affine, defs: &mut Defs) {
    let Some(node) = doc.get(id) else { return };
    // The same composition `emit_node` makes, so the region and the element that
    // references it are described in one space rather than two.
    let transform = outer * node.transform();
    if ondin_render::effects::any_ink(node.effects())
        && let Some(region) = effect_region(res, id, transform, node.effects())
    {
        defs.add(
            effect_id(id),
            effect_def(&effect_id(id), node.effects(), region),
        );
    }
    // **A rail is a def because `<textPath>` can only name one**, which is the
    // only reason it is in this pass at all — it is geometry belonging to one
    // element, unlike every other entry here, which is a paint server (§15 D405).
    // In the text node's **own** space, not `transform`'s: the element that
    // references it carries that transform itself, exactly as the glyph
    // coordinates beside it do.
    if let NodeKind::Text {
        on_path: Some(rail),
        on_path_flip,
        ..
    } = node.kind()
    {
        // ⚠️ **A flipped node is written as a *reversed rail*, not as SVG 2's
        // `side="right"`** (§15 D406). Two reasons and the second is the one that
        // decides it. `side` is SVG 2 and unevenly implemented, where a reversed
        // `d` is SVG 1.1 and cannot be got wrong by anybody. And it is not an
        // approximation: our own flip *is* traversing the rail backwards
        // (`text::PathWarp::flip`), so the reversed path is the identical drawing
        // rather than a stand-in for it — the `startOffset` percentages below go on
        // meaning what they meant, measured from the text's own start.
        //
        // The cost is that `svg_in` reads it back as a reversed rail with the flag
        // clear. Same picture, different spelling; said out loud on the reader's
        // side too.
        let d = if *on_path_flip {
            path_d(&rail.reverse_subpaths())
        } else {
            path_d(rail)
        };
        defs.add(
            rail_id(id),
            format!(
                "    <path id=\"{name}\" d=\"{d}\" fill=\"none\"/>\n",
                name = rail_id(id),
            ),
        );
    }
    let mut consider = |brush: &Brush, slot: &str| {
        match brush {
            Brush::Gradient(g) => {
                let name = gradient_id(id, slot);
                // **Only a sweep needs the shape's box, and finding one costs an
                // outline walk** (§15 D647) — so it is asked for on the one kind
                // that reads it rather than on every gradient in the document.
                // The other two carry their own geometry: `Linear` its endpoints,
                // `Radial` its radius.
                let frame = matches!(g.gradient.kind, GradientKind::Sweep(_))
                    .then(|| paint_frame(node, res, id))
                    .flatten();
                let body = gradient_def(&name, g, frame);
                defs.add(name, body);
            }
            // **An image resolves to a `<pattern>`, which is a paint server like
            // a gradient** — so it goes through the same machinery rather than
            // through the element writer, which is what the image plan guessed
            // it would need (§15 D185, which records the reversal).
            // A pattern *can* be named by a `fill` attribute; what an
            // image cannot be is a `fill` attribute's own value.
            //
            // Only when the table can answer for it. A reference the document has
            // no entry for gets no def — never a `url(#…)` pointing at a paint
            // server that is not in the file — and the fill emission draws the
            // missing-image placeholder in its place instead (`emit_missing`,
            // §15 D179), which is the same picture the canvas paints.
            Brush::Image(img) => {
                if let (Some(entry), Some(f)) = (
                    doc.image(&img.image.id),
                    framing_of(doc, node, res, id, brush),
                ) {
                    // The adjustments are a def of their own — a `<filter>` the
                    // `<image>` inside the pattern names — so a neutral picture
                    // carries no filter at all and the markup for one is exactly
                    // what it was before adjustments existed.
                    let filter = img.image.adjust.pipeline().map(|p| {
                        let name = adjust_id(id, slot);
                        defs.add(name.clone(), adjust_def(&name, &p));
                        name
                    });
                    // The payload first, keyed by the picture — one copy however
                    // many nodes show it (§15 D595). `Defs::add` dedupes on the
                    // id, so this is a no-op for every node after the first.
                    let payload = payload_id(&img.image.id);
                    defs.add(payload.clone(), image_payload_def(&payload, entry));
                    let name = image_id(id, slot);
                    let body = image_def(
                        &name,
                        &payload,
                        entry,
                        f.transform,
                        img.sampler.alpha,
                        filter.as_deref(),
                    );
                    defs.add(name, body);
                }
            }
            Brush::Solid(_) => {}
        }
    };
    for (i, f) in visible_fills(node) {
        consider(&f.brush, &format!("fill-{i}"));
    }
    for (i, s) in visible_strokes(node) {
        consider(&s.brush, &format!("stroke-{i}"));
    }
    // A frame's ground used to need a `consider(bg, "bg")` of its own here. It is
    // an entry in `visible_fills` now (§15 D400), so the loop above collects its
    // gradient stops and image patterns under the same `fill-{i}` names every
    // other layer's use.
    //
    // Identity for children, because the node's own `<g transform>` establishes
    // their space — `emit_node`'s recursion says the same thing.
    for child in node.children() {
        collect_defs(doc, res, *child, Affine::IDENTITY, defs);
    }
}

/// The box an image fill is framed in — **the same box the scene walk frames
/// against**, which is the whole of what makes the export match the canvas.
///
/// Three cases, and they are `scene.rs`'s three. The one that bites is **text**:
/// the frame is the *layout's* box, not the glyph outlines' bounding box, so an
/// image-filled headline shows one picture behind the words rather than one
/// cropped to the ink. Reaching for [`outline`] here — which is right for every
/// other consumer in this writer — would silently frame it to the ink instead,
/// and the difference is only visible on a picture you already know.
fn paint_frame(node: &Node, res: &Resolved, id: NodeId) -> Option<Rect> {
    match node.kind() {
        NodeKind::Artboard { size, .. } => Some(Rect::new(0.0, 0.0, size.width, size.height)),
        NodeKind::Text { .. } => res.text_layout(id).map(|l| l.bounds()),
        _ => outline(node, res, id).map(|p| p.bounding_box()),
    }
}

/// How `brush` sits in this node's frame, for the one brush kind that needs
/// telling — [`None`] for a solid, a gradient, an image the document has no
/// entry for, or a node with no frame to speak of.
///
/// The mirror of `scene::framing_of`, and it takes the intrinsic size from the
/// **document's table** for the same reason: that is what the table is for, and
/// an export must not depend on anything having been decoded.
fn framing_of(
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    brush: &Brush,
) -> Option<Framing> {
    let Brush::Image(img) = brush else {
        return None;
    };
    let entry = doc.image(&img.image.id)?;
    let frame = paint_frame(node, res, id)?;
    Some(img.image.framing(frame, entry.width, entry.height))
}

/// One image definition: a `<pattern>` in the node's own local space, wrapping
/// the encoded original as an `<image>`.
///
/// **The tile is the source's intrinsic pixel box**, because that is the space
/// [`ondin_core::ImageRef::framing`] maps *from* — including the flips and
/// quarter turns, which it composes on the right — so the whole framing is the
/// one `patternTransform` and this element never has to know which mode it is.
///
/// **A pattern always tiles and three of the four modes want `Extend::Pad`**,
/// which is not a divergence in the cases the app can author: Fill and Crop cover
/// the frame, so nothing outside the picture is ever painted, and Fit's letterbox
/// is cut by [`with_image_clip`]. What repeats where the canvas would smear an
/// edge pixel is a crop rectangle reaching outside `0..1`, which the gesture
/// cannot produce — `cover_crop` is the widest it zooms out to — and which a
/// hand-edited file can.
///
/// `alpha` is the sampler's, which is what an image fill's opacity *is* (§5.5a).
/// `filter` is the id of an [`adjust_def`] when the picture carries adjustments,
/// and hung on the `<use>` rather than on the `<pattern>` so that it colours the
/// picture and not the shape the pattern ends up filling.
///
/// 🚨 **The payload is a `<use>` of a shared `<image>`** (§15 D595,
/// `[S9.1-L4-03]`) — see [`payload_id`] for the 26.6× this removes. Both
/// attributes that used to sit on the inline `<image>` sit on the `<use>` now,
/// which is where they have to be: they are the *node's*, and the `<image>` is
/// shared by every node showing that picture. `<use>` referencing an `<image>` in
/// `<defs>` is plain SVG 1.1, and `opacity` and `filter` on a `<use>` apply to
/// the content it generates.
///
/// ⚠️ **No `width`/`height` on the `<use>`**, deliberately: those apply only when
/// the reference is to an `<svg>` or a `<symbol>`, and the `<image>` already
/// carries the tile's own size. Writing them would be inert and would read as
/// though they were doing something.
fn image_def(
    name: &str,
    payload: &str,
    entry: &ondin_core::ImageEntry,
    transform: Affine,
    alpha: f32,
    filter: Option<&str>,
) -> String {
    format!(
        "    <pattern id=\"{name}\" patternUnits=\"userSpaceOnUse\" \
         width=\"{w}\" height=\"{h}\"{t}>\n      \
         <use href=\"#{payload}\"{op}{f}/>\n    </pattern>\n",
        w = entry.width,
        h = entry.height,
        t = matrix_attr("patternTransform", transform),
        op = opacity_attr(alpha),
        f = filter.map_or(String::new(), |id| format!(r#" filter="url(#{id})""#)),
    )
}

/// The encoded picture itself, written once per distinct [`ondin_core::ImageId`]
/// however many nodes show it (§15 D595).
///
/// The tile's size is the source's intrinsic pixel box, which is the space
/// [`ondin_core::ImageRef::framing`] maps *from* — so this element is the same
/// for every node and every framing, and the whole per-node difference stays in
/// the `<pattern>`'s `patternTransform`.
fn image_payload_def(name: &str, entry: &ondin_core::ImageEntry) -> String {
    format!(
        "    <image id=\"{name}\" href=\"{href}\" x=\"0\" y=\"0\" \
         width=\"{w}\" height=\"{h}\" preserveAspectRatio=\"none\"/>\n",
        w = entry.width,
        h = entry.height,
        href = escape_xml(&entry.href()),
    )
}

/// The seven adjustments as a `<filter>`: a colour matrix, then a tone curve.
///
/// **This writes what `ImageAdjust::pipeline` says and invents nothing**, which
/// is the whole reason that function has the shape it has: the two stages *are*
/// `feColorMatrix type="matrix"` and `feComponentTransfer type="table"`, and the
/// two pixel backends evaluate the same twenty and thirty-three numbers. So the
/// export is not an approximation of the canvas — it is the same description read
/// by a different reader (§5.5a).
///
/// Two attributes here are load-bearing rather than decorative:
///
/// - **`color-interpolation-filters="sRGB"`**. A filter's default working space is
///   *linear* RGB, so without this a viewer would linearise the picture, run the
///   matrix, and convert back — a visibly different exposure and a visibly
///   different saturation from the canvas, on every adjusted image. It is the one
///   attribute whose absence would be a silent, total mismatch.
/// - **The filter region is the element's own box** (`x`/`y`/`width`/`height` at
///   the object bounding box). The default region is 10% larger on each side,
///   which inside a `<pattern>` would let a tile's filter output bleed into its
///   neighbours.
fn adjust_def(name: &str, p: &ondin_core::AdjustPipeline) -> String {
    let mut out = format!(
        "    <filter id=\"{name}\" color-interpolation-filters=\"sRGB\" \
         x=\"0\" y=\"0\" width=\"1\" height=\"1\">\n",
    );
    if let Some(m) = &p.matrix {
        let values = m
            .iter()
            .map(|v| fmt(*v as f64))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(
            out,
            "      <feColorMatrix type=\"matrix\" values=\"{values}\"/>"
        );
    }
    if let Some(c) = &p.curve {
        let table = c
            .iter()
            .map(|v| fmt(*v as f64))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str("      <feComponentTransfer>\n");
        // The same table on all three channels, and **no `feFuncA`**: the tone
        // curve shapes the picture's light, and running it on alpha would make a
        // shadow lift turn a soft edge opaque.
        for ch in ["R", "G", "B"] {
            let _ = writeln!(
                out,
                "        <feFunc{ch} type=\"table\" tableValues=\"{table}\"/>"
            );
        }
        out.push_str("      </feComponentTransfer>\n");
    }
    out.push_str("    </filter>\n");
    out
}

/// Whether `brush` is a picture this writer cannot draw (§15 D179).
///
/// **The writer's unresolvable set is smaller than a backend's, by
/// construction.** Of the three states the canvas merges, only the dangling id
/// reaches here: a *link* is emitted as an `href` for the viewer to resolve, and
/// bytes that will not decode are the viewer's problem with the encoded original
/// this file hands it — neither is something an SVG has to stand in for. So the
/// placeholder goes out for the one state where the markup itself would have
/// nothing to say.
fn missing_picture(doc: &Document, brush: &Brush) -> bool {
    match brush {
        Brush::Image(img) => doc.image(&img.image.id).is_none(),
        _ => false,
    }
}

/// The missing-image placeholder as markup: a grey ground, then a rim and a
/// cross cut to the shape.
///
/// **The same drawing the canvas paints, off the same `missing_placeholder`** —
/// the ground, the ink, the width and the cross's geometry all come from core,
/// so this cannot drift from `scene::paint_missing` the way two copies of a
/// picture would (§15 D179, and `ImageRef::framing`'s reasoning about who owns
/// arithmetic three writers share).
///
/// The clip is the shape's own outline, which is what keeps an image fill in a
/// circle looking like a circle that lost its picture rather than a rectangle
/// dropped on top of one.
fn emit_missing(
    out: &mut String,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    derived: Option<&BezPath>,
    slot: &str,
    pad: &str,
) {
    let Some(frame) = paint_frame(node, res, id) else {
        return;
    };
    let Some(m) = ondin_core::missing_placeholder(frame) else {
        return;
    };
    let ground = format!(r##" fill="{}""##, hex(m.ground));
    write_element(out, node, derived, pad, "", &ground, "", "");
    let cid = format!("mi-{}-{slot}", id.to_wire().replace(':', "_"));
    // The shape as a clip, so the rim reads as an inner border and the cross
    // stops at the edge — `outline` rather than the box, for a rounded rect and
    // for anything that is not a rectangle at all.
    let Some(shape) = outline(node, res, id) else {
        return;
    };
    emit_missing_ink(out, &m, &shape, &cid, pad);
}

/// The rim and the cross of a placeholder, cut to `shape`.
///
/// **Split out so there is one of it.** Two callers now draw the placeholder — a
/// paint whose picture is missing and a boolean whose arithmetic was abandoned
/// (§15 D298) — and they differ only in what the ground is written as and what
/// the shape is. The ink is identical, and a second copy of these four lines is
/// the drift `emit_missing`'s own doc comment warns about one level up.
fn emit_missing_ink(
    out: &mut String,
    m: &ondin_core::Missing,
    shape: &BezPath,
    cid: &str,
    pad: &str,
) {
    let stroke = format!(
        r##" fill="none" stroke="{}" stroke-width="{}""##,
        hex(m.ink),
        fmt(m.width),
    );
    let _ = writeln!(
        out,
        r#"{pad}<clipPath id="{cid}"><path d="{}"/></clipPath>"#,
        path_d(shape)
    );
    let _ = writeln!(out, r#"{pad}<g clip-path="url(#{cid})">"#);
    let _ = writeln!(out, r#"{pad}  <path d="{}"{stroke}/>"#, path_d(&m.cross));
    let _ = writeln!(out, r#"{pad}  <path d="{}"{stroke}/>"#, path_d(shape));
    let _ = writeln!(out, "{pad}</g>");
}

/// The placeholder for a boolean whose arithmetic was abandoned, or `false` when
/// this node is not one (§15 D298).
///
/// **The markup half of `scene::paint_shape`'s placeholder branch**, and the same
/// relationship [`emit_missing`] has with `scene::paint_missing`: the ground, the
/// ink, the width and the cross all come out of `missing_placeholder`, and the box
/// out of `boolean_placeholder`, so the SVG and the canvas cannot disagree about a
/// drawing neither of them owns.
///
/// The ground is written as a plain `<path>` rather than through `write_element`,
/// which the missing-picture case uses: that helper writes the *node's* element,
/// and a boolean's element is the outline that failed to exist. The box is the
/// only shape there is.
fn emit_failed_boolean(
    out: &mut String,
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    pad: &str,
) -> bool {
    let Some(area) = ondin_core::boolean_placeholder(doc, res, id) else {
        return false;
    };
    let Some(m) = ondin_core::missing_placeholder(area) else {
        return false;
    };
    // Any tolerance: a rectangle's path is four straight lines and flattens to
    // itself.
    let shape = area.to_path(0.1);
    let _ = writeln!(
        out,
        r#"{pad}<path d="{}" fill="{}"/>"#,
        path_d(&shape),
        hex(m.ground)
    );
    let cid = format!("mb-{}", id.to_wire().replace(':', "_"));
    emit_missing_ink(out, &m, &shape, &cid, pad);
    true
}

/// `#rrggbb` for a colour that is always opaque — the placeholder's two.
fn hex(c: Color) -> String {
    let rgba = c.to_rgba8();
    format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b)
}

/// Emit `body` inside the letterbox clip `framing` asked for, or plainly when it
/// asked for none.
///
/// The markup shape of `scene::clipped`, and it exists for the same reason: peniko
/// has no transparent extend and SVG has no non-repeating pattern, so the band
/// beside a *contained* picture has to be cut in both writers or the edge of the
/// picture fills it. Only [`ondin_core::ImageFit::Fit`] with differing aspects
/// ever asks.
fn with_image_clip(
    out: &mut String,
    clip: Option<Rect>,
    id: NodeId,
    slot: &str,
    pad: &str,
    body: impl FnOnce(&mut String, &str),
) {
    let Some(r) = clip else {
        body(out, pad);
        return;
    };
    let cid = image_clip_id(id, slot);
    let _ = writeln!(
        out,
        r#"{pad}<clipPath id="{cid}"><rect x="{x}" y="{y}" width="{w}" height="{h}"/></clipPath>"#,
        x = fmt(r.x0),
        y = fmt(r.y0),
        w = fmt(r.width()),
        h = fmt(r.height()),
    );
    let _ = writeln!(out, r#"{pad}<g clip-path="url(#{cid})">"#);
    body(out, &format!("{pad}  "));
    let _ = writeln!(out, "{pad}</g>");
}

/// The radius the sweep-to-radial approximation is written at: far enough from
/// `center` that the last stop reaches every corner of `frame` (§15 D647).
///
/// ⚠️ **Half the frame's diagonal is the obvious answer and it is wrong.** That
/// covers the shape only from the frame's own centre; a sweep centred on a
/// corner — which is exactly where a sweep is most often put — needs the whole
/// diagonal, and anywhere in between needs something else again. So the four
/// corner distances are measured and the largest kept, which is exact from any
/// centre and costs three more subtractions.
///
/// **In the gradient's own space, not the node's.** `cx`/`cy`/`r` are read
/// *before* `gradientTransform`, and this arm writes `p.center` raw exactly as
/// the radial arm writes `end_center`, so the box has to be pulled back through
/// the brush transform or a squashed gradient gets a radius measured in the
/// wrong units. [`None`] for a singular or non-finite transform, where there is
/// no such pull-back — the caller keeps its literal there.
fn sweep_radius(
    center: ondin_core::kurbo::Point,
    frame: Option<Rect>,
    xform: Affine,
) -> Option<f64> {
    let frame = frame?;
    // **`affine_is_invertible`** (§15 D762), replacing `!det.is_finite() || det ==
    // 0.0` — which was byte-for-byte the same question `svg_in`'s transform fold
    // asked with the operands the other way round, in another crate.
    //
    // 🚨 **This function is why D713's "revisit when a second reader wants an
    // inverse" had already fired when it was written**: it is a third inverting
    // reader of `GradientBrush::transform`, it predates that entry, and it is one
    // of the four guards the entry counted while arguing the count was a reason to
    // wait.
    if !ondin_core::build::affine_is_invertible(xform) {
        return None;
    }
    let inv = xform.inverse();
    let r = [
        (frame.x0, frame.y0),
        (frame.x1, frame.y0),
        (frame.x0, frame.y1),
        (frame.x1, frame.y1),
    ]
    .into_iter()
    .map(|(x, y)| (inv * ondin_core::kurbo::Point::new(x, y)).distance(center))
    .fold(0.0_f64, f64::max);
    (r.is_finite() && r > 0.0).then_some(r)
}

/// One gradient definition. Coordinates are in the node's local space, which is
/// exactly what `gradientUnits="userSpaceOnUse"` means inside the `<g>` the
/// shape sits in.
///
/// `frame` is the shape's own box in that same space, and **only the sweep arm
/// reads it** (§15 D647): a sweep carries a centre and two angles and no radius
/// at all, so the radius of the radial gradient it is approximated by has to
/// come from somewhere, and the only honest source is the shape. [`None`] where
/// the caller had no box to give — a node with no outline paints nothing, so the
/// number is unobservable there — and the arm keeps the literal it always wrote.
fn gradient_def(name: &str, brush: &ondin_core::GradientBrush, frame: Option<Rect>) -> String {
    let g = &brush.gradient;
    // **`gradientTransform`, which is what an elliptical radial gradient *is***
    // (§15 D412). The model's two circles cannot be an ellipse, so the squash
    // lives on the brush; SVG's spelling for the same idea is this attribute, and
    // omitting it would draw round what the document says is a streak. Identity
    // is written as nothing — the attribute's own default — so no existing file's
    // markup moves.
    let xform = if brush.transform == ondin_core::kurbo::Affine::IDENTITY {
        String::new()
    } else {
        let [a, b, c, d, e, f] = brush.transform.as_coeffs();
        format!(
            r#" gradientTransform="matrix({} {} {} {} {} {})""#,
            fmt(a),
            fmt(b),
            fmt(c),
            fmt(d),
            fmt(e),
            fmt(f)
        )
    };
    // ⚠️ **The third door of §15 D455, and the one that was missed.** Nothing
    // clamps a descending ramp on *load*, so a hand-edited `.ondin` — or any file
    // imported before D455 landed — still holds one; the canvas and a PNG export
    // are right, both going through `color::brush_to_backend`, and this wrote
    // `g.stops` verbatim. So the app drew the ramp correctly and handed a browser
    // the flat fill: `[S11.1-L1-02]`'s own symptom, exported. Found by
    // `arch-scribe` reading D455's brief against the code.
    //
    // A clamp here rather than at `io::load` because this is the writer's own
    // question — *what does this file mean* — and the answer is SVG's, which is
    // where the rule came from. The model is untouched, so nothing about the
    // document changes because it was exported.
    let mut ramp = g.stops.clone();
    ondin_core::image::make_stops_monotonic(&mut ramp);
    let stops: String = ramp
        .iter()
        .map(|s| {
            let c = s.color.to_alpha_color::<ondin_core::peniko::color::Srgb>();
            let rgba = c.to_rgba8();
            format!(
                "      <stop offset=\"{}\" stop-color=\"#{:02x}{:02x}{:02x}\"{}/>\n",
                fmt(s.offset as f64),
                rgba.r,
                rgba.g,
                rgba.b,
                alpha_attr("stop-opacity", rgba.a),
            )
        })
        .collect();

    // **`extend` is `spreadMethod`, and omitting it is not the same as writing
    // `pad`.** It is only ever non-default on a gradient that came *in* from SVG —
    // `svg_in` reads it — so a writer that dropped it turned every reflected sheen
    // into a flat band on the way back out, and did it silently, since the round-trip
    // still produced a gradient with the same stops in the same place.
    let spread = match g.extend {
        Extend::Reflect => r#" spreadMethod="reflect""#,
        Extend::Repeat => r#" spreadMethod="repeat""#,
        Extend::Pad => "",
    };

    match &g.kind {
        GradientKind::Linear(p) => format!(
            "    <linearGradient id=\"{name}\" gradientUnits=\"userSpaceOnUse\"{spread}{xform} x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">\n{stops}    </linearGradient>\n",
            fmt(p.start.x),
            fmt(p.start.y),
            fmt(p.end.x),
            fmt(p.end.y),
        ),
        GradientKind::Radial(p) => format!(
            "    <radialGradient id=\"{name}\" gradientUnits=\"userSpaceOnUse\"{spread}{xform} cx=\"{}\" cy=\"{}\" r=\"{}\" fx=\"{}\" fy=\"{}\">\n{stops}    </radialGradient>\n",
            fmt(p.end_center.x),
            fmt(p.end_center.y),
            fmt(p.end_radius as f64),
            fmt(p.start_center.x),
            fmt(p.start_center.y),
        ),
        // A sweep gradient has no SVG 1.1 equivalent; approximate it as a
        // radial one rather than dropping the paint entirely.
        //
        // ⚠️ **The radius came from the shape only from §15 D647; before it this
        // arm wrote `r="50"` as a literal, whatever the shape's size.** The two
        // arms above take their geometry from the gradient and this one had
        // nowhere to take it from — a sweep's payload is a centre and two angles
        // — so a number was invented, and it is the one thing here that does not
        // survive a change of scale: on a 2 000-unit shape the export was a
        // 50-unit blob at the centre with the last stop flooding the rest, and on
        // a 10-unit shape it overflowed the other way. `[S8.1-L1-06]`.
        GradientKind::Sweep(p) => format!(
            "    <radialGradient id=\"{name}\" gradientUnits=\"userSpaceOnUse\"{spread}{xform} cx=\"{}\" cy=\"{}\" r=\"{}\">\n{stops}    </radialGradient>\n",
            fmt(p.center.x),
            fmt(p.center.y),
            fmt(sweep_radius(p.center, frame, brush.transform).unwrap_or(50.0)),
        ),
    }
}

/// Emit a node (if visible) and recurse into its children in z-order.
///
/// Containers become `<g>` elements so their transform and opacity nest the way
/// the model means them to. `outer` is an extra transform folded into this
/// node's own — only non-identity for the root of a single-artboard export.
/// Every way this writer is about to say something inexactly about `node`.
///
/// **One function, read off the node, called once per emitted node** (§15 D780,
/// `[S8.1-L7-05]`). The alternative was a push at each of the five sites, which
/// means threading the sink through `gradient_def`, `effect_def`, `shadow_graph`,
/// `emit_shape`, `emit_strokes` and `write_element` — six signatures, to report
/// things that are all decidable from the node itself.
///
/// 🚨 **The cost of reading it off the node is that this is a *second* statement
/// of each rule**, which is `[A1-L7-07]`'s shape and this project's most-recorded
/// one: the arm that writes the sweep as a radial and the arm here that says so
/// are two places that have to agree. `svg::fidelity_tests` is the gate — it
/// exports a fixture holding one of every feature and checks the report against
/// the **markup**, so a writer that stops approximating (or starts) without this
/// following fails rather than lying quietly.
fn report_node(node: &Node, filter_written: bool, report: &mut Report) {
    // A sweep gradient has no SVG 1.1 equivalent and `gradient_def` writes it as a
    // radial one rather than dropping the paint — see that function's own arm.
    let sweep = |brush: &Brush| {
        matches!(
            brush,
            Brush::Gradient(g) if matches!(g.gradient.kind, GradientKind::Sweep(_))
        )
    };
    let picture = |brush: &Brush| matches!(brush, Brush::Image(_));
    let brushes = || {
        visible_fills(node)
            .map(|(_, f)| &f.brush)
            .chain(visible_strokes(node).map(|(_, s)| &s.brush))
    };
    if brushes().any(sweep) {
        report.approximate("a sweep gradient, written as a radial one");
    }
    if brushes().any(picture) {
        // Not "approximated": `[S8.1-L7-05]` put this very markup through Chrome
        // over HTTP and the picture lands exactly where the document puts it. What
        // no reader this project ships can do is read a `<pattern>` back
        // (`[A5-L6-01]`).
        report.round_trip("an image fill, written as a pattern this app cannot read back");
    }
    // `feMorphology` is a square kernel, so a large spread on a curved shape comes
    // out boxier here than on the canvas — `shadow_graph` says so at the site.
    //
    // 🚨 **Three conditions, and two of them were missing from the first draft of
    // this function.** A spread of zero emits no `feMorphology` at all; an
    // effect with `visible: false` is skipped by `effect_def`'s own loop; and the
    // whole `<filter>` is absent unless `effect_region` could answer, which is
    // what `filter_written` carries in from the caller. Each of the three is a way
    // to warn a user about markup that is **not in the file** — which is the exact
    // failure mode of a report that restates a rule instead of observing it.
    if filter_written
        && node.effects().iter().any(|e| {
            e.visible
                && match e.kind {
                    EffectKind::DropShadow(ref s) | EffectKind::InnerShadow(ref s) => {
                        s.spread != 0.0
                    }
                    _ => false,
                }
        })
    {
        report.approximate("a shadow's spread, drawn with a square kernel");
    }
    if let NodeKind::Text {
        on_path: Some(_),
        on_path_flip,
        ..
    } = node.kind()
    {
        // The lines and baselines are ours, but a `<textPath>`'s glyph positions
        // along the rail are the viewer's — §15 D405's *"what a viewer may
        // therefore differ on"*.
        report.approximate("type on a path, whose glyph placement is left to the viewer");
        if *on_path_flip {
            // **Not an approximation and the site argues it at length**: our flip
            // *is* traversing the rail backwards, so a reversed `d` is the
            // identical drawing rather than a stand-in. `svg_in` reads it back as
            // a reversed rail with the flag clear (§15 D406).
            report.round_trip("flipped type on a path, written as a reversed rail");
        }
    }
}

// **Eight genuinely independent parameters, not a bundle waiting to be made**
// (§15 D780, which added the eighth). The alternative considered and rejected was
// folding `defs` and `report` into one struct purely to get under the counter:
// they are an immutable lookup table and a mutable sink, related only by both
// being pass-wide, and a type that exists to please a lint is worse code than the
// lint. Ten other functions in this workspace carry this allow for the same reason.
#[allow(clippy::too_many_arguments)]
fn emit_node(
    out: &mut String,
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    outer: Affine,
    defs: &Defs,
    depth: usize,
    report: &mut Report,
) {
    let Some(node) = doc.get(id) else { return };
    if !node.visible() {
        return;
    }
    // **The effect stack gets a wrapper of its own, outside everything this node
    // emits.** Not an attribute on the node's own element, and the reason is
    // SVG's rendering order: `filter` is applied *before* `clip-path` on the same
    // element, so a clipping frame's drop shadow would be drawn and then cut off
    // at the frame's edge — the one place a shadow most obviously has to escape.
    // Outside is also the only position that works for a shape, whose markup may
    // be several elements (`emit_stacked`).
    //
    // The wrapper carries no transform, so the region in `effect_region` is
    // written in this node's *parent's* space. The two have to agree and they are
    // computed from the same `transform`.
    let fx = ondin_render::effects::any_ink(node.effects()) && defs.has(&effect_id(id));
    // **Here rather than at the top of the function, and under the visibility
    // guard** (§15 D780): the report is a claim about the file, so it is made
    // where what the file contains is known — a hidden layer is not in it, and
    // neither is a `<filter>` whose region could not be computed.
    report_node(node, fx, report);
    let wrap_pad = "  ".repeat(depth);
    let depth = depth + usize::from(fx);
    if fx {
        let _ = writeln!(out, "{wrap_pad}<g filter=\"url(#{})\">", effect_id(id));
    }
    let close_fx = |out: &mut String| {
        if fx {
            let _ = writeln!(out, "{wrap_pad}</g>");
        }
    };

    let pad = "  ".repeat(depth);
    let transform = outer * node.transform();
    let opacity = node.opacity();

    let is_container = matches!(
        node.kind(),
        NodeKind::Root | NodeKind::Group | NodeKind::Artboard { .. }
    );

    if !is_container {
        emit_shape(out, doc, node, res, id, transform, opacity, defs, &pad);
        close_fx(out);
        return;
    }

    // Root carries nothing of its own; skip its wrapper to keep output tidy.
    let wrap = !matches!(node.kind(), NodeKind::Root);
    let strokes: Vec<(usize, &Stroke)> = visible_strokes(node).collect();
    // **A frame's stroke cannot live inside the group that clips its contents**, so
    // when there is both a clip and a stroke the frame becomes two groups: an outer
    // one carrying the transform and the opacity, an inner one carrying only the
    // `clip-path`, and the stroke between the inner group's close and the outer's.
    // That is the markup shape of the canvas's own arrangement (§15 D144) — over the
    // contents, outside the clip, and inside one composited unit so a translucent
    // frame fades as a whole rather than in two pieces.
    //
    // Only when both are present, so no document that never put a stroke on a frame
    // changes by a byte.
    let split_clip = wrap && !strokes.is_empty() && node.clip();
    // Where the frame's contents sit: one level in from this node, and one more when
    // the clip group is a level of its own.
    let content_depth = depth + usize::from(wrap) + usize::from(split_clip);
    let content_pad = "  ".repeat(content_depth);
    // Where the frame's own border sits: inside the outer group, beside the clip one.
    let own_pad = "  ".repeat(depth + usize::from(wrap));
    if wrap {
        let clip = match node.kind() {
            NodeKind::Artboard { size, .. } if node.clip() => {
                let cid = format!("clip-{}", id.to_wire().replace(':', "_"));
                let _ = writeln!(
                    out,
                    "{pad}<clipPath id=\"{cid}\"><rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\"/></clipPath>",
                    fmt(size.width),
                    fmt(size.height),
                );
                format!(r#" clip-path="url(#{cid})""#)
            }
            _ => String::new(),
        };
        // ⚠️ **On the *outer* `<g>` in the split case**, which is the one that
        // stands for the node: the inner one exists only to hold the clip apart
        // from the transform, and a reader taking the name off it would report a
        // container the document does not have.
        if split_clip {
            let _ = writeln!(
                out,
                "{pad}<g{}{}{}>",
                name_attr(node),
                transform_attr(transform),
                opacity_attr(opacity),
            );
            let _ = writeln!(out, "{own_pad}<g{clip}>");
        } else {
            let _ = writeln!(
                out,
                "{pad}<g{}{}{}{}>",
                name_attr(node),
                transform_attr(transform),
                opacity_attr(opacity),
                clip,
            );
        }
    }

    // A frame's fills are the first thing inside its group, in list order, behind
    // its children — the markup shape of `scene::paint_shape`'s frame arm, and its
    // strokes are written later by `emit_strokes`, outside the clip (§15 D144).
    //
    // **Each is a fill like any other, and both of its awkward cases are the ones
    // a shape's fill has.** A *contained* picture letterboxes inside the frame
    // exactly as it does inside a shape, so it takes the same clip — the frame's
    // own `clip-path` cannot stand in for it, being the whole box the letterbox is
    // part of. And a picture that is *missing* draws the placeholder, through the
    // same `emit_missing` a shape's fill goes through.
    //
    // That second case was written as `fill="none"` first and caught by reading
    // this against the walk: `scene::fill_or_placeholder` takes a frame's fills
    // and a shape's alike, so a frame whose picture had gone showed a grey cross
    // on the canvas, wore the amber glyph in the layers panel — and exported a
    // hole (§15 D179).
    //
    // ⚠️ **The slot name is `fill-{i}` and used to be the literal `"bg"`**
    // (§15 D400). It is the id of the `<clipPath>`/`<pattern>` this rect refers
    // to, so it has to be the same string `collect_defs` wrote — which is now the
    // one `visible_fills` hands both of them.
    if let NodeKind::Artboard { size } = node.kind() {
        for (i, f) in visible_fills(node) {
            let slot = format!("fill-{i}");
            if missing_picture(doc, &f.brush) {
                emit_missing(out, node, res, id, None, &slot, &content_pad);
                continue;
            }
            let clip = framing_of(doc, node, res, id, &f.brush).and_then(|f| f.clip);
            with_image_clip(out, clip, id, &slot, &content_pad, |out, pad| {
                let _ = writeln!(
                    out,
                    "{pad}<rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\"{fill}/>",
                    w = fmt(size.width),
                    h = fmt(size.height),
                    fill = paint_attr("fill", &f.brush, id, &slot, defs),
                );
            });
        }
    }

    // **A mask becomes a `<g clip-path>` over the run of siblings above it** — the
    // markup shape of `scene::MaskRun`, and the mirror this writer is written to
    // be. The mask itself emits no element at all: it is not artwork, so what
    // reaches the file is its outline in a `<clipPath>` and nothing else.
    //
    // `clipPath` is `userSpaceOnUse` by default, and the space in force where the
    // `<g>` is *referenced* is this node's own — which is exactly the space
    // `mask_path` answers in, so no transform is written on either.
    //
    // Non-zero, by writing no `clip-rule` at all: that is SVG's default and the
    // rule the same outline is filled with, on the canvas and here.
    let mut masked = false;
    let mut child_depth = content_depth;
    for child in node.children() {
        if let Some(child_node) = doc.get(*child)
            && child_node.mask()
        {
            if masked {
                let _ = writeln!(out, "{content_pad}</g>");
                masked = false;
                child_depth = content_depth;
            }
            let mid = format!("mask-{}", child.to_wire().replace(':', "_"));
            // ⚠️ **Matched on the node, not on `doc.get(…).map(…)`** (§15 D648,
            // `[S8.1-L3-07]`). `Node::mask_mode` returns a `MaskMode` and never an
            // `Option` of one, so the `Option` this used to match came from the
            // lookup — which the `if let` above has *already* made, two lines up.
            // The dead `None` arm that left behind carried the hidden-mask
            // sentence, which is true of `mask_path` and not of the lookup, so a
            // reader tracing "what happens to a hidden mask" was sent to an
            // unreachable line. Two arms, exactly as `scene.rs`'s walk has, which
            // is the walk this writer is mirrored on.
            match child_node.mask_mode() {
                // **A shape mask is a `<clipPath>`**, which is the right element
                // for it: a hard region, no ink, and no compositing.
                MaskMode::Shape => {
                    // **[`Resolved::mask_path`]'s `None` masks nothing, so the run
                    // simply carries on unwrapped** — a hidden mask, an empty text
                    // node, a group with nothing in it, a boolean whose operands
                    // cancel. D282's rule, and the same reading as the canvas's.
                    // This `if` is where it happens.
                    if let Some(path) = res.mask_path(doc, *child) {
                        let _ = writeln!(
                            out,
                            r#"{content_pad}<clipPath id="{mid}"><path d="{}"/></clipPath>"#,
                            path_d(&path),
                        );
                        let _ = writeln!(out, r#"{content_pad}<g clip-path="url(#{mid})">"#);
                        masked = true;
                        child_depth = content_depth + 1;
                    }
                }
                // **An alpha mask is a `<mask>` holding the layer's own markup**,
                // because what masks is its *ink* and the only way to say that in
                // SVG is to draw it.
                //
                // `mask-type: alpha` is load-bearing and is written as a CSS
                // property rather than the presentation attribute of the same
                // name, which has the poorer support of the two spellings. SVG's
                // `<mask>` defaults to **luminance**, so without it every alpha
                // mask we export would silently become the one mode this app
                // deliberately does not have — and would look right whenever the
                // mask happened to be white.
                //
                // No emptiness test, matching the canvas: an alpha mask that lays
                // down no ink erases the run, and that is the mode being honest.
                //
                // ⚠️ **The `visible()` guard is the other half of §15 D566 and
                // this writer did not have it** (§15 D648). D282's rule opens with
                // the case — *"`None` means masks nothing, never 'clip everything
                // away': a **hidden mask** … answers it"* — and the `Shape` arm
                // honours it through `mask_path`'s first act after its lookup, while this one
                // reached `emit_node`, which returns early for an invisible node.
                // The result was an empty `<mask>` over the run, i.e. **the whole
                // drawing erased**, which is the one reading D282 forbids: hiding a
                // mask would have looked like deleting the artwork. The canvas was
                // repaired in D566 and the export was not, so the same document
                // exported one picture and drew another.
                MaskMode::Alpha if child_node.visible() => {
                    // ⚠️ **The region is written, and SVG's default for it is
                    // wrong for us in exactly the way [`effect_region`]'s doc
                    // says a filter's is** (§15 D458). `<mask>` defaults to
                    // `-10%, -10%, 120%, 120%`, and under `userSpaceOnUse` a
                    // percentage is a fraction of the **viewport** read as a
                    // length in the *current* user space — which here is this
                    // node's local space, not the page. Everything outside the
                    // region is mask value 0, i.e. erased.
                    //
                    // `[S8.1-L1-01]`, measured in Chrome on a 400×400 artboard
                    // holding a group with an alpha mask and a coincident red
                    // rectangle. The drawing came back opaque to x=399 at
                    // identity, **x=219** under `scale(0.5)`, **x=43** under
                    // `scale(0.1)` — exactly `1.1 × 400 × scale`, which pins the
                    // mechanism rather than merely observing a loss — and under
                    // `translate(-1000)` with the children at local `+1000`,
                    // **nothing at all**. That last case costs everything: a
                    // group whose children sit past 110% of the viewport *in the
                    // group's own coordinates* loses its masked artwork entirely,
                    // at any scale, and the file still opens and still looks
                    // deliberate.
                    //
                    // **The mask's own ink box is the right region, not the run's.**
                    // Outside it the mask draws nothing, so its value is 0 there
                    // whatever the region says — an infinite region and this one
                    // give the same picture. Inflated by a tenth, which is SVG's
                    // own margin computed in the space that was meant: a region
                    // too large is inert, and §5.9's rule is that too small is the
                    // one error these bounds may not make.
                    //
                    // `<clipPath>` needs none of this and `MaskMode::Shape` is
                    // therefore untouched; `<mask>` is the third region-bearing
                    // element this writer emits and was the only one written
                    // without one.
                    let region = res.ink_bounds(*child).and_then(|ink| {
                        res.world_transform(id)
                            .map(|w| w.inverse().transform_rect_bbox(ink))
                    });
                    let region = match region {
                        // A mask with no ink erases the run wherever it is
                        // evaluated, so the absent region is already correct and
                        // writing a degenerate one would only be a second way of
                        // saying it.
                        None => String::new(),
                        Some(r) => {
                            let r = r.inflate(r.width() * 0.1, r.height() * 0.1);
                            format!(
                                r#" x="{}" y="{}" width="{}" height="{}""#,
                                fmt(r.x0),
                                fmt(r.y0),
                                fmt(r.width()),
                                fmt(r.height()),
                            )
                        }
                    };
                    let _ = writeln!(
                        out,
                        r#"{content_pad}<mask id="{mid}" maskUnits="userSpaceOnUse"{region} style="mask-type:alpha">"#,
                    );
                    emit_node(
                        out,
                        doc,
                        res,
                        *child,
                        Affine::IDENTITY,
                        defs,
                        content_depth + 1,
                        report,
                    );
                    let _ = writeln!(out, "{content_pad}</mask>");
                    let _ = writeln!(out, r#"{content_pad}<g mask="url(#{mid})">"#);
                    masked = true;
                    child_depth = content_depth + 1;
                }
                // A mask the user has switched off has not been asked to erase
                // anything, so the run above it is written with no wrapper at all
                // — `masked` stays false and the next sibling is emitted at the
                // depth it was already at. The `Shape` arm's spelling of this is
                // `mask_path` answering `None`.
                MaskMode::Alpha => {}
            }
            continue;
        }
        emit_node(
            out,
            doc,
            res,
            *child,
            Affine::IDENTITY,
            defs,
            child_depth,
            report,
        );
    }
    if masked {
        let _ = writeln!(out, "{content_pad}</g>");
    }

    if split_clip {
        let _ = writeln!(out, "{own_pad}</g>");
    }
    // The frame's own border, last inside its group so it covers the contents.
    if !strokes.is_empty() {
        emit_strokes(out, doc, node, res, id, &own_pad, &strokes, defs);
    }

    if wrap {
        let _ = writeln!(out, "{pad}</g>");
    }
    close_fx(out);
}

/// A shape and its paints.
///
/// **One plain element when the node has at most one fill and one centred
/// stroke**, which is what the overwhelming majority of shapes are: a `<rect>`
/// with `fill` and `stroke` attributes, exactly the markup this writer has always
/// produced. Anything else — several fills, several strokes, or a stroke that has
/// to be *built* out of a clipped double-width one because SVG has no alignment
/// either — needs the shape emitted once per paint, and goes to
/// [`emit_stacked`].
#[allow(clippy::too_many_arguments)]
fn emit_shape(
    out: &mut String,
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    transform: Affine,
    opacity: f32,
    defs: &Defs,
    pad: &str,
) {
    // **Before the paints, because it replaces them.** A boolean that could not be
    // computed has no shape for a fill or a stroke to be on, so this is not one more
    // layer of ink over the node's own — it is what the node draws instead (§15
    // D298), exactly as the branch in `scene::paint_shape` is.
    if emit_failed_boolean(out, doc, res, id, pad) {
        return;
    }
    let fills: Vec<(usize, &Fill)> = visible_fills(node).collect();
    let strokes: Vec<(usize, &Stroke)> = visible_strokes(node).collect();
    let derived = res.boolean_path(id);
    let shape = outline(node, res, id);
    // **A letterboxing image is as much a reason to stack as an aligned stroke
    // is**, and for the same reason: the clip belongs to one paint, so it has to
    // wrap that paint's own element rather than the shape's. Asked of the fills
    // and the strokes both, since either can carry a picture.
    let letterboxed = fills
        .iter()
        .map(|(_, f)| &f.brush)
        .chain(strokes.iter().map(|(_, s)| &s.brush))
        .any(|b| framing_of(doc, node, res, id, b).is_some_and(|f| f.clip.is_some()));
    // And so is a picture that is not there: the placeholder is a ground, a rim
    // and a cross — three elements and a clip, where a fill attribute is one
    // word (§15 D179).
    let missing = fills.iter().any(|(_, f)| missing_picture(doc, &f.brush));
    let plain = !letterboxed
        && !missing
        && fills.len() <= 1
        && match strokes.as_slice() {
            [] => true,
            // A per-side stroke is not the shape's outline at all — it is one
            // `<path>` per edge — so it can never ride the shape's own element. Nor
            // can a fitted dash on a multi-subpath path: that is one `<path>` per
            // subpath, each with its own `stroke-dasharray`, and a single element
            // carries a single pattern.
            [(_, s)] => {
                align_clip(node, shape.as_ref(), s).is_none()
                    && geometry::effective_sides(node.kind(), s).is_all()
                    && dash_pieces(shape.as_ref(), s, s.width).is_none()
            }
            _ => false,
        };
    if !plain {
        emit_stacked(
            out, doc, node, res, id, transform, opacity, pad, &fills, &strokes, defs,
        );
        return;
    }

    // The name rides in front of the transform, so every element this writer
    // opens for a node carries it through one string ([`name_attr`]).
    let t = format!("{}{}", name_attr(node), transform_attr(transform));
    let op = opacity_attr(opacity);
    let fill = match fills.first() {
        Some((i, f)) => paint_attr("fill", &f.brush, id, &format!("fill-{i}"), defs),
        // **A text node with no ink at all is black here too**, which the walk has
        // always done and this writer did not: it wrote `fill="none"`, so an
        // unpainted text node was in the file, in the viewBox and invisible. Found
        // while adding text strokes (§15 D145) — the fallback's *condition* is what
        // changed there, and the two had to agree about it before that could be
        // stated. No fill and no stroke, so outlined type keeps its hollow letters.
        None if matches!(node.kind(), NodeKind::Text { .. }) && strokes.is_empty() => {
            r##" fill="#000000""##.to_string()
        }
        None => r#" fill="none""#.to_string(),
    };
    // **`fill-rule` rides with the fill, and is written only when it is not the
    // default** (§15 D239). SVG's own default is `nonzero`, so a node that has never
    // been given a rule adds no attribute and no bytes — the same discipline
    // `NodeDto` follows for the field itself. Emitted even when the fill is `none`:
    // a shape can be stroke-only and still lend its interior to a `clipPath`.
    let fill = with_fill_rule(fill, node);
    let stroke = match strokes.first() {
        Some((i, s)) => stroke_attr(
            id,
            *i,
            s,
            s.width,
            &resolved_dashes(shape.as_ref(), s, s.width),
            defs,
        ),
        None => String::new(),
    };
    write_element(out, node, derived, pad, &t, &fill, &stroke, &op);
}

/// The pieces a fitted dash pattern has to be laid along, or `None` when one
/// pattern says it all — `geometry::dash_fit_pieces` with this writer's
/// already-derived outline.
fn dash_pieces(
    shape: Option<&BezPath>,
    s: &Stroke,
    width: f64,
) -> Option<Vec<(BezPath, Vec<f64>)>> {
    geometry::dash_fit_pieces(s, width, shape?)
}

/// The outline the *paints* are built against: a boolean's derived result, a text
/// node's glyph outlines, or the one the kind describes.
///
/// **Every consumer of a shape's own path in this writer goes through here**, and it
/// is asked once per node rather than once per paint. Reading
/// `geometry::local_path(kind)` directly answers `None` for a boolean *and* for a
/// text node, and the symptoms are silent: an aligned stroke on a boolean exported
/// centred while the canvas drew it inside or outside, a fit-to-corners dash pattern
/// measured nothing and came out at its unscaled period, and outlined type exported
/// with its stroke centred on the glyph edges instead of outside them.
fn outline(node: &Node, res: &Resolved, id: NodeId) -> Option<BezPath> {
    match node.kind() {
        // Glyph outlines, built from the cached layout — the same path the scene walk
        // strokes, so the export cannot clip a text stroke differently (§6.3).
        NodeKind::Text { .. } => res.text_layout(id).map(ondin_core::text::outline),
        _ => match res.boolean_path(id) {
            Some(path) => Some(path.clone()),
            None => geometry::local_path(node.kind()),
        },
    }
}

/// The dash pattern for a stroke drawn on `shape`, at `width`.
///
/// A thin wrapper so the two emit paths cannot forget it: the dot substitution
/// and the fit-to-corners scaling are the model's meaning rather than the
/// canvas's, and an export that skipped them would draw a solid line where the
/// canvas drew dots (§6.3).
fn resolved_dashes(shape: Option<&BezPath>, s: &Stroke, width: f64) -> Vec<f64> {
    match shape {
        Some(path) => geometry::resolved_dashes(s, width, path),
        // No outline to measure. Fitting needs one; the substitution does not,
        // and an empty path is long enough for `fitted_dashes` to leave alone.
        None => geometry::resolved_dashes(s, width, &ondin_core::kurbo::BezPath::new()),
    }
}

/// A shape whose paints do not fit one element: a group carrying the node's
/// transform and opacity, then every visible fill, then every visible stroke.
///
/// **Fills first, then strokes, in list order** — the same z-order the scene walk
/// composites them in, so canvas and export agree about which paint is on top.
///
/// The transform moves up to the group for the reason the aligned-stroke
/// construction always needed it to: the several elements are all the one shape
/// and must sit in the one user space, which is also the space a `clipPath`'s
/// coordinates are read in. Opacity moves with it because it applies to the whole
/// stack, not to each layer of it — fading the paints separately would let the
/// lower ones show through the upper.
#[allow(clippy::too_many_arguments)]
fn emit_stacked(
    out: &mut String,
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    transform: Affine,
    opacity: f32,
    pad: &str,
    fills: &[(usize, &Fill)],
    strokes: &[(usize, &Stroke)],
    defs: &Defs,
) {
    let inner = format!("{pad}  ");
    let derived = res.boolean_path(id);
    // The stacked arm's `<g>` **is** the node — its children are that node's own
    // fills and strokes — so the name goes here and not on any of them.
    let _ = writeln!(
        out,
        "{pad}<g{}{}{}>",
        name_attr(node),
        transform_attr(transform),
        opacity_attr(opacity)
    );
    for (i, f) in fills {
        let slot = format!("fill-{i}");
        if missing_picture(doc, &f.brush) {
            emit_missing(out, node, res, id, derived, &slot, &inner);
            continue;
        }
        let clip = framing_of(doc, node, res, id, &f.brush).and_then(|f| f.clip);
        // Per fill, so one contained picture does not cut the fill under it.
        with_image_clip(out, clip, id, &slot, &inner, |out, pad| {
            // ⚠️ **Through `with_fill_rule`, which this loop used to skip.** The
            // rule was written on `emit_shape`'s single-element arm only, so a
            // shape that took *this* path — two fills, or an aligned stroke, or
            // a letterbox — exported non-zero however it was drawn. The case that
            // matters is a `Boolean` with `BoolOp::Exclude`, whose rule
            // `Node::fill_rule` derives as even-odd unconditionally: give one an
            // inside or outside stroke, which is an ordinary thing to do, and the
            // export read its outline as a **union** — the exclusion's holes
            // filled in — while the canvas drew it correctly. Same document, two
            // pictures, no warning.
            let fill = with_fill_rule(paint_attr("fill", &f.brush, id, &slot, defs), node);
            write_element(out, node, derived, pad, "", &fill, "", "");
        });
    }
    emit_strokes(out, doc, node, res, id, &inner, strokes, defs);
    let _ = writeln!(out, "{pad}</g>");
}

/// `fill` with the node's `fill-rule` on it, when that is not SVG's own default.
///
/// **One spelling, because there are two arms that write a fill** and they
/// diverged: the single-element arm carried the rule and the stacked one did not,
/// so whether an even-odd shape exported as even-odd depended on how many paints
/// it happened to have (§15 D239, D438).
///
/// **Written only when it is not `nonzero`**, which is SVG's default too, so a
/// shape that has never been given a rule adds no attribute and no bytes — the
/// same discipline `NodeDto` follows for the field itself. Emitted even when the
/// fill is `none`: a shape can be stroke-only and still lend its interior to a
/// `clipPath`.
fn with_fill_rule(fill: String, node: &Node) -> String {
    match node.fill_rule() {
        ondin_core::FillRule::NonZero => fill,
        ondin_core::FillRule::EvenOdd => format!(r#"{fill} fill-rule="evenodd""#),
    }
}

/// Every visible stroke of `node`, one after another, inside a group that is already
/// carrying the transform and the opacity.
///
/// Shared by the stacked-shape path and the **frame** path, which differ only in
/// which element carries the ink — and that difference lives in `write_element`,
/// where every other per-kind decision does. A second copy here is how a frame's
/// border would end up clipped differently from a rectangle's.
#[allow(clippy::too_many_arguments)]
fn emit_strokes(
    out: &mut String,
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    inner: &str,
    strokes: &[(usize, &Stroke)],
    defs: &Defs,
) {
    let derived = res.boolean_path(id);
    let shape = outline(node, res, id);
    for (i, s) in strokes {
        // Unfilled: the fills above already painted, and a stroke element that
        // filled as well would paint over the ones under it.
        const UNFILLED: &str = r#" fill="none""#;
        // **A per-side stroke is one `<path>` per edge**, not the shape's own
        // element with a stroke on it — the same split the scene walk makes, so
        // canvas and export draw the same edges (§6.3). The alignment clip is
        // still built from the whole shape, and wraps every side of one stroke
        // together: the sides are one paint, and a clip each would be three more
        // identical `clipPath`s in the file.
        let sides = geometry::effective_sides(node.kind(), s).per_side(s.width);
        // **And a fitted dash on a multi-subpath path is one `<path>` per subpath**,
        // each carrying the pattern fitted to that subpath's own perimeter, because
        // an element can only say one `stroke-dasharray` (`geometry::dash_fit_pieces`).
        let pieces = dash_pieces(shape.as_ref(), s, s.width);
        let ink = |out: &mut String, pad: &str, doubled: bool| {
            let scale = if doubled { 2.0 } else { 1.0 };
            let segments = |out: &mut String, parts: &[(BezPath, Vec<f64>)], w: f64| {
                for (part, dashes) in parts {
                    let _ = writeln!(
                        out,
                        r#"{pad}<path d="{d}"{UNFILLED}{stroke}/>"#,
                        d = path_d(part),
                        stroke = stroke_attr(id, *i, s, w * scale, dashes, defs),
                    );
                }
            };
            match (&sides, &pieces) {
                (Some(sides), _) => {
                    for (side, w) in sides {
                        let Some(edge) = geometry::side_path(node.kind(), *side) else {
                            continue;
                        };
                        // A side is one subpath, so its own fit is exact and needs no
                        // splitting — `dash_fit_pieces` would answer `None` for it.
                        let dashes = geometry::resolved_dashes(s, *w, &edge);
                        segments(out, &[(edge, dashes)], *w);
                    }
                }
                (None, Some(parts)) => segments(out, parts, s.width),
                (None, None) => write_element(
                    out,
                    node,
                    derived,
                    pad,
                    "",
                    UNFILLED,
                    &stroke_attr(
                        id,
                        *i,
                        s,
                        s.width * scale,
                        &resolved_dashes(shape.as_ref(), s, s.width),
                        defs,
                    ),
                    "",
                ),
            }
        };
        // A stroke can be painted with a picture too, and a contained one
        // letterboxes inside the shape's box exactly as a fill does. **Outside
        // the alignment clip**, because the two cut different things: alignment
        // keeps the half of a doubled stroke that is on the right side, and this
        // keeps the part of the frame the picture actually reaches. Nesting them
        // the other way round would work as geometry and read as an accident.
        let box_clip = framing_of(doc, node, res, id, &s.brush).and_then(|f| f.clip);
        with_image_clip(
            out,
            box_clip,
            id,
            &format!("stroke-{i}"),
            inner,
            |out, inner| {
                match align_clip(node, shape.as_ref(), s) {
                    Some((clip_d, rule)) => {
                        // Per stroke, so two aligned strokes on one shape get a clip each
                        // rather than sharing — and the index is the paint's own, so
                        // hiding one does not renumber the others.
                        let clip_id = format!("sa-{}-{i}", id.to_wire().replace(':', "-"));
                        let _ = writeln!(
                            out,
                            r#"{inner}<clipPath id="{clip_id}"{rule}><path d="{clip_d}"/></clipPath>"#
                        );
                        let _ = writeln!(out, r#"{inner}<g clip-path="url(#{clip_id})">"#);
                        ink(out, &format!("{inner}  "), true);
                        let _ = writeln!(out, "{inner}</g>");
                    }
                    None => ink(out, inner, false),
                }
            },
        );
    }
}

/// The node as one SVG element, with the paint, transform and opacity attributes
/// already built.
///
/// Which *element* a kind becomes is the whole of what this decides, and it is
/// spelled once because the stacked path emits the same shape several times over
/// with different paints — a second per-kind writer is a second place for a
/// polygon to round differently from the canvas.
#[allow(clippy::too_many_arguments)]
fn write_element(
    out: &mut String,
    node: &Node,
    // `derived` is the outline `Resolved` computed for a `Boolean` node — its
    // whole geometry, and `None` for every other kind, which builds its own.
    derived: Option<&BezPath>,
    pad: &str,
    t: &str,
    fill: &str,
    stroke: &str,
    op: &str,
) {
    match node.kind() {
        // `<rect rx>` can only say one number, so unequal corners are emitted
        // as the same outline the canvas draws — `local_path`, not a second
        // construction that could round them differently.
        NodeKind::Rect { size, corner_radii } => match geometry::uniform_radius(corner_radii) {
            Some(r) => {
                let rx = if r > 0.0 {
                    format!(r#" rx="{}""#, fmt(r))
                } else {
                    String::new()
                };
                let _ = writeln!(
                    out,
                    r#"{pad}<rect{t} x="0" y="0" width="{w}" height="{h}"{rx}{fill}{stroke}{op}/>"#,
                    w = fmt(size.width),
                    h = fmt(size.height),
                );
            }
            None => {
                let d = geometry::local_path(node.kind())
                    .map(|p| path_d(&p))
                    .unwrap_or_default();
                let _ = writeln!(out, r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#);
            }
        },
        // `<polygon>` is a point list, which is exactly what both of these are —
        // and it stays editable in another tool as a polygon rather than as a
        // pile of line segments. The points come from `local_path`, so the
        // export cannot round a shape the canvas drew differently.
        NodeKind::Polygon { .. } | NodeKind::Star { .. } => {
            let points = geometry::local_path(node.kind())
                .map(|p| {
                    p.elements()
                        .iter()
                        .filter_map(|el| match el {
                            ondin_core::kurbo::PathEl::MoveTo(v)
                            | ondin_core::kurbo::PathEl::LineTo(v) => {
                                Some(format!("{},{}", fmt(v.x), fmt(v.y)))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let _ = writeln!(
                out,
                r#"{pad}<polygon{t} points="{points}"{fill}{stroke}{op}/>"#
            );
        }
        NodeKind::Ellipse { size } => {
            let _ = writeln!(
                out,
                r#"{pad}<ellipse{t} cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}"{fill}{stroke}{op}/>"#,
                cx = fmt(size.width * 0.5),
                cy = fmt(size.height * 0.5),
                rx = fmt(size.width * 0.5),
                ry = fmt(size.height * 0.5),
            );
        }
        NodeKind::Line { end } => {
            let _ = writeln!(
                out,
                r#"{pad}<line{t} x1="0" y1="0" x2="{x2}" y2="{y2}"{stroke}{op}/>"#,
                x2 = fmt(end.x),
                y2 = fmt(end.y),
            );
        }
        // **The *resolved* outline, so a rounded corner exports rounded.** SVG has
        // no per-vertex radius to write, and `path` alone is the unrounded
        // authored geometry — so this goes through `geometry::local_path`, the
        // same resolution the canvas draws (§15 D119). Free when there are no
        // radii: it hands the path straight back.
        NodeKind::Path { .. } => {
            if let Some(resolved) = geometry::local_path(node.kind()) {
                let _ = writeln!(
                    out,
                    r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#,
                    d = path_d(&resolved),
                );
            }
        }
        // **A boolean exports as its result, not as its operands.** SVG has no
        // boolean container — there is nothing to nest the children inside that
        // would combine them — so the derived outline is the only faithful thing to
        // write, and it is the same path the canvas draws. The operands are
        // deliberately *not* emitted alongside it: they are inputs, and a viewer
        // drawing them too would show the shapes the boolean was meant to consume.
        //
        // This is the one place the non-destructive model is necessarily flattened,
        // and it is flattened at the boundary rather than in the document.
        NodeKind::Boolean { .. } => {
            if let Some(path) = derived {
                let _ = writeln!(
                    out,
                    r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#,
                    d = path_d(path),
                );
            }
        }
        // **One `<text>` per line, one `<tspan>` per run** — what §15 D81 asked for
        // and what this used to be unable to say. It emitted a single unwrapped
        // `<text>` built from the node's *defaults*, so every character span was
        // dropped and a wrapped node exported as one long line.
        //
        // The lines come from `text::export_lines`, which re-shapes: the cached
        // `TextLayout` cannot supply them, because `GlyphRun` holds glyph **ids**
        // that do not map back to characters, and it has no line structure at all.
        //
        // **The node's ink stays on the `<text>` elements and the face moves to the
        // `<tspan>`s.** That split is what keeps `{fill}`/`{stroke}`/`{op}` meaning
        // exactly what they did — a stroke still rides the text element rather than
        // an outlined copy of the glyphs, so the output stays re-shapeable and the
        // alignment construction wrapping it (doubled width inside a `clipPath`
        // built from the glyph outlines) is still the one a shape gets (§6.3).
        // ⚠️ **A rail is the one place this writer hands *placement* back to the
        // viewer, and it is a deliberate exception to D81 rather than an
        // oversight** (§15 D405). Everything above is built on "we place, they
        // shape": one `<text>` per line at a baseline we computed, so a viewer
        // whose shaping differs from ours cannot move the lines. Type on a path
        // cannot be written that way. The exact spelling would be one `<text>` per
        // *glyph* under its own rotation — and that is unreachable here for a
        // stated reason, not an unexplored one: `export_lines`' own doc records
        // that glyph ids do not map back to characters through ligatures and
        // reordering, so this writer has no way to say which characters a rotated
        // glyph is. `<textPath>` is what is left that keeps text as text; the
        // alternative that *is* exact, filling the glyph outlines as a `<path>`,
        // gives up selectable text, searchable content and re-shaping in a
        // different family, which is most of what an SVG of type is for.
        //
        // **What a viewer may therefore differ on**: where each glyph lands along
        // the rail, since it re-shapes and re-measures. Alignment survives as
        // `startOffset` in *percent*, which is the one spelling that does not also
        // depend on our measure of the text.
        NodeKind::Text { on_path, .. } => {
            let Some(parts) = ondin_core::TextRef::of(node.kind()) else {
                return;
            };
            if on_path.is_some() {
                let mut spans = String::new();
                for line in ondin_core::text::export_lines(parts) {
                    for run in &line.runs {
                        let _ = write!(
                            spans,
                            "<tspan{attrs}>{text}</tspan>",
                            attrs = run_attrs(&run.style),
                            text = escape_xml(&run.text),
                        );
                    }
                }
                // **Percent, and anchored to match.** `startOffset` in user units
                // would be our own measure of the text handed to a viewer that has
                // just measured it differently; the percentage plus `text-anchor`
                // says *where on the rail the text is centred*, which is what the
                // alignment control actually means and survives a different font.
                //
                // ⚠️ **The node's own offset is added to the percentage and does
                // *not* touch the anchor** (§15 D409). The two say different
                // things — the anchor is how the text sits about a point on the
                // rail, the offset is which point — so a node centred a third of
                // the way along writes `83.3%` with `middle`, and a viewer that
                // measures the text differently still centres it on the same spot.
                // Folding the offset into an absolute start with `text-anchor` at
                // `start` would give up exactly that.
                let anchored = match parts.paragraph.align {
                    ondin_core::TextAlign::Center => (50.0, " text-anchor=\"middle\""),
                    ondin_core::TextAlign::End | ondin_core::TextAlign::Right => {
                        (100.0, " text-anchor=\"end\"")
                    }
                    _ => (0.0, ""),
                };
                let anchor = anchored.1;
                let offset = format!("{}%", fmt(anchored.0 + parts.on_path_offset * 100.0));
                let _ = writeln!(
                    out,
                    // `r##`, because `href="#` contains the `"#` that would close
                    // an `r#` string on the spot.
                    r##"{pad}<text{t}{fill}{stroke}{op}{anchor}><textPath href="#{rail}" startOffset="{offset}">{spans}</textPath></text>"##,
                    // `node.id()`, because this writer takes the node and not its
                    // id — and it is the same id `collect_defs` named the rail
                    // with, which is what makes the reference resolve.
                    rail = rail_id(node.id()),
                );
                return;
            }
            for line in ondin_core::text::export_lines(parts) {
                // Every line is placed, so nothing here needs `text-anchor` — which
                // this used to carry, and which was an *approximation*: it asked the
                // viewer to re-align text it had re-shaped itself. Alignment,
                // indent and justification are all already in the run's `x`,
                // because `export_lines` reads it after `Layout::align` has run.
                let mut spans = String::new();
                for run in &line.runs {
                    let _ = write!(
                        spans,
                        "<tspan{attrs}>{text}</tspan>",
                        attrs = run_attrs(&run.style),
                        text = escape_xml(&run.text),
                    );
                }
                let _ = writeln!(
                    out,
                    r#"{pad}<text{t} x="{x}" y="{y}"{fill}{stroke}{op}>{spans}</text>"#,
                    x = fmt(line.x),
                    y = fmt(line.baseline),
                );
            }
        }
        // **A frame's own outline, and only its stroke ever asks for it.** Its
        // background is written by `emit_node` as the first thing inside its group
        // (behind the children), and its children are the group's contents — so this
        // arm is reached exactly once per visible frame stroke, from `emit_strokes`,
        // after the contents and outside the clip (§15 D144).
        NodeKind::Artboard { size, .. } => {
            let _ = writeln!(
                out,
                r#"{pad}<rect{t} x="0" y="0" width="{w}" height="{h}"{fill}{stroke}{op}/>"#,
                w = fmt(size.width),
                h = fmt(size.height),
            );
        }
        NodeKind::Root | NodeKind::Group => {}
    }
}

/// Everything one run's own style contributes to its `<tspan>`: the face, the
/// spacing, and the decorations.
///
/// **Per run, where all of this used to be per node.** Before `<tspan>` existed
/// here these were written once on the single `<text>` element from the node's
/// *defaults*, so a span that changed the size or added an underline was silently
/// dropped (§15 D81). The element that carries them is the only thing that
/// changed; each attribute means exactly what it did.
///
/// `agreed` comes along unchanged in kind, and this is the place to say why: it
/// resolves an underline disagreeing with a **strikethrough on the same run**, and
/// splitting runs into separate elements does not separate those two — a run
/// carries both. What per-run elements newly express is one run's decoration
/// differing from *another run's*, which no single element could ever say.
fn run_attrs(style: &ondin_core::TextStyle) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        r#" font-family="{family}" font-size="{size}" font-weight="{weight}""#,
        family = escape_xml(&style.font_family),
        size = fmt(style.font_size),
        weight = style.weight,
    );
    if style.italic {
        out.push_str(r#" font-style="italic""#);
    }
    // **A run's own ink, overriding the `<text>` element's.** Absence inherits, which
    // is exactly what `TextStyle::color`'s `None` means — so an uncoloured run needs
    // no attribute at all and keeps drawing the node's fill (§15 D154). Alpha rides
    // `fill-opacity` rather than an eight-digit hex, because `fill` has one; the
    // decoration properties do not, which is why they take the other route.
    if let Some(c) = style.color {
        let rgba = c.to_rgba8();
        let _ = write!(
            out,
            r##" fill="#{:02x}{:02x}{:02x}"{}"##,
            rgba.r,
            rgba.g,
            rgba.b,
            // The helper every other paint's alpha already goes through, so a run's
            // opacity cannot round differently from a gradient stop's.
            alpha_attr("fill-opacity", rgba.a),
        );
    }
    if !style.letter_spacing.is_zero() {
        let _ = write!(
            out,
            r#" letter-spacing="{}""#,
            fmt(style.letter_spacing.resolve(style.font_size))
        );
    }
    if !style.word_spacing.is_zero() {
        let _ = write!(
            out,
            r#" word-spacing="{}""#,
            fmt(style.word_spacing.resolve(style.font_size))
        );
    }
    let names: Vec<&str> = [
        style.underline.map(|_| "underline"),
        style.strikethrough.map(|_| "line-through"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if names.is_empty() {
        return out;
    }
    let each: Vec<Decoration> = [style.underline, style.strikethrough]
        .into_iter()
        .flatten()
        .collect();
    let _ = write!(out, r#" text-decoration="{}""#, names.join(" "));
    // **The three refinements come *after* the shorthand, and that is
    // load-bearing.** A presentation attribute contributes its declaration in
    // attribute order, and `text-decoration` is a shorthand whose omitted parts
    // reset to their initial values — so written first it is overridden, and
    // written last it would quietly undo all three.
    //
    // **Each is written only when every decoration on this run agrees about it.**
    // CSS gives an element one colour, one thickness and one style; a run carrying
    // both an underline and a strikethrough that disagree cannot say both, so the
    // property is dropped rather than one of them picked and the consumer falls
    // back to the text's own colour and the font's own metrics.
    //
    // ⚠️ **Whether a consumer honours these at all is one question for all three.**
    // They are SVG *presentation attributes* standing for CSS properties, so a
    // reader that ignores the `text-decoration-*` family ignores the colour, the
    // thickness and the style together — and if that answer ever turns out to be
    // no, all three move into a `style` attribute together. (Kept from the
    // skip-ink note below, which was the fourth of them, became the absence of one
    // at §15 D356, and is a fourth again at §15 D357 — written for the runs that
    // switch skipping off. It is not one of the three `agreed` cases, though: it is
    // the underline's alone, for the reason recorded down there.)
    //
    if let Some(c) = agreed(&each, |d| d.color).flatten() {
        // There is no `text-decoration-color-opacity` to put alpha in, the way
        // `fill` has `fill-opacity`, so it rides in the hex: CSS Color 4's
        // eight-digit form, and only when the colour is not opaque, so the
        // ordinary case stays six.
        let rgba = c.to_rgba8();
        let hex = if rgba.a == 255 {
            format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b, rgba.a)
        };
        let _ = write!(out, r#" text-decoration-color="{hex}""#);
    }
    if let Some(t) = agreed(&each, |d| d.thickness).flatten() {
        // `px`, where `letter-spacing` above is unitless: that one is an SVG
        // property, whose `<length>` permits a bare number, and this one is CSS's,
        // where a unitless length is invalid and the whole declaration would be
        // dropped. A CSS `px` in SVG *is* the user unit, so the number itself is
        // the same either way.
        // ⚠️ **Floored, because the canvas floors it and the two must agree**
        // (§15 D546). `[S6.3-L1-02]`: `core::text::decoration_ink` ends its
        // resolve with `.max(0.0)`, so a negative thickness drew **nothing** on
        // screen; this line had no counterpart and wrote
        // `text-decoration-thickness="-5px"` verbatim, so the file and the screen
        // disagreed about whether there was a line at all — and re-importing gave
        // a *third* answer, because `svg_in` reads no decoration thickness and it
        // came back as "the font decides".
        //
        // ⚠️ **`canonical_decoration` clamps at the model since D546 and it does
        // not cover this, which a flip is what established.** That clamp is
        // reached from `TextStyle::set` — so from `CharSpans::set` for a span,
        // and from `panels::typography::char_attrs_tx`, which calls `set` on a
        // local style *before* it builds the op. **What it is never reached from
        // is the op layer**: `Document::op_set_text_style` is
        // `mem::replace(s, Box::new(style.clone()))`, the whole struct written
        // raw, and `Operation::CreateNode` is the same. So a `TextStyle`
        // assembled by anything that does not go through `set` arrives
        // unclamped, and this floor is load-bearing rather than
        // defence-in-depth. Removing it fails
        // `a_decorations_colour_thickness_and_style_are_exported`, measured — the
        // comment here said the opposite until that flip was run.
        //
        // **That third door is a finding this one did not name** and is recorded
        // in `review/findings.md` rather than fixed here: clamping inside
        // `op_set_text_style` would make `Operation::changes_nothing`'s
        // `**s == *style` compare a raw incoming style against a canonicalized
        // stored one, which is a decision about D428's seam and not about
        // decorations.
        let _ = write!(
            out,
            r#" text-decoration-thickness="{}px""#,
            fmt(t.resolve(style.font_size).max(0.0))
        );
    }
    // Solid is CSS's initial value as well as ours; writing it is noise.
    if let Some(s) = agreed(&each, |d| d.style).filter(|s| *s != LineStyle::Solid) {
        let _ = write!(out, r#" text-decoration-style="{}""#, line_style_css(s));
    }
    // **Skip-ink, and only when it is switched off.** CSS's initial value is `auto`
    // — browsers break an underline around every descender it passes through — and
    // since §15 D356 that is what the canvas does too, so an ordinary underline
    // writes nothing here and the silence is the accurate statement. The attribute
    // that was written unconditionally from 2026-08-21 (§15 D281), back when ours
    // drew a continuous band over the top of the descenders, is now written for
    // exactly the runs that have *asked* for that band (§15 D357) — the same
    // sentence, gone from a standing lie to a statement about one setting.
    //
    // **Read off the underline alone, and no `agreed` call.** Skip-ink is not one of
    // the three element-scoped properties above: css-text-decor-4 applies it to
    // underlines and overlines and never to a `line-through`, so a strikethrough
    // sharing the element has no opinion to disagree with. It is also not a
    // sub-property of the `text-decoration` shorthand, so unlike the three above it
    // is not order-dependent; it is written here to keep the family together.
    //
    // ⚠️ **In the `auto` case the two implementations agree in kind, not in pixels.**
    // Ours breaks the band where a glyph contour really crosses it, widened by up to
    // one band thickness either side; a browser's `auto` is its own geometry with its
    // own clearance. So an exported underline and the canvas one gap in the same
    // places to within a fraction of a letter rather than identically. `none` is the
    // one of the two values that *is* exact, which is the small irony of this line:
    // the setting nobody will use round-trips perfectly. If `auto` ever needs to be
    // exact, the answer is not this attribute — it is writing the band as a `<path>`
    // from `DecorationInk::gaps`, and it would cost the run its `text-decoration-*`
    // family (§15 D112's trade, one level up).
    if style.underline.is_some_and(|d| !d.skip_ink) {
        let _ = write!(out, r#" text-decoration-skip-ink="none""#);
    }
    // **Offset, for the one run shape that can say it.** `text-underline-offset`
    // is underline-only and has no strikethrough counterpart, so a run carrying
    // both still cannot express two offsets and drops it — a real limit rather
    // than an omission (§15 D112). A run with an underline *alone* is
    // unambiguous, which is this case, and since a decoration is exclusive in the
    // panel it is also the ordinary one.
    //
    // **The sign flips, and that is the thing to get right.** Ours is parley's,
    // measured from the baseline with positive **up** (`Decoration::offset`, and
    // `text::decoration_ink` computing `top = baseline − offset`). CSS measures
    // from the alphabetic baseline with positive **under** — css-text-decor-4:
    // "Positive offsets represent distances outward from the text", the zero
    // position being the alphabetic baseline under the default
    // `text-underline-position: auto`. So the number written is the negation of
    // the number stored: a stored `+2` is 2px above the baseline and exports as
    // `-2px`.
    //
    // Outside the shorthand-order argument above, because the spec puts it
    // outside: `text-underline-offset` is explicitly *not* a sub-property of
    // `text-decoration`, so nothing here resets it.
    if style.strikethrough.is_none()
        && let Some(offset) = style.underline.and_then(|d| d.offset)
    {
        let _ = write!(
            out,
            r#" text-underline-offset="{}px""#,
            fmt(-offset.resolve(style.font_size))
        );
    }
    out
}

/// The value every decoration on one element agrees on, or `None` when they
/// disagree or there are none at all.
///
/// The question the three `text-decoration-*` properties have to ask: they are
/// element-scoped in CSS, and a text node carries an underline and a
/// strikethrough that each have their own colour, thickness and line style.
fn agreed<T: PartialEq>(ds: &[Decoration], f: impl Fn(&Decoration) -> T) -> Option<T> {
    let mut values = ds.iter().map(f);
    let first = values.next()?;
    values.all(|v| v == first).then_some(first)
}

/// The CSS keyword for a line style. All four of ours are CSS keywords verbatim,
/// which is the reason `text-decoration-style` is expressible at all.
fn line_style_css(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Solid => "solid",
        LineStyle::Dashed => "dashed",
        LineStyle::Dotted => "dotted",
        LineStyle::Wavy => "wavy",
    }
}

/// ` transform="matrix(..)"`, omitted entirely when it is the identity.
fn transform_attr(a: Affine) -> String {
    matrix_attr("transform", a)
}

/// ` data-name="…"` — the layer's own name, on the element that *is* the layer.
///
/// 🚨 **No name reached the file at all until §15 D611, `[S8.1-L6-04]`**, while `svg_in`
/// reads an element's `id` **as** the layer name at six sites. So the pair was
/// asymmetric in the one direction that loses data: a document with named layers
/// exported and re-imported came back with every layer unnamed, and nothing said
/// so — the round-trip suite asserts geometry, paint and structure and never a
/// name. `naming.rs` exists to produce those names and the `.ondin` format keeps
/// them; handing another tool a structure with no labels is most of what
/// "editable elsewhere" means.
///
/// **`data-name` and not `id`, which is the decision** (§15 D611). An `id` must be an
/// XML `Name` and unique in the document: "Rectangle 1" is neither, so writing
/// names as ids means sanitising and uniquifying them — which mangles ordinary
/// names on the way out and reads them back mangled — and it would put layer
/// names in the same namespace as the `grad-…`/`clip-…`/`mask-…` ids this writer
/// synthesises from `NodeId::to_wire`. `data-*` is valid markup that every viewer
/// ignores, it is what Figma writes for exactly this, and it round-trips the name
/// **byte for byte**. The reader prefers it and falls back to `id`, so a file from
/// another tool still reads the way it always did.
///
/// **Empty names write nothing**, which is the discipline every other attribute
/// here keeps: a node that has never been named adds no bytes.
fn name_attr(node: &Node) -> String {
    match node.name() {
        "" => String::new(),
        name => format!(r#" data-name="{}""#, escape_xml(name)),
    }
}

/// ` <name>="matrix(..)"`, omitted entirely when it says nothing.
///
/// Two callers and two attribute names — an element's `transform` and a
/// pattern's `patternTransform` — under one rule, because "only when it is not
/// the default" is what every other attribute here keeps ([`stroke_attr`]'s mitre
/// limit says the same thing about the same trade).
///
/// **Identity is judged on the printed form, not on the `Affine`.** The two part
/// company exactly where it matters: a pattern's transform is *computed* against
/// a shape's bounding box, and a rounded rect's box has a top of `-8.88e-16`
/// (see [`fmt`]), so a picture sitting perfectly still in its frame is an affine
/// a hair from the identity and a matrix of `1 0 0 1 0 0` on paper. Comparing
/// what would be written cannot disagree with what is written.
fn matrix_attr(name: &str, a: Affine) -> String {
    let [m0, m1, m2, m3, m4, m5] = a.as_coeffs().map(fmt);
    if [&m0, &m1, &m2, &m3, &m4, &m5] == ["1", "0", "0", "1", "0", "0"] {
        return String::new();
    }
    format!(r#" {name}="matrix({m0} {m1} {m2} {m3} {m4} {m5})""#)
}

/// The clip an aligned stroke has to be built out of — the path to clip against
/// and the fill rule — or `None` when the stroke needs no construction.
///
/// SVG has no stroke alignment, so the canvas's recipe applies here too: stroke
/// at twice the width, which reaches a full width either side of the outline, and
/// clip away the half that does not belong (§6.3). Inside clips to the shape;
/// outside clips to everything *but* the shape — the shape plus an enclosing box,
/// taken even-odd, so a shape with holes gets it right.
///
/// `None` for a centred stroke, for an open shape (which has no interior to be
/// inside of), and for a kind with no outline at all.
fn align_clip(node: &Node, shape: Option<&BezPath>, s: &Stroke) -> Option<(String, &'static str)> {
    if s.align == StrokeAlign::Center || !geometry::stroke_align_applies(node.kind()) {
        return None;
    }
    let path = shape?;
    Some(match s.align {
        StrokeAlign::Outside => {
            let b = path.bounding_box();
            // **The ring has to clear the ink, not the shape.** The widest side's
            // doubled stroke reaches that width past the outline and `limit` times
            // further at a mitre, and padding by the box's own size alone left the
            // complement's outer edge *inside* a wide stroke — which clipped it into
            // straight rectangular bites. `scene::outer_bounds` states the same rule for
            // the canvas; the two must agree or an export differs from what was drawn.
            let reach = geometry::effective_sides(node.kind(), s).max_width(s.width)
                * s.miter_limit.max(1.0);
            let pad_out = (b.width() + b.height()).max(1.0) + reach;
            let mut complement = b.inflate(pad_out, pad_out).to_path(0.1);
            complement.extend(path.iter());
            (path_d(&complement), r#" clip-rule="evenodd""#)
        }
        _ => (path_d(path), ""),
    })
}

/// Stroke attributes for one stroke, at `width` — which is the stroke's own
/// except on a `Custom` side, which carries its own, and in the aligned
/// construction, where whichever of those is drawn double and half of it clipped
/// away.
///
/// `dashes` is the pattern as resolved for the path this will sit on
/// ([`geometry::resolved_dashes`]), not the model's own: SVG can express neither
/// a zero-length dot nor "fit to corners", so both have to arrive here already
/// turned into numbers.
fn stroke_attr(
    id: NodeId,
    index: usize,
    s: &Stroke,
    width: f64,
    dashes: &[f64],
    defs: &Defs,
) -> String {
    let paint = paint_attr("stroke", &s.brush, id, &format!("stroke-{index}"), defs);
    let join = match s.join {
        ondin_core::kurbo::Join::Miter => "miter",
        ondin_core::kurbo::Join::Round => "round",
        ondin_core::kurbo::Join::Bevel => "bevel",
    };
    let cap = match s.cap {
        ondin_core::kurbo::Cap::Butt => "butt",
        ondin_core::kurbo::Cap::Round => "round",
        ondin_core::kurbo::Cap::Square => "square",
    };
    let dash = if dashes.is_empty() {
        String::new()
    } else {
        let list = dashes.iter().map(|d| fmt(*d)).collect::<Vec<_>>().join(",");
        format!(
            r#" stroke-dasharray="{list}" stroke-dashoffset="{}""#,
            fmt(s.dash_offset)
        )
    };
    // **Only when it is not the default**, which SVG puts at 4 — the same number
    // kurbo does. Emitting it unconditionally would rewrite the stroke attributes
    // of every document ever exported to say what they already meant. This comment
    // used to give a second reason — that the golden SVGs are byte-compared —
    // which was false for as long as it stood, there being no goldens, and was
    // corrected to say so. **The goldens arrived on 2026-08-22**
    // (`tests/goldens.rs`, §15 D304) and the second reason is now true, exactly as
    // the correction predicted it would be if they ever did. It is still not what
    // this branch rests on: the first reason held on its own the whole time.
    let miter = if s.join == ondin_core::kurbo::Join::Miter && s.miter_limit != 4.0 {
        format!(r#" stroke-miterlimit="{}""#, fmt(s.miter_limit))
    } else {
        String::new()
    };
    format!(
        r#"{paint} stroke-width="{w}" stroke-linejoin="{join}" stroke-linecap="{cap}"{dash}{miter}"#,
        w = fmt(width),
    )
}

/// A paint attribute (`fill` or `stroke`): a hex colour for solids, a `url(#…)`
/// reference for gradients and for images.
///
/// **`defs` is asked rather than trusted**, and only an image needs the asking: a
/// gradient always collected a def, while an image collected one only if the
/// document's table could answer for it. Writing `url(#…)` for a paint server
/// that is not in the file is worse than writing nothing, because SVG calls that
/// an error where "no paint" is a picture.
fn paint_attr(attr: &str, brush: &Brush, id: NodeId, slot: &str, defs: &Defs) -> String {
    match brush {
        Brush::Solid(c) => {
            let rgba = c.to_rgba8();
            format!(
                r##" {attr}="#{:02x}{:02x}{:02x}"{}"##,
                rgba.r,
                rgba.g,
                rgba.b,
                alpha_attr(&format!("{attr}-opacity"), rgba.a),
            )
        }
        // **`{attr}-opacity` beside the `url(…)`, which is what SVG has for exactly
        // this** (§15 D767): `fill-opacity`/`stroke-opacity` on the shape multiplies
        // the paint server's own alpha, so the ramp's opacity is expressed natively
        // and the stops keep their `stop-opacity`. No `<g>` wrapper, no change to
        // `gradient_def`, and the `{attr}` interpolation gives the stroke case free.
        //
        // ⚠️ **`alpha_attr`, not a fresh `format!`** — `svg.rs`'s own rule that
        // *"a run's opacity cannot round differently from a gradient stop's"*, which
        // means going through the `u8` it rounds to.
        Brush::Gradient(g) => format!(
            r#" {attr}="url(#{})"{}"#,
            gradient_id(id, slot),
            alpha_attr(
                &format!("{attr}-opacity"),
                (g.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            ),
        ),
        // **An image is a `<pattern>` naming the encoded original** — see
        // [`image_def`]. `none` where the document cannot answer for the
        // reference, and that arm is now only reached by a **stroke**: a *fill*
        // is diverted to the placeholder before it gets here (`emit_missing`),
        // where a stroke keeps the older behaviour of drawing nothing. Both
        // writers agree about that, and for the same reason — a placeholder is a
        // statement about an area, and a stroke has none (§15 D179).
        // ⚠️ **A `none` fallback after the `url(…)` was tried here and taken back
        // out, because it fixes nothing** (§15 D477). `[A5-L6-01]` reports that a
        // picture round-trips into an *"opaque black rectangle"* because an
        // unresolvable reference falls back to the inherited `fill`, whose initial
        // value is black — which would make ` none` a one-word fix. **Neither half
        // reproduces.** `svg_in::parse_paint` returns `None` for a `url(…)` it
        // cannot resolve with no fallback, so the shape comes back *unpainted*;
        // and measured in Chrome over HTTP, a `<rect fill="url(#nope)"/>` and a
        // `<rect fill="url(#nope) none"/>` both paint **0 black pixels** — SVG 2's
        // rule, and the one browsers follow. The black the finding saw is almost
        // certainly the artboard `<clipPath>`'s own rect, which comes back as a
        // **mask node** carrying SVG's initial black because the writer gives it
        // no `fill`; a mask paints nothing, so it is harmless. A whole-tree sweep
        // for black finds it, and the first draft of this round-trip test did.
        Brush::Image(_) => match defs.has(&image_id(id, slot)) {
            true => format!(r#" {attr}="url(#{})""#, image_id(id, slot)),
            false => format!(r#" {attr}="none""#),
        },
    }
}

fn alpha_attr(name: &str, alpha: u8) -> String {
    if alpha == 255 {
        String::new()
    } else {
        format!(r#" {name}="{}""#, fmt(alpha as f64 / 255.0))
    }
}

fn opacity_attr(opacity: f32) -> String {
    if opacity >= 1.0 {
        String::new()
    } else {
        format!(r#" opacity="{}""#, fmt(opacity as f64))
    }
}

/// Format a float compactly: integers without a decimal point, else trimmed.
/// A `d` attribute, with coordinates rounded the way every other number in the
/// file is.
///
/// `BezPath::to_svg` prints full `f64` precision, which is merely verbose for a
/// hand-drawn path but genuinely unreadable for a flattened rounded rect —
/// tangent points there come out as `-0.0000000000000008881784197001252`.
fn path_d(path: &ondin_core::kurbo::BezPath) -> String {
    use ondin_core::kurbo::PathEl;
    let p = |pt: ondin_core::kurbo::Point| format!("{},{}", fmt(pt.x), fmt(pt.y));
    let mut out = String::new();
    for el in path.elements() {
        if !out.is_empty() {
            out.push(' ');
        }
        match el {
            PathEl::MoveTo(a) => out.push_str(&format!("M{}", p(*a))),
            PathEl::LineTo(a) => out.push_str(&format!("L{}", p(*a))),
            PathEl::QuadTo(a, b) => out.push_str(&format!("Q{} {}", p(*a), p(*b))),
            PathEl::CurveTo(a, b, c) => {
                out.push_str(&format!("C{} {} {}", p(*a), p(*b), p(*c)));
            }
            PathEl::ClosePath => out.push('Z'),
        }
    }
    out
}

fn fmt(v: f64) -> String {
    // **A number that is not a number is written as `0`** (§15 D508,
    // `[S8.1-L1-03]`). Every number in this writer comes through here, and both
    // arms below print a non-finite one verbatim: the fast path's `fract() == 0.0`
    // is false for `NaN` and `inf`, and `format!("{v:.4}")` then emits the literal
    // strings `NaN` and `inf`. Neither is a valid SVG `<number>`, so one of them
    // anywhere makes the **whole file** unparseable — measured on an imported
    // `matrix(0,0,0,0,0,0)`: `<svg width="-inf" height="-inf" viewBox="-inf -inf
    // -inf -inf">`.
    //
    // **A wrong-but-drawable file beats an unopenable one**, which is the same
    // choice `-0` makes below and the same direction D495 took in core. It is a
    // floor rather than a fix: the value was already meaningless by the time it
    // reached a formatter, and the places that can produce one are guarded at
    // their own ends — D495 for the model's bounds, [`effect_region`] for the
    // singular-transform case this finding also names.
    if !v.is_finite() {
        return "0".to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    // **`-0` is never worth writing.** A value that rounds to nothing but keeps
    // its sign is dust from an earlier calculation, not a coordinate: a rounded
    // rect's outline has a bounding box whose top is `-8.88e-16` — the same
    // artefact [`path_d`] documents — so a picture *fitted* into that box came out
    // translated by `-0`. It reads as a number somebody meant, and it is the first
    // thing an eye stops on in a diff.
    match s {
        "-0" => "0".to_string(),
        s => s.to_string(),
    }
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Unused now that gradients resolve to defs, but kept as the single place a
/// brush collapses to one colour if a future consumer needs it.
#[allow(dead_code)]
fn brush_color(brush: &Brush) -> Color {
    match brush {
        Brush::Solid(c) => *c,
        _ => Color::from_rgba8(128, 128, 128, 255),
    }
}
