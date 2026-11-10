//! Charts (`docs/design/charts.md`): a frame carrying a [`Chart`] description,
//! whose children are ordinary shapes and text generated from it.
//!
//! This module holds the description and the rules that keep its parts derived:
//! [`guard`] refuses an operation that writes a part outside the chart's own
//! regeneration, and [`check`] refuses a chart where one may not sit. A
//! regeneration is a transaction carrying [`crate::Operation::SetChartParts`] for
//! its chart; nothing else writes under a chart frame.

use crate::id::NodeId;
use crate::node::{Node, NodeKind};
use crate::op::Operation;
use crate::variable::BrushSource;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

/// The most rows a table holds.
pub const MAX_ROWS: usize = 10_000;
/// The most columns a table holds.
pub const MAX_COLUMNS: usize = 64;
/// The most characters in one cell.
pub const MAX_CELL_CHARS: usize = 256;
/// The most bars, points or slices a chart draws; past it, it draws nothing new.
pub const MAX_MARKS: usize = 2_000;
/// The most slices a pie draws.
pub const MAX_SLICES: usize = 100;
/// The most nodes a Sankey draws.
pub const MAX_SANKEY_NODES: usize = 200;
/// The most flows a Sankey draws.
pub const MAX_FLOWS: usize = 1_000;
/// The longest a chart's text option (an axis title, a family, a prefix) may be.
pub const MAX_TEXT_CHARS: usize = 256;

/// A chart's description: what it draws and how. `Node::chart` on a frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chart {
    pub kind: ChartKind,
    pub table: Table,
    #[serde(default)]
    pub mapping: Mapping,
    /// One per series column, keyed by its column, so a reorder or a rename keeps
    /// its colour.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub series: Vec<SeriesStyle>,
    /// A Sankey's node colours, keyed by node name, so a re-import keeps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeStyle>,
    /// Every kind's options at once, so switching kind and back keeps them.
    #[serde(default)]
    pub options: Options,
    /// The file the table was read from, while linked. Never carried by a copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DataLink>,
    /// The generated nodes by role, written only by a regeneration. Ids in the
    /// file's own spelling (`NodeId::to_wire`).
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "wire_parts")]
    pub parts: Vec<(PartKey, NodeId)>,
}

/// `serde(with)` for [`Chart::parts`]: each id as the save format spells a node
/// id, so a part reads like every other reference in the file.
mod wire_parts {
    use super::{NodeId, PartKey};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(parts: &[(PartKey, NodeId)], s: S) -> Result<S::Ok, S::Error> {
        let wire: Vec<(PartKey, String)> = parts.iter().map(|(k, id)| (*k, id.to_wire())).collect();
        wire.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Vec<(PartKey, NodeId)>, D::Error> {
        let wire = Vec::<(PartKey, String)>::deserialize(d)?;
        wire.into_iter()
            .map(|(k, id)| {
                NodeId::from_wire(&id)
                    .map(|id| (k, id))
                    .ok_or_else(|| serde::de::Error::custom(format!("malformed node id {id:?}")))
            })
            .collect()
    }
}

/// The seven kinds a chart can be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChartKind {
    #[default]
    Column,
    Bar,
    Line,
    Area,
    Pie,
    Scatter,
    Sankey,
}

impl ChartKind {
    pub const ALL: [ChartKind; 7] = [
        ChartKind::Column,
        ChartKind::Bar,
        ChartKind::Line,
        ChartKind::Area,
        ChartKind::Pie,
        ChartKind::Scatter,
        ChartKind::Sankey,
    ];

    /// The kind's name as the UI says it.
    pub fn label(self) -> &'static str {
        match self {
            ChartKind::Column => "Column",
            ChartKind::Bar => "Bar",
            ChartKind::Line => "Line",
            ChartKind::Area => "Area",
            ChartKind::Pie => "Pie",
            ChartKind::Scatter => "Scatter",
            ChartKind::Sankey => "Sankey",
        }
    }

    /// Whether the kind has a value and a category axis.
    pub fn has_axes(self) -> bool {
        !matches!(self, ChartKind::Pie | ChartKind::Sankey)
    }
}

