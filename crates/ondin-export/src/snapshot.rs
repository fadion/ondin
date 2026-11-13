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
/// - **13** the `"slice"` kind and `Geometry::Slice`.
/// - **14** `NodeSnapshot::blend`, absent at the kind's default, and
///   `EffectSummary::blend`, absent at Normal.
/// - **15** the `"spiral"` and `"arc"` kinds, `Geometry::Spiral` and
///   `Geometry::Arc`.
/// - **16** `PaintSummary::Image` gained `duotone`, the two colours a duotone
///   maps onto, present while its amount is above zero.
/// - **17** `NodeSnapshot::chart` on a chart frame and `NodeSnapshot::role` on
///   each of its parts.
/// - **18** `NodeSnapshot::display`, `item` and `insets`, the layout a node
///   gives its children and takes from its parent.
/// - **19** `NodeSnapshot::bindings`, `styles`, `modes`, `component` and
///   `fill_indices`; `StrokeSummary::id` and `index`, `EffectSummary::id`.
/// - **20** `PaintSummary::Image` gained `pixel_size`, the picture's own width
///   and height.
/// - **21** `fills` became `FillSummary` (`id`, `index`, `paint`) and
///   `fill_indices` went; `Geometry::Text` traded `align` and `styled_runs` for
///   `color`, `paragraph`, `block`, `runs` and `paragraphs`; `NodeSnapshot::specified`;
///   `ComponentSummary::properties` and `overrides`; a missing family in a run warns.
/// - **22** `Geometry::Path` gained `subpaths`, the anchors `set_point` edits.
pub const SNAPSHOT_VERSION: u32 = 22;

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
    /// How the node mixes with what is beneath it, as the menu names it
    /// (`"Multiply"`, `"Pass through"`), absent at its kind's default: Pass
    /// through on a frame or group, Normal elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<String>,
    /// Whether this node clips its children to its own frame. Only frames act
    /// on it; it is reported for every node because the model carries it there.
    pub clip: bool,
    /// Whether this node is a mask for the siblings above it — it draws nothing,
    /// and its outline clips them. Not `clip` above, which is about children.
    pub mask: bool,
    /// *How* it masks: `"Shape"`, `"Alpha"` or `"Luminance"`, reported for every
    /// node.
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
    /// The fills, visible entries only, bottom first.
    pub fills: Vec<FillSummary>,
    pub strokes: Vec<StrokeSummary>,
    /// The effect stack (§5.3a), visible entries only, in compositing order — the
    /// same filter `fills` and `strokes` apply. Absent when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectSummary>,
    /// The HTML element a code export writes the node as, by tag (`"nav"`,
    /// `"h2"`), absent on Automatic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    /// The address a node written as a link goes to, absent when it has none
    /// or is not a link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// A chart frame's description, absent on every other layer. Its parts are
    /// ordinary nodes beside it, which the chart's verbs edit and no other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<ChartSummary>,
    /// A chart part's role in its chart (`"Bar"`, `"TickLabel"`, `"Legend"`),
    /// absent outside a chart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// How the node lays out its children, flex or grid, in the shape
    /// `set_display` takes; absent when they are placed by their own
    /// transforms. Bound gaps, padding and flow are reported as drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<ondin_core::container::Display>,
    /// The node's properties as a layout item, in the shape `set_layout_item`
    /// takes; absent at the default. Bound limits are reported as drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<ondin_core::container::LayoutItem>,
    /// The node's insets and auto margins in its frame, in the shape
    /// `set_insets` takes; absent when it is placed by its transform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insets: Option<ondin_core::container::Insets>,
    /// Each field bound to a variable, named as `bind_variable` takes it; absent
    /// when none is. The values above are what the variables draw.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<BindingSummary>,
    /// The style bound in each slot, by the slot names `apply_style` takes;
    /// absent when the layer uses none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub styles: Option<ondin_core::style::StyleRefs>,
    /// The modes this frame sets, one per collection at most, by mode id; absent
    /// when it sets none and reads its parent's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,
    /// The layer's part in a component, absent outside one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentSummary>,
    /// The transform and geometry as authored, where layout draws the node
    /// otherwise; absent where it is drawn as authored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specified: Option<Specified>,
}

