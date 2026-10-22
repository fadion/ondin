//! Semantic SVG export (§7.1).
//!
//! A pure function of `Document` + `Resolved`. Groups and artboards become nested
//! `<g>` elements carrying their local transform and opacity (so group opacity
//! composites as a unit), with shapes in local coordinates. Artboards clip with a
//! `clipPath` unless the layer's `clip` flag is off.
//!
//! Doubles as a model-completeness gate: anything the model can express and this
//! cannot write is a gap worth failing on.
//!
//! Text is one `<text>` per line with one `<tspan>` per run, so a viewer re-shapes
//! with its own fonts; line breaks and baselines are ours, from
//! `text::export_lines`.

use ondin_core::kurbo::{Affine, BezPath, Insets, Rect, Shape};
use ondin_core::peniko::{Color, Extend, GradientKind};
use ondin_core::{Brush, Keyed};
use ondin_core::{
    Decoration, Document, Effect, EffectKind, Fill, Framing, LineStyle, MaskMode, Node, NodeId,
    NodeKind, Paint, Resolved, Shadow, Stroke, StrokeAlign, geometry,
};
use std::fmt::Write;

/// An export and what it could not say exactly — the writer's side of
/// `svg_in::Import`'s `skipped`/`approximated`.
///
/// [`Self::approximated`] is where the picture differs from the canvas;
/// [`Self::round_trip`] is where the picture is right but reading the file back
/// gives a different document (a flipped rail, an image `<pattern>`). Filing those
/// as approximated would tell a user their drawing is wrong.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Export {
    /// The markup.
    pub svg: String,
    /// Features a viewer draws slightly differently. Deduplicated, in order met.
    pub approximated: Vec<String>,
    /// Features drawn exactly whose document does not survive our own reader.
    pub round_trip: Vec<String>,
}

/// The sink [`emit_node`] pushes into. Not built in `collect_defs`, which has no
/// visibility guard and would report features on layers the file lacks.
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
/// The viewBox covers the union of the world bounds, and each subtree is emitted
/// under its own parent's transform. Order is the caller's and kept (SVG has no
/// z-index; invariant 9). Nothing is deduplicated (`build::outermost` filters).
pub fn svg_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> String {
    svg_of_reported(doc, res, origins).svg
}

/// [`svg_of`], and what it could not say exactly — see [`Export`]. [`svg_of`] is
/// the markup-only shim; the export panel and *Copy as SVG* ask for the report.
pub fn svg_of_reported(doc: &Document, res: &Resolved, origins: &[NodeId]) -> Export {
    // `crate::extent`, shared with the PNG writer so the two frame the same box.
    let view = crate::extent(res, origins).unwrap_or_else(|| Rect::new(0.0, 0.0, 100.0, 100.0));

    // Defs first so the header can carry them; ids follow document order.
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

/// The `outer` an origin is emitted under — its parent's world transform (identity
/// for the root), since `emit_node` composes `outer * node.transform()`. Shared by
/// the defs and emit passes so a filter region and its element share a space.
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
    /// Insertion-ordered, which [`Defs::emit`] needs for byte-stability.
    entries: Vec<(DefKey, String)>,
    /// The same ids as a set, so `add` and `has` are O(1) rather than Θ(D²) overall;
    /// std's `HashSet`, as the win is not worth a hasher dependency.
    seen: std::collections::HashSet<DefKey>,
}

/// A def's id: a `String` whose equality is counted in tests. Blind spot: a
/// comparison spelled on the inner `String` (`e.0 == id.0`) goes uncounted.
#[derive(Clone, Eq)]
struct DefKey(String);

/// Hashes the inner `String` as `str` does, which makes the `Borrow<str>` lookup
/// in [`Defs::has`] sound.
impl std::hash::Hash for DefKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for DefKey {
    fn eq(&self, other: &Self) -> bool {
        #[cfg(test)]
        DEF_KEY_COMPARISONS.with(|c| c.set(c.get() + 1));
        self.0 == other.0
    }
}