/// The data: named columns and rows of text cells. A column is a number column
/// while every non-empty cell in it parses as one ([`parse_number`]); the type
/// is read, never set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<Column>,
    /// Each row holds one cell per column, in column order.
    #[serde(default)]
    pub rows: Vec<Vec<String>>,
}

/// A column: its id, which the mapping and the series styles name it by, and
/// its header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub id: ColumnId,
    pub name: String,
}

/// A column's identity within its table, kept across renames and reorders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColumnId(pub u32);

impl Table {
    /// The column `id`'s place, `None` when the table has none.
    pub fn index_of(&self, id: ColumnId) -> Option<usize> {
        self.columns.iter().position(|c| c.id == id)
    }

    /// The column `id`, `None` when the table has none.
    pub fn column(&self, id: ColumnId) -> Option<&Column> {
        self.columns.iter().find(|c| c.id == id)
    }

    /// The cells of column `id`, top to bottom; empty when the table has none.
    pub fn cells(&self, id: ColumnId) -> impl Iterator<Item = &str> + '_ {
        let at = self.index_of(id);
        self.rows
            .iter()
            .filter_map(move |r| at.and_then(|i| r.get(i)).map(String::as_str))
    }

    /// Whether column `id` is a number column: at least one cell, and every
    /// non-empty cell a number.
    pub fn is_numeric(&self, id: ColumnId) -> bool {
        let mut any = false;
        for c in self.cells(id) {
            if c.trim().is_empty() {
                continue;
            }
            if parse_number(c).is_none() {
                return false;
            }
            any = true;
        }
        any
    }

    /// An id no column of this table has.
    pub fn next_column_id(&self) -> ColumnId {
        ColumnId(self.columns.iter().map(|c| c.id.0 + 1).max().unwrap_or(0))
    }
}

/// A cell read as a number: an optional sign, digits with an optional `,`
/// thousands separator in groups of three, an optional `.` fraction, and space
/// around. A decimal comma does not parse, and neither does anything that is not
/// finite. `None` for an empty cell.
pub fn parse_number(cell: &str) -> Option<f64> {
    let s = cell.trim();
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    if body.is_empty() {
        return None;
    }
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    if frac.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    if int.is_empty() && frac.is_none() {
        return None;
    }
    if !int.is_empty() {
        let groups: Vec<&str> = int.split(',').collect();
        let digits = |g: &str| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit());
        let ok = match groups.as_slice() {
            [one] => digits(one),
            [first, rest @ ..] => {
                digits(first) && first.len() <= 3 && rest.iter().all(|g| digits(g) && g.len() == 3)
            }
            [] => false,
        };
        if !ok {
            return None;
        }
    }
    let plain: String = s.chars().filter(|c| *c != ',').collect();
    plain.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Which columns the chart reads, by role.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mapping {
    /// The category column; a scatter's X.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<ColumnId>,
    /// The series columns, in the order drawn; a scatter's Y columns.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub series: Vec<ColumnId>,
    /// A bubble chart's size column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<ColumnId>,
    /// A scatter's group column, which splits its points into series.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<ColumnId>,
    /// A Sankey's From, To and Value columns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<ColumnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<ColumnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<ColumnId>,
}

impl Mapping {
    /// Every column the mapping names.
    pub fn columns(&self) -> impl Iterator<Item = ColumnId> + '_ {
        self.category
            .into_iter()
            .chain(self.series.iter().copied())
            .chain(self.size)
            .chain(self.group)
            .chain(self.from)
            .chain(self.to)
            .chain(self.value)
    }
}

/// How one series is drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesStyle {
    pub column: ColumnId,
    pub fill: BrushSource,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub dash: Dash,
    #[serde(default, skip_serializing_if = "is_default")]
    pub point: PointShape,
}

/// A Sankey node's colour, by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeStyle {
    pub name: String,
    pub fill: BrushSource,
}

/// A line's dash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dash {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// A point's shape: a scatter's marks and a line's points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointShape {
    #[default]
    Circle,
    Square,
    Diamond,
    Triangle,
}

/// The linked file a chart's table was read from (`charts.md`, *Import and the
/// linked file*): its path and the stamp it had when last read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataLink {
    pub path: String,
    #[serde(default)]
    pub size: u64,
    /// Modified time in whole seconds since the Unix epoch.
    #[serde(default)]
    pub modified: u64,
}