/// One bound field and the variable it reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingSummary {
    /// The field with its place (`{"fill": 0}`, `"opacity"`), the form
    /// `bind_variable`'s `field` takes.
    pub field: ondin_core::build::BoundField,
    pub variable: ondin_core::VarId,
}

/// A layer's part in a component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentSummary {
    /// `"main"`, `"set"` (a frame of variants), `"instance"` (an instance's
    /// root) or `"copy"` (a layer inside an instance, linked to its main's).
    pub role: String,
    /// An instance's main, the one it shows after any swap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    /// The layer an instance or copy is linked to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// A variant's value per property of its set, by property name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variant: BTreeMap<String, String>,
    /// An instance's component properties with the values it is set to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<PropertySummary>,
    /// The fields an instance or copy holds otherwise than the layer it is
    /// linked to, each of which `reset_overrides` puts back (`"fills"`,
    /// `"text"`, `"geometry.size"`, `"swap"`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<String>,
}

/// One of an instance's component properties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertySummary {
    pub name: String,
    /// `"boolean"`, `"text"` or `"swap"`.
    pub kind: String,
    /// The value as `set_instance_property` takes it: `true` or `false`, a
    /// text, or the id of the main a swap shows.
    pub value: serde_json::Value,
    /// Whether the instance sets it otherwise than its main.
    pub overridden: bool,
}

/// A chart's description as an agent reads it: what it draws, from which
/// columns, without the table's cells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartSummary {
    /// `"Column"`, `"Bar"`, `"Line"`, `"Area"`, `"Pie"`, `"Scatter"` or `"Sankey"`.
    pub kind: String,
    /// The kind's variant as the Chart card names it (`"Stacked"`, `"Donut"`),
    /// absent for a Sankey.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// The table's header, in order.
    pub columns: Vec<ColumnSummary>,
    pub rows: usize,
    /// Which columns it reads, by column id, in the form `set_chart` takes.
    pub mapping: ondin_core::chart::Mapping,
    /// One per series column, in the order drawn.
    pub series: Vec<SeriesSummary>,
    /// Whether the table is read from a linked file.
    pub linked: bool,
}

/// One column of a chart's table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnSummary {
    pub id: u32,
    pub name: String,
    /// Whether every non-empty cell is a number.
    pub numeric: bool,
}

/// One series of a chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesSummary {
    pub column: u32,
    pub name: String,
    pub visible: bool,
    /// Its own colour, absent while it takes the palette's next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<PaintSummary>,
}

/// One effect, flattened for a reader that is not this program. `kind` is a
/// string, like `mask_mode`. The numbers are the authored ones — a blur's radius,
/// not the gaussian deviation the writers take (`ondin_core::effect::BLUR_DEVIATION`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectSummary {
    /// The effect's item id, which `bind_variable` names an effect's number by.
    pub id: ondin_core::ItemId,
    /// `"DropShadow"`, `"InnerShadow"`, `"LayerBlur"`, `"BackgroundBlur"` or
    /// `"Filters"`.
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
    /// Shadows only: the blend mode as the menu names it, absent at Normal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<String>,
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