impl std::borrow::Borrow<str> for DefKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
thread_local! {
    /// How many times two `DefKey`s have been compared on this thread. Plain
    /// backticks: this item is `cfg(test)`.
    static DEF_KEY_COMPARISONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

impl Defs {
    fn add(&mut self, id: String, body: String) {
        let id = DefKey(id);
        if self.seen.insert(id.clone()) {
            self.entries.push((id, body));
        }
    }

    /// Whether a def of this name was collected; [`paint_attr`] asks before
    /// writing a `url(#…)` for an image.
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

/// Stable per-node, per-slot gradient id (`grad-<node>-fill-0`); the slot always
/// carries the paint's index.
fn gradient_id(node: NodeId, slot: &str) -> String {
    format!("grad-{}-{slot}", node.to_wire().replace(':', "_"))
}

/// Stable per-node, per-slot pattern id (`img-<node>-fill-0`), spelled like
/// [`gradient_id`].
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

/// The payload's id — keyed by the picture, so N shapes sharing a photograph write
/// one base64 copy. Sanitised, since a non-XML-name id would make the picture
/// silently not draw.
fn payload_id(image: &ondin_core::ImageId) -> String {
    let safe: String = image
        .0
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("pix-{safe}")
}

/// The effect stack's filter id (§5.3a): one per node, the stack being one graph.
fn effect_id(node: NodeId) -> String {
    format!("fx-{}", node.to_wire().replace(':', "_"))
}

/// Stable per-node id for a text node's rail (`rail-<node>`); no slot, because a
/// text node has exactly one rail (`NodeKind::Text::on_path`).
fn rail_id(node: NodeId) -> String {
    format!("rail-{}", node.to_wire().replace(':', "_"))
}

/// The box a node's effect filter may paint in, in the wrapper's user space (the
/// node's parent's, see [`origin_outer`]).
///
/// SVG's default region clips any shadow reaching past a tenth of the layer, so it
/// is written from [`Resolved::ink_bounds`], padded by [`inner_working_pad`].
fn effect_region(
    res: &Resolved,
    id: NodeId,
    transform: Affine,
    effects: &[Keyed<Effect>],
) -> Option<Rect> {
    // `None` drops the filter and keeps the artwork, like `ScenePainter::push_effect_layer`.
    Some(ink_in(res, id, id, transform)? + Insets::uniform(inner_working_pad(effects)))
}

/// `ink_of`'s ink box in the space `outer` maps `frame`'s local space into, the
/// base of every region this writer emits.
///
/// `None` where `frame` is scaled flat: its inverse is `NaN`s, and an unparseable
/// region means the referencing element is not rendered.
fn ink_in(res: &Resolved, ink_of: NodeId, frame: NodeId, outer: Affine) -> Option<Rect> {
    let ink = res.ink_bounds(ink_of)?;
    let world = res.world_transform(frame)?;
    if !ondin_core::build::affine_is_invertible(world) {
        return None;
    }
    Some((outer * world.inverse()).transform_rect_bbox(ink))
}

/// How far outside the layer this stack's inner shadows must be able to look.
/// `EffectKind::escape` is `ZERO` for one, but an inner shadow is cast from
/// everything outside the layer and SVG makes results outside the region
/// transparent, so a region at the ink bounds draws nothing.
///
/// Uniform and generous on purpose: too small silently crops, and the final
/// `feComposite operator="in"` confines the result. `0.0` with no inner shadow.
fn inner_working_pad(effects: &[Keyed<Effect>]) -> f64 {
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
/// Two stages, as `ondin_render::effects::run`: the layer's own appearance, then
/// every shadow cast from that. `color-interpolation-filters="sRGB"` is mandatory;
/// the `linearRGB` default would differ from the canvas.
fn effect_def(name: &str, effects: &[Keyed<Effect>], region: Rect) -> String {
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
        // A filter with no primitives renders the element as nothing, so a stack
        // whose every entry was skipped would erase the layer; a bare `feOffset`
        // is the shortest "unchanged".
        if graphic == "SourceGraphic" {
            out.push_str("      <feOffset/>\n");
        }
    } else {
        // Bottom to top: drop shadows, the layer, inner shadows — each group in
        // panel order.
        out.push_str("      <feMerge>\n");
        for r in behind.iter().chain(std::iter::once(&graphic)).chain(&over) {
            let _ = writeln!(out, "        <feMergeNode in=\"{r}\"/>");
        }
        out.push_str("      </feMerge>\n");
    }
    out.push_str("    </filter>\n");
    out
}

/// The primitives for one shadow, returning its result's name. An inner shadow
/// inverts the silhouette and clips the result back into the layer's alpha.
fn shadow_graph(out: &mut String, graphic: &str, s: &Shadow, n: usize, inner: bool) -> String {
    let a = format!("fxa{n}");
    let t = format!("fxt{n}");
    // The layer's alpha as a black silhouette — `SourceAlpha`, but from `graphic`
    // since earlier effects may have changed the layer.
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
        // `feMorphology` is a square kernel, so a large spread on a curved shape
        // comes out slightly boxier than on the canvas; it is SVG's only spread
        // primitive, and dropping spread would get the size visibly wrong.
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
    // Colour it: flood the region, cut to the silhouette. `feDropShadow` has no
    // spread and cannot read from `graphic`.
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
        // Confine it to the layer, or the inverted silhouette paints outside it.
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

/// The paint list's visible entries with their index, which names the gradient
/// slot and alignment clip so a hidden paint does not renumber the rest. Of a paint
/// already resolved (`Document::resolved_paint`); read brushes with `drawn`.
fn visible_fills(paint: &Paint) -> impl Iterator<Item = (usize, &Fill)> {
    paint
        .fills
        .iter()
        .enumerate()
        .filter(|(_, f)| f.visible)
        .map(|(i, f)| (i, &f.value))
}

fn visible_strokes(paint: &Paint) -> impl Iterator<Item = (usize, &Stroke)> {
    paint
        .strokes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.visible)
        .map(|(i, s)| (i, &s.value))
}

fn collect_defs(doc: &Document, res: &Resolved, id: NodeId, outer: Affine, defs: &mut Defs) {
    let Some(node) = doc.get(id) else { return };
    // The same composition `emit_node` makes, so region and element share a space.
    let transform = outer * res.used_local_of(node);
    // As drawn: a bound effect number takes its variable's value.
    let effects = doc.resolved_effects(id, node.effects());
    if ondin_render::effects::any_ink(&effects)
        && let Some(region) = effect_region(res, id, transform, &effects)
    {
        defs.add(effect_id(id), effect_def(&effect_id(id), &effects, region));
    }
    // A rail is a def because `<textPath>` can name nothing else; it is in the
    // text node's own space, as the referencing element carries the transform.
    if let NodeKind::Text {
        on_path: Some(rail),
        on_path_flip,
        ..
    } = node.kind()
    {
        // A flip is a reversed rail, not SVG 2's unevenly implemented
        // `side="right"`: our flip *is* traversing backwards (`PathWarp::flip`).
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
                // Only a sweep needs the shape's box, and finding it costs an
                // outline walk; `Linear` and `Radial` carry their own geometry.
                let frame = matches!(g.gradient.kind, GradientKind::Sweep(_))
                    .then(|| paint_frame(node, res, id))
                    .flatten();
                let body = gradient_def(&name, g, frame);
                defs.add(name, body);
            }
            // A `<pattern>`, only when the table can answer; otherwise no def
            // (never a dangling `url(#…)`) and `emit_missing` draws a placeholder.
            Brush::Image(img) => {
                if let (Some(entry), Some(f)) = (
                    doc.image(&img.image.id),
                    framing_of(doc, node, res, id, brush),
                ) {
                    // Adjustments are a `<filter>` def; a neutral picture has none.
                    let filter = img.image.adjust.pipeline().map(|p| {
                        let name = adjust_id(id, slot);
                        defs.add(name.clone(), adjust_def(&name, &p));
                        name
                    });
                    // `Defs::add` dedupes, so the payload is a no-op after the first node.
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
    let paint = doc.resolved_paint(id, node.paint());
    for (i, f) in visible_fills(&paint) {
        consider(f.brush.drawn(), &format!("fill-{i}"));
    }
    for (i, s) in visible_strokes(&paint) {
        consider(s.brush.drawn(), &format!("stroke-{i}"));
    }
    // Identity for children: the node's own `<g transform>` establishes their
    // space, as in `emit_node`'s recursion.
    for child in node.children() {
        collect_defs(doc, res, *child, Affine::IDENTITY, defs);
    }
}

/// The box an image fill is framed in, as in `scene.rs`. For text it is the
/// layout's box; [`outline`] here would silently frame to the ink.
fn paint_frame(node: &Node, res: &Resolved, id: NodeId) -> Option<Rect> {
    match res.used_kind_of(node) {
        NodeKind::Artboard { size, .. } => Some(Rect::new(0.0, 0.0, size.width, size.height)),
        NodeKind::Text { .. } => res.text_layout(id).map(|l| l.bounds()),
        _ => outline(node, res, id).map(|p| p.bounding_box()),
    }
}

/// How an image `brush` sits in this node's frame; [`None`] for anything else, a
/// missing entry or no frame. Mirrors `scene::framing_of`, taking the intrinsic
/// size from the document's table so nothing need be decoded.
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

/// Whether an image `brush` leaves part of the frame to its pad extend — a crop
/// outside `0..1`, which the canvas fills with the edge pixels and a `<pattern>`
/// with the next repeat. Fit's letterbox is clipped instead, so it does not count.
fn crops_past_picture(
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    brush: &Brush,
) -> bool {
    let Some(f) = framing_of(doc, node, res, id, brush) else {
        return false;
    };
    let (Brush::Image(img), Some(frame)) = (brush, paint_frame(node, res, id)) else {
        return false;
    };
    let Some(entry) = doc.image(&img.image.id) else {
        return false;
    };
    if f.extend != Extend::Pad || f.clip.is_some() {
        return false;
    }
    let source = Rect::new(0.0, 0.0, f64::from(entry.width), f64::from(entry.height));
    let picture = f.transform.transform_rect_bbox(source);
    let tol = 1e-6;
    picture.x0 > frame.x0 + tol
        || picture.y0 > frame.y0 + tol
        || picture.x1 < frame.x1 - tol
        || picture.y1 < frame.y1 - tol
}

/// One image definition: a `<pattern>` in the node's own local space, wrapping
/// the encoded original as an `<image>`.
///
/// The tile is the intrinsic pixel box [`ondin_core::ImageRef::framing`] maps
/// *from*, so the whole framing is the one `patternTransform`. A pattern tiles
/// where the canvas pads, which differs only for a hand-edited crop outside
/// `0..1` ([`crops_past_picture`] reports it); Fit's letterbox is cut by
/// [`with_image_clip`].
///
/// `alpha` is the sampler's (§5.5a); `filter` is an [`adjust_def`] id. Both are
/// the node's, so they sit on the `<use>` of the shared `<image>`
/// ([`payload_id`]), with no `width`/`height` (inert there).
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

/// The encoded picture itself, written once per distinct [`ondin_core::ImageId`];
/// the per-node difference stays in the `<pattern>`'s `patternTransform`.
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
/// Writes what `ImageAdjust::pipeline` says (§5.5a). Load-bearing: `sRGB`
/// interpolation (the linear default mismatches the canvas) and a region of the
/// element's own box (the default would bleed into neighbouring tiles).
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
        // The same table on three channels and no `feFuncA`: on alpha, a shadow
        // lift would turn a soft edge opaque.
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

/// Whether `brush` is a picture this writer cannot draw: only a dangling id counts
/// (a link is an `href`; undecodable bytes are the viewer's problem).
fn missing_picture(doc: &Document, brush: &Brush) -> bool {
    match brush {
        Brush::Image(img) => doc.image(&img.image.id).is_none(),
        _ => false,
    }
}

/// The missing-image placeholder as markup, from core's `missing_placeholder` so it
/// cannot drift from `scene::paint_missing`; clipped to the shape's outline.
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
    write_element(
        out,
        node,
        res.used_kind_of(node),
        derived,
        pad,
        "",
        &ground,
        "",
        "",
    );
    let cid = format!("mi-{}-{slot}", id.to_wire().replace(':', "_"));
    // The shape as a clip, so the rim reads as an inner border and the cross stops
    // at the edge; `outline` rather than the box.
    let Some(shape) = outline(node, res, id) else {
        return;
    };
    emit_missing_ink(out, &m, &shape, &cid, pad);
}

/// The rim and cross of a placeholder, cut to `shape`; shared by a missing picture
/// and an abandoned boolean.
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
/// this node is not one; the markup half of `scene::paint_shape`'s branch. The
/// ground is a plain `<path>` since the boolean has no outline of its own.
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

/// Emit `body` inside the letterbox clip `framing` asked for, or plainly. Mirrors
/// `scene::clipped`: neither peniko nor SVG has a transparent extend, so the band
/// beside a contained picture must be cut (only `ImageFit::Fit` asks).
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
/// `center` that the last stop reaches every corner of `frame` (the largest of the
/// four corner distances, since half the diagonal is wrong off-centre).
///
/// In the gradient's own space: the box is pulled back through the brush
/// transform, as `cx`/`cy`/`r` are read before `gradientTransform`. [`None`] for a
/// singular or non-finite transform.
fn sweep_radius(
    center: ondin_core::kurbo::Point,
    frame: Option<Rect>,
    xform: Affine,
) -> Option<f64> {
    let frame = frame?;
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

/// One gradient definition, in the node's local space. `frame` is the shape's own
/// box there, read only by the sweep arm (a sweep has no radius); [`None`] where
/// nothing paints.
fn gradient_def(name: &str, brush: &ondin_core::GradientBrush, frame: Option<Rect>) -> String {
    let g = &brush.gradient;
    // `gradientTransform` is how an elliptical radial gradient is spelled: the
    // squash lives on the brush. Identity is written as nothing, its default.
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
    // Nothing clamps a descending ramp on load; the canvas clamps via
    // `color::brush_to_backend`, so clamp here too or a browser draws a flat fill.
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

    // `extend` is `spreadMethod`; dropping it would silently turn a reflected sheen
    // (read in by `svg_in`) into a flat band.
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
        // No SVG 1.1 sweep: a radial reaching the farthest corner. The `50` is
        // unobservable: no box, or a singular transform, paints nothing.
        GradientKind::Sweep(p) => format!(
            "    <radialGradient id=\"{name}\" gradientUnits=\"userSpaceOnUse\"{spread}{xform} cx=\"{}\" cy=\"{}\" r=\"{}\">\n{stops}    </radialGradient>\n",
            fmt(p.center.x),
            fmt(p.center.y),
            fmt(sweep_radius(p.center, frame, brush.transform).unwrap_or(50.0)),
        ),
    }
}

/// Every way this writer is about to say something inexactly about `node`.
///
/// Read off the node once per emitted node, so it restates each rule and
/// `tests/fidelity.rs` checks the report against the markup. `effects` is the
/// stack as drawn (`Document::resolved_effects`).
fn report_node(
    node: &Node,
    effects: &[ondin_core::Keyed<Effect>],
    filter_written: bool,
    report: &mut Report,
) {
    // `gradient_def` writes a sweep as a radial rather than dropping the paint.
    let sweep = |brush: &Brush| {
        matches!(
            brush,
            Brush::Gradient(g) if matches!(g.gradient.kind, GradientKind::Sweep(_))
        )
    };
    let picture = |brush: &Brush| matches!(brush, Brush::Image(_));
    // Unresolved: a bound paint is a solid, so `drawn` is still right.
    let brushes = || {
        visible_fills(node.paint())
            .map(|(_, f)| f.brush.drawn())
            .chain(visible_strokes(node.paint()).map(|(_, s)| s.brush.drawn()))
    };
    if brushes().any(sweep) {
        report.approximate("a sweep gradient, written as a radial one");
    }
    if brushes().any(picture) {
        // Not "approximated": the picture lands where the document puts it; only
        // no shipped reader can read a `<pattern>` back.
        report.round_trip("an image fill, written as a pattern this app cannot read back");
    }
    // Square-kernel `feMorphology` (see `shadow_graph`), only if it is in the file:
    // nonzero spread, visible effect, and the `<filter>` written.
    if filter_written
        && effects.iter().any(|e| {
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
        // A `<textPath>`'s glyph positions along the rail are the viewer's.
        report.approximate("type on a path, whose glyph placement is left to the viewer");
        if *on_path_flip {
            // Not an approximation: a reversed `d` is the identical drawing (see
            // `collect_defs`); `svg_in` reads it back with the flag clear.
            report.round_trip("flipped type on a path, written as a reversed rail");
        }
    }
}

/// Emit a node (if visible) and recurse into its children in z-order.
///
/// Containers become `<g>` elements so their transform and opacity nest the way
/// the model means them to. `outer` is an extra transform folded into this
/// node's own — only non-identity for the root of a single-artboard export.
// `defs` and `report` are related only by being pass-wide; no struct to please the lint.
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
    // Shown, a bound visibility resolved.
    if !doc.shown(id) {
        return;
    }
    // The effect stack gets a transform-free wrapper, not an attribute: `filter`
    // applies before `clip-path` (a frame's shadow would be cut), and a shape may be
    // several elements.
    let drawn_fx = doc.resolved_effects(id, node.effects());
    let fx = ondin_render::effects::any_ink(&drawn_fx) && defs.has(&effect_id(id));
    // Under the visibility guard: the report is a claim about the file.
    report_node(node, &drawn_fx, fx, report);
    let paint = doc.resolved_paint(id, node.paint());
    if visible_fills(&paint).any(|(_, f)| crops_past_picture(doc, node, res, id, f.brush.drawn())) {
        report
            .approximate("a crop past the picture's edge, repeated where the canvas stretches it");
    }
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
    // Used geometry from here down; what sort of node this is stays on `node.kind()`.
    let transform = outer * res.used_local_of(node);
    // As drawn, a bound opacity resolved.
    let opacity = doc.drawn_opacity(node.id());

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
    let paint = doc.resolved_paint(id, node.paint());
    let strokes: Vec<(usize, &Stroke)> = visible_strokes(&paint).collect();
    // A frame's stroke cannot live inside the group that clips its contents, so a
    // clip plus a stroke makes an outer group (transform, opacity) holding an inner
    // `clip-path` group, then the stroke.
    let split_clip = wrap && !strokes.is_empty() && node.clip();
    // Where the contents sit: one level in, and one more under a clip group.
    let content_depth = depth + usize::from(wrap) + usize::from(split_clip);
    let content_pad = "  ".repeat(content_depth);
    // Where the frame's own border sits: inside the outer group, beside the clip one.
    let own_pad = "  ".repeat(depth + usize::from(wrap));
    if wrap {
        let clip = match res.used_kind_of(node) {
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
        // The name goes on the outer `<g>` in the split case; the inner one only
        // holds the clip.
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

    // A frame's fills come first inside its group, behind its children; strokes
    // follow via `emit_strokes`, outside the clip. Each is a fill like any other
    // (letterbox clip, `emit_missing` placeholder). The slot `fill-{i}` must match
    // what `collect_defs` wrote.
    if let NodeKind::Artboard { size } = res.used_kind_of(node) {
        for (i, f) in visible_fills(&paint) {
            let slot = format!("fill-{i}");
            if missing_picture(doc, f.brush.drawn()) {
                emit_missing(out, node, res, id, None, &slot, &content_pad);
                continue;
            }
            let clip = framing_of(doc, node, res, id, f.brush.drawn()).and_then(|f| f.clip);
            with_image_clip(out, clip, id, &slot, &content_pad, |out, pad| {
                let _ = writeln!(
                    out,
                    "{pad}<rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\"{fill}/>",
                    w = fmt(size.width),
                    h = fmt(size.height),
                    fill = paint_attr("fill", f.brush.drawn(), id, &slot, defs),
                );
            });
        }
    }

    // A mask becomes a `<g clip-path>` over the run of siblings above it (as
    // `scene::MaskRun`), emitting no artwork of its own. No transform: the space is
    // this node's, which `mask_path` answers in. No `clip-rule`: non-zero is both
    // SVG's default and the fill rule.
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
            match child_node.mask_mode() {
                // A shape mask is a `<clipPath>`: a hard region, no compositing.
                MaskMode::Shape => {
                    // [`Resolved::mask_path`]'s `None` masks nothing (a hidden
                    // mask, an empty text node, a group with nothing in it, a
                    // boolean whose operands cancel), so the run carries on
                    // unwrapped, as on the canvas.
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
                // An alpha mask is a `<mask>` holding the layer's own markup, since
                // what masks is its ink. `mask-type: alpha` is load-bearing (SVG
                // defaults to luminance), written as CSS for better support. An
                // ink-less alpha mask erases the run, as on the canvas.
                //
                // The `doc.shown` guard: a hidden mask would leave an empty `<mask>`
                // and erase the whole run.
                MaskMode::Alpha if doc.shown(*child) => {
                    // The region is written ([`ink_in`] inflated by a tenth):
                    // `<mask>`'s default is relative to the viewport and erases
                    // everything outside it.
                    let region = ink_in(res, *child, id, Affine::IDENTITY);
                    let region = match region {
                        // No ink erases the run anywhere; no region is needed.
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
                // A hidden mask erases nothing: no wrapper, `masked` stays false.
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
/// One plain element when the node has at most one fill and one centred stroke;
/// anything else emits the shape once per paint via [`emit_stacked`].
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
    // Before the paints, which it replaces, as in `scene::paint_shape`.
    if emit_failed_boolean(out, doc, res, id, pad) {
        return;
    }
    let paint = doc.resolved_paint(id, node.paint());
    let fills: Vec<(usize, &Fill)> = visible_fills(&paint).collect();
    let strokes: Vec<(usize, &Stroke)> = visible_strokes(&paint).collect();
    let derived = res.boolean_path(id);
    let shape = outline(node, res, id);
    // A letterboxing image forces stacking, as an aligned stroke does: the clip
    // belongs to one paint's element.
    let letterboxed = fills
        .iter()
        .map(|(_, f)| f.brush.drawn())
        .chain(strokes.iter().map(|(_, s)| s.brush.drawn()))
        .any(|b| framing_of(doc, node, res, id, b).is_some_and(|f| f.clip.is_some()));
    // So does a missing picture: the placeholder is several elements and a clip.
    let missing = fills
        .iter()
        .any(|(_, f)| missing_picture(doc, f.brush.drawn()));
    let plain = !letterboxed
        && !missing
        && fills.len() <= 1
        && match strokes.as_slice() {
            [] => true,
            // A per-side stroke or a fitted multi-subpath dash is several
            // `<path>`s, so neither can ride the shape's own element.
            [(_, s)] => {
                align_clip(res.used_kind_of(node), shape.as_ref(), s).is_none()
                    && geometry::effective_sides(res.used_kind_of(node), s).is_all()
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

    // The name rides in front of the transform ([`name_attr`]).
    let t = format!("{}{}", name_attr(node), transform_attr(transform));
    let op = opacity_attr(opacity);
    let fill = match fills.first() {
        Some((i, f)) => paint_attr("fill", f.brush.drawn(), id, &format!("fill-{i}"), defs),
        // Text with no ink at all is black, as in the scene walk; with a stroke
        // the letters stay hollow.
        None if matches!(node.kind(), NodeKind::Text { .. }) && strokes.is_empty() => {
            r##" fill="#000000""##.to_string()
        }
        None => r#" fill="none""#.to_string(),
    };
    // `fill-rule` rides with the fill, even when it is `none` (see `with_fill_rule`).
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
    write_element(
        out,
        node,
        res.used_kind_of(node),
        derived,
        pad,
        &t,
        &fill,
        &stroke,
        &op,
    );
}

/// The pieces a fitted dash pattern is laid along, or `None` when one pattern
/// says it all (`geometry::dash_fit_pieces`).
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
/// Every consumer of a shape's own path goes through here, since
/// `geometry::local_path(kind)` alone answers `None` for a boolean and a text node.
fn outline(node: &Node, res: &Resolved, id: NodeId) -> Option<BezPath> {
    let kind = res.used_kind_of(node);
    match kind {
        // Glyph outlines from the cached layout, the path the scene walk strokes (§6.3).
        NodeKind::Text { .. } => res.text_layout(id).map(ondin_core::text::outline),
        _ => match res.boolean_path(id) {
            Some(path) => Some(path.clone()),
            None => geometry::local_path(kind),
        },
    }
}

/// The dash pattern for a stroke drawn on `shape`, at `width`.
///
/// Wrapped so neither emit path forgets the dot substitution and fit-to-corners
/// scaling (§6.3).
fn resolved_dashes(shape: Option<&BezPath>, s: &Stroke, width: f64) -> Vec<f64> {
    match shape {
        Some(path) => geometry::resolved_dashes(s, width, path),
        // No outline to fit; the dot substitution needs none.
        None => geometry::resolved_dashes(s, width, &ondin_core::kurbo::BezPath::new()),
    }
}

/// A shape whose paints do not fit one element: a group carrying the node's
/// transform and opacity, then every visible fill, then every visible stroke.
///
/// Fills first, then strokes, in list order. The transform and opacity sit on the
/// group: one user space for a `clipPath`, and fading paints separately would let
/// the lower ones show through.
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
    // The stacked `<g>` is the node, so the name goes here and not on its children.
    let _ = writeln!(
        out,
        "{pad}<g{}{}{}>",
        name_attr(node),
        transform_attr(transform),
        opacity_attr(opacity)
    );
    for (i, f) in fills {
        let slot = format!("fill-{i}");
        if missing_picture(doc, f.brush.drawn()) {
            emit_missing(out, node, res, id, derived, &slot, &inner);
            continue;
        }
        let clip = framing_of(doc, node, res, id, f.brush.drawn()).and_then(|f| f.clip);
        // Per fill, so one contained picture does not cut the fill under it.
        with_image_clip(out, clip, id, &slot, &inner, |out, pad| {
            // Through `with_fill_rule`: a `Boolean` with `BoolOp::Exclude` is
            // even-odd, and without the rule its holes fill in on this path.
            let fill = with_fill_rule(paint_attr("fill", f.brush.drawn(), id, &slot, defs), node);
            write_element(
                out,
                node,
                res.used_kind_of(node),
                derived,
                pad,
                "",
                &fill,
                "",
                "",
            );
        });
    }
    emit_strokes(out, doc, node, res, id, &inner, strokes, defs);
    let _ = writeln!(out, "{pad}</g>");
}

/// `fill` with the node's `fill-rule` on it, when that is not SVG's own default.
///
/// Shared by both arms that write a fill. Written only when not `nonzero`, and
/// even when the fill is `none`: a stroke-only shape can still lend its interior to
/// a `clipPath`.
fn with_fill_rule(fill: String, node: &Node) -> String {
    match node.fill_rule() {
        ondin_core::FillRule::NonZero => fill,
        ondin_core::FillRule::EvenOdd => format!(r#"{fill} fill-rule="evenodd""#),
    }
}

/// Every visible stroke of `node`, one after another, inside a group that is already
/// carrying the transform and the opacity.
///
/// Shared by the stacked-shape and frame paths; `write_element` picks the element.
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
        // Unfilled, or it would paint over the fills beneath.
        const UNFILLED: &str = r#" fill="none""#;
        // A per-side stroke is one `<path>` per edge (§6.3); the alignment clip still
        // comes from the whole shape and wraps every side together.
        let sides = geometry::effective_sides(res.used_kind_of(node), s).per_side(s.width);
        // A fitted multi-subpath dash is one `<path>` per subpath: an element has
        // one `stroke-dasharray`.
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
                        let Some(edge) = geometry::side_path(res.used_kind_of(node), *side) else {
                            continue;
                        };
                        // A side is one subpath, so its own fit is exact.
                        let dashes = geometry::resolved_dashes(s, *w, &edge);
                        segments(out, &[(edge, dashes)], *w);
                    }
                }
                (None, Some(parts)) => segments(out, parts, s.width),
                (None, None) => write_element(
                    out,
                    node,
                    res.used_kind_of(node),
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
        // A contained picture letterboxes a stroke as a fill; outside the
        // alignment clip, which cuts a different thing.
        let box_clip = framing_of(doc, node, res, id, s.brush.drawn()).and_then(|f| f.clip);
        with_image_clip(
            out,
            box_clip,
            id,
            &format!("stroke-{i}"),
            inner,
            |out, inner| {
                match align_clip(res.used_kind_of(node), shape.as_ref(), s) {
                    Some((clip_d, rule)) => {
                        // Indexed by the paint's own index, stable under hiding.
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
/// Which element a kind becomes is spelled once; the stacked path calls it per paint.
#[allow(clippy::too_many_arguments)]
fn write_element(
    out: &mut String,
    node: &Node,
    // The node's used kind; every size and outline below is read from it.
    kind: &NodeKind,
    // The outline `Resolved` computed for a `Boolean` node, `None` for any other kind.
    derived: Option<&BezPath>,
    pad: &str,
    t: &str,
    fill: &str,
    stroke: &str,
    op: &str,
) {
    match kind {
        // `<rect rx>` can only say one number, so unequal corners go out as the
        // canvas's own outline (`local_path`).
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
                let d = geometry::local_path(kind)
                    .map(|p| path_d(&p))
                    .unwrap_or_default();
                let _ = writeln!(out, r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#);
            }
        },
        // `<polygon>` keeps both editable as polygons; the points come from
        // `local_path`.
        NodeKind::Polygon { .. } | NodeKind::Star { .. } => {
            let points = geometry::local_path(kind)
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
        // The resolved outline, so a rounded corner exports rounded: SVG has no
        // per-vertex radius and `path` alone is the unrounded authored geometry.
        NodeKind::Path { .. } => {
            if let Some(resolved) = geometry::local_path(kind) {
                let _ = writeln!(
                    out,
                    r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#,
                    d = path_d(&resolved),
                );
            }
        }
        // A boolean exports as its result, not its operands: SVG has no boolean
        // container, and the operands would show the shapes it consumes.
        NodeKind::Boolean { .. } => {
            if let Some(path) = derived {
                let _ = writeln!(
                    out,
                    r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#,
                    d = path_d(path),
                );
            }
        }
        // One `<text>` per line, one `<tspan>` per run, from `text::export_lines`
        // (the cached `TextLayout` holds glyph ids that do not map back to
        // characters). Ink stays on the `<text>`, the face moves to the `<tspan>`s.
        //
        // A rail is the one place placement goes back to the viewer: a `<text>` per
        // glyph is unreachable (ids do not survive ligatures and reordering) and
        // outlines would lose selectable text. Alignment survives as a
        // `startOffset` percentage.
        NodeKind::Text { on_path, .. } => {
            let Some(parts) = ondin_core::TextRef::of(kind) else {
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
                // Percent plus `text-anchor` survives a different font. The node's
                // offset is added to the percentage and leaves the anchor alone: the
                // anchor is how text sits about a point, the offset which point.
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
                    // The same id `collect_defs` named the rail with.
                    rail = rail_id(node.id()),
                );
                return;
            }
            for line in ondin_core::text::export_lines(parts) {
                // No `text-anchor`: alignment, indent and justification are already
                // in the run's `x`, read after `Layout::align`.
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
        // Only a frame's stroke asks for this; `emit_node` writes the background.
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

/// Everything one run's style contributes to its `<tspan>`: face, spacing and
/// decorations. `agreed` resolves an underline disagreeing with a strikethrough on
/// the same run.
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
    // A run's own ink overrides the `<text>`'s; `None` inherits. Alpha rides
    // `fill-opacity`.
    if let Some(c) = style.color {
        let rgba = c.to_rgba8();
        let _ = write!(
            out,
            r##" fill="#{:02x}{:02x}{:02x}"{}"##,
            rgba.r,
            rgba.g,
            rgba.b,
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
    // The three refinements come after the shorthand, which resets its omitted
    // parts. Each is written only when every decoration on the run agrees (CSS
    // gives an element one colour, thickness and style). They are presentation
    // attributes, so a reader ignoring the family ignores all three.
    if let Some(c) = agreed(&each, |d| d.color).flatten() {
        // No `text-decoration-color-opacity` exists, so alpha rides in CSS Color
        // 4's eight-digit hex, only when not opaque.
        let rgba = c.to_rgba8();
        let hex = if rgba.a == 255 {
            format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b, rgba.a)
        };
        let _ = write!(out, r#" text-decoration-color="{hex}""#);
    }
    if let Some(t) = agreed(&each, |d| d.thickness).flatten() {
        // `px`: CSS's unitless length is invalid, unlike `letter-spacing`'s.
        // Floored as the canvas floors it (`core::text::decoration_ink`); the
        // model's clamp is not reached from the op layer, so this is load-bearing:
        // removing it fails `a_decorations_colour_thickness_and_style_are_exported`.
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
    // Skip-ink, only when switched off: CSS's initial `auto` is what the canvas does.
    // Read off the underline alone (it never applies to a `line-through`). `auto`
    // agrees in kind, not pixels; exactness would mean a `<path>` from
    // `DecorationInk::gaps`, at the cost of the `text-decoration-*` family.
    if style.underline.is_some_and(|d| !d.skip_ink) {
        let _ = write!(out, r#" text-decoration-skip-ink="none""#);
    }
    // `text-underline-offset` is underline-only, so a run with both decorations
    // drops it. The sign flips: ours is positive up from the baseline, CSS's
    // positive outward, so a stored `+2` exports as `-2px`. It is not a
    // sub-property of `text-decoration`, so order does not matter.
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
/// disagree or there are none; `text-decoration-*` is element-scoped in CSS.
fn agreed<T: PartialEq>(ds: &[Decoration], f: impl Fn(&Decoration) -> T) -> Option<T> {
    let mut values = ds.iter().map(f);
    let first = values.next()?;
    values.all(|v| v == first).then_some(first)
}

/// The CSS keyword for a line style; all four of ours are CSS keywords verbatim.
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

/// ` data-name="…"` — the layer's name, on the element that is the layer. Not
/// `id`, which must be a unique XML `Name` and would share a namespace with the
/// synthesised ids; `svg_in` prefers it and falls back to `id`. Empty writes nothing.
fn name_attr(node: &Node) -> String {
    match node.name() {
        "" => String::new(),
        name => format!(r#" data-name="{}""#, escape_xml(name)),
    }
}

/// ` <name>="matrix(..)"` for `transform` or `patternTransform`, omitted when it
/// says nothing. Identity is judged on the printed form: a rounded rect's box top
/// is `-8.88e-16` (see [`fmt`]), a hair from identity yet printing as one.
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
/// SVG has no stroke alignment, so as on the canvas (§6.3) the stroke is doubled
/// and the wrong half clipped away: inside clips to the shape, outside to
/// everything but it (shape plus enclosing box, even-odd). `None` for a centred
/// stroke, an open shape, or no outline.
fn align_clip(
    kind: &NodeKind,
    shape: Option<&BezPath>,
    s: &Stroke,
) -> Option<(String, &'static str)> {
    if s.align == StrokeAlign::Center || !geometry::stroke_align_applies(kind) {
        return None;
    }
    let path = shape?;
    Some(match s.align {
        StrokeAlign::Outside => {
            let b = path.bounding_box();
            // The ring must clear the ink, not the shape (as `scene::outer_bounds`):
            // the doubled stroke reaches `limit` times further at a mitre.
            let reach =
                geometry::effective_sides(kind, s).max_width(s.width) * s.miter_limit.max(1.0);
            let pad_out = (b.width() + b.height()).max(1.0) + reach;
            let mut complement = b.inflate(pad_out, pad_out).to_path(0.1);
            complement.extend(path.iter());
            (path_d(&complement), r#" clip-rule="evenodd""#)
        }
        _ => (path_d(path), ""),
    })
}

/// Stroke attributes for one stroke at `width` (a `Custom` side's own, doubled in
/// the aligned construction). `dashes` is already resolved
/// ([`geometry::resolved_dashes`]): SVG has no zero-length dot or "fit to corners".
fn stroke_attr(
    id: NodeId,
    index: usize,
    s: &Stroke,
    width: f64,
    dashes: &[f64],
    defs: &Defs,
) -> String {
    let paint = paint_attr(
        "stroke",
        s.brush.drawn(),
        id,
        &format!("stroke-{index}"),
        defs,
    );
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
    // Only when not the default, which SVG puts at 4, as kurbo does.
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
/// `defs` is asked only for an image: `url(#…)` for a paint server not in the file
/// is an SVG error where "no paint" is a picture.
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
        // `{attr}-opacity` multiplies the paint server's own alpha, so stops keep
        // `stop-opacity`; through `alpha_attr` so opacity rounds the same everywhere.
        Brush::Gradient(g) => format!(
            r#" {attr}="url(#{})"{}"#,
            gradient_id(id, slot),
            alpha_attr(
                &format!("{attr}-opacity"),
                (g.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            ),
        ),
        // A `<pattern>` (see [`image_def`]), else `none`. Only a stroke reaches
        // `none`: a fill gets the placeholder, a stroke has no area for one. A
        // ` none` fallback after the `url(…)` would fix nothing.
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

/// A `d` attribute, rounded like every other number (`BezPath::to_svg` prints
/// full `f64` precision). The JSON snapshot's path is this string too: full
/// precision carried the platform's math library into the 16th digit.
pub(crate) fn path_d(path: &ondin_core::kurbo::BezPath) -> String {
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

/// Format a float compactly: integers without a decimal point, else trimmed.
/// Shared with the code export, whose CSS numbers want the same spelling.
pub(crate) fn fmt(v: f64) -> String {
    // A non-finite number is written as `0`: `NaN`/`inf` is no SVG `<number>` and
    // would make the whole file unparseable. A floor; producers are guarded at their
    // own ends (e.g. [`effect_region`]).
    if !v.is_finite() {
        return "0".to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    // `-0` is dust (a rounded rect's box top is `-8.88e-16`), never worth writing.
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

#[cfg(test)]
mod def_count_tests {
    //! Collecting defs does not compare ids quadratically — counted, not timed.
    use super::*;

    /// `n` rects, each with its own linear gradient fill: `n` defs.
    fn gradients(n: usize) -> (Document, Resolved) {
        use ondin_core::kurbo::{Point, Size};
        use ondin_core::peniko::Gradient;
        use ondin_core::{Fill, IdSource, NodeKind, Operation, Transaction};
        let mut ids = IdSource::new(0xDEF5);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut ops = Vec::with_capacity(n * 2);
        for i in 0..n {
            let id = ids.mint();
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
            let g =
                Gradient::new_linear(Point::new(0.0, 0.0), Point::new(10.0, 10.0)).with_stops([
                    (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
                    (1.0_f32, Color::from_rgba8((i % 256) as u8, 0, 255, 255)),
                ]);
            ops.push(Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: Brush::Gradient(g.into()).into(),
                    visible: true,
                }]),
            });
        }
        doc.apply(&Transaction(ops)).expect("the fixture");
        let res = Resolved::rebuild(&doc);
        (doc, res)
    }

    /// An export of D defs compares fewer than D ids (a scan compares about D²/2).
    ///
    /// flip: `Defs::add` back to the linear scan → fails at the comparison-count assertion.
    #[test]
    fn collecting_defs_compares_fewer_ids_than_there_are_defs() {
        const N: usize = 2_000;
        let (doc, res) = gradients(N);
        DEF_KEY_COMPARISONS.with(|c| c.set(0));
        let out = svg(&doc, &res, None);
        let compared = DEF_KEY_COMPARISONS.with(std::cell::Cell::get);
        assert_eq!(
            out.matches("<linearGradient").count(),
            N,
            "the fixture: every rect must earn a def"
        );
        assert!(
            compared < N as u64,
            "collecting {N} defs compared ids {compared} times — a set compares a \
             handful per insert and a scan compares every id already collected, so \
             a count near N²/2 = {} is the square coming back",
            (N * N / 2)
        );
    }
}