/// A generated part's role in its chart, which regeneration keeps its id by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PartKey {
    pub role: PartRole,
    /// The series (its column id) or the axis the part belongs to; 0 when none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub series: u32,
    /// The part's place among its role's: a row, a tick, a node; 0 when one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub index: u32,
}

/// What a generated part is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PartRole {
    Legend,
    LegendItem,
    LegendSwatch,
    LegendLabel,
    Body,
    ValueAxis,
    CategoryAxis,
    Plot,
    Ground,
    Grid,
    Gridline,
    AxisLine,
    Tick,
    TickLabel,
    CategoryLabel,
    AxisTitle,
    Series,
    Bar,
    Line,
    Area,
    Point,
    Slice,
    PieFrame,
    Values,
    ValueLabel,
    LeaderLine,
    Flows,
    Flow,
    Nodes,
    SankeyNode,
    NodeLabel,
}

/// Every option of every kind (`charts.md`, *Options*).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub palette: Palette,
    pub bars: Bars,
    pub lines: Lines,
    pub slices: Slices,
    pub points: Points,
    pub flows: Flows,
    pub value_axis: Axis,
    pub category_axis: Axis,
    pub values: Values,
    pub legend: Legend,
    pub text: ChartText,
    /// Space between the frame's edge and the chart, and between the legend and
    /// the body.
    pub padding: f64,
    pub gap: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            palette: Palette::default(),
            bars: Bars::default(),
            lines: Lines::default(),
            slices: Slices::default(),
            points: Points::default(),
            flows: Flows::default(),
            value_axis: Axis::default(),
            category_axis: Axis {
                grid: Gridlines {
                    show: false,
                    ..Gridlines::default()
                },
                ..Axis::default()
            },
            values: Values::default(),
            legend: Legend::default(),
            text: ChartText::default(),
            padding: 16.0,
            gap: 12.0,
        }
    }
}

/// The colours new series take, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Palette {
    #[default]
    Ondin,
    Cool,
    Warm,
    Mono,
}

/// Whether series stack: a column or bar chart's Grouped, Stacked and 100%, an
/// area chart's Overlapping, Stacked and 100%.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stacking {
    #[default]
    Apart,
    Stacked,
    Percent,
}

/// Column and bar options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bars {
    pub stacking: Stacking,
    /// The gap between groups, as a percentage of a band.
    pub group_gap: f64,
    /// The gap between a group's bars, as a percentage of a bar.
    pub bar_gap: f64,
    pub radius: f64,
    /// Whether the radius rounds every corner, not only the end away from the baseline.
    pub radius_all: bool,
    /// A stroke width; 0 for none.
    pub stroke: f64,
}

impl Default for Bars {
    fn default() -> Self {
        Self {
            stacking: Stacking::Apart,
            group_gap: 30.0,
            bar_gap: 10.0,
            radius: 2.0,
            radius_all: false,
            stroke: 0.0,
        }
    }
}

/// A line's curve between its points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Curve {
    #[default]
    Straight,
    /// A monotone cubic (Fritsch–Carlson), which never overshoots a peak.
    Smooth,
    Step,
}

/// How a line's points are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointStyle {
    #[default]
    None,
    Hollow,
    Filled,
}

/// Line and area options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lines {
    pub width: f64,
    pub curve: Curve,
    pub points: PointStyle,
    pub point_size: f64,
    /// An area's fill opacity, 0 to 1.
    pub area_opacity: f64,
    pub connect_gaps: bool,
    /// An area chart's Overlapping, Stacked and 100%.
    pub area_stacking: Stacking,
}

impl Default for Lines {
    fn default() -> Self {
        Self {
            width: 2.0,
            curve: Curve::Straight,
            points: PointStyle::None,
            point_size: 6.0,
            area_opacity: 0.3,
            connect_gaps: false,
            area_stacking: Stacking::Apart,
        }
    }
}