/// A node's kind-specific geometry, as drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[expect(
    clippy::large_enum_variant,
    reason = "built once per read and serialized at once; boxing Text would only move its size"
)]
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
        /// Start and sweep in degrees clockwise from three o'clock, and the hole
        /// as a fraction of the radius; absent for the whole ellipse.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arc: Option<ondin_core::EllipseArc>,
    },
    Polygon {
        width: f64,
        height: f64,
        sides: u32,
        /// One radius on every vertex; absent when square-cornered.
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_radius: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_smoothing: f64,
    },
    Star {
        width: f64,
        height: f64,
        points: u32,
        /// Inner radius as a fraction of the outer one, 0..=1.
        inner_ratio: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_radius: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_smoothing: f64,
    },
    Line {
        end_x: f64,
        end_y: f64,
    },
    /// An open spiral filling its box: `turns` turns clockwise out from the middle
    /// to three o'clock.
    Spiral {
        width: f64,
        height: f64,
        turns: f64,
    },
    /// An open circular arc from the local origin to the end, `sweep` degrees,
    /// positive clockwise on screen.
    Arc {
        end_x: f64,
        end_y: f64,
        sweep: f64,
    },
    Path {
        /// The outline as drawn, its corners rounded.
        svg: String,
        /// The anchors `set_point` edits, in the layer's own space. A rounded
        /// corner is one point here and several segments in `svg`.
        #[serde(default)]
        subpaths: Vec<PathRun>,
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
        /// The glyphs' own ink as `#rrggbbaa`; absent when they take the layer's
        /// fills.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        /// `auto` when the box grows both ways, else the width it wraps at.
        sizing: TextSizingSummary,
        /// Every paragraph's defaults, as `set_paragraph_style` takes them: align,
        /// spacing, indents, wrapping, direction, list marker and level, each
        /// absent at its default.
        #[serde(default, skip_serializing_if = "is_default")]
        paragraph: ondin_core::ParagraphStyle,
        /// The box's vertical alignment, trim, overflow and line limit, as
        /// `set_block_style` takes them; absent at the defaults.
        #[serde(default, skip_serializing_if = "is_default")]
        block: ondin_core::BlockStyle,
        /// The character runs over the defaults, as `style_text` takes them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        runs: Vec<TextRun<ondin_core::CharAttr>>,
        /// The paragraphs that override the defaults, a list item's marker and
        /// level among them, as `style_paragraphs` takes them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paragraphs: Vec<TextRun<ondin_core::ParaAttr>>,
    },
    /// A frame. It has no background here: its ground is an ordinary entry in
    /// `NodeSnapshot::fills`.
    Artboard {
        width: f64,
        height: f64,
        /// Per corner; absent when square-cornered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        corner_radii: Option<RoundedRectRadii>,
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_smoothing: f64,
    },
    /// An export region's box; it draws nothing.
    Slice {
        width: f64,
        height: f64,
    },
}

/// One subpath of a path's anchors, as `set_point` numbers them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathRun {
    /// Whether the last point joins back to the first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
    pub points: Vec<PathPoint>,
}

/// One anchor of a path, with the ends of its handles: `in` arrives at it, `out`
/// leaves it. A handle lying on the point is left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathPoint {
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_y: Option<f64>,
    /// How far the corner at the point is rounded; absent when sharp.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub radius: f64,
}

/// The anchors of a path's `path` and `radii`, as [`PathRun`]s.
fn path_runs(path: &ondin_core::kurbo::BezPath, radii: &[f64]) -> Vec<PathRun> {
    let handle = |v: ondin_core::kurbo::Vec2, end: ondin_core::kurbo::Point| {
        (v.hypot() >= 1e-9).then(|| (round4(end.x), round4(end.y)))
    };
    ondin_core::edit::pen_anchors(path, radii)
        .into_iter()
        .map(|sub| PathRun {
            closed: sub.closed,
            points: sub
                .anchors
                .iter()
                .map(|a| {
                    let back = handle(a.back, a.arriving());
                    let out = handle(a.out, a.leaving());
                    PathPoint {
                        x: round4(a.at.x),
                        y: round4(a.at.y),
                        in_x: back.map(|p| p.0),
                        in_y: back.map(|p| p.1),
                        out_x: out.map(|p| p.0),
                        out_y: out.map(|p| p.1),
                        radius: round4(a.radius),
                    }
                })
                .collect(),
        })
        .collect()
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Attributes over a range of a text counted in characters, `end` exclusive: the
/// arguments `style_text` and `style_paragraphs` take to write them back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRun<A> {
    pub start: usize,
    pub end: usize,
    pub attrs: Vec<A>,
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
        /// The picture's own width and height in pixels, which set its aspect
        /// under `fit` and `crop`; absent when the table has no such picture.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pixel_size: Option<[u32; 2]>,
        /// `fill` | `fit` | `crop` | `tile`.
        fit: String,
        /// The sampler's alpha, which is what an image fill's opacity is (§5.5a).
        alpha: f32,
        /// The adjustments that are not at rest, by name — `{"exposure": 0.3}`;
        /// absent when none has moved. Values are the model's `-1..=1`, not the
        /// panel's percentages.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        adjust: BTreeMap<String, f32>,
        /// Duotone's dark and light ends as `#rrggbb`, while `adjust` holds a
        /// `duotone` above zero; absent otherwise, since they then draw nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duotone: Option<[String; 2]>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradientStopSummary {
    pub offset: f32,
    /// `#rrggbb`.
    pub color: String,
    pub alpha: f32,
}

