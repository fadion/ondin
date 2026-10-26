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

/// The snapshot schema version, bumped whenever a reader of an older version
/// would misdescribe the document or an untouched document's bytes change. It is
/// independent of the save schema (`ondin_core::io::CURRENT_SCHEMA_VERSION`, §7.3).
///
/// - **2** text style detail, gradient stop detail and `warnings`.
/// - **3** the text attribute model (§5.4): `line_height` became a string, `align`
///   gained `start`/`end`, sizing gained `AutoHeight`, `styled_runs` was added.
/// - **4** `PaintSummary::Image` carries the id, framing mode and alpha (§15 D185).
/// - **5** `PaintSummary::Image` gained `adjust`, absent when nothing is adjusted.
/// - **6** `NodeSnapshot::mask`, reported for every node beside `clip` (§5.3).
/// - **7** `NodeSnapshot::mask_mode`, `"Shape"` or `"Alpha"`.
/// - **8** `NodeSnapshot::effects`, absent when empty (§5.3a).
/// - **9** `Geometry::Artboard` lost `background`; a frame's paint arrives in
///   `fills` like every other layer's, so one paint is reported in one place.
/// - **10** `PaintSummary::Gradient` gained `opacity`, so a faded gradient differs
///   from an opaque one.
/// - **11** every platform-sensitive number (`geometry.svg`, `local_transform`,
///   `world_transform`, `world_bounds`) is written at the SVG writer's four places.
/// - **12** pages: `root` became `pages`, and every node says which page it is on.
pub const SNAPSHOT_VERSION: u32 = 12;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub snapshot_version: u32,
    /// The pages in order. Each page's root is also in `nodes`, kind `Root`.
    pub pages: Vec<PageSnapshot>,
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

/// One page: its root's id and name, and the page-level settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageSnapshot {
    pub id: String,
    pub name: String,
    /// `#rrggbb`; a ground is always opaque.
    pub ground: String,
    /// Left out of kit publishing, exporting all pages and presenting on past it.
    pub draft: bool,
    /// The mode the page's frames read in unless they set their own, by mode id.
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub id: String,
    pub kind: String,
    pub name: String,
    /// The page this node is on (its own id for a page's root).
    pub page: String,
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
    /// *How* it masks: `"Shape"` or `"Alpha"`, reported for every node.
    ///
    /// A string, not the enum, so a name survives a mode being added; owned
    /// because the type is `Deserialize` too.
    pub mask_mode: String,
    /// Where the node's transforms turn and mirror about, in its local space, or
    /// absent while it sits on the centre of its box. Reported as the resolved
    /// point, not the model's fraction-or-point enum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<[f64; 2]>,
    /// Kind-specific geometry (size, endpoints, text content, …).
    pub geometry: Geometry,
    pub fills: Vec<PaintSummary>,
    pub strokes: Vec<StrokeSummary>,
    /// The effect stack (§5.3a), visible entries only, in compositing order — the
    /// same filter `fills` and `strokes` apply. Absent when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectSummary>,
}

