//! Stable public projection for AI handoff / MCP reads (§7.3).
//!
//! A documented, serde-serializable view of the document that MCP read tools
//! return. Versioned independently (`snapshot_version`) and decoupled from the
//! internal save schema: it exposes derived data (world transform, world
//! bounds) that the save format never stores, and a flattened paint summary
//! rather than raw `peniko` brushes.

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, RoundedRectRadii};
use ondin_core::{Document, Fill, Node, NodeId, NodeKind, Resolved, Stroke};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bumped to 2 when text style detail (line height, alignment, sizing),
/// gradient stop detail, and the `warnings` list were added. Versioned
/// independently of the save schema (§7.3) precisely so this can move without
/// touching document compatibility.
///
/// **3** is the text attribute model (§5.4): `line_height` became a string, so
/// the three modes are distinguishable; `align` gained `start`/`end`; sizing
/// gained `AutoHeight`; and `styled_runs` reports whether the text is uniform.
/// `line_height` changing *type* is the reason this is a bump rather than an
/// addition — a reader that expected a number gets a string.
///
/// **4** is images: `PaintSummary::Image` went from a bare variant to one
/// carrying the id, the framing mode and the alpha (§7.1, §15 D185). Bumped
/// on the precedent of 2 rather than because a field changed type — a variant
/// growing detail is what that bump was for, and a reader deserializing
/// `{"type":"Image", …}` into the old unit variant is the case a version number
/// exists to make legible.
///
/// **5** is image adjustments: `PaintSummary::Image` gained `adjust`, the
/// non-neutral sliders as a map (§5.5a). A field, not a type change — but a
/// reader that has never heard of adjustments would describe an adjusted
/// photograph as the file it came from, which is the same class of wrong 4 was
/// bumped for. Absent from the JSON entirely when nothing is adjusted, so an
/// untouched picture's summary is byte-for-byte what version 4 wrote.
///
/// **6** is masks: `NodeSnapshot` gained `mask` beside `clip` (§5.3, §15 D282).
/// The strongest case in this list, and by 5's own argument. A reader that has
/// never heard of masks gets **two** statements wrong about the same page — it
/// describes the mask as a shape sitting there in whatever fill it carries, when
/// nothing of it is drawn, and it describes every layer above it as whole, when
/// each is cut to that shape. Version 5's bump was for one such misreading of one
/// layer; this is two, and the second is about layers the flag does not appear on.
///
/// Unlike `adjust` it is **not** absent when false — it sits beside `clip`, which
/// has always been reported for every node because the model carries it there — so
/// this is also the first bump where an untouched document's snapshot changes. That
/// is a reason for the number to move rather than an argument against reporting it:
/// a consumer diffing snapshots reads the version first.
///
/// **7** is the mask *mode*: `NodeSnapshot::mask_mode`, `"Shape"` or `"Alpha"`
/// (§5.3, §15 D288). Same class as 6 and the same argument one step on — a reader
/// on 6 knows a layer is a mask and describes a soft-edged alpha mask as a hard
/// clip, which is wrong about every pixel along its edge.
///
/// **8** is the effect stack: `NodeSnapshot::effects` (§5.3a, §15 D333). The same
/// argument `adjust` made at 5, and absent for the same reason — a layer with no
/// effect carries no key, so an untouched document's snapshot is byte-identical
/// and only the number moves. A reader on 7 cannot tell "no effects" from
/// "effects this writer did not know about", and answers the first: it describes
/// a layer with a drop shadow as a flat one.
///
/// **9** is a frame's fill: `Geometry::Artboard` lost its `background`, because a
/// frame's paint is its fill list now and arrives in [`NodeSnapshot::fills`] with
/// every other layer's (§5.3, §15 D400).
///
/// **Two of this list's arguments at once, which is why it is not a judgement
/// call.** It is 5's: a reader on 8 looks for a frame's ground where the schema
/// told it to look, finds nothing, and describes a filled frame as an empty one —
/// and worse than 5's case, because it would *also* be reading the fill it wanted
/// in `fills` and have no way to know the two were ever one thing. And it is 6's:
/// the key is not absent-when-default but simply gone, so **an untouched
/// document's snapshot changes bytes**, and a consumer diffing snapshots reads the
/// version first.
///
/// ⚠️ **Reporting it in one place rather than two is the point of the removal.**
/// Leaving `background` beside `fills` would have described one paint twice and
/// left a writer two places to send it — which is the model's own old problem
/// (§15 D400) surviving in the snapshot after it had gone from the document.
/// **10** — `PaintSummary::Gradient` gains `opacity` (§15 D767). A gradient's ramp
/// carries its own multiplier now, so a reader on 9 sees a half-faded gradient and
/// an opaque one as the same paint: the stops are identical in both and the
/// difference lived nowhere in the summary. The image arm has carried its sampler
/// alpha since D185 for exactly this reason, and this is the arm that had no
/// multiplier to carry until the document gained one.
///
/// ⚠️ **This is not the `.ondin` format version** and bumping it says nothing about
/// a saved document — that is `ondin_core::io::CURRENT_SCHEMA_VERSION`, which D767
/// did **not** move: the new field is `#[serde(default)]` plus
/// `skip_serializing_if`, so every gradient ever saved loads unchanged and re-saves
/// byte-identical. The two versions are different artefacts and a reader looking
/// for the document format will find this constant first.
pub const SNAPSHOT_VERSION: u32 = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub snapshot_version: u32,
    pub root: String,
    pub nodes: Vec<NodeSnapshot>,
    /// Problems an agent should know about before reasoning over the document
    /// — currently font families the machine does not have, which are being
    /// rendered with a fallback face (§5.4a, §7.3). Empty when all is well.
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