/// One visible fill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FillSummary {
    /// The fill's item id, which follows it through reorders and ties an
    /// instance's fill to its main's.
    pub id: ondin_core::ItemId,
    /// Its index in the layer's whole list, which `bind_variable` names a fill's
    /// colour by; hidden fills are counted and not listed.
    pub index: usize,
    pub paint: PaintSummary,
}

/// A node's authored values where layout draws it otherwise: CSS's specified
/// beside the used values the rest of the entry reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Specified {
    /// The transform as stored, where layout placed the node elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_transform: Option<[f64; 6]>,
    /// The geometry as stored, where layout drew it at another size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrokeSummary {
    /// The stroke's item id, which `bind_variable` names a stroke's width by.
    pub id: ondin_core::ItemId,
    /// Its index in the layer's whole list, which `bind_variable` names a
    /// stroke's colour by; hidden strokes are counted and not listed.
    pub index: usize,
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
            ground: hex(doc.ground(p.id)).0,
            draft: p.draft,
            mode: doc
                .get(p.id)
                .and_then(|n| n.modes().first().copied())
                .map(|m| m.0.to_wire()),
        })
        .collect();
    Snapshot {
        snapshot_version: SNAPSHOT_VERSION,
        pages,
        nodes,
        warnings,
    }
}