/// Pie and donut options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slices {
    /// The Donut variant; its hole is [`Self::hole`].
    pub donut: bool,
    /// The hole, as a percentage of the radius.
    pub hole: f64,
    /// Degrees between slices.
    pub pad_angle: f64,
    pub radius: f64,
    /// Degrees clockwise from twelve o'clock.
    pub start_angle: f64,
    pub sort: bool,
    /// Slices under this percentage fold into *Other*; `None` keeps every slice.
    pub group_below: Option<f64>,
}

impl Default for Slices {
    fn default() -> Self {
        Self {
            donut: false,
            hole: 60.0,
            pad_angle: 0.0,
            radius: 0.0,
            start_angle: 0.0,
            sort: false,
            group_below: None,
        }
    }
}

/// Scatter and bubble options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Points {
    pub size: f64,
    pub shape: PointShape,
    /// 0 to 1.
    pub opacity: f64,
    /// The largest bubble's size.
    pub max_size: f64,
}

impl Default for Points {
    fn default() -> Self {
        Self {
            size: 8.0,
            shape: PointShape::Circle,
            opacity: 0.8,
            max_size: 40.0,
        }
    }
}

/// Where a Sankey moves nodes after depth places them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowAlign {
    #[default]
    Justify,
    Left,
    Right,
    Centre,
}

/// How a Sankey's flow is coloured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowColour {
    #[default]
    Source,
    Target,
    Gradient,
    Neutral,
}

/// What a Sankey node's label says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeLabels {
    #[default]
    Name,
    NameAndValue,
    Off,
}

/// Sankey options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Flows {
    pub node_width: f64,
    pub node_gap: f64,
    pub node_radius: f64,
    pub align: FlowAlign,
    /// Whether the relaxation passes run; off keeps table order.
    pub relax: bool,
    pub colour: FlowColour,
    /// 0 to 1.
    pub opacity: f64,
    pub labels: NodeLabels,
}

impl Default for Flows {
    fn default() -> Self {
        Self {
            node_width: 12.0,
            node_gap: 10.0,
            node_radius: 2.0,
            align: FlowAlign::Justify,
            relax: true,
            colour: FlowColour::Source,
            opacity: 0.4,
            labels: NodeLabels::Name,
        }
    }
}

/// A tick label's angle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelAngle {
    /// Turned 45° and then thinned to every *n*th when they would touch.
    #[default]
    Auto,
    Flat,
    Slanted,
    Upright,
}

/// Which side of the plot an axis sits on: left or bottom at `Start`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxisSide {
    #[default]
    Start,
    End,
}

/// One axis's options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Axis {
    pub show: bool,
    pub line: bool,
    pub ticks: bool,
    pub labels: bool,
    pub angle: LabelAngle,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// A typed min and max; `None` is Auto.
    pub range: Option<(f64, f64)>,
    /// Always on for columns and bars, whose length is the value.
    pub start_at_zero: bool,
    /// `None` aims for five ticks on 1-2-5 steps.
    pub tick_count: Option<u32>,
    pub side: AxisSide,
    pub format: NumberFormat,
    pub grid: Gridlines,
    pub colour: BrushSource,
}

impl Default for Axis {
    fn default() -> Self {
        Self {
            show: true,
            line: true,
            ticks: false,
            labels: true,
            angle: LabelAngle::Auto,
            title: String::new(),
            range: None,
            start_at_zero: true,
            tick_count: None,
            side: AxisSide::Start,
            format: NumberFormat::default(),
            grid: Gridlines::default(),
            colour: BrushSource::Literal(crate::Brush::Solid(grey(0.75))),
        }
    }
}

/// An axis's gridlines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gridlines {
    pub show: bool,
    pub dashed: bool,
    pub colour: BrushSource,
}

impl Default for Gridlines {
    fn default() -> Self {
        Self {
            show: true,
            dashed: false,
            colour: BrushSource::Literal(crate::Brush::Solid(grey(0.9))),
        }
    }
}

/// How a number is written on an axis or a value label.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NumberFormat {
    /// `None` writes as many as the value needs, up to the tick step's.
    pub decimals: Option<u8>,
    pub thousands: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub suffix: String,
    /// 1.2k, 3.4M.
    pub compact: bool,
    /// ×100 with `%`.
    pub percent: bool,
}

/// Where value labels sit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValuePosition {
    #[default]
    Off,
    Inside,
    Outside,
    End,
}