/// A non-fatal problem with the document as it currently resolves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Warning {
    /// A text node asks for a family that is not registered; it is being laid
    /// out with the bundled fallback, so its measurements are not what the
    /// author intended.
    MissingFont { node: String, family: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub parent: Option<String>,
    pub children: Vec<String>,
    /// The artboard this node belongs to (its nearest artboard ancestor), if any.
    pub artboard: Option<String>,
    /// Local transform coefficients `[a, b, c, d, e, f]`.
    pub local_transform: [f64; 6],
    /// World transform coefficients `[a, b, c, d, e, f]`.
    pub world_transform: [f64; 6],
    /// World-space, stroke-expanded bounds `[min_x, min_y, max_x, max_y]`, if any.
    pub world_bounds: Option<[f64; 4]>,
    pub visible: bool,
    pub locked: bool,
    /// Whether a resize of this node holds its aspect ratio. Not `locked`
    /// above, which says the layer refuses edits altogether.
    pub proportions_locked: bool,
    pub opacity: f32,
    /// Whether this node clips its children to its own frame. Only frames act
    /// on it; it is reported for every node because the model carries it there.
    pub clip: bool,
    /// Whether this node is a mask for the siblings above it — it draws nothing,
    /// and its outline clips them. Not `clip` above, which is about children.
    pub mask: bool,
    /// *How* it masks: `"Shape"` or `"Alpha"`. Reported for every node, like
    /// `mask` and `clip`, because the model carries it on every node.
    ///
    /// A **string**, not the enum, on the rule §7.3 already follows for
    /// `line_height`: this projection is read by things that are not this
    /// program, and a name survives a mode being added where an integer
    /// discriminant silently shifts meaning.
    ///
    /// Owned rather than borrowed because this type is `Deserialize` too — a
    /// snapshot is read back as well as written, and `&'static str` cannot be.
    pub mask_mode: String,
    /// Where the node's transforms turn and mirror about, in its own local
    /// space, or absent while it still sits on the centre of the node's box.
    ///
    /// Reported as the resolved *point* rather than as the model's fraction-or-
    /// point enum: a reader of the snapshot is asking where the thing pivots, and
    /// which of the two ways the model stores that is a detail of how it survives
    /// a resize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<[f64; 2]>,
    /// Kind-specific geometry (size, endpoints, text content, …).
    pub geometry: Geometry,
    pub fills: Vec<PaintSummary>,
    pub strokes: Vec<StrokeSummary>,
    /// The effect stack (§5.3a), **visible entries only**, in compositing order —
    /// the same filter `fills` and `strokes` above apply.
    ///
    /// Absent for the overwhelming majority of nodes, so it costs nothing in a
    /// document that carries none. Reported at all because the alternative is a
    /// projection that describes a layer with a drop shadow as a layer with no
    /// shadow: a reader of this file cannot tell "no effects" from "effects this
    /// writer does not know about", and silently answering the first is the
    /// failure mode this whole file is careful about elsewhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectSummary>,
}