/// One warning per text node and family it sets, its default's or a run's, that
/// is not registered. Derived from the already-built node list so the order is
/// deterministic too.
fn collect_warnings(nodes: &[NodeSnapshot]) -> Vec<Warning> {
    let mut out = Vec::new();
    for n in nodes {
        let Geometry::Text {
            font_family, runs, ..
        } = &n.geometry
        else {
            continue;
        };
        let run_families = runs.iter().flat_map(|r| &r.attrs).filter_map(|a| match a {
            ondin_core::CharAttr::Family(f) => Some(f),
            _ => None,
        });
        let mut seen: Vec<&String> = Vec::new();
        for family in std::iter::once(font_family).chain(run_families) {
            if seen.contains(&family) || ondin_core::text::is_family_available(family) {
                continue;
            }
            seen.push(family);
            out.push(Warning::MissingFont {
                node: n.id.clone(),
                family: family.clone(),
            });
        }
    }
    out
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
        blend: Some(node.blend())
            .filter(|b| *b != ondin_core::BlendMode::default_for(node.kind()))
            .map(|b| b.label().to_string()),
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
            .enumerate()
            .filter(|(_, f): &(usize, &ondin_core::Keyed<Fill>)| f.visible)
            // The resolved value, not the binding: a reader sees what is drawn.
            .map(|(index, f)| FillSummary {
                id: f.id,
                index,
                paint: paint_summary(doc, &doc.resolve_brush(id, &f.brush)),
            })
            .collect(),
        strokes: node
            .paint()
            .strokes
            .iter()
            .enumerate()
            .filter(|(_, s): &(usize, &ondin_core::Keyed<Stroke>)| s.visible)
            .map(|(index, s)| StrokeSummary {
                id: s.id,
                index,
                paint: paint_summary(doc, &doc.resolve_brush(id, &s.brush)),
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
            .map(|e| effect_summary(e.id, e))
            .collect(),
        element: node.element().map(|e| e.tag().to_string()),
        href: (node.element() == Some(ondin_core::element::HtmlElement::Link)
            && !node.href().is_empty())
        .then(|| node.href().to_string()),
        chart: node.chart().map(|c| chart_summary(doc, id, c)),
        role: ondin_core::chart::owner(doc, id)
            .and_then(|c| doc.get(c)?.chart()?.parts.iter().find(|(_, p)| *p == id))
            .and_then(|(k, _)| serde_json::to_value(k.role).ok())
            .and_then(|v| v.as_str().map(str::to_string)),
        display: doc.resolved_display(id).map(std::borrow::Cow::into_owned),
        item: Some(drawn_item(doc, node)).filter(|i| *i != Default::default()),
        insets: Some(*node.insets()).filter(|i| !i.is_unset()),
        bindings: bindings(node),
        styles: Some(node.styles()).filter(|s| *s != Default::default()),
        modes: node.modes().iter().map(|m| m.0.to_wire()).collect(),
        component: component_summary(doc, node),
        specified: specified(doc, res, node),
    }
}

/// What `node`'s authored transform and geometry say where layout drew it
/// otherwise, or `None` where it is drawn as authored. A bound field is read as
/// drawn, since its stored literal is a placeholder.
fn specified(doc: &Document, res: &Resolved, node: &Node) -> Option<Specified> {
    let id = node.id();
    let local = node.transform().as_coeffs().map(round4);
    let local_transform =
        (local != res.used_local_of(node).as_coeffs().map(round4)).then_some(local);
    let mut kind = doc
        .resolved_kind(id)
        .map_or_else(|| node.kind().clone(), std::borrow::Cow::into_owned);
    if let Some(radii) = kind.corner_radii().and_then(|r| doc.resolve_corners(id, r)) {
        kind.set_corner_radii(radii);
    }
    let used = res.used_kind_of(node);
    let authored = (kind != *used)
        .then(|| geometry(&kind))
        .filter(|g| *g != geometry(used));
    (local_transform.is_some() || authored.is_some()).then_some(Specified {
        local_transform,
        geometry: authored,
    })
}

/// Every field of `node` bound to a variable, in `bind_variable`'s terms.
fn bindings(node: &Node) -> Vec<BindingSummary> {
    use ondin_core::build::BoundField as F;
    use ondin_core::variable::{TEXT_CONTENT, TEXT_FAMILY};
    let mut out = Vec::new();
    let mut push = |field, var: Option<ondin_core::VarId>| {
        if let Some(variable) = var {
            out.push(BindingSummary { field, variable });
        }
    };
    let paint = node.paint();
    for (i, f) in paint.fills.iter().enumerate() {
        push(F::Fill(i), f.brush.var());
    }
    for (i, s) in paint.strokes.iter().enumerate() {
        push(F::Stroke(i), s.brush.var());
        push(F::StrokeWidth(s.id), s.width_var);
    }
    for (k, v) in node.corner_vars().into_iter().enumerate() {
        push(F::Corner(k), v);
    }
    for (k, v) in node.spacing_vars().into_iter().enumerate() {
        push(F::Spacing(1 << k), v);
    }
    for (k, v) in node.flow_vars().into_iter().enumerate() {
        push(F::Flow(k), v);
    }
    push(F::Visible, node.visible_var());
    push(F::Opacity, node.opacity_var());
    for (k, v) in node.size_vars().into_iter().enumerate() {
        push(F::Size(k), v);
    }
    for (k, v) in node.text_vars().into_iter().enumerate() {
        push(
            match k {
                TEXT_CONTENT => F::Content,
                TEXT_FAMILY => F::Family,
                _ => F::Type(k),
            },
            v,
        );
    }
    for (k, v) in node.item().limit_vars.into_iter().enumerate() {
        push(F::Limit(k), v);
    }
    for e in node.effects() {
        for (k, v) in e.vars.into_iter().enumerate() {
            push(F::Effect(e.id, k), v);
        }
    }
    out
}

/// `node`'s part in a component, or `None` outside one. A layer added inside an
/// instance is linked to nothing, and is none.
fn component_summary(doc: &Document, node: &Node) -> Option<ComponentSummary> {
    let id = node.id();
    let instance =
        node.link().is_some() && ondin_core::component::instance_root(doc, id) == Some(id);
    let role = match () {
        _ if node.set().is_some() => "set",
        _ if node.component() => "main",
        _ if instance => "instance",
        _ if node.link().is_some() => "copy",
        _ => return None,
    };
    let set = node.parent().and_then(|p| doc.get(p)?.set());
    Some(ComponentSummary {
        role: role.to_string(),
        main: instance
            .then(|| ondin_core::component::main_of(doc, id))
            .flatten()
            .map(|m| m.to_wire()),
        source: node.link().map(|l| l.to_wire()),
        variant: match set {
            Some(set) if node.component() => set
                .props
                .iter()
                .zip(node.variant())
                .map(|(p, v)| (p.name.clone(), v.clone()))
                .collect(),
            _ => BTreeMap::new(),
        },
        properties: match instance {
            true => properties(doc, id),
            false => Vec::new(),
        },
        overrides: ondin_core::reset::overrides(doc, id)
            .iter()
            .filter_map(|o| override_field(&o.reset))
            .collect(),
    })
}

/// The component properties the instance rooted at `root` offers, with its values.
fn properties(doc: &Document, root: NodeId) -> Vec<PropertySummary> {
    use ondin_core::variant::{PropKind, PropValue};
    ondin_core::variant::instance_properties(doc, root)
        .iter()
        .filter_map(|p| {
            let kind = match p.kind {
                PropKind::Boolean => "boolean",
                PropKind::Text => "text",
                PropKind::Swap => "swap",
                PropKind::Nested => return None,
            };
            let (value, overridden) = ondin_core::variant::property_state(doc, root, p)?;
            Some(PropertySummary {
                name: p.name.clone(),
                kind: kind.to_string(),
                value: match value {
                    PropValue::Boolean(b) => serde_json::Value::Bool(b),
                    PropValue::Text(t) => serde_json::Value::String(t),
                    PropValue::Swap(main) => serde_json::Value::String(main.to_wire()),
                },
                overridden,
            })
        })
        .collect()
}

/// The field a reset operation writes, in snake case: `SetFills` is `"fills"`,
/// a geometry patch `"geometry."` and its own (`"geometry.size"`).
fn override_field(op: &ondin_core::Operation) -> Option<String> {
    fn snake(name: &str) -> String {
        let mut out = String::new();
        for (i, c) in name.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i > 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }
    let v = serde_json::to_value(op).ok()?;
    let (variant, body) = v.as_object()?.iter().next()?;
    let field = snake(variant.strip_prefix("Set").unwrap_or(variant));
    let patch = body
        .get("geometry")
        .and_then(|g| match g {
            serde_json::Value::Object(m) => m.keys().next().cloned(),
            serde_json::Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .map(|p| snake(&p));
    Some(match patch {
        Some(p) => format!("{field}.{p}"),
        None => field,
    })
}

/// `node`'s layout item as authored, its bound limits read as drawn. Not
/// `Document::resolved_item` whole: a frame with width queries is laid with its
/// width at its padding, which an agent writing the item back would then keep.
fn drawn_item(doc: &Document, node: &Node) -> ondin_core::container::LayoutItem {
    let mut item = *node.item();
    if item.limit_vars.iter().any(Option::is_some) {
        let r = doc.resolved_item(node.id());
        item.min_width = r.min_width;
        item.min_height = r.min_height;
        item.max_width = r.max_width;
        item.max_height = r.max_height;
    }
    item
}

fn chart_summary(doc: &Document, id: NodeId, c: &ondin_core::chart::Chart) -> ChartSummary {
    let t = &c.table;
    ChartSummary {
        kind: c.kind.label().to_string(),
        variant: c.variant().map(str::to_string),
        columns: t
            .columns
            .iter()
            .map(|col| ColumnSummary {
                id: col.id.0,
                name: col.name.clone(),
                numeric: t.is_numeric(col.id),
            })
            .collect(),
        rows: t.rows.len(),
        mapping: c.mapping.clone(),
        series: c
            .mapping
            .series
            .iter()
            .map(|col| {
                let style = c.series.iter().find(|s| s.column == *col);
                SeriesSummary {
                    column: col.0,
                    name: t.column(*col).map_or_else(String::new, |x| x.name.clone()),
                    visible: style.is_none_or(|s| s.visible),
                    fill: style.map(|s| paint_summary(doc, &doc.resolve_brush(id, &s.fill))),
                }
            })
            .collect(),
        linked: c.source.is_some(),
    }
}

fn effect_summary(id: ondin_core::ItemId, e: &ondin_core::Effect) -> EffectSummary {
    use ondin_core::EffectKind;
    let base = |kind: &str| EffectSummary {
        id,
        kind: kind.to_string(),
        offset: None,
        blur: None,
        spread: None,
        color: None,
        opacity: None,
        blend: None,
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
                blend: s.blends().then(|| s.blend.label().to_string()),
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
        EffectKind::BackgroundBlur { radius } => EffectSummary {
            blur: Some(*radius),
            ..base("BackgroundBlur")
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
        NodeKind::Spiral { .. } => "spiral",
        NodeKind::Arc { .. } => "arc",
        NodeKind::Path { .. } => "path",
        NodeKind::Text { .. } => "text",
        NodeKind::Slice { .. } => "slice",
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
        NodeKind::Artboard {
            size,
            corner_radii,
            corner_smoothing,
        } => Geometry::Artboard {
            width: size.width,
            height: size.height,
            corner_radii: (!ondin_core::geometry::is_square_cornered(corner_radii))
                .then_some(*corner_radii),
            corner_smoothing: *corner_smoothing,
        },
        NodeKind::Slice { size } => Geometry::Slice {
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
        NodeKind::Ellipse { size, arc } => Geometry::Ellipse {
            width: size.width,
            height: size.height,
            arc: (!arc.is_default()).then_some(*arc),
        },
        NodeKind::Polygon {
            size,
            sides,
            corner_radius,
            corner_smoothing,
        } => Geometry::Polygon {
            width: size.width,
            height: size.height,
            sides: *sides,
            corner_radius: *corner_radius,
            corner_smoothing: *corner_smoothing,
        },
        NodeKind::Star {
            size,
            points,
            inner_ratio,
            corner_radius,
            corner_smoothing,
        } => Geometry::Star {
            width: size.width,
            height: size.height,
            points: *points,
            inner_ratio: *inner_ratio,
            corner_radius: *corner_radius,
            corner_smoothing: *corner_smoothing,
        },
        NodeKind::Line { end } => Geometry::Line {
            end_x: end.x,
            end_y: end.y,
        },
        NodeKind::Spiral { size, turns } => Geometry::Spiral {
            width: size.width,
            height: size.height,
            turns: *turns,
        },
        NodeKind::Arc { end, sweep } => Geometry::Arc {
            end_x: end.x,
            end_y: end.y,
            sweep: *sweep,
        },
        // The resolved outline spelled by the SVG writer (`svg::path_d`, four
        // places), not `BezPath::to_svg`: full `f64` digits differ across platforms.
        NodeKind::Path {
            path, corner_radii, ..
        } => Geometry::Path {
            svg: crate::svg::path_d(&ondin_core::geometry::local_path(kind).unwrap_or_default()),
            subpaths: path_runs(path, corner_radii),
        },
        NodeKind::Text {
            content,
            style,
            spans,
            para_spans,
            paragraph,
            block,
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
            color: style.color.map(hex8),
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
            paragraph: paragraph.clone(),
            block: *block,
            runs: text_runs(content, spans.as_slice()),
            paragraphs: text_runs(content, para_spans.as_slice()),
        },
    }
}

/// `spans`, counted in bytes of `content`, as runs counted in characters, the
/// spans over one range gathered into one run, in order of where they start.
fn text_runs<A: Clone>(content: &str, spans: &[ondin_core::Span<A>]) -> Vec<TextRun<A>> {
    // Counted, not sliced: a bound text draws other content than its spans index.
    let char_at = |byte: usize| {
        content
            .char_indices()
            .take_while(|(b, _)| *b < byte)
            .count()
    };
    let mut runs: Vec<TextRun<A>> = Vec::new();
    for s in spans {
        let (start, end) = (char_at(s.start), char_at(s.end));
        match runs.iter_mut().find(|r| r.start == start && r.end == end) {
            Some(r) => r.attrs.push(s.attr.clone()),
            None => runs.push(TextRun {
                start,
                end,
                attrs: vec![s.attr.clone()],
            }),
        }
    }
    runs.sort_by_key(|r| (r.start, r.end));
    runs
}

/// `c` as `#rrggbbaa`, the spelling `review_colours` and `merge_colours` use.
fn hex8(c: ondin_core::peniko::Color) -> String {
    let rgba = c.to_rgba8();
    format!("#{:02x}{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b, rgba.a)
}

fn hex(c: ondin_core::peniko::Color) -> (String, f32) {
    let rgba = c.to_rgba8();
    (
        format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b),
        rgba.a as f32 / 255.0,
    )
}

fn paint_summary(doc: &Document, brush: &Brush) -> PaintSummary {
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
            pixel_size: doc.image(&img.image.id).map(|e| [e.width, e.height]),
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
            duotone: (img.image.adjust.duotone != 0.0).then(|| {
                let a = &img.image.adjust;
                [hex(a.duotone_dark).0, hex(a.duotone_light).0]
            }),
        },
    }
}