/// What a pie's labels say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PieLabel {
    #[default]
    Value,
    Percent,
    Category,
}

/// Value label options.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Values {
    pub position: ValuePosition,
    pub pie: PieLabel,
    /// Whether a pie's labels sit outside with a leader line.
    pub pie_outside: bool,
    pub format: NumberFormat,
}

/// Where the legend sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegendPosition {
    Off,
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

/// A legend's or a row's alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Start,
    Centre,
    End,
}

/// Legend options.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Legend {
    pub position: LegendPosition,
    pub align: Align,
}

/// The chart's type: one family and weight, a size per kind of label, and the ink.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartText {
    /// Empty is the document's default text family.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub family: String,
    pub weight: u16,
    pub tick_size: f64,
    pub value_size: f64,
    pub legend_size: f64,
    pub title_size: f64,
    pub ink: BrushSource,
}

impl Default for ChartText {
    fn default() -> Self {
        Self {
            family: String::new(),
            weight: 400,
            tick_size: 11.0,
            value_size: 11.0,
            legend_size: 12.0,
            title_size: 12.0,
            ink: BrushSource::Literal(crate::Brush::Solid(grey(0.35))),
        }
    }
}

fn grey(l: f32) -> peniko::Color {
    peniko::Color::new([l, l, l, 1.0])
}

fn yes() -> bool {
    true
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Why a description is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChartRule {
    #[error("a chart table holds at most {MAX_ROWS} rows and {MAX_COLUMNS} columns")]
    TooLarge,
    #[error("a chart cell holds at most {MAX_CELL_CHARS} characters")]
    CellTooLong,
    #[error("a chart table's rows must each hold one cell per column")]
    RaggedRow,
    #[error("a chart table's column ids must be unique")]
    DuplicateColumn,
    #[error("a chart's mapping or series names a column its table does not have")]
    UnknownColumn,
    #[error("a chart's series are styled once each")]
    DuplicateSeries,
    #[error("a chart option is out of range")]
    BadOption,
    #[error("a chart's colours cannot bind variables yet")]
    BoundColour,
    #[error("a chart's parts name a node twice or a role twice")]
    DuplicatePart,
}

impl Chart {
    /// Whether this description may be stored: the table's limits and shape, the
    /// mapping naming columns the table has, every number finite and in range.
    /// The same rule for an operation and the loader.
    pub fn validate(&self) -> Result<(), ChartRule> {
        let t = &self.table;
        if t.rows.len() > MAX_ROWS || t.columns.len() > MAX_COLUMNS {
            return Err(ChartRule::TooLarge);
        }
        let mut ids = FxHashSet::default();
        for c in &t.columns {
            if !ids.insert(c.id) {
                return Err(ChartRule::DuplicateColumn);
            }
            if c.name.chars().count() > MAX_CELL_CHARS {
                return Err(ChartRule::CellTooLong);
            }
        }
        for r in &t.rows {
            if r.len() != t.columns.len() {
                return Err(ChartRule::RaggedRow);
            }
            if r.iter().any(|c| c.chars().count() > MAX_CELL_CHARS) {
                return Err(ChartRule::CellTooLong);
            }
        }
        if self.mapping.columns().any(|c| !ids.contains(&c)) {
            return Err(ChartRule::UnknownColumn);
        }
        let mut styled = FxHashSet::default();
        for s in &self.series {
            if !ids.contains(&s.column) {
                return Err(ChartRule::UnknownColumn);
            }
            if !styled.insert(s.column) {
                return Err(ChartRule::DuplicateSeries);
            }
        }
        let mut names = FxHashSet::default();
        for n in &self.nodes {
            if !names.insert(n.name.as_str()) || n.name.chars().count() > MAX_CELL_CHARS {
                return Err(ChartRule::DuplicateSeries);
            }
        }
        self.options.validate()?;
        if self.brushes().any(|b| b.is_bound()) {
            return Err(ChartRule::BoundColour);
        }
        let mut keys = FxHashSet::default();
        let mut nodes = FxHashSet::default();
        for (k, id) in &self.parts {
            if !keys.insert(*k) || !nodes.insert(*id) {
                return Err(ChartRule::DuplicatePart);
            }
        }
        Ok(())
    }

    /// Every colour the description holds.
    pub fn brushes(&self) -> impl Iterator<Item = &BrushSource> + '_ {
        let o = &self.options;
        self.series
            .iter()
            .map(|s| &s.fill)
            .chain(self.nodes.iter().map(|n| &n.fill))
            .chain([
                &o.value_axis.colour,
                &o.value_axis.grid.colour,
                &o.category_axis.colour,
                &o.category_axis.grid.colour,
                &o.text.ink,
            ])
    }

    /// Whether `other` describes the same chart, whatever parts either holds.
    pub fn same_description(&self, other: &Chart) -> bool {
        let Chart {
            kind,
            table,
            mapping,
            series,
            nodes,
            options,
            source,
            parts: _,
        } = self;
        *kind == other.kind
            && *table == other.table
            && *mapping == other.mapping
            && *series == other.series
            && *nodes == other.nodes
            && *options == other.options
            && *source == other.source
    }

    /// This description as a copy carries it: the linked file dropped, since
    /// pasted content must not make the receiving document read a file the copy
    /// chose.
    pub fn copied(&self) -> Chart {
        Chart {
            source: None,
            ..self.clone()
        }
    }
}