/// One effect, flattened for a reader that is not this program. `kind` is a
/// string, like `mask_mode`. The numbers are the authored ones — a blur's radius,
/// not the gaussian deviation the writers take (`ondin_core::effect::BLUR_DEVIATION`).
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
    /// The shadow's opacity, which in the model is the colour's alpha, split out
    /// as `PaintSummary::Solid` splits a fill's.
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
        /// Per corner, named so a reader need not know the positional order.
        corner_radii: RoundedRectRadii,
        /// `0..=1`, absent at 0 (circular corners).
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_smoothing: f64,
    },
    Ellipse {
        width: f64,
        height: f64,
    },
    Polygon {
        width: f64,
        height: f64,
        sides: u32,
        /// One radius on every vertex; absent when square-cornered.
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_radius: f64,
    },
    Star {
        width: f64,
        height: f64,
        points: u32,
        /// Inner radius as a fraction of the outer one, 0..=1.
        inner_ratio: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_radius: f64,
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
        /// `auto` (the font's own line height), `N%` of the font size, or `Npx`;
        /// a string because a bare number could report only one of the three modes.
        line_height: String,
        /// `start` | `end` | `left` | `center` | `right` | `justify`.
        align: String,
        /// `auto` when the box grows both ways, else the width it wraps at.
        sizing: TextSizingSummary,
        /// How many character-attribute overrides the node carries; `0` for
        /// uniform text.
        styled_runs: usize,
    },
    /// A frame. It has no background here: its ground is an ordinary entry in
    /// `NodeSnapshot::fills`.
    Artboard {
        width: f64,
        height: f64,
    },
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
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
        /// The ramp's own opacity, `0..=1`, multiplied over every stop's alpha;
        /// without it a faded gradient and an opaque one read identically.
        opacity: f32,
        stops: Vec<GradientStopSummary>,
    },
    /// A picture, named by the id that keys the document's image table — its
    /// content hash, so it also says when two fills are the same file.
    Image {
        /// The content hash the table is keyed by.
        id: String,
        /// `fill` | `fit` | `crop` | `tile`.
        fit: String,
        /// The sampler's alpha, which is what an image fill's opacity is (§5.5a).
        alpha: f32,
        /// The adjustments that are not at rest, by name — `{"exposure": 0.3}`;
        /// absent when none has moved. Values are the model's `-1..=1`, not the
        /// panel's percentages.
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
    for page in doc.page_ids() {
        collect(doc, res, page, page, None, &mut nodes);
    }
    // Deterministic order by id.
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    let warnings = collect_warnings(&nodes);
    let pages = doc
        .pages()
        .iter()
        .map(|p| PageSnapshot {
            id: p.id.to_wire(),
            name: doc
                .get(p.id)
                .map_or_else(String::new, |n| n.name().to_string()),
            ground: hex(p.ground).0,
            draft: p.draft,
            mode: doc.get(p.id).and_then(|n| n.mode()).map(|m| m.0.to_wire()),
        })
        .collect();
    Snapshot {
        snapshot_version: SNAPSHOT_VERSION,
        pages,
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
    page: NodeId,
    id: NodeId,
    artboard: Option<NodeId>,
    out: &mut Vec<NodeSnapshot>,
) {
    let Some(node) = doc.get(id) else { return };
    out.push(node_snapshot(doc, node, res, page, artboard));

    // A node under an artboard reports that artboard as its membership; the
    // artboard itself is the membership root for its descendants.
    let child_artboard = match node.kind() {
        NodeKind::Artboard { .. } => Some(id),
        _ => artboard,
    };
    for child in node.children() {
        collect(doc, res, page, *child, child_artboard, out);
    }
}

fn node_snapshot(
    doc: &Document,
    node: &Node,
    res: &Resolved,
    page: NodeId,
    artboard: Option<NodeId>,
) -> NodeSnapshot {
    let id = node.id();
    NodeSnapshot {
        id: id.to_wire(),
        kind: kind_name(node.kind()).to_string(),
        name: node.name().to_string(),
        page: page.to_wire(),
        parent: node.parent().map(|p| p.to_wire()),
        children: node.children().iter().map(|c| c.to_wire()).collect(),
        artboard: artboard.map(|a| a.to_wire()),
        // The used local, not the specified one: `world_transform` below composes
        // used locals, so a specified local would stop multiplying out to it once
        // container layout places something. Rounded to the SVG writer's four places
        // because platforms' maths libraries differ in the last digit.
        local_transform: res.used_local_of(node).as_coeffs().map(round4),
        world_transform: res
            .world_transform(id)
            .unwrap_or(Affine::IDENTITY)
            .as_coeffs()
            .map(round4),
        world_bounds: res
            .world_bounds(id)
            .map(|b| [b.min_x(), b.min_y(), b.max_x(), b.max_y()].map(round4)),
        // As drawn: a bound visibility and opacity are resolved.
        visible: doc.shown(id),
        locked: node.locked(),
        proportions_locked: node.proportions_locked(),
        opacity: doc.drawn_opacity(id),
        clip: node.clip(),
        mask: node.mask(),
        mask_mode: node.mask_mode().label().to_string(),
        pivot: node.pivot().map(|p| {
            let local = ondin_core::local_box(doc, res, id).unwrap_or_default();
            let at = ondin_core::geometry::pivot_point(Some(p), local);
            [at.x, at.y]
        }),
        // Used, for `local_transform`'s reason: `world_bounds` is measured from it.
        geometry: geometry(res.used_kind_of(node)),
        fills: node
            .paint()
            .fills
            .iter()
            .filter(|f: &&ondin_core::Keyed<Fill>| f.visible)
            // The resolved value, not the binding: a reader sees what is drawn.
            .map(|f| paint_summary(&doc.resolve_brush(id, &f.brush)))
            .collect(),
        strokes: node
            .paint()
            .strokes
            .iter()
            .filter(|s: &&ondin_core::Keyed<Stroke>| s.visible)
            .map(|s| StrokeSummary {
                paint: paint_summary(&doc.resolve_brush(id, &s.brush)),
                width: match s.width_var {
                    Some(v) => doc.stroke_width_drawn_in(doc.variables(), id, v),
                    None => s.width,
                },
            })
            .collect(),
        effects: doc
            .resolved_effects(id, node.effects())
            .iter()
            .filter(|e| e.visible)
            .map(|e| effect_summary(e))
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

/// `v` at four decimal places, the SVG writer's precision (`svg::fmt`), kept as a
/// number. A non-finite value is `0` (JSON has no spelling for one) and `-0` is
/// `0`, so dust does not print as `-0.0`.
fn round4(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    let r = (v * 1e4).round() / 1e4;
    if r == 0.0 { 0.0 } else { r }
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
        // Not "boolean": a reader wants to know which operation.
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
        // A boolean reports no geometry, like a group: it comes from its children.
        // Its derived outline is `Resolved`'s, not the document's.
        NodeKind::Root | NodeKind::Group | NodeKind::Boolean { .. } => Geometry::None,
        NodeKind::Artboard { size } => Geometry::Artboard {
            width: size.width,
            height: size.height,
        },
        NodeKind::Rect {
            size,
            corner_radii,
            corner_smoothing,
        } => Geometry::Rect {
            width: size.width,
            height: size.height,
            corner_radii: *corner_radii,
            corner_smoothing: *corner_smoothing,
        },
        NodeKind::Ellipse { size } => Geometry::Ellipse {
            width: size.width,
            height: size.height,
        },
        NodeKind::Polygon {
            size,
            sides,
            corner_radius,
        } => Geometry::Polygon {
            width: size.width,
            height: size.height,
            sides: *sides,
            corner_radius: *corner_radius,
        },
        NodeKind::Star {
            size,
            points,
            inner_ratio,
            corner_radius,
        } => Geometry::Star {
            width: size.width,
            height: size.height,
            points: *points,
            inner_ratio: *inner_ratio,
            corner_radius: *corner_radius,
        },
        NodeKind::Line { end } => Geometry::Line {
            end_x: end.x,
            end_y: end.y,
        },
        // The resolved outline spelled by the SVG writer (`svg::path_d`, four
        // places), not `BezPath::to_svg`: full `f64` digits differ across platforms.
        NodeKind::Path { .. } => Geometry::Path {
            svg: crate::svg::path_d(&ondin_core::geometry::local_path(kind).unwrap_or_default()),
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
        // The brush's transform is deliberately absent, like the gradient's
        // geometry: a squash alone would be the only placement a reader could see.
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