/// One effect, flattened for a reader that is not this program.
///
/// **`kind` is a string** on the same rule `mask_mode` follows above: a name
/// survives a variant being added where an integer discriminant silently shifts
/// meaning. The numbers are the authored ones — a blur's *radius*, not the
/// gaussian deviation the writers take (`ondin_core::effect::BLUR_DEVIATION`) —
/// because those are what the panel shows and what a caller would set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectSummary {
    /// `"DropShadow"`, `"InnerShadow"`, `"LayerBlur"` or `"Filters"`.
    pub kind: String,
    /// Shadows only: offset, blur radius, spread, and the colour whose alpha is
    /// the shadow's opacity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The shadow's opacity — which in the model *is* the colour's alpha, split
    /// out here exactly as `PaintSummary::Solid` splits a fill's, so a reader
    /// gets one convention rather than two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    /// `Filters` only: factors with 1.0 neutral, and degrees for the hue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Geometry {
    None,
    Rect {
        width: f64,
        height: f64,
        /// Per corner, named rather than positional so a reader never has to
        /// know whether the four are clockwise or in CSS order.
        corner_radii: RoundedRectRadii,
    },
    Ellipse {
        width: f64,
        height: f64,
    },
    Polygon {
        width: f64,
        height: f64,
        sides: u32,
    },
    Star {
        width: f64,
        height: f64,
        points: u32,
        /// Inner radius as a fraction of the outer one, 0..=1.
        inner_ratio: f64,
    },
    Line {
        end_x: f64,
        end_y: f64,
    },
    Path {
        svg: String,
    },
    Text {
        content: String,
        font_family: String,
        font_size: f64,
        weight: u16,
        italic: bool,
        /// `auto` (the font's own line height), `N%` of the font size, or `Npx`.
        ///
        /// A string because the model's three modes are genuinely different
        /// things, and a bare number could only report one of them — which is
        /// how a snapshot ends up claiming a line height of `1` for text set at
        /// the font's own leading.
        line_height: String,
        /// `start` | `end` | `left` | `center` | `right` | `justify`.
        align: String,
        /// `auto` when the box grows both ways, else the width it wraps at.
        sizing: TextSizingSummary,
        /// How many character-attribute overrides the node carries; `0` for
        /// uniform text. Enough for an agent to know the text is not uniform
        /// without the snapshot growing a whole span list.
        styled_runs: usize,
    },
    /// A frame. **No `background` here**: it had one until 2026-09-01, when a
    /// frame's ground became an ordinary entry in its fill list (§15 D400) and so
    /// started arriving in `NodeSummary::fills` with every other layer's. Repeating
    /// it here would be one paint reported twice, and an agent reading a frame
    /// would have had to know which of the two to write to.
    Artboard {
        width: f64,
        height: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TextSizingSummary {
    Auto,
    AutoHeight { width: f64 },
    Fixed { width: f64, height: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PaintSummary {
    /// Solid color as `#rrggbb` plus 0..=1 alpha.
    Solid { color: String, alpha: f32 },
    /// A gradient, with enough detail for an agent to describe or recreate it
    /// rather than just know that one exists.
    Gradient {
        /// `linear` | `radial` | `sweep`.
        kind: String,
        /// The ramp's own opacity, `0..=1`, multiplied over every stop's alpha
        /// (§15 D767).
        ///
        /// **Version 10 exists for this field.** Without it a half-faded gradient
        /// and an opaque one describe identically — the stops are the same in both
        /// — which is the same hole the image arm's `alpha` was added to close: a
        /// summary that omits a multiplier is not a summary an agent can recreate
        /// the paint from. The image fill has carried its sampler alpha since
        /// §15 D185 for exactly this reason, and the gradient is the arm that had
        /// no multiplier to carry until D767 gave it one.
        opacity: f32,
        stops: Vec<GradientStopSummary>,
    },
    /// A picture, named by the id that keys the document's image table — which
    /// is its content hash, so it also says when two fills are the same file.
    ///
    /// **The image plan asked for exactly this and it was a bare variant**
    /// (§15 D185): "carries
    /// enough to identify *which* image". A summary that says only "there is a
    /// photograph here" cannot answer the first question anyone asks about one.
    /// The mode comes with it because it is the difference between a picture that
    /// covers the shape and one that repeats across it, and neither is visible
    /// from the id.
    Image {
        /// The content hash the table is keyed by.
        id: String,
        /// `fill` | `fit` | `crop` | `tile`.
        fit: String,
        /// The sampler's alpha, which is what an image fill's opacity is (§5.5a).
        alpha: f32,
        /// The adjustments that are not at rest, by name — `{"exposure": 0.3}`.
        ///
        /// **Only the moved ones, and absent altogether when none has moved.**
        /// Seven zeroes on every photograph in a document would be seven zeroes
        /// an agent has to read past to find the one that matters, and the
        /// overwhelming majority of pictures carry none. The values are the
        /// model's own `-1..=1`, not the panel's percentages: a snapshot
        /// describes the document.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        adjust: BTreeMap<String, f32>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradientStopSummary {
    pub offset: f32,
    /// `#rrggbb`.
    pub color: String,
    pub alpha: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrokeSummary {
    pub paint: PaintSummary,
    pub width: f64,
}

/// Project a document + resolved layer into the public snapshot schema.
pub fn snapshot(doc: &Document, res: &Resolved) -> Snapshot {
    let mut nodes = Vec::new();
    collect(doc, res, doc.root(), None, &mut nodes);
    // Deterministic order by id.
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    let warnings = collect_warnings(&nodes);
    Snapshot {
        snapshot_version: SNAPSHOT_VERSION,
        root: doc.root().to_wire(),
        nodes,
        warnings,
    }
}

/// One warning per text node whose family is not registered. Derived from the
/// already-built node list so the order is deterministic too.
fn collect_warnings(nodes: &[NodeSnapshot]) -> Vec<Warning> {
    nodes
        .iter()
        .filter_map(|n| match &n.geometry {
            Geometry::Text { font_family, .. }
                if !ondin_core::text::is_family_available(font_family) =>
            {
                Some(Warning::MissingFont {
                    node: n.id.clone(),
                    family: font_family.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

fn collect(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    artboard: Option<NodeId>,
    out: &mut Vec<NodeSnapshot>,
) {
    let Some(node) = doc.get(id) else { return };
    out.push(node_snapshot(doc, node, res, artboard));

    // A node under an artboard reports that artboard as its membership; the
    // artboard itself is the membership root for its descendants.
    let child_artboard = match node.kind() {
        NodeKind::Artboard { .. } => Some(id),
        _ => artboard,
    };
    for child in node.children() {
        collect(doc, res, *child, child_artboard, out);
    }
}

fn node_snapshot(
    doc: &Document,
    node: &Node,
    res: &Resolved,
    artboard: Option<NodeId>,
) -> NodeSnapshot {
    let id = node.id();
    NodeSnapshot {
        id: id.to_wire(),
        kind: kind_name(node.kind()).to_string(),
        name: node.name().to_string(),
        parent: node.parent().map(|p| p.to_wire()),
        children: node.children().iter().map(|c| c.to_wire()).collect(),
        artboard: artboard.map(|a| a.to_wire()),
        local_transform: node.transform().as_coeffs(),
        world_transform: res
            .world_transform(id)
            .unwrap_or(Affine::IDENTITY)
            .as_coeffs(),
        world_bounds: res
            .world_bounds(id)
            .map(|b| [b.min_x(), b.min_y(), b.max_x(), b.max_y()]),
        visible: node.visible(),
        locked: node.locked(),
        proportions_locked: node.proportions_locked(),
        opacity: node.opacity(),
        clip: node.clip(),
        mask: node.mask(),
        mask_mode: node.mask_mode().label().to_string(),
        pivot: node.pivot().map(|p| {
            let local = ondin_core::local_box(doc, res, id).unwrap_or_default();
            let at = ondin_core::geometry::pivot_point(Some(p), local);
            [at.x, at.y]
        }),
        geometry: geometry(node.kind()),
        fills: node
            .paint()
            .fills
            .iter()
            .filter(|f: &&Fill| f.visible)
            .map(|f| paint_summary(&f.brush))
            .collect(),
        strokes: node
            .paint()
            .strokes
            .iter()
            .filter(|s: &&Stroke| s.visible)
            .map(|s| StrokeSummary {
                paint: paint_summary(&s.brush),
                width: s.width,
            })
            .collect(),
        effects: node
            .effects()
            .iter()
            .filter(|e| e.visible)
            .map(effect_summary)
            .collect(),
    }
}

fn effect_summary(e: &ondin_core::Effect) -> EffectSummary {
    use ondin_core::EffectKind;
    let base = |kind: &str| EffectSummary {
        kind: kind.to_string(),
        offset: None,
        blur: None,
        spread: None,
        color: None,
        opacity: None,
        brightness: None,
        contrast: None,
        saturation: None,
        hue: None,
    };
    match &e.kind {
        EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => {
            let (color, alpha) = hex(s.color);
            EffectSummary {
                offset: Some([s.offset.x, s.offset.y]),
                blur: Some(s.blur),
                spread: Some(s.spread),
                color: Some(color),
                opacity: Some(alpha),
                ..base(match e.kind {
                    EffectKind::DropShadow(_) => "DropShadow",
                    _ => "InnerShadow",
                })
            }
        }
        EffectKind::LayerBlur { radius } => EffectSummary {
            blur: Some(*radius),
            ..base("LayerBlur")
        },
        EffectKind::Filters(f) => EffectSummary {
            brightness: Some(f.brightness),
            contrast: Some(f.contrast),
            saturation: Some(f.saturation),
            hue: Some(f.hue),
            ..base("Filters")
        },
    }
}

fn kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Root => "root",
        NodeKind::Artboard { .. } => "artboard",
        NodeKind::Group => "group",
        NodeKind::Rect { .. } => "rect",
        NodeKind::Ellipse { .. } => "ellipse",
        NodeKind::Polygon { .. } => "polygon",
        NodeKind::Star { .. } => "star",
        NodeKind::Line { .. } => "line",
        NodeKind::Path { .. } => "path",
        NodeKind::Text { .. } => "text",
        // Not "boolean": an agent reading the snapshot wants to know *which*
        // operation, and it is the one kind whose behaviour is a field.
        NodeKind::Boolean { op } => match op {
            ondin_core::BoolOp::Union => "boolean-union",
            ondin_core::BoolOp::Subtract => "boolean-subtract",
            ondin_core::BoolOp::Intersect => "boolean-intersect",
            ondin_core::BoolOp::Exclude => "boolean-exclude",
        },
    }
}

fn geometry(kind: &NodeKind) -> Geometry {
    match kind {
        // A boolean reports no geometry of its own for the same reason a group
        // does not: what it *is* comes from its children, and the snapshot already
        // carries those. Its derived outline is not reported — the snapshot
        // describes the document, and that path is `Resolved`'s.
        NodeKind::Root | NodeKind::Group | NodeKind::Boolean { .. } => Geometry::None,
        NodeKind::Artboard { size } => Geometry::Artboard {
            width: size.width,
            height: size.height,
        },
        NodeKind::Rect { size, corner_radii } => Geometry::Rect {
            width: size.width,
            height: size.height,
            corner_radii: *corner_radii,
        },
        NodeKind::Ellipse { size } => Geometry::Ellipse {
            width: size.width,
            height: size.height,
        },
        NodeKind::Polygon { size, sides } => Geometry::Polygon {
            width: size.width,
            height: size.height,
            sides: *sides,
        },
        NodeKind::Star {
            size,
            points,
            inner_ratio,
        } => Geometry::Star {
            width: size.width,
            height: size.height,
            points: *points,
            inner_ratio: *inner_ratio,
        },
        NodeKind::Line { end } => Geometry::Line {
            end_x: end.x,
            end_y: end.y,
        },
        // The resolved outline, as the SVG writer emits — the snapshot is the
        // AI-handoff surface and a shape described unrounded there is a shape
        // described wrong (§15 D119).
        NodeKind::Path { .. } => Geometry::Path {
            svg: ondin_core::geometry::local_path(kind)
                .unwrap_or_default()
                .to_svg(),
        },
        NodeKind::Text {
            content,
            style,
            spans,
            paragraph,
            sizing,
            ..
        } => Geometry::Text {
            content: content.clone(),
            font_family: style.font_family.clone(),
            font_size: style.font_size,
            weight: style.weight,
            italic: style.italic,
            line_height: match style.line_height {
                None => "auto".to_string(),
                Some(ondin_core::Length::Em(m)) => format!("{}%", m * 100.0),
                Some(ondin_core::Length::Px(v)) => format!("{v}px"),
            },
            align: match paragraph.align {
                ondin_core::TextAlign::Start => "start",
                ondin_core::TextAlign::End => "end",
                ondin_core::TextAlign::Left => "left",
                ondin_core::TextAlign::Center => "center",
                ondin_core::TextAlign::Right => "right",
                ondin_core::TextAlign::Justify => "justify",
            }
            .to_string(),
            sizing: match sizing {
                ondin_core::TextSizing::Auto => TextSizingSummary::Auto,
                ondin_core::TextSizing::AutoHeight(w) => {
                    TextSizingSummary::AutoHeight { width: *w }
                }
                ondin_core::TextSizing::Fixed(s) => TextSizingSummary::Fixed {
                    width: s.width,
                    height: s.height,
                },
            },
            styled_runs: spans.as_slice().len(),
        },
    }
}

fn hex(c: ondin_core::peniko::Color) -> (String, f32) {
    let rgba = c.to_rgba8();
    (
        format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b),
        rgba.a as f32 / 255.0,
    )
}

fn paint_summary(brush: &Brush) -> PaintSummary {
    match brush {
        Brush::Solid(c) => {
            let (color, alpha) = hex(*c);
            PaintSummary::Solid { color, alpha }
        }
        // ⚠️ **The brush's transform is deliberately absent, like the geometry it
        // belongs to** (§15 D412). This summary is a *kind* and a ramp — no start,
        // no end, no centre — so a squash would be the only piece of a gradient's
        // placement an agent could see, which is worse than seeing none of it.
        // It goes in when the geometry does, which is a decision nobody has taken.
        Brush::Gradient(g) => PaintSummary::Gradient {
            kind: match g.gradient.kind {
                ondin_core::peniko::GradientKind::Linear(_) => "linear",
                ondin_core::peniko::GradientKind::Radial(_) => "radial",
                ondin_core::peniko::GradientKind::Sweep(_) => "sweep",
            }
            .to_string(),
            opacity: g.opacity,
            stops: g
                .gradient
                .stops
                .iter()
                .map(|s| {
                    let (color, alpha) =
                        hex(s.color.to_alpha_color::<ondin_core::peniko::color::Srgb>());
                    GradientStopSummary {
                        offset: s.offset,
                        color,
                        alpha,
                    }
                })
                .collect(),
        },
        Brush::Image(img) => PaintSummary::Image {
            id: img.image.id.0.clone(),
            fit: match img.image.fit {
                ondin_core::ImageFit::Fill => "fill",
                ondin_core::ImageFit::Fit => "fit",
                ondin_core::ImageFit::Crop => "crop",
                ondin_core::ImageFit::Tile => "tile",
            }
            .to_string(),
            alpha: img.sampler.alpha,
            adjust: ondin_core::Adjustment::ALL
                .iter()
                .filter(|k| k.of(&img.image.adjust) != 0.0)
                .map(|k| (k.label().to_ascii_lowercase(), k.of(&img.image.adjust)))
                .collect(),
        },
    }
}