impl Options {
    fn validate(&self) -> Result<(), ChartRule> {
        let len = |v: f64, max: f64| v.is_finite() && (0.0..=max).contains(&v);
        let unit = |v: f64| len(v, 1.0);
        let pct = |v: f64| len(v, 100.0);
        let px = |v: f64| len(v, crate::build::GEOMETRY_LIMIT);
        let size = |v: f64| len(v, 4096.0);
        let text = |s: &str| s.chars().count() <= MAX_TEXT_CHARS;
        let format = |f: &NumberFormat| {
            f.decimals.is_none_or(|d| d <= 10) && text(&f.prefix) && text(&f.suffix)
        };
        let axis = |a: &Axis| {
            text(&a.title)
                && a.range
                    .is_none_or(|(lo, hi)| lo.is_finite() && hi.is_finite() && lo < hi)
                && a.tick_count.is_none_or(|n| (1..=100).contains(&n))
                && format(&a.format)
        };
        let b = &self.bars;
        let l = &self.lines;
        let s = &self.slices;
        let p = &self.points;
        let f = &self.flows;
        let t = &self.text;
        let ok = pct(b.group_gap)
            && pct(b.bar_gap)
            && px(b.radius)
            && size(b.stroke)
            && size(l.width)
            && size(l.point_size)
            && unit(l.area_opacity)
            && pct(s.hole)
            && len(s.pad_angle, 45.0)
            && px(s.radius)
            && s.start_angle.is_finite()
            && (-360.0..=360.0).contains(&s.start_angle)
            && s.group_below.is_none_or(pct)
            && size(p.size)
            && unit(p.opacity)
            && size(p.max_size)
            && size(f.node_width)
            && size(f.node_gap)
            && px(f.node_radius)
            && unit(f.opacity)
            && axis(&self.value_axis)
            && axis(&self.category_axis)
            && format(&self.values.format)
            && text(&t.family)
            && (1..=1000).contains(&t.weight)
            && [t.tick_size, t.value_size, t.legend_size, t.title_size]
                .iter()
                .all(|v| *v > 0.0 && size(*v))
            && px(self.padding)
            && px(self.gap);
        if ok {
            Ok(())
        } else {
            Err(ChartRule::BadOption)
        }
    }
}

/// Why a chart may not sit where it is (`charts.md`, *Where a chart may sit*).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlacementRule {
    #[error("is a chart on a layer that is not a frame")]
    NotAFrame,
    /// Inside a main component, an instance, a kit main, a boolean or a mask.
    #[error("is a chart that can't be part of a component — flatten it first")]
    InComponent,
    #[error("is a chart that sets modes, width queries, guides or layout grids")]
    FrameSettings,
}

/// The chart rules over the whole document, after every transaction and on load:
/// a chart is a frame, outside any main, instance, kit main, boolean or mask, and
/// it sets no modes or width queries, holds no guides or layout grids and is not
/// a component set. Whole-document for `component::check`'s reason: moving a
/// chart's ancestor into a main dirties the ancestor, not the chart.
pub fn check(
    nodes: &FxHashMap<NodeId, Node>,
    vault: Option<NodeId>,
    guide_owners: impl IntoIterator<Item = NodeId>,
) -> Result<(), (NodeId, PlacementRule)> {
    let charts: Vec<&Node> = nodes.values().filter(|n| n.chart.is_some()).collect();
    if charts.is_empty() {
        return Ok(());
    }
    let owners: FxHashSet<NodeId> = guide_owners.into_iter().collect();
    for n in charts {
        if !matches!(n.kind, NodeKind::Artboard { .. }) {
            return Err((n.id, PlacementRule::NotAFrame));
        }
        if !n.grids.is_empty()
            || !n.modes.is_empty()
            || !n.queries.is_empty()
            || n.set.is_some()
            || owners.contains(&n.id)
        {
            return Err((n.id, PlacementRule::FrameSettings));
        }
        let mut at = Some(n);
        let mut first = true;
        while let Some(m) = at {
            let barred = m.component
                || m.link.is_some()
                || m.mask
                || Some(m.id) == vault
                || (!first && matches!(m.kind, NodeKind::Boolean { .. }));
            if barred {
                return Err((n.id, PlacementRule::InComponent));
            }
            first = false;
            at = m.parent.and_then(|p| nodes.get(&p));
        }
    }
    Ok(())
}

/// The chart frame `id` is a part of: its nearest strict ancestor carrying a
/// chart. `None` for a node outside every chart and for a chart frame itself.
pub fn owner(doc: &crate::Document, id: NodeId) -> Option<NodeId> {
    let mut at = doc.get(id)?.parent;
    while let Some(p) = at {
        let n = doc.get(p)?;
        if n.chart.is_some() {
            return Some(p);
        }
        at = n.parent;
    }
    None
}

/// The charts `ops` regenerate: those it writes parts for.
pub fn regenerated(ops: &[Operation]) -> FxHashSet<NodeId> {
    ops.iter()
        .filter_map(|op| match op {
            Operation::SetChartParts { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

/// Refuse `op`, against `doc` before it applies, when it writes a chart's parts
/// outside that chart's regeneration (`regenerated`): any edit of a node under a
/// chart frame, a child added to or moved into one, and the frame's own layout
/// (`display`), which the regeneration writes. A whole chart created, inserted,
/// moved or deleted is not a part edit.
pub fn guard(
    doc: &crate::Document,
    op: &Operation,
    regenerated: &FxHashSet<NodeId>,
) -> Result<(), crate::op::OpError> {
    let free = |chart: NodeId| regenerated.contains(&chart);
    let is_chart = |id: NodeId| doc.get(id).is_some_and(|n| n.chart.is_some());
    let refuse = |id: NodeId| Err(crate::op::OpError::ChartPart(id));
    // A node under a chart, or (for `into`) a chart frame or a node under one.
    let part = |id: NodeId| owner(doc, id).filter(|c| !free(*c));
    let into = |parent: NodeId| {
        if is_chart(parent) && !free(parent) {
            Some(parent)
        } else {
            part(parent)
        }
    };
    let hit = match op {
        Operation::CreateNode { parent, .. } | Operation::InsertSubtree { parent, .. } => {
            into(*parent).map(|_| *parent)
        }
        Operation::Reparent { id, new_parent, .. } => part(*id)
            .map(|_| *id)
            .or_else(|| into(*new_parent).map(|_| *id)),
        Operation::SetDisplay { id, .. } if is_chart(*id) && !free(*id) => Some(*id),
        other => {
            let ids = match other {
                Operation::DeleteNode { id } | Operation::Reorder { id, .. } => vec![*id],
                _ => other.overwrites().into_iter().collect(),
            };
            ids.into_iter().find(|id| part(*id).is_some())
        }
    };
    match hit {
        Some(id) => refuse(id),
        None => Ok(()),
    }
}

/// *Flatten chart*: the frame keeps everything it is, its parts stay where they
/// are, and the description goes. `None` for a node that is not a chart.
pub fn flatten(doc: &crate::Document, id: NodeId) -> Option<Operation> {
    doc.get(id)?.chart.as_ref()?;
    Some(Operation::SetChart { id, chart: None })
}

#[cfg(test)]
mod tests;
