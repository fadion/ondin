//! `chart::generate`: a description laid out into a size, as the parts that
//! draw it (`docs/design/charts.md`, *Laid out, not placed*).
//!
//! Every part carries both its place at this size (its stored transform and
//! geometry) and the layout that keeps it there as the frame resizes (insets in
//! percent of the plot, flex and grid on the containers), so the parts draw
//! right before any layout runs and reflow with it after.

use super::scale::{self, Ticks};
use super::{
    Axis, AxisSide, Chart, ChartKind, ColumnId, LabelAngle, LegendPosition, PartKey, PartRole,
    Stacking, ValuePosition, parse_number,
};
use crate::container::{
    AlignContent, AlignItems, AutoMargins, Dimension, Display, Flex, FlexDirection, FlexWrap, Grid,
    GridLines, GridPlacement, Insets, JustifyContent, LayoutItem, LengthPct, Track, TrackBreadth,
    TrackSize,
};
use crate::node::{Fill, NodeKind, Stroke, TextSizing};
use crate::typography::TextStyle;
use crate::variable::BrushSource;
use kurbo::{Affine, Point, Rect, RoundedRectRadii, Shape, Size};

/// One generated node: its role, where it hangs, and everything it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub key: PartKey,
    /// The part it is a child of; `None` hangs it on the chart frame.
    pub parent: Option<PartKey>,
    pub name: String,
    pub kind: NodeKind,
    pub transform: Affine,
    pub fills: Vec<Fill>,
    pub strokes: Vec<Stroke>,
    pub opacity: f32,
    pub insets: Insets,
    pub display: Option<Display>,
    pub item: LayoutItem,
}

/// What [`generate`] makes of a chart at a size.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    /// The chart frame's own layout.
    pub display: Display,
    /// Parents before their children, siblings bottom to top.
    pub parts: Vec<Part>,
    /// Values the kind cannot draw, each with its reason, for the card's line.
    pub left_out: Vec<String>,
}

/// Why a chart draws nothing new.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// More marks than [`super::MAX_MARKS`]: the count and what they are.
    TooManyMarks { count: usize, what: &'static str },
    /// A Sankey whose flows lead back to a node already passed, named as the
    /// loop runs: `Signup → Search → Signup`.
    LoopsBack(String),
}

/// The gap between an axis's labels and the plot.
const AXIS_GAP: f64 = 6.0;
/// The legend's gap between items, and between a swatch and its label.
const LEGEND_GAP: f64 = 12.0;
const SWATCH_GAP: f64 = 6.0;
/// A legend swatch's side.
const SWATCH: f64 = 10.0;
/// The space between a value label and its bar.
const VALUE_GAP: f64 = 4.0;
/// A tick mark's length, out from the plot into its axis's track.
const TICK_LEN: f64 = 4.0;
/// A dashed gridline's dash and gap.
const GRID_DASH: [f64; 2] = [3.0, 3.0];

/// The palettes' colours, in the order series take them.
pub fn palette(p: super::Palette) -> &'static [[u8; 3]] {
    match p {
        // Distinct under the three dichromacies the vision simulation shows.
        super::Palette::Ondin => &[
            [0x3B, 0x6F, 0xE0],
            [0xF0, 0x8C, 0x2E],
            [0x2E, 0xA8, 0x8A],
            [0xD9, 0x4E, 0x7A],
            [0x8A, 0x63, 0xD2],
            [0xC9, 0xA2, 0x27],
            [0x4A, 0xA8, 0xD8],
            [0x7A, 0x7A, 0x7A],
        ],
        super::Palette::Cool => &[
            [0x1F, 0x4E, 0x99],
            [0x2E, 0x86, 0xC1],
            [0x48, 0xC9, 0xB0],
            [0x5D, 0x6D, 0x7E],
            [0x76, 0xD7, 0xEA],
            [0x1A, 0x93, 0x6F],
        ],
        super::Palette::Warm => &[
            [0xC0, 0x39, 0x2B],
            [0xE6, 0x7E, 0x22],
            [0xF1, 0xC4, 0x0F],
            [0xA0, 0x40, 0x00],
            [0xE8, 0x8E, 0x8E],
            [0x8E, 0x44, 0xAD],
        ],
        super::Palette::Mono => &[
            [0x1E, 0x3A, 0x5F],
            [0x2F, 0x5C, 0x93],
            [0x4A, 0x7F, 0xC1],
            [0x7B, 0xA4, 0xD9],
            [0xAE, 0xC8, 0xEA],
        ],
    }
}

fn rgb(c: [u8; 3]) -> BrushSource {
    BrushSource::Literal(crate::Brush::Solid(peniko::Color::from_rgb8(
        c[0], c[1], c[2],
    )))
}

/// One series as drawn: its column, name, colour and values by row (`None` for
/// an empty cell or one left out).
struct SeriesData {
    column: ColumnId,
    name: String,
    fill: BrushSource,
    values: Vec<Option<f64>>,
}

/// What the table says, read for the kind: the categories (rows whose category
/// is not empty) and the visible series.
struct Data {
    categories: Vec<String>,
    series: Vec<SeriesData>,
    left_out: Vec<String>,
}

fn read(chart: &Chart) -> Data {
    let t = &chart.table;
    let mut left_out = Vec::new();
    let cat = chart.mapping.category.and_then(|c| t.index_of(c));
    let mut rows = Vec::new();
    for (i, r) in t.rows.iter().enumerate() {
        match cat {
            Some(c) if r[c].trim().is_empty() => {
                left_out.push(format!("Row {}: no category", i + 1));
            }
            Some(c) => rows.push((i, r[c].trim().to_string())),
            None => rows.push((i, format!("{}", i + 1))),
        }
    }
    let colours = palette(chart.options.palette);
    let mut series = Vec::new();
    for (n, &col) in chart.mapping.series.iter().enumerate() {
        let Some(at) = t.index_of(col) else { continue };
        let style = chart.series.iter().find(|s| s.column == col);
        if style.is_some_and(|s| !s.visible) {
            continue;
        }
        let name = t.columns[at].name.clone();
        let mut values = Vec::with_capacity(rows.len());
        for (i, _) in &rows {
            let cell = t.rows[*i][at].trim();
            if cell.is_empty() {
                values.push(None);
            } else if let Some(v) = parse_number(cell) {
                values.push(Some(v));
            } else {
                left_out.push(format!("{name}, row {}: “{cell}” is not a number", i + 1));
                values.push(None);
            }
        }
        series.push(SeriesData {
            column: col,
            name,
            fill: style.map_or_else(|| rgb(colours[n % colours.len()]), |s| s.fill.clone()),
            values,
        });
    }
    Data {
        categories: rows.into_iter().map(|(_, c)| c).collect(),
        series,
        left_out,
    }
}

/// Lays `chart` out into `size` as parts. `family` is the document's default
/// text family, used when the chart names none.
pub fn generate(chart: &Chart, size: Size, family: &str) -> Result<Generated, Refusal> {
    let mut data = read(chart);
    // A pie's slices are its legend's entries; every other kind's are its series.
    let slices = (chart.kind == ChartKind::Pie).then(|| pie_slices(chart, &mut data));
    let clouds = (chart.kind == ChartKind::Scatter).then(|| scatter_clouds(chart, &mut data));
    let entries: Vec<Entry> = match (&slices, &clouds) {
        (Some(s), _) => s
            .iter()
            .map(|s| Entry {
                series: s.index,
                name: s.name.clone(),
                fill: s.fill.clone(),
            })
            .collect(),
        (_, Some(c)) => c
            .iter()
            .map(|c| Entry {
                series: c.id,
                name: c.name.clone(),
                fill: c.fill.clone(),
            })
            .collect(),
        _ => data
            .series
            .iter()
            .map(|s| Entry {
                series: s.column.0,
                name: s.name.clone(),
                fill: s.fill.clone(),
            })
            .collect(),
    };
    let data = data;
    let o = &chart.options;
    let text = Text {
        family: if o.text.family.is_empty() {
            family.to_string()
        } else {
            o.text.family.clone()
        },
        weight: o.text.weight,
        ink: o.text.ink.clone(),
    };
    let mut parts = Vec::new();
    let mut out = Out {
        parts: &mut parts,
        text: &text,
    };

    let legend_at = match o.legend.position {
        LegendPosition::Off => None,
        p if entries.is_empty() || matches!(chart.kind, ChartKind::Sankey) => {
            let _ = p;
            None
        }
        p => Some(p),
    };
    let row = matches!(
        legend_at,
        Some(LegendPosition::Left | LegendPosition::Right)
    );
    let pad = o.padding;
    let inner = Size::new(
        (size.width - 2.0 * pad).max(0.0),
        (size.height - 2.0 * pad).max(0.0),
    );

    // The legend first: its size decides the body's.
    let legend = legend_at.map(|at| legend_items(&entries, o.text.legend_size, &text, at, inner));
    let legend_box = legend.as_ref().map_or(Size::ZERO, |l| l.size);
    let body = if row {
        Size::new(
            (inner.width - legend_box.width - o.gap).max(0.0),
            inner.height,
        )
    } else if legend.is_some() {
        Size::new(
            inner.width,
            (inner.height - legend_box.height - o.gap).max(0.0),
        )
    } else {
        inner
    };
    let legend_first = matches!(legend_at, Some(LegendPosition::Top | LegendPosition::Left));
    let body_at = Point::new(
        pad + if row && legend_first {
            legend_box.width + o.gap
        } else {
            0.0
        },
        pad + if !row && legend_first {
            legend_box.height + o.gap
        } else {
            0.0
        },
    );
    let legend_origin = Point::new(
        pad + if row && !legend_first {
            body.width + o.gap
        } else {
            0.0
        },
        pad + if !row && !legend_first {
            body.height + o.gap
        } else {
            0.0
        },
    );

    let emit_legend = |out: &mut Out| {
        if let Some(l) = &legend {
            out.legend(l, legend_origin, row, o.legend.align);
        }
    };
    if legend_first {
        emit_legend(&mut out);
    }
    let mut sankey_left = Vec::new();
    match chart.kind {
        ChartKind::Column | ChartKind::Bar => {
            bars(chart, &data, body, body_at, &mut out)?;
        }
        ChartKind::Line | ChartKind::Area => {
            lines(chart, &data, body, body_at, &mut out)?;
        }
        ChartKind::Sankey => {
            let mut left = Vec::new();
            sankey(chart, &mut left, body, body_at, &mut out)?;
            sankey_left = left;
        }
        ChartKind::Scatter => {
            scatter(
                chart,
                &data,
                clouds.as_deref().unwrap_or(&[]),
                body,
                body_at,
                &mut out,
            )?;
        }
        ChartKind::Pie => {
            pie(
                chart,
                slices.as_deref().unwrap_or(&[]),
                body,
                body_at,
                row,
                &mut out,
            )?;
        }
    }
    if !legend_first {
        emit_legend(&mut out);
    }

    let display = Display::Flex(Flex {
        direction: if row {
            FlexDirection::Row
        } else {
            FlexDirection::Column
        },
        wrap: FlexWrap::NoWrap,
        justify_content: JustifyContent::Start,
        align_items: AlignItems::Stretch,
        align_content: AlignContent::default(),
        row_gap: o.gap,
        column_gap: o.gap,
        padding: [pad; 4],
        ..Flex::default()
    });
    let mut left_out = data.left_out;
    left_out.extend(sankey_left);
    Ok(Generated {
        display,
        parts,
        left_out,
    })
}

fn key(role: PartRole, series: u32, index: u32) -> PartKey {
    PartKey {
        role,
        series,
        index,
    }
}

/// The body fills what the legend leaves, along the frame's axis and across it.
fn body_item(row: bool) -> LayoutItem {
    let mut item = LayoutItem {
        grow: 1.0,
        basis: Dimension::Px(0.0),
        ..LayoutItem::default()
    };
    if row {
        item.min_width = Dimension::Px(0.0);
        item.height = Dimension::Percent(100.0);
    } else {
        item.min_height = Dimension::Px(0.0);
        item.width = Dimension::Percent(100.0);
    }
    item
}

/// The chart's type, resolved.
struct Text {
    family: String,
    weight: u16,
    ink: BrushSource,
}

impl Text {
    fn kind(&self, content: &str, size: f64) -> NodeKind {
        NodeKind::Text {
            content: content.to_string(),
            style: Box::new(TextStyle {
                font_family: self.family.clone(),
                font_size: size,
                weight: self.weight,
                ..TextStyle::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    /// The box `content` sets in at `size`, from the same layout a text node uses.
    fn measure(&self, content: &str, size: f64) -> Rect {
        crate::geometry::local_bounds(&self.kind(content, size), None).unwrap_or(Rect::new(
            0.0,
            0.0,
            0.0,
            size * 1.2,
        ))
    }
}

/// The parts being written, and the type they set labels in.
struct Out<'a> {
    parts: &'a mut Vec<Part>,
    text: &'a Text,
}

fn fill(brush: BrushSource) -> Vec<Fill> {
    vec![Fill {
        brush,
        visible: true,
    }]
}

/// A mark's paint in `brush`: filled solid, or outlined as `Options::outline`
/// says, the stroke laid inside so a mark keeps the size its value gives it.
fn mark_paint(o: &super::Options, brush: &BrushSource) -> (Vec<Fill>, Vec<Stroke>) {
    let Some(width) = o.outline else {
        return (fill(brush.clone()), Vec::new());
    };
    let mut stroke = line_stroke(brush.clone(), width, super::Dash::Solid);
    stroke.align = crate::node::StrokeAlign::Inside;
    (fill(tinted(brush, super::OUTLINE_TINT)), vec![stroke])
}

/// `brush` at `k` of its alpha; a bound colour keeps its binding.
fn tinted(brush: &BrushSource, k: f32) -> BrushSource {
    match brush {
        BrushSource::Bound { var, alpha } => BrushSource::Bound {
            var: *var,
            alpha: alpha * k,
        },
        BrushSource::Literal(crate::Brush::Solid(c)) => {
            BrushSource::Literal(crate::Brush::Solid(c.multiply_alpha(k)))
        }
        BrushSource::Literal(crate::Brush::Gradient(g)) => {
            let mut g = g.clone();
            for s in g.gradient.stops.iter_mut() {
                s.color = s.color.multiply_alpha(k);
            }
            BrushSource::Literal(crate::Brush::Gradient(g))
        }
        other => other.clone(),
    }
}

fn pct(v: f64) -> Option<LengthPct> {
    Some(LengthPct::Percent(v))
}

fn px(v: f64) -> Option<LengthPct> {
    Some(LengthPct::Px(v))
}

impl Out<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        key: PartKey,
        parent: Option<PartKey>,
        name: String,
        kind: NodeKind,
        at: Point,
        fills: Vec<Fill>,
        insets: Insets,
        item: LayoutItem,
    ) {
        self.parts.push(Part {
            key,
            parent,
            name,
            kind,
            transform: Affine::translate(at.to_vec2()),
            fills,
            strokes: Vec::new(),
            opacity: 1.0,
            insets,
            display: None,
            item,
        });
    }

    /// An unpainted container frame at `bx` in its parent.
    fn frame(
        &mut self,
        key: PartKey,
        parent: Option<PartKey>,
        name: &str,
        bx: Rect,
        display: Option<Display>,
        item: LayoutItem,
    ) {
        self.frame_pinned(key, parent, name, bx, display, item, Insets::default());
    }

    #[allow(clippy::too_many_arguments)]
    fn frame_pinned(
        &mut self,
        key: PartKey,
        parent: Option<PartKey>,
        name: &str,
        bx: Rect,
        display: Option<Display>,
        item: LayoutItem,
        insets: Insets,
    ) {
        self.push(
            key,
            parent,
            name.to_string(),
            NodeKind::Artboard {
                size: bx.size(),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            bx.origin(),
            Vec::new(),
            insets,
            item,
        );
        self.parts.last_mut().expect("pushed").display = display;
    }

    /// A text label, its box's corner at `at`.
    #[allow(clippy::too_many_arguments)]
    fn label(
        &mut self,
        key: PartKey,
        parent: PartKey,
        name: String,
        content: &str,
        size: f64,
        at: Point,
        insets: Insets,
        turn: f64,
    ) {
        let kind = self.text.kind(content, size);
        let ink = self.text.ink.clone();
        self.push(
            key,
            Some(parent),
            name,
            kind,
            at,
            fill(ink),
            insets,
            LayoutItem::default(),
        );
        if turn != 0.0 {
            let b = self.text.measure(content, size);
            let p = self.parts.last_mut().expect("pushed");
            // Turned about its box's centre, the way layout turns a placed box.
            p.transform = crate::container::placed_at(at, b, Affine::rotate(turn));
        }
    }

    /// The part [`Self::rect`] just pushed, repainted as a mark ([`mark_paint`]).
    fn as_mark(&mut self, o: &super::Options) {
        let p = self.parts.last_mut().expect("pushed");
        if let Some(brush) = p.fills.first().map(|f| f.brush.clone()) {
            (p.fills, p.strokes) = mark_paint(o, &brush);
        }
    }

    fn rect(&mut self, key: PartKey, parent: PartKey, name: String, bx: Rect, f: Spec) {
        self.push(
            key,
            Some(parent),
            name,
            NodeKind::Rect {
                size: bx.size(),
                corner_radii: f.radii,
                corner_smoothing: 0.0,
            },
            bx.origin(),
            fill(f.fill),
            f.insets,
            LayoutItem::default(),
        );
    }

    /// A gridline across the plot at `share` along the value axis (see
    /// [`rule_at`]): a 1 px rect, or a dashed line when the grid is dashed, since
    /// a dash is a stroke's. The line is flat on one axis, so pinned on both
    /// edges it keeps its place by its near inset.
    #[allow(clippy::too_many_arguments)]
    fn gridline(
        &mut self,
        key: PartKey,
        parent: PartKey,
        name: String,
        horizontal: bool,
        share: f64,
        plot: Size,
        grid: &super::Gridlines,
    ) {
        if !grid.dashed {
            let (bx, insets) = rule_at(horizontal, share, plot);
            let radii = RoundedRectRadii::default();
            let fill = grid.colour.clone();
            self.rect(
                key,
                parent,
                name,
                bx,
                Spec {
                    fill,
                    radii,
                    insets,
                },
            );
            return;
        }
        let mut p = kurbo::BezPath::new();
        if horizontal {
            let x = share * plot.width;
            p.move_to((x, 0.0));
            p.line_to((x, plot.height));
        } else {
            let y = (1.0 - share) * plot.height;
            p.move_to((0.0, y));
            p.line_to((plot.width, y));
        }
        let mut stroke = line_stroke(grid.colour.clone(), 1.0, super::Dash::Solid);
        stroke.dashes = GRID_DASH.to_vec();
        stroke.cap = kurbo::Cap::Butt;
        self.path(key, parent, name, p, plot, Vec::new(), vec![stroke], 1.0);
    }

    /// A path drawn in its parent's space, which is `plot` big: pinned on every
    /// edge by its own box's place as percentages, so it stretches with the plot
    /// (`container::inset_stretch`) and lands exactly where regeneration would.
    #[allow(clippy::too_many_arguments)]
    fn path(
        &mut self,
        key: PartKey,
        parent: PartKey,
        name: String,
        path: kurbo::BezPath,
        plot: Size,
        fills: Vec<Fill>,
        strokes: Vec<Stroke>,
        opacity: f32,
    ) {
        self.path_rounded(
            key,
            parent,
            name,
            (path, Vec::new()),
            plot,
            fills,
            strokes,
            opacity,
        );
    }

    /// [`Self::path`] with a corner radius per anchor, pinned by the box of the
    /// rounded outline it draws, since that is the box layout stretches.
    #[allow(clippy::too_many_arguments)]
    fn path_rounded(
        &mut self,
        key: PartKey,
        parent: PartKey,
        name: String,
        (path, corner_radii): (kurbo::BezPath, Vec<f64>),
        plot: Size,
        fills: Vec<Fill>,
        strokes: Vec<Stroke>,
        opacity: f32,
    ) {
        if path.elements().is_empty() {
            return;
        }
        let b = crate::geometry::round_corners(&path, &corner_radii).bounding_box();
        let (w, h) = (plot.width.max(1e-9), plot.height.max(1e-9));
        let insets = Insets {
            left: pct(b.x0 / w * 100.0),
            right: pct((1.0 - b.x1 / w) * 100.0),
            top: pct(b.y0 / h * 100.0),
            bottom: pct((1.0 - b.y1 / h) * 100.0),
            margin_auto: AutoMargins::default(),
        };
        self.parts.push(Part {
            key,
            parent: Some(parent),
            name,
            kind: NodeKind::Path {
                path,
                corner_radii,
                corner_smoothing: 0.0,
                mirror: crate::symmetry::PathMirror::NONE,
            },
            transform: Affine::IDENTITY,
            fills,
            strokes,
            opacity,
            insets,
            display: None,
            item: LayoutItem::default(),
        });
    }

    fn legend(&mut self, l: &LegendLayout, at: Point, row: bool, align: super::Align) {
        let k = key(PartRole::Legend, 0, 0);
        let justify = match align {
            super::Align::Start => JustifyContent::Start,
            super::Align::Centre => JustifyContent::Center,
            super::Align::End => JustifyContent::End,
        };
        let display = Display::Flex(Flex {
            direction: if row {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            },
            wrap: if row {
                FlexWrap::NoWrap
            } else {
                FlexWrap::Wrap
            },
            justify_content: justify,
            align_items: AlignItems::Center,
            row_gap: SWATCH_GAP,
            column_gap: LEGEND_GAP,
            ..Flex::default()
        });
        let item = if row {
            LayoutItem {
                width: Dimension::FitContent,
                height: Dimension::Percent(100.0),
                shrink: 0.0,
                ..LayoutItem::default()
            }
        } else {
            LayoutItem {
                width: Dimension::Percent(100.0),
                height: Dimension::FitContent,
                shrink: 0.0,
                ..LayoutItem::default()
            }
        };
        self.frame(
            k,
            None,
            "Legend",
            Rect::from_origin_size(at, l.size),
            Some(display),
            item,
        );
        for it in &l.items {
            let ik = key(PartRole::LegendItem, it.series, 0);
            self.frame(
                ik,
                Some(k),
                &format!("Legend · {}", it.name),
                it.bx,
                Some(Display::Flex(Flex {
                    direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: SWATCH_GAP,
                    ..Flex::default()
                })),
                LayoutItem {
                    width: Dimension::FitContent,
                    height: Dimension::FitContent,
                    shrink: 0.0,
                    ..LayoutItem::default()
                },
            );
            let sy = (it.bx.height() - SWATCH) / 2.0;
            self.rect(
                key(PartRole::LegendSwatch, it.series, 0),
                ik,
                format!("Swatch · {}", it.name),
                Rect::new(0.0, sy, SWATCH, sy + SWATCH),
                Spec {
                    fill: it.fill.clone(),
                    radii: RoundedRectRadii::from_single_radius(2.0),
                    insets: Insets::default(),
                },
            );
            let ly = (it.bx.height() - it.label.height()) / 2.0 - it.label.y0;
            self.label(
                key(PartRole::LegendLabel, it.series, 0),
                ik,
                format!("Label · {}", it.name),
                &it.name,
                it.size,
                Point::new(SWATCH + SWATCH_GAP - it.label.x0, ly),
                Insets::default(),
                0.0,
            );
        }
    }
}

/// A filled rect's paint, corners and insets.
struct Spec {
    fill: BrushSource,
    radii: RoundedRectRadii,
    insets: Insets,
}

struct LegendItem {
    series: u32,
    name: String,
    fill: BrushSource,
    label: Rect,
    size: f64,
    bx: Rect,
}

struct LegendLayout {
    size: Size,
    items: Vec<LegendItem>,
}

/// The legend's items placed in rows that wrap at the width (top or bottom), or
/// in one column (left or right).
fn legend_items(
    entries: &[Entry],
    size: f64,
    text: &Text,
    at: LegendPosition,
    inner: Size,
) -> LegendLayout {
    let column = matches!(at, LegendPosition::Left | LegendPosition::Right);
    let mut items = Vec::new();
    let (mut x, mut y, mut line_h, mut widest) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for s in entries {
        let label = text.measure(&s.name, size);
        let w = SWATCH + SWATCH_GAP + label.width();
        let h = label.height().max(SWATCH);
        if column {
            if !items.is_empty() {
                y += SWATCH_GAP;
            }
            items.push((s, label, Rect::new(0.0, y, w, y + h)));
            y += h;
            widest = widest.max(w);
        } else {
            if x > 0.0 && x + w > inner.width {
                x = 0.0;
                y += line_h + SWATCH_GAP;
                line_h = 0.0;
            }
            items.push((s, label, Rect::new(x, y, x + w, y + h)));
            x += w + LEGEND_GAP;
            line_h = line_h.max(h);
            widest = widest.max(x - LEGEND_GAP);
        }
    }
    let size_box = if column {
        Size::new(widest, y)
    } else {
        Size::new(inner.width, y + line_h)
    };
    LegendLayout {
        size: size_box,
        items: items
            .into_iter()
            .map(|(s, label, bx)| LegendItem {
                series: s.series,
                name: s.name.clone(),
                fill: s.fill.clone(),
                label,
                size,
                bx,
            })
            .collect(),
    }
}

/// A value read along a value axis: 0 at its min, 1 at its max.
fn ticks_for(axis: &Axis, lo: f64, hi: f64, length: f64, label_h: f64) -> Ticks {
    let fit = (length / (label_h * 2.2)).floor().max(1.0) as u32;
    let count = axis.tick_count.unwrap_or(5.min(fit));
    let (lo, hi) = if axis.start_at_zero {
        (lo.min(0.0), hi.max(0.0))
    } else {
        (lo, hi)
    };
    scale::ticks(lo, hi, count, axis.range)
}

/// Column and bar charts: the body grid with its two axes, the plot's grid,
/// bars and value labels.
fn bars(
    chart: &Chart,
    data: &Data,
    body: Size,
    body_at: Point,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let horizontal = chart.kind == ChartKind::Bar;
    let stacking = o.bars.stacking;
    let n_cat = data.categories.len();
    let n_ser = data.series.len();
    let marks = n_cat * n_ser;
    if marks > super::MAX_MARKS {
        return Err(Refusal::TooManyMarks {
            count: marks,
            what: "bars",
        });
    }

    // Each category's extent: stacked positives up and negatives down, apart
    // the values themselves.
    let mut lo = 0.0f64;
    let mut hi = 0.0f64;
    let totals: Vec<f64> = (0..n_cat)
        .map(|c| {
            data.series
                .iter()
                .filter_map(|s| s.values[c])
                .map(f64::abs)
                .sum::<f64>()
        })
        .collect();
    let shown = |c: usize, v: f64| -> f64 {
        match stacking {
            Stacking::Percent if totals[c] > 0.0 => v / totals[c],
            _ => v,
        }
    };
    for c in 0..n_cat {
        let (mut up, mut down) = (0.0, 0.0);
        for s in &data.series {
            let Some(v) = s.values[c] else { continue };
            let v = shown(c, v);
            match stacking {
                Stacking::Apart => {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                _ => {
                    if v >= 0.0 {
                        up += v;
                    } else {
                        down += v;
                    }
                }
            }
        }
        lo = lo.min(down);
        hi = hi.max(up);
    }
    if stacking == Stacking::Percent {
        hi = if hi > 0.0 { 1.0 } else { 0.0 };
        lo = if lo < 0.0 { -1.0 } else { 0.0 };
    }
    let spec = AxisSpec {
        horizontal,
        lo,
        hi,
        // A bar's length is its value, so its axis always holds zero.
        zero: true,
        percent: stacking == Stacking::Percent,
        x: None,
    };
    let ax = axes(chart, data, body, body_at, out, spec);
    bar_marks(chart, data, &ax, &totals, out);
    ax.category_labels(data, out);
    Ok(())
}

/// What a value axis spans and how it reads: across the plot for a bar chart,
/// up it otherwise; whether it must hold zero; whether it writes shares.
struct AxisSpec {
    horizontal: bool,
    lo: f64,
    hi: f64,
    zero: bool,
    percent: bool,
    /// A scatter's X domain: the category axis read as numbers.
    x: Option<(f64, f64)>,
}

/// The body grid built: the plot and what the marks need of it, and what the
/// category labels are drawn with.
struct Axes {
    horizontal: bool,
    t: Ticks,
    plot: Size,
    plot_key: PartKey,
    /// The plot's length across the categories, and one category's band of it.
    across_len: f64,
    band: f64,
    ca_show: bool,
    every: usize,
    cat_boxes: Vec<Rect>,
    turn: f64,
    side_box: Rect,
    under_box: Rect,
    left_at_end: bool,
    bottom_at_top: bool,
    tick_size: f64,
    /// The category labels' distance from the plot, past any tick marks.
    ca_gap: f64,
    /// The category axis's tick mark colour, when it draws them.
    ca_ticks: Option<BrushSource>,
    /// A scatter's X ticks and their text; `None` reads categories.
    x: Option<Ticks>,
    x_text: Vec<String>,
}

/// A categorical chart's body: its grid with the value axis's track and labels,
/// the category axis's track, and the plot with its gridlines and baseline.
fn axes(
    chart: &Chart,
    data: &Data,
    body: Size,
    body_at: Point,
    out: &mut Out,
    spec: AxisSpec,
) -> Axes {
    let o = &chart.options;
    let AxisSpec {
        horizontal,
        lo,
        hi,
        zero,
        percent,
        x,
    } = spec;
    let n_cat = data.categories.len();
    // The value axis's labels decide its thickness, and the category labels
    // the other axis's.
    let va = &o.value_axis;
    let ca = &o.category_axis;
    let tick_size = o.text.tick_size;
    let label_h = out.text.measure("0", tick_size).height();
    let mut value_fmt = va.format.clone();
    if percent {
        value_fmt.percent = true;
    }
    // A first guess of the plot's length along the value axis, for the count.
    let along = if horizontal { body.width } else { body.height } * 0.8;
    let mut axis_va = va.clone();
    axis_va.start_at_zero = zero || va.start_at_zero;
    let t = ticks_for(&axis_va, lo, hi, along, label_h);
    let tick_text: Vec<String> = t
        .values
        .iter()
        .map(|v| {
            // A percent axis counts places in the shares it writes, not the step.
            let d = if value_fmt.percent {
                scale::decimals_of(t.step * 100.0)
            } else {
                t.decimals()
            };
            scale::format(*v, &value_fmt, d)
        })
        .collect();
    let tick_boxes: Vec<Rect> = tick_text
        .iter()
        .map(|s| out.text.measure(s, tick_size))
        .collect();
    let va_show = va.show && va.labels;
    let ca_show = ca.show && ca.labels;
    // Tick marks stand between the plot and the labels, pushing them out.
    let va_tick = if va.show && va.ticks { TICK_LEN } else { 0.0 };
    let ca_tick = if ca.show && ca.ticks { TICK_LEN } else { 0.0 };
    let (va_gap, ca_gap) = (AXIS_GAP + va_tick, AXIS_GAP + ca_tick);
    let va_thick = if va_show {
        if horizontal {
            label_h + va_gap
        } else {
            tick_boxes.iter().map(Rect::width).fold(0.0, f64::max) + va_gap
        }
    } else {
        va_tick
    };
    // A title's text and box, in its own track past its axis's labels.
    let title_of = |a: &Axis| {
        let t = a.title.trim();
        (a.show && !t.is_empty()).then(|| (t.to_string(), out.text.measure(t, o.text.title_size)))
    };
    let (va_title, ca_title) = (title_of(va), title_of(ca));
    let title_thick =
        |t: &Option<(String, Rect)>| t.as_ref().map_or(0.0, |(_, b)| b.height() + AXIS_GAP);

    // A scatter's X axis is numbers on ticks, spaced for labels four high.
    let x_ticks = x.map(|(lo, hi)| {
        let fit = ((body.width - va_thick) / (label_h * 4.0)).floor().max(1.0) as u32;
        let count = ca.tick_count.unwrap_or(5.min(fit));
        let (lo, hi) = if ca.start_at_zero {
            (lo.min(0.0), hi.max(0.0))
        } else {
            (lo, hi)
        };
        scale::ticks(lo, hi, count, ca.range)
    });
    let x_text: Vec<String> = x_ticks.as_ref().map_or_else(Vec::new, |xt| {
        xt.values
            .iter()
            .map(|v| scale::format(*v, &ca.format, xt.decimals()))
            .collect()
    });
    // Category labels: flat, or turned and thinned where they would touch.
    let cat_boxes: Vec<Rect> = match &x_ticks {
        Some(_) => x_text
            .iter()
            .map(|s| out.text.measure(s, tick_size))
            .collect(),
        None => data
            .categories
            .iter()
            .map(|c| out.text.measure(c, tick_size))
            .collect(),
    };
    let band_len_guess = if horizontal {
        body.height
    } else {
        body.width - va_thick
    } / n_cat.max(1) as f64;
    let (turn, every) = if x_ticks.is_some() {
        (0.0, 1)
    } else if horizontal {
        (0.0, thin_every(label_h * 1.2, band_len_guess))
    } else {
        category_turn(ca.angle, &cat_boxes, band_len_guess, label_h)
    };
    let ca_thick = if !ca_show {
        ca_tick
    } else if horizontal {
        cat_boxes.iter().map(Rect::width).fold(0.0, f64::max) + ca_gap
    } else {
        let (s, c) = turn.sin_cos();
        cat_boxes
            .iter()
            .map(|b| b.width() * s.abs() + b.height() * c.abs())
            .fold(0.0, f64::max)
            + ca_gap
    };

    // The body grid: the value axis beside the plot, the category axis under it
    // (a bar chart has them the other way round), each title in a track past
    // its axis's. An axis frame spans its title's track too.
    let (side_title, under_title) = if horizontal {
        (&ca_title, &va_title)
    } else {
        (&va_title, &ca_title)
    };
    let (side_axis_w, under_axis_h) = if horizontal {
        (ca_thick, va_thick)
    } else {
        (va_thick, ca_thick)
    };
    let (side_title_w, under_title_h) = (title_thick(side_title), title_thick(under_title));
    let (left_w, bottom_h) = (side_axis_w + side_title_w, under_axis_h + under_title_h);
    let left_at_end = if horizontal {
        ca.side == AxisSide::End
    } else {
        va.side == AxisSide::End
    };
    let bottom_at_top = if horizontal {
        va.side == AxisSide::End
    } else {
        ca.side == AxisSide::End
    };
    let plot = Size::new(
        (body.width - left_w).max(0.0),
        (body.height - bottom_h).max(0.0),
    );
    let px_track = |v: f64| Track::Size(TrackSize::Breadth(TrackBreadth::Px(v)));
    // `minmax(0, 1fr)`: a bare `1fr` keeps the plot's stored size as its least
    // (`auto`'s minimum), so a chart made smaller would not shrink its plot.
    let fr = Track::Size(TrackSize::MinMax {
        min: TrackBreadth::Px(0.0),
        max: TrackBreadth::Fr(1.0),
    });
    // An axis's tracks out from the plot: its labels', then its title's.
    let outward = |axis: f64, title: &Option<(String, Rect)>, title_w: f64| {
        let mut t = vec![px_track(axis)];
        if title.is_some() {
            t.push(px_track(title_w));
        }
        t
    };
    let side_tracks = outward(side_axis_w, side_title, side_title_w);
    let under_tracks = outward(under_axis_h, under_title, under_title_h);
    let (side_span, under_span) = (side_tracks.len() as u16, under_tracks.len() as u16);
    let lay = |at_end: bool, mut tracks: Vec<Track>| {
        if at_end {
            tracks.insert(0, fr.clone());
        } else {
            tracks.reverse();
            tracks.push(fr.clone());
        }
        tracks
    };
    let columns = lay(left_at_end, side_tracks);
    let rows = lay(!bottom_at_top, under_tracks);
    let (n_cols, n_rows) = (columns.len() as i16, rows.len() as i16);
    let body_key = key(PartRole::Body, 0, 0);
    out.frame(
        body_key,
        None,
        "Body",
        Rect::from_origin_size(body_at, body),
        Some(Display::Grid(Grid {
            columns,
            rows,
            ..Grid::default()
        })),
        body_item(
            matches!(
                o.legend.position,
                LegendPosition::Left | LegendPosition::Right
            ) && !data.series.is_empty(),
        ),
    );
    let (plot_col, side_col) = if left_at_end { (1, 2) } else { (n_cols, 1) };
    let (plot_row, under_row) = if bottom_at_top { (n_rows, 1) } else { (1, 2) };
    let lines = |start: i16, span: u16| GridLines {
        start: GridPlacement::Line(start),
        end: if span > 1 {
            GridPlacement::Span(span)
        } else {
            GridPlacement::Auto
        },
    };
    let cell = |(col, col_span): (i16, u16), (row, row_span): (i16, u16)| LayoutItem {
        width: Dimension::Percent(100.0),
        height: Dimension::Percent(100.0),
        grid_column: lines(col, col_span),
        grid_row: lines(row, row_span),
        ..LayoutItem::default()
    };
    let plot_x = if left_at_end { 0.0 } else { left_w };
    let plot_y = if bottom_at_top { bottom_h } else { 0.0 };
    let plot_box = Rect::from_origin_size(Point::new(plot_x, plot_y), plot);
    let side_box = Rect::from_origin_size(
        Point::new(if left_at_end { plot.width } else { 0.0 }, plot_y),
        Size::new(left_w, plot.height),
    );
    let under_box = Rect::from_origin_size(
        Point::new(plot_x, if bottom_at_top { 0.0 } else { plot.height }),
        Size::new(plot.width, bottom_h),
    );

    // Keys and names of the two axis frames, by which one is the value axis.
    let (side_role, under_role) = if horizontal {
        (PartRole::CategoryAxis, PartRole::ValueAxis)
    } else {
        (PartRole::ValueAxis, PartRole::CategoryAxis)
    };
    let side_key = key(side_role, 0, 0);
    let under_key = key(under_role, 0, 0);
    let plot_key = key(PartRole::Plot, 0, 0);
    let axis_name = |r: PartRole| {
        if r == PartRole::ValueAxis {
            "Value axis"
        } else {
            "Category axis"
        }
    };
    out.frame(
        side_key,
        Some(body_key),
        axis_name(side_role),
        side_box,
        None,
        cell((side_col, side_span), (plot_row, 1)),
    );
    out.frame(
        under_key,
        Some(body_key),
        axis_name(under_role),
        under_box,
        None,
        cell((plot_col, 1), (under_row, under_span)),
    );
    out.frame(
        plot_key,
        Some(body_key),
        "Plot",
        plot_box,
        None,
        cell((plot_col, 1), (plot_row, 1)),
    );

    // Along the value axis, as a share of the plot from its start: up from the
    // bottom for columns, right from the left for bars.
    let across_len = if horizontal { plot.height } else { plot.width };
    let value_share = |v: f64| t.t(v);

    // Value tick labels.
    let value_axis_key = key(PartRole::ValueAxis, 0, 0);
    if va_show {
        let (frame_w, frame_h) = if horizontal {
            (under_box.width(), under_box.height())
        } else {
            (side_box.width(), side_box.height())
        };
        for (i, (s, b)) in tick_text.iter().zip(&tick_boxes).enumerate() {
            let share = value_share(t.values[i]);
            let name = format!("Tick {s}");
            let k = key(PartRole::TickLabel, 0, i as u32);
            if horizontal {
                let cx = share * frame_w;
                let at = Point::new(cx - b.width() / 2.0 - b.x0, va_gap - b.y0);
                let at = if bottom_at_top {
                    Point::new(at.x, frame_h - va_gap - b.height() - b.y0)
                } else {
                    at
                };
                let mut insets = centred_x(share * 100.0);
                if bottom_at_top {
                    insets.bottom = px(va_gap);
                } else {
                    insets.top = px(va_gap);
                }
                out.label(k, value_axis_key, name, s, tick_size, at, insets, 0.0);
            } else {
                let cy = (1.0 - share) * frame_h;
                let x = if left_at_end {
                    va_gap - b.x0
                } else {
                    frame_w - va_gap - b.width() - b.x0
                };
                let at = Point::new(x, cy - b.height() / 2.0 - b.y0);
                let mut insets = centred_y((1.0 - share) * 100.0);
                if left_at_end {
                    insets.left = px(va_gap);
                } else {
                    insets.right = px(va_gap);
                }
                out.label(k, value_axis_key, name, s, tick_size, at, insets, 0.0);
            }
        }
    }
    // Value tick marks, out from the plot's edge at each tick.
    if va_tick > 0.0 {
        let (frame, from_start) = if horizontal {
            (under_box.size(), !bottom_at_top)
        } else {
            (side_box.size(), left_at_end)
        };
        for (i, v) in t.values.iter().enumerate() {
            let share = value_share(*v);
            let along = if horizontal { share } else { 1.0 - share };
            let (bx, insets) = tick_mark(!horizontal, along, from_start, frame);
            out.rect(
                key(PartRole::Tick, 0, i as u32),
                value_axis_key,
                format!("Tick mark {}", tick_text[i]),
                bx,
                Spec {
                    fill: va.colour.clone(),
                    radii: RoundedRectRadii::default(),
                    insets,
                },
            );
        }
    }
    // Each title in its own track, centred on the plot along its axis.
    let titles = [
        (side_title, side_key, true, left_at_end),
        (under_title, under_key, false, !bottom_at_top),
    ];
    for (title, axis_key, turned, edge) in titles {
        let Some((text, b)) = title else { continue };
        let series = u32::from(axis_key.role == PartRole::CategoryAxis);
        let frame = if turned { side_box } else { under_box };
        let (at, insets, turn) = title_place(*b, frame.size(), turned, edge);
        out.label(
            key(PartRole::AxisTitle, series, 0),
            axis_key,
            format!("Title {text}"),
            text,
            o.text.title_size,
            at,
            insets,
            turn,
        );
    }

    // The plot's grid and its lines.
    let grid_key = key(PartRole::Grid, 0, 0);
    out.frame_pinned(
        grid_key,
        Some(plot_key),
        "Grid",
        Rect::from_origin_size(Point::ZERO, plot),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    if va.show && va.grid.show {
        for (i, v) in t.values.iter().enumerate() {
            let share = value_share(*v);
            let name = format!("Gridline {}", tick_text[i]);
            let k = key(PartRole::Gridline, 0, i as u32);
            out.gridline(k, grid_key, name, horizontal, share, plot, &va.grid);
        }
    }
    if let Some(xt) = &x_ticks
        && ca.show
        && ca.grid.show
    {
        for (i, v) in xt.values.iter().enumerate() {
            out.gridline(
                key(PartRole::Gridline, 1, i as u32),
                grid_key,
                format!("Gridline {}", x_text[i]),
                true,
                xt.t(*v),
                plot,
                &ca.grid,
            );
        }
    }
    if ca.show && ca.line {
        // The baseline at zero; at the plot's edge on its axis's side when zero
        // is out of range.
        let zero = value_share(0.0);
        let at_zero = (-1e-9..=1.0 + 1e-9).contains(&zero);
        let edge = if horizontal {
            left_at_end
        } else {
            bottom_at_top
        };
        let share = if at_zero {
            zero
        } else {
            f64::from(u8::from(edge))
        };
        let (bx, insets) = rule_at(horizontal, share, plot);
        out.rect(
            key(PartRole::AxisLine, 1, 0),
            grid_key,
            if at_zero { "Baseline" } else { "Axis line" }.into(),
            bx,
            Spec {
                fill: ca.colour.clone(),
                radii: RoundedRectRadii::default(),
                insets,
            },
        );
    }
    if va.show && va.line {
        // Along the plot's edge on the value axis's side.
        let edge = if horizontal {
            bottom_at_top
        } else {
            left_at_end
        };
        let (bx, insets) = rule_at(!horizontal, f64::from(u8::from(edge)), plot);
        out.rect(
            key(PartRole::AxisLine, 0, 0),
            grid_key,
            "Value axis line".into(),
            bx,
            Spec {
                fill: va.colour.clone(),
                radii: RoundedRectRadii::default(),
                insets,
            },
        );
    }

    Axes {
        horizontal,
        t,
        plot,
        plot_key,
        across_len,
        band: across_len / n_cat.max(1) as f64,
        ca_show,
        every,
        cat_boxes,
        turn,
        side_box,
        under_box,
        left_at_end,
        bottom_at_top,
        tick_size,
        ca_gap,
        ca_ticks: (ca_tick > 0.0).then(|| ca.colour.clone()),
        x: x_ticks,
        x_text,
    }
}

/// Column and bar marks: one series frame of bars each, and the value labels.
fn bar_marks(chart: &Chart, data: &Data, ax: &Axes, totals: &[f64], out: &mut Out) {
    let o = &chart.options;
    let (horizontal, plot, plot_key, across_len) =
        (ax.horizontal, ax.plot, ax.plot_key, ax.across_len);
    let stacking = o.bars.stacking;
    let (n_cat, n_ser) = (data.categories.len(), data.series.len());
    let value_share = |v: f64| ax.t.t(v);
    let shown = |c: usize, v: f64| -> f64 {
        match stacking {
            Stacking::Percent if totals[c] > 0.0 => v / totals[c],
            _ => v,
        }
    };
    let band = ax.band;
    let group_gap = (o.bars.group_gap / 100.0).clamp(0.0, 0.95);
    let group = band * (1.0 - group_gap);
    let slots = match stacking {
        Stacking::Apart => n_ser.max(1),
        _ => 1,
    };
    let bar_gap = (o.bars.bar_gap / 100.0).clamp(0.0, 0.95);
    // `slots` bars and the gaps between them fill the group.
    let bar = group / (slots as f64 + bar_gap * (slots as f64 - 1.0).max(0.0));
    let mut values_parts = Vec::new();
    let mut ups = vec![0.0f64; n_cat];
    let mut downs = vec![0.0f64; n_cat];
    for (si, s) in data.series.iter().enumerate() {
        let series_key = key(PartRole::Series, s.column.0, 0);
        out.frame_pinned(
            series_key,
            Some(plot_key),
            &format!("Series {}", s.name),
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for (c, cat) in data.categories.iter().enumerate() {
            let Some(raw) = s.values[c] else { continue };
            let v = shown(c, raw);
            let (from, to) = match stacking {
                Stacking::Apart => (0.0, v),
                _ if v >= 0.0 => {
                    let from = ups[c];
                    ups[c] += v;
                    (from, ups[c])
                }
                _ => {
                    let from = downs[c];
                    downs[c] += v;
                    (from, downs[c])
                }
            };
            let slot = if stacking == Stacking::Apart { si } else { 0 };
            let a0 = c as f64 * band + (band - group) / 2.0 + slot as f64 * bar * (1.0 + bar_gap);
            let a1 = a0 + bar;
            let (s0, s1) = (value_share(from.min(to)), value_share(from.max(to)));
            // Across as shares of the plot, along as shares of the value axis.
            let (x0, x1, y0, y1) = if horizontal {
                (s0, s1, a0 / across_len, a1 / across_len)
            } else {
                (a0 / across_len, a1 / across_len, 1.0 - s1, 1.0 - s0)
            };
            let bx = Rect::new(
                x0 * plot.width,
                y0 * plot.height,
                x1 * plot.width,
                y1 * plot.height,
            );
            let insets = Insets {
                left: pct(x0 * 100.0),
                right: pct((1.0 - x1) * 100.0),
                top: pct(y0 * 100.0),
                bottom: pct((1.0 - y1) * 100.0),
                margin_auto: AutoMargins::default(),
            };
            let r = o
                .bars
                .radius
                .min(bx.width() / 2.0)
                .min(bx.height() / 2.0)
                .max(0.0);
            let end_up = to >= from && v >= 0.0;
            let radii = if o.bars.radius_all {
                RoundedRectRadii::from_single_radius(r)
            } else if horizontal {
                if end_up {
                    RoundedRectRadii::new(0.0, r, r, 0.0)
                } else {
                    RoundedRectRadii::new(r, 0.0, 0.0, r)
                }
            } else if end_up {
                RoundedRectRadii::new(r, r, 0.0, 0.0)
            } else {
                RoundedRectRadii::new(0.0, 0.0, r, r)
            };
            out.rect(
                key(PartRole::Bar, s.column.0, c as u32),
                series_key,
                format!("{} · {}", s.name, cat),
                bx,
                Spec {
                    fill: s.fill.clone(),
                    radii,
                    insets,
                },
            );
            out.as_mark(o);
            if o.values.position != ValuePosition::Off {
                values_parts.push((s.column.0, c, raw, bx, end_up));
            }
        }
    }

    // Value labels, where they fit.
    if !values_parts.is_empty() {
        let values_key = key(PartRole::Values, 0, 0);
        out.frame_pinned(
            values_key,
            Some(plot_key),
            "Values",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        let vf = &o.values.format;
        let size = o.text.value_size;
        // In a stack only the outermost segment each way has an outside to sit
        // in; the ones it covers take their label inside, where it fits.
        let outermost = |i: usize| {
            let (_, c, _, _, up) = values_parts[i];
            stacking == Stacking::Apart
                || !values_parts[i + 1..]
                    .iter()
                    .any(|(_, c2, _, _, up2)| *c2 == c && *up2 == up)
        };
        let flags: Vec<bool> = (0..values_parts.len()).map(outermost).collect();
        for ((series, c, raw, bx, up), outer) in values_parts.into_iter().zip(flags) {
            let shown_v = if stacking == Stacking::Percent && totals[c] > 0.0 {
                raw / totals[c]
            } else {
                raw
            };
            let mut f = vf.clone();
            if stacking == Stacking::Percent {
                f.percent = true;
            }
            // As many places as the value has, up to three; a share in whole percent.
            let decimals = if f.percent {
                0
            } else {
                scale::decimals_of(raw).min(3)
            };
            let s = scale::format(shown_v, &f, decimals);
            let b = out.text.measure(&s, size);
            let position = match o.values.position {
                ValuePosition::Outside if !outer => ValuePosition::Inside,
                p => p,
            };
            let Some((at, insets)) = value_place(position, horizontal, up, bx, b, plot) else {
                continue;
            };
            out.label(
                key(PartRole::ValueLabel, series, c as u32),
                values_key,
                format!("Value {s}"),
                &s,
                size,
                at,
                insets,
                0.0,
            );
        }
    }
}

impl Axes {
    /// The category labels, every `every`th, centred on their band.
    fn category_labels(&self, data: &Data, out: &mut Out) {
        let (horizontal, left_at_end, bottom_at_top) =
            (self.horizontal, self.left_at_end, self.bottom_at_top);
        let (band, across_len, every, turn, tick_size) = (
            self.band,
            self.across_len,
            self.every,
            self.turn,
            self.tick_size,
        );
        let (side_box, under_box, cat_boxes) = (self.side_box, self.under_box, &self.cat_boxes);
        let cat_axis_key = key(PartRole::CategoryAxis, 0, 0);
        let frame = if horizontal { side_box } else { under_box };
        // Tick marks: at a scatter's X ticks, else at every category's centre.
        if let Some(colour) = &self.ca_ticks {
            let at: Vec<(f64, String)> = match &self.x {
                Some(xt) => xt
                    .values
                    .iter()
                    .zip(&self.x_text)
                    .map(|(v, s)| (xt.t(*v), s.clone()))
                    .collect(),
                None => data
                    .categories
                    .iter()
                    .enumerate()
                    .map(|(c, name)| ((c as f64 + 0.5) * band / across_len.max(1e-9), name.clone()))
                    .collect(),
            };
            let from_start = if horizontal {
                left_at_end
            } else {
                !bottom_at_top
            };
            for (i, (along, name)) in at.into_iter().enumerate() {
                let (bx, insets) = tick_mark(horizontal, along, from_start, frame.size());
                out.rect(
                    key(PartRole::Tick, 1, i as u32),
                    cat_axis_key,
                    format!("Tick mark {name}"),
                    bx,
                    Spec {
                        fill: colour.clone(),
                        radii: RoundedRectRadii::default(),
                        insets,
                    },
                );
            }
        }
        if !self.ca_show {
            return;
        }
        let gap = self.ca_gap;
        // A scatter's X axis writes number ticks at their place; any other,
        // every `every`th category at its band's centre.
        let items: Vec<(f64, &String, Rect, PartKey, String)> = match &self.x {
            Some(xt) => self
                .x_text
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    (
                        xt.t(xt.values[i]) * across_len,
                        s,
                        cat_boxes[i],
                        key(PartRole::TickLabel, 1, i as u32),
                        format!("Tick {s}"),
                    )
                })
                .collect(),
            None => data
                .categories
                .iter()
                .enumerate()
                .filter(|(c, _)| c % every == 0)
                .map(|(c, name)| {
                    (
                        (c as f64 + 0.5) * band,
                        name,
                        cat_boxes[c],
                        key(PartRole::CategoryLabel, 0, c as u32),
                        format!("Category {name}"),
                    )
                })
                .collect(),
        };
        for (centre, name, b, k, label_name) in items {
            if horizontal {
                let x = if left_at_end {
                    gap - b.x0
                } else {
                    frame.width() - gap - b.width() - b.x0
                };
                let at = Point::new(x, centre - b.height() / 2.0 - b.y0);
                let mut insets = centred_y(centre / across_len * 100.0);
                if left_at_end {
                    insets.left = px(gap);
                } else {
                    insets.right = px(gap);
                }
                out.label(
                    k,
                    cat_axis_key,
                    label_name,
                    name,
                    tick_size,
                    at,
                    insets,
                    0.0,
                );
            } else {
                // A turned label's box is its unturned one, turned about its
                // centre; its centre sits under the band, as deep as it reaches.
                let (sn, cs) = turn.sin_cos();
                let reach = b.width() * sn.abs() + b.height() * cs.abs();
                let cy = if bottom_at_top {
                    frame.height() - gap - reach / 2.0
                } else {
                    gap + reach / 2.0
                };
                let at = Point::new(
                    centre - b.width() / 2.0 - b.x0,
                    cy - b.height() / 2.0 - b.y0,
                );
                let mut insets = centred_x(centre / across_len * 100.0);
                let edge = gap + reach / 2.0 - b.height() / 2.0;
                if bottom_at_top {
                    insets.bottom = px(edge);
                } else {
                    insets.top = px(edge);
                }
                out.label(
                    k,
                    cat_axis_key,
                    label_name,
                    name,
                    tick_size,
                    at,
                    insets,
                    turn,
                );
            }
        }
    }
}

/// One legend entry: a series, or a pie's slice.
struct Entry {
    /// The series' column id, or the slice's place.
    series: u32,
    name: String,
    fill: BrushSource,
}

/// The relaxation passes a Sankey's order takes: a fixed count, not a
/// convergence test, so the same table always draws the same chart.
const RELAX_PASSES: usize = 6;
/// The gap between a Sankey node and its label.
const NODE_LABEL_GAP: f64 = 6.0;

/// A Sankey node: its name, value, column and place.
struct Node {
    name: String,
    value: f64,
    column: usize,
    y: f64,
    h: f64,
    fill: BrushSource,
}

/// A Sankey flow: from and to node indices, its value, and its ribbon's ends
/// as offsets down each node.
struct Flow {
    from: usize,
    to: usize,
    value: f64,
    sy: f64,
    ty: f64,
}

/// A Sankey: flows between named nodes, the nodes in columns by depth, the
/// order in each column relaxed toward what the nodes connect to, the flows as
/// ribbons as thick as their value. Rows that cannot be drawn are pushed onto
/// `left` with their reasons.
fn sankey(
    chart: &Chart,
    left: &mut Vec<String>,
    body: Size,
    body_at: Point,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let f = &o.flows;
    let t = &chart.table;
    let m = &chart.mapping;
    let at = |c: Option<ColumnId>| c.and_then(|c| t.index_of(c));
    let (Some(fc), Some(tc), Some(vc)) = (at(m.from), at(m.to), at(m.value)) else {
        body_frame(out, body, body_at);
        return Ok(());
    };

    // Nodes in first-seen order; same-named flows added up.
    let mut nodes: Vec<Node> = Vec::new();
    let mut flows: Vec<Flow> = Vec::new();
    let colours = palette(o.palette);
    let index = |name: &str, nodes: &mut Vec<Node>| -> usize {
        if let Some(i) = nodes.iter().position(|n| n.name == name) {
            return i;
        }
        let i = nodes.len();
        let fill = chart
            .nodes
            .iter()
            .find(|s| s.name == name)
            .map_or_else(|| rgb(colours[i % colours.len()]), |s| s.fill.clone());
        nodes.push(Node {
            name: name.to_string(),
            value: 0.0,
            column: 0,
            y: 0.0,
            h: 0.0,
            fill,
        });
        i
    };
    for (r, row) in t.rows.iter().enumerate() {
        let (from, to) = (row[fc].trim(), row[tc].trim());
        if from.is_empty() || to.is_empty() {
            left.push(format!("Row {}: no From or To", r + 1));
            continue;
        }
        if from == to {
            left.push(format!("Row {}: {from} flows into itself", r + 1));
            continue;
        }
        let Some(v) = parse_number(&row[vc]).filter(|v| *v > 0.0) else {
            left.push(format!("Row {}: a flow's value must be above zero", r + 1));
            continue;
        };
        let (a, b) = (index(from, &mut nodes), index(to, &mut nodes));
        match flows.iter_mut().find(|fl| fl.from == a && fl.to == b) {
            Some(fl) => fl.value += v,
            None => flows.push(Flow {
                from: a,
                to: b,
                value: v,
                sy: 0.0,
                ty: 0.0,
            }),
        }
    }
    if nodes.len() > super::MAX_SANKEY_NODES {
        return Err(Refusal::TooManyMarks {
            count: nodes.len(),
            what: "nodes",
        });
    }
    if flows.len() > super::MAX_FLOWS {
        return Err(Refusal::TooManyMarks {
            count: flows.len(),
            what: "flows",
        });
    }
    if let Some(cycle) = find_loop(nodes.len(), &flows) {
        let mut names: Vec<&str> = cycle.iter().map(|i| nodes[*i].name.as_str()).collect();
        names.push(names[0]);
        return Err(Refusal::LoopsBack(names.join(" → ")));
    }

    // Columns by depth, then aligned.
    let n = nodes.len();
    let ins = |i: usize| flows.iter().filter(move |fl| fl.to == i);
    let outs = |i: usize| flows.iter().filter(move |fl| fl.from == i);
    let mut depth = vec![0usize; n];
    // Longest path from a source: repeat until stable, at most n rounds (no loop).
    for _ in 0..n {
        let mut moved = false;
        for fl in &flows {
            if depth[fl.to] < depth[fl.from] + 1 {
                depth[fl.to] = depth[fl.from] + 1;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let last = depth.iter().copied().max().unwrap_or(0);
    let mut height = vec![0usize; n];
    for _ in 0..n {
        let mut moved = false;
        for fl in &flows {
            if height[fl.from] < height[fl.to] + 1 {
                height[fl.from] = height[fl.to] + 1;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    for i in 0..n {
        let sink = outs(i).next().is_none();
        let source = ins(i).next().is_none();
        nodes[i].column = match f.align {
            super::FlowAlign::Left => depth[i],
            super::FlowAlign::Right => last - height[i],
            super::FlowAlign::Justify if sink => last,
            super::FlowAlign::Justify => depth[i],
            super::FlowAlign::Centre if source => outs(i)
                .map(|fl| depth[fl.to])
                .min()
                .map_or(0, |d| d.saturating_sub(1)),
            super::FlowAlign::Centre => depth[i],
        };
        nodes[i].value = ins(i)
            .map(|fl| fl.value)
            .sum::<f64>()
            .max(outs(i).map(|fl| fl.value).sum::<f64>());
    }
    let columns = last + 1;

    // Room for the labels outside the first and last columns.
    let label_text = |nd: &Node| match f.labels {
        super::NodeLabels::Name => nd.name.clone(),
        super::NodeLabels::NameAndValue => format!(
            "{} {}",
            nd.name,
            scale::format(
                nd.value,
                &o.values.format,
                scale::decimals_of(nd.value).min(2)
            )
        ),
        super::NodeLabels::Off => String::new(),
    };
    let size = o.text.legend_size;
    let widest = |col: usize| {
        nodes
            .iter()
            .filter(|nd| nd.column == col)
            .map(|nd| out.text.measure(&label_text(nd), size).width())
            .fold(0.0, f64::max)
    };
    let (room_l, room_r) = if f.labels == super::NodeLabels::Off {
        (0.0, 0.0)
    } else {
        (
            widest(0) + NODE_LABEL_GAP,
            if columns > 1 {
                widest(last) + NODE_LABEL_GAP
            } else {
                0.0
            },
        )
    };
    let plot = body;
    let nw = f.node_width.min(plot.width / 4.0).max(1.0);
    let span = (plot.width - room_l - room_r - nw).max(0.0);
    let x_of = |col: usize| {
        room_l
            + if columns > 1 {
                span * col as f64 / (columns - 1) as f64
            } else {
                span / 2.0
            }
    };

    // Heights: one scale for every column, so a value is as tall everywhere.
    let gap = f.node_gap;
    let ky = (0..columns)
        .filter_map(|c| {
            let members: Vec<&Node> = nodes.iter().filter(|nd| nd.column == c).collect();
            let total: f64 = members.iter().map(|nd| nd.value).sum();
            (total > 0.0).then(|| {
                (plot.height - gap * (members.len() as f64 - 1.0).max(0.0)).max(0.0) / total
            })
        })
        .fold(f64::INFINITY, f64::min);
    let ky = if ky.is_finite() { ky } else { 0.0 };
    for nd in nodes.iter_mut() {
        nd.h = nd.value * ky;
    }
    // Table order first, stacked from the top of each column.
    let mut order: Vec<Vec<usize>> = vec![Vec::new(); columns];
    for (i, nd) in nodes.iter().enumerate() {
        order[nd.column].push(i);
    }
    let settle = |nodes: &mut Vec<Node>, col: &[usize]| {
        // Overlaps pushed down, then back up from the bottom.
        let mut ids = col.to_vec();
        ids.sort_by(|a, b| nodes[*a].y.total_cmp(&nodes[*b].y));
        let mut y = 0.0;
        for &i in &ids {
            nodes[i].y = nodes[i].y.max(y);
            y = nodes[i].y + nodes[i].h + gap;
        }
        let mut bottom = plot.height;
        for &i in ids.iter().rev() {
            if nodes[i].y + nodes[i].h > bottom {
                nodes[i].y = (bottom - nodes[i].h).max(0.0);
            }
            bottom = nodes[i].y - gap;
        }
    };
    for col in &order {
        let mut y = 0.0;
        for &i in col {
            nodes[i].y = y;
            y += nodes[i].h + gap;
        }
    }
    if f.relax {
        let centre = |nd: &Node| nd.y + nd.h / 2.0;
        for pass in 0..RELAX_PASSES {
            let cols: Vec<usize> = if pass % 2 == 0 {
                (1..columns).collect()
            } else {
                (0..columns.saturating_sub(1)).rev().collect()
            };
            for c in cols {
                for &i in &order[c] {
                    let (mut sum, mut weight) = (0.0, 0.0);
                    for fl in &flows {
                        let other = if fl.to == i && pass % 2 == 0 {
                            Some(fl.from)
                        } else if fl.from == i && pass % 2 == 1 {
                            Some(fl.to)
                        } else {
                            None
                        };
                        if let Some(o) = other {
                            sum += centre(&nodes[o]) * fl.value;
                            weight += fl.value;
                        }
                    }
                    if weight > 0.0 {
                        nodes[i].y = sum / weight - nodes[i].h / 2.0;
                    }
                }
                settle(&mut nodes, &order[c]);
            }
        }
    }

    // Ribbons stack on each node in the order of the nodes at their other ends,
    // so they cross as little as they can.
    let mut by_source: Vec<usize> = (0..flows.len()).collect();
    by_source.sort_by(|a, b| {
        let (fa, fb) = (&flows[*a], &flows[*b]);
        fa.from
            .cmp(&fb.from)
            .then(nodes[fa.to].y.total_cmp(&nodes[fb.to].y))
    });
    let mut down = vec![0.0f64; n];
    for &k in &by_source {
        let s = flows[k].from;
        flows[k].sy = down[s];
        down[s] += flows[k].value * ky;
    }
    let mut by_target: Vec<usize> = (0..flows.len()).collect();
    by_target.sort_by(|a, b| {
        let (fa, fb) = (&flows[*a], &flows[*b]);
        fa.to
            .cmp(&fb.to)
            .then(nodes[fa.from].y.total_cmp(&nodes[fb.from].y))
    });
    let mut down = vec![0.0f64; n];
    for &k in &by_target {
        let d = flows[k].to;
        flows[k].ty = down[d];
        down[d] += flows[k].value * ky;
    }

    let body_key = body_frame(out, body, body_at);
    let flows_key = key(PartRole::Flows, 0, 0);
    out.frame_pinned(
        flows_key,
        Some(body_key),
        "Flows",
        Rect::from_origin_size(Point::ZERO, plot),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    let neutral = o.value_axis.colour.clone();
    for (k, fl) in flows.iter().enumerate() {
        let (a, b) = (&nodes[fl.from], &nodes[fl.to]);
        let th = fl.value * ky;
        let x0 = x_of(a.column) + nw;
        let x1 = x_of(b.column);
        let (y0, y1) = (a.y + fl.sy, b.y + fl.ty);
        let mid = (x0 + x1) / 2.0;
        let mut p = kurbo::BezPath::new();
        p.move_to((x0, y0));
        p.curve_to((mid, y0), (mid, y1), (x1, y1));
        p.line_to((x1, y1 + th));
        p.curve_to((mid, y1 + th), (mid, y0 + th), (x0, y0 + th));
        p.close_path();
        let brush = match f.colour {
            super::FlowColour::Source => a.fill.clone(),
            super::FlowColour::Gradient => flow_gradient(&a.fill, &b.fill, x0, x1),
            super::FlowColour::Target => b.fill.clone(),
            super::FlowColour::Neutral => neutral.clone(),
        };
        out.path(
            key(PartRole::Flow, 0, k as u32),
            flows_key,
            format!("Flow · {} → {}", a.name, b.name),
            p,
            plot,
            fill(brush),
            Vec::new(),
            f.opacity as f32,
        );
    }
    let nodes_key = key(PartRole::Nodes, 0, 0);
    out.frame_pinned(
        nodes_key,
        Some(body_key),
        "Nodes",
        Rect::from_origin_size(Point::ZERO, plot),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    let (w, h) = (plot.width.max(1e-9), plot.height.max(1e-9));
    for (i, nd) in nodes.iter().enumerate() {
        let x = x_of(nd.column);
        let bx = Rect::new(x, nd.y, x + nw, nd.y + nd.h);
        // Its place and height follow the plot; its width stays.
        out.rect(
            key(PartRole::SankeyNode, 0, i as u32),
            nodes_key,
            format!("Node · {}", nd.name),
            bx,
            Spec {
                fill: nd.fill.clone(),
                radii: RoundedRectRadii::from_single_radius(
                    f.node_radius.min(nw / 2.0).min(nd.h / 2.0).max(0.0),
                ),
                insets: Insets {
                    left: pct(x / w * 100.0),
                    top: pct(nd.y / h * 100.0),
                    bottom: pct((1.0 - (nd.y + nd.h) / h) * 100.0),
                    ..Insets::default()
                },
            },
        );
        out.as_mark(o);
        let text = label_text(nd);
        if text.is_empty() {
            continue;
        }
        let b = out.text.measure(&text, size);
        // Outside on the first and last columns; elsewhere away from the middle.
        let on_left = if nd.column == 0 && columns > 1 {
            true
        } else if nd.column == last {
            false
        } else {
            x + nw / 2.0 < plot.width / 2.0
        };
        let cy = nd.y + nd.h / 2.0;
        let mut insets = centred_y(cy / h * 100.0);
        let lx = if on_left {
            let right = x - NODE_LABEL_GAP;
            insets.right = pct((1.0 - right / w) * 100.0);
            right - b.width()
        } else {
            let left = x + nw + NODE_LABEL_GAP;
            insets.left = pct(left / w * 100.0);
            left
        };
        out.label(
            key(PartRole::NodeLabel, 0, i as u32),
            nodes_key,
            format!("Label · {}", nd.name),
            &text,
            size,
            Point::new(lx - b.x0, cy - b.height() / 2.0 - b.y0),
            insets,
            0.0,
        );
    }
    Ok(())
}

/// A *Gradient* flow's fill: a linear gradient across the ribbon from `x0` (the
/// source node's edge) in the source's colour to `x1` in the target's, in the
/// ribbon's own space. A colour that is no literal solid (one bound to a
/// variable) has no stop to give, so the flow takes the source's paint whole.
fn flow_gradient(from: &BrushSource, to: &BrushSource, x0: f64, x1: f64) -> BrushSource {
    let solid = |s: &BrushSource| match s.literal() {
        Some(crate::Brush::Solid(c)) => Some(*c),
        _ => None,
    };
    let (Some(a), Some(b)) = (solid(from), solid(to)) else {
        return from.clone();
    };
    let stop = |offset: f32, color: peniko::Color| peniko::ColorStop {
        offset,
        color: peniko::color::DynamicColor::from_alpha_color(color),
    };
    let gradient =
        peniko::Gradient::new_linear((x0, 0.0), (x1, 0.0)).with_stops([stop(0.0, a), stop(1.0, b)]);
    BrushSource::Literal(crate::Brush::Gradient(crate::GradientBrush::from(gradient)))
}

/// The Body frame with nothing laid out in it, for the kinds that draw straight
/// into it.
fn body_frame(out: &mut Out, body: Size, body_at: Point) -> PartKey {
    let k = key(PartRole::Body, 0, 0);
    out.frame(
        k,
        None,
        "Body",
        Rect::from_origin_size(body_at, body),
        None,
        body_item(false),
    );
    k
}

/// The nodes of a loop among `flows`, in the order it runs, if there is one.
fn find_loop(n: usize, flows: &[Flow]) -> Option<Vec<usize>> {
    // 0 unvisited, 1 on the current path, 2 done.
    let mut state = vec![0u8; n];
    let mut path = Vec::new();
    fn visit(
        i: usize,
        flows: &[Flow],
        state: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        state[i] = 1;
        path.push(i);
        for fl in flows.iter().filter(|fl| fl.from == i) {
            match state[fl.to] {
                1 => {
                    let at = path.iter().position(|p| *p == fl.to).expect("on the path");
                    return Some(path[at..].to_vec());
                }
                0 => {
                    if let Some(c) = visit(fl.to, flows, state, path) {
                        return Some(c);
                    }
                }
                _ => {}
            }
        }
        path.pop();
        state[i] = 2;
        None
    }
    (0..n).find_map(|i| {
        if state[i] == 0 {
            visit(i, flows, &mut state, &mut path)
        } else {
            None
        }
    })
}

/// One series of a scatter as drawn: a Y column, or one group's rows of the
/// first Y column, with its points as (x, y, size, row).
struct Cloud {
    /// The Y column's id, or past every column id for a group.
    id: u32,
    name: String,
    fill: BrushSource,
    /// The series' own shape, else `Points::shape`; a group takes its series'.
    shape: super::PointShape,
    points: Vec<ScatterPoint>,
}

/// A scatter's point: x, y, its bubble size, and the row it came from.
type ScatterPoint = (f64, f64, Option<f64>, usize);

/// The id a scatter's `n`th group draws under, clear of every column id.
fn group_id(n: usize) -> u32 {
    super::MAX_COLUMNS as u32 + n as u32
}

/// A scatter's series: X read off the category column as numbers (a row whose
/// X is not a number left out and said), Y off each series column, or off the
/// first split by the group column; a bubble's size off the size column, a row
/// with none left out.
fn scatter_clouds(chart: &Chart, data: &mut Data) -> Vec<Cloud> {
    let t = &chart.table;
    let m = &chart.mapping;
    let colours = palette(chart.options.palette);
    let xs: Vec<Option<f64>> = data.categories.iter().map(|c| parse_number(c)).collect();
    let mut left = Vec::new();
    for (i, x) in xs.iter().enumerate() {
        if x.is_none() {
            left.push(format!(
                "Row {}: “{}” is not a number",
                i + 1,
                data.categories[i]
            ));
        }
    }
    let rows: Vec<usize> = {
        // The table's rows that survived `read` (an empty X), in order.
        let cat = m.category.and_then(|c| t.index_of(c));
        (0..t.rows.len())
            .filter(|r| cat.is_none_or(|c| !t.rows[*r][c].trim().is_empty()))
            .collect()
    };
    let column = |id: Option<ColumnId>| -> Vec<Option<String>> {
        let at = id.and_then(|c| t.index_of(c));
        rows.iter()
            .map(|r| at.map(|a| t.rows[*r][a].trim().to_string()))
            .collect()
    };
    let sizes: Vec<Option<f64>> = column(m.size)
        .into_iter()
        .map(|c| c.and_then(|c| parse_number(&c)))
        .collect();
    let bubble = m.size.is_some();
    let shape_of = |col: ColumnId| {
        chart
            .series
            .iter()
            .find(|s| s.column == col)
            .and_then(|s| s.point)
            .unwrap_or(chart.options.points.shape)
    };
    let mut out = Vec::new();
    let groups = column(m.group);
    match (m.group.is_some(), data.series.first()) {
        (true, Some(s)) => {
            let mut names: Vec<String> = Vec::new();
            for (i, g) in groups.iter().enumerate() {
                let g = g.clone().unwrap_or_default();
                let n = match names.iter().position(|n| *n == g) {
                    Some(n) => n,
                    None => {
                        names.push(g.clone());
                        out.push(Cloud {
                            id: group_id(names.len() - 1),
                            name: if g.is_empty() { "(none)".into() } else { g },
                            fill: rgb(colours[(names.len() - 1) % colours.len()]),
                            shape: shape_of(s.column),
                            points: Vec::new(),
                        });
                        names.len() - 1
                    }
                };
                if let (Some(x), Some(y)) = (xs[i], s.values[i]) {
                    if bubble && sizes[i].is_none() {
                        left.push(format!("Row {}: no size", rows[i] + 1));
                        continue;
                    }
                    out[n].points.push((x, y, sizes[i], rows[i]));
                }
            }
        }
        _ => {
            for s in &data.series {
                let mut points = Vec::new();
                for (i, y) in s.values.iter().enumerate() {
                    if let (Some(x), Some(y)) = (xs[i], y) {
                        if bubble && sizes[i].is_none() {
                            continue;
                        }
                        points.push((x, *y, sizes[i], rows[i]));
                    }
                }
                out.push(Cloud {
                    id: s.column.0,
                    name: s.name.clone(),
                    fill: s.fill.clone(),
                    shape: shape_of(s.column),
                    points,
                });
            }
        }
    }
    data.left_out.extend(left);
    out
}

/// A scatter or bubble chart: numbers on both axes, each series' points at
/// their place, kept at their size as the plot resizes.
fn scatter(
    chart: &Chart,
    data: &Data,
    clouds: &[Cloud],
    body: Size,
    body_at: Point,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let p = &o.points;
    let marks: usize = clouds.iter().map(|c| c.points.len()).sum();
    if marks > super::MAX_MARKS {
        return Err(Refusal::TooManyMarks {
            count: marks,
            what: "points",
        });
    }
    let all = || clouds.iter().flat_map(|c| c.points.iter());
    let span = |f: fn(&ScatterPoint) -> f64| {
        let (lo, hi) = all()
            .map(f)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            });
        if lo.is_finite() { (lo, hi) } else { (0.0, 1.0) }
    };
    let (x_lo, x_hi) = span(|p| p.0);
    let (y_lo, y_hi) = span(|p| p.1);
    let spec = AxisSpec {
        horizontal: false,
        lo: y_lo,
        hi: y_hi,
        zero: false,
        percent: false,
        x: Some((x_lo, x_hi)),
    };
    let ax = axes(chart, data, body, body_at, out, spec);
    let plot = ax.plot;
    let xt = ax.x.as_ref().expect("a scatter's X axis is numeric");
    let largest = all()
        .filter_map(|p| p.2)
        .fold(0.0f64, |a, v| a.max(v.abs()));
    for c in clouds {
        let series_key = key(PartRole::Series, c.id, 0);
        out.frame_pinned(
            series_key,
            Some(ax.plot_key),
            &format!("Series {}", c.name),
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for (x, y, size, row) in &c.points {
            let at = Point::new(xt.t(*x) * plot.width, (1.0 - ax.t.t(*y)) * plot.height);
            // A bubble's area follows its size, so its side follows the root.
            let d = match size {
                Some(s) if largest > 0.0 => (p.max_size * (s.abs() / largest).sqrt()).max(1.0),
                _ => p.size,
            };
            let bx = Rect::from_center_size(at, (d, d));
            let kind = point_kind(c.shape, bx.size());
            let (fills, strokes) = mark_paint(o, &c.fill);
            out.parts.push(Part {
                key: key(PartRole::Point, c.id, *row as u32),
                parent: Some(series_key),
                name: format!("{} · row {}", c.name, row + 1),
                kind,
                transform: Affine::translate(bx.origin().to_vec2()),
                fills,
                strokes,
                opacity: p.opacity as f32,
                insets: centred(
                    at.x / plot.width.max(1e-9) * 100.0,
                    at.y / plot.height.max(1e-9) * 100.0,
                ),
                display: None,
                item: LayoutItem::default(),
            });
        }
    }
    ax.category_labels(data, out);
    Ok(())
}

/// A point's shape at `size`: a circle, a square, or a diamond or triangle
/// path, which its centring insets keep at its size.
fn point_kind(shape: super::PointShape, size: Size) -> NodeKind {
    let (w, h) = (size.width, size.height);
    let path = |pts: &[(f64, f64)]| {
        let mut p = kurbo::BezPath::new();
        p.move_to(pts[0]);
        for q in &pts[1..] {
            p.line_to(*q);
        }
        p.close_path();
        NodeKind::Path {
            path: p,
            corner_radii: Vec::new(),
            corner_smoothing: 0.0,
            mirror: crate::symmetry::PathMirror::NONE,
        }
    };
    match shape {
        super::PointShape::Circle => NodeKind::Ellipse {
            size,
            arc: Default::default(),
        },
        super::PointShape::Square => NodeKind::Rect {
            size,
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        super::PointShape::Diamond => {
            path(&[(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)])
        }
        super::PointShape::Triangle => path(&[(w / 2.0, 0.0), (w, h), (0.0, h)]),
    }
}

/// One slice of a pie as drawn: its place, category, value and colour.
struct Slice {
    index: u32,
    name: String,
    value: f64,
    fill: BrushSource,
}

/// The name a pie's folded small slices go by.
const OTHER: &str = "Other";

/// A pie's slices: the first series' values by category, a negative left out
/// and said, a zero left out, sorted if asked, and those under *Group small
/// slices*' share folded into *Other*. Colours run through the palette by
/// category.
fn pie_slices(chart: &Chart, data: &mut Data) -> Vec<Slice> {
    let o = &chart.options.slices;
    let colours = palette(chart.options.palette);
    let Some(s) = data.series.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut left = Vec::new();
    for (c, v) in s.values.iter().enumerate() {
        let Some(v) = v else { continue };
        if *v < 0.0 {
            left.push(format!(
                "{}, {}: a pie cannot draw a negative value",
                s.name, data.categories[c]
            ));
            continue;
        }
        if *v == 0.0 {
            continue;
        }
        out.push(Slice {
            index: c as u32,
            name: data.categories[c].clone(),
            value: *v,
            fill: rgb(colours[c % colours.len()]),
        });
    }
    data.left_out.extend(left);
    if o.sort {
        out.sort_by(|a, b| b.value.total_cmp(&a.value));
    }
    if let Some(below) = o.group_below {
        let total: f64 = out.iter().map(|s| s.value).sum();
        let small = |s: &Slice| total > 0.0 && s.value / total * 100.0 < below;
        let folded: f64 = out.iter().filter(|s| small(s)).map(|s| s.value).sum();
        if out.iter().filter(|s| small(s)).count() > 1 {
            out.retain(|s| !small(s));
            out.push(Slice {
                index: super::MAX_ROWS as u32,
                name: OTHER.into(),
                value: folded,
                fill: rgb(*colours.last().expect("a palette has colours")),
            });
        }
    }
    out
}

/// A pie or a donut: its slices in a square frame centred in the body, each a
/// wedge path pinned by its own box, and their labels inside or outside with a
/// leader line.
fn pie(
    chart: &Chart,
    slices: &[Slice],
    body: Size,
    body_at: Point,
    row: bool,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let so = &o.slices;
    if slices.len() > super::MAX_SLICES {
        return Err(Refusal::TooManyMarks {
            count: slices.len(),
            what: "slices",
        });
    }
    let body_key = key(PartRole::Body, 0, 0);
    out.frame(
        body_key,
        None,
        "Body",
        Rect::from_origin_size(body_at, body),
        None,
        body_item(row),
    );
    let side = body.width.min(body.height).max(0.0);
    let frame = Rect::from_center_size(
        Rect::from_origin_size(Point::ZERO, body).center(),
        (side, side),
    );
    let pie_key = key(PartRole::PieFrame, 0, 0);
    // Centred at its own size: the pie keeps round as the body resizes.
    out.frame_pinned(
        pie_key,
        Some(body_key),
        "Pie",
        frame,
        None,
        LayoutItem::default(),
        centred(50.0, 50.0),
    );
    let outside = o.values.position != ValuePosition::Off && o.values.pie_outside;
    let size = o.text.value_size;
    let centre = Point::new(side / 2.0, side / 2.0);
    // Room for labels outside: the widest, and a leader line's length.
    let labels: Vec<String> = slices.iter().map(|s| pie_label(chart, s, slices)).collect();
    let widest = labels
        .iter()
        .map(|l| out.text.measure(l, size).width())
        .fold(0.0, f64::max);
    let r = if outside {
        (side / 2.0 - widest - LEADER).max(side / 4.0)
    } else {
        side / 2.0
    };
    let hole = if so.donut {
        r * (so.hole / 100.0).clamp(0.0, 0.95)
    } else {
        0.0
    };
    // One slice of at most 100 is a progress ring: its share of 100, over a
    // track of what is left.
    let ring = slices.len() == 1 && slices[0].value <= 100.0;
    let total: f64 = if ring {
        100.0
    } else {
        slices.iter().map(|s| s.value).sum()
    };
    let series_key = key(PartRole::Series, 0, 0);
    out.frame_pinned(
        series_key,
        Some(pie_key),
        "Slices",
        Rect::from_origin_size(Point::ZERO, Size::new(side, side)),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    let pad = so.pad_angle.to_radians();
    let mut a = so.start_angle.to_radians() - std::f64::consts::FRAC_PI_2;
    if ring {
        out.path(
            key(PartRole::Ground, 0, 0),
            series_key,
            "Track".into(),
            wedge(centre, r, hole, a, a + std::f64::consts::TAU),
            Size::new(side, side),
            fill(o.value_axis.grid.colour.clone()),
            Vec::new(),
            1.0,
        );
    }
    let mut mids = Vec::new();
    for s in slices {
        let sweep = if total > 0.0 {
            s.value / total * std::f64::consts::TAU
        } else {
            0.0
        };
        let (a0, a1) = if slices.len() > 1 {
            (a + pad / 2.0, a + sweep - pad / 2.0)
        } else {
            (a, a + sweep)
        };
        mids.push((a + sweep / 2.0, s));
        a += sweep;
        if a1 <= a0 {
            continue;
        }
        let (path, corners) = wedge_corners(centre, r, hole, a0, a1);
        let radii = if so.radius > 0.0 {
            let mut radii = vec![0.0; corners.iter().max().map_or(0, |m| m + 1)];
            for i in corners {
                radii[i] = so.radius;
            }
            radii
        } else {
            Vec::new()
        };
        let (fills, strokes) = mark_paint(o, &s.fill);
        out.path_rounded(
            key(PartRole::Slice, 0, s.index),
            series_key,
            format!("Slice · {}", s.name),
            (path, radii),
            Size::new(side, side),
            fills,
            strokes,
            1.0,
        );
    }

    if o.values.position != ValuePosition::Off {
        let values_key = key(PartRole::Values, 0, 0);
        out.frame_pinned(
            values_key,
            Some(pie_key),
            "Values",
            Rect::from_origin_size(Point::ZERO, Size::new(side, side)),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for ((mid, s), text) in mids.into_iter().zip(&labels) {
            let b = out.text.measure(text, size);
            let dir = kurbo::Vec2::new(mid.cos(), mid.sin());
            let at = if outside {
                let from = centre + dir * r;
                let to = centre + dir * (r + LEADER * 0.6);
                let mut lead = kurbo::BezPath::new();
                lead.move_to(from);
                lead.line_to(to);
                out.path(
                    key(PartRole::LeaderLine, 0, s.index),
                    values_key,
                    format!("Leader · {}", s.name),
                    lead,
                    Size::new(side, side),
                    Vec::new(),
                    vec![line_stroke(o.text.ink.clone(), 1.0, super::Dash::Solid)],
                    1.0,
                );
                // Beside the leader's end, on the side it points to.
                let end = centre + dir * (r + LEADER * 0.75);
                let x = if dir.x >= 0.0 {
                    end.x
                } else {
                    end.x - b.width()
                };
                Point::new(x, end.y - b.height() / 2.0)
            } else {
                // At the middle of the ring, or two thirds out on a pie.
                let at_r = if hole > 0.0 {
                    (r + hole) / 2.0
                } else {
                    r * 0.62
                };
                let p = centre + dir * at_r;
                Point::new(p.x - b.width() / 2.0, p.y - b.height() / 2.0)
            };
            let bx = Rect::from_origin_size(at, b.size());
            out.label(
                key(PartRole::ValueLabel, 0, s.index),
                values_key,
                format!("Value {text}"),
                text,
                size,
                Point::new(at.x - b.x0, at.y - b.y0),
                centred(
                    bx.center().x / side.max(1e-9) * 100.0,
                    bx.center().y / side.max(1e-9) * 100.0,
                ),
                0.0,
            );
        }
    }
    Ok(())
}

/// The room a pie's outside labels leave for their leader lines.
const LEADER: f64 = 16.0;

/// What a slice's label says: its value, its share or its category.
fn pie_label(chart: &Chart, s: &Slice, slices: &[Slice]) -> String {
    let f = &chart.options.values.format;
    match chart.options.values.pie {
        super::PieLabel::Category => s.name.clone(),
        super::PieLabel::Percent => {
            // A progress ring's share is of 100, as it is drawn.
            let total: f64 = match slices {
                [one] if one.value <= 100.0 => 100.0,
                _ => slices.iter().map(|s| s.value).sum(),
            };
            let mut f = f.clone();
            f.percent = true;
            scale::format(s.value / total.max(1e-12), &f, 0)
        }
        super::PieLabel::Value => scale::format(s.value, f, scale::decimals_of(s.value).min(3)),
    }
}

/// A ring's wedge from `a0` to `a1` (radians, clockwise from three o'clock on
/// screen) between radii `inner` and `outer` about `c`; a pie's when `inner` is 0.
fn wedge(c: Point, outer: f64, inner: f64, a0: f64, a1: f64) -> kurbo::BezPath {
    wedge_corners(c, outer, inner, a0, a1).0
}

/// [`wedge`], and the anchors where its edges meet at an angle (in
/// `geometry::anchor_runs`' order, which counts one anchor per element after the
/// move): the ones a slice's corner radius rounds. A whole ring has none.
fn wedge_corners(
    c: Point,
    outer: f64,
    inner: f64,
    a0: f64,
    a1: f64,
) -> (kurbo::BezPath, Vec<usize>) {
    let mut p = kurbo::BezPath::new();
    let on = |r: f64, a: f64| c + kurbo::Vec2::new(a.cos(), a.sin()) * r;
    let arc = |p: &mut kurbo::BezPath, r: f64, from: f64, to: f64| {
        let a = kurbo::Arc::new(c, (r, r), from, to - from, 0.0);
        a.to_cubic_beziers(0.01, |p1, p2, p3| p.curve_to(p1, p2, p3));
    };
    let whole = a1 - a0 >= std::f64::consts::TAU - 1e-9;
    let last = |p: &kurbo::BezPath| p.elements().len() - 1;
    let mut corners = vec![0];
    p.move_to(on(outer, a0));
    arc(&mut p, outer, a0, a1);
    corners.push(last(&p));
    if inner > 0.0 {
        p.line_to(on(inner, a1));
        corners.push(last(&p));
        arc(&mut p, inner, a1, a0);
        corners.push(last(&p));
    } else if !whole {
        p.line_to(c);
        corners.push(last(&p));
    }
    p.close_path();
    if whole {
        corners.clear();
    }
    (p, corners)
}

/// Line and area charts: per series its area (an area chart), its line and its
/// points, each a path or a shape pinned by percentage insets of the plot.
fn lines(
    chart: &Chart,
    data: &Data,
    body: Size,
    body_at: Point,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let l = &o.lines;
    let area = chart.kind == ChartKind::Area;
    let stacking = if area {
        l.area_stacking
    } else {
        Stacking::Apart
    };
    let n_cat = data.categories.len();
    let points_on = l.points != super::PointStyle::None;
    if points_on {
        let marks = n_cat * data.series.len();
        if marks > super::MAX_MARKS {
            return Err(Refusal::TooManyMarks {
                count: marks,
                what: "points",
            });
        }
    }

    // Each series' drawn values: its own, or stacked on the ones below it (an
    // empty cell stacks as zero), or as shares of the category's total.
    let totals: Vec<f64> = (0..n_cat)
        .map(|c| {
            data.series
                .iter()
                .filter_map(|s| s.values[c])
                .map(f64::abs)
                .sum::<f64>()
        })
        .collect();
    // Positives stack up from zero and negatives down, apart, as bars do.
    let mut ups = vec![0.0f64; n_cat];
    let mut downs = vec![0.0f64; n_cat];
    let mut layers: Vec<(Vec<Option<f64>>, Vec<f64>)> = Vec::new();
    for s in &data.series {
        let mut top = Vec::with_capacity(n_cat);
        let mut base = vec![0.0; n_cat];
        for c in 0..n_cat {
            let v = s.values[c];
            let v = match stacking {
                Stacking::Percent if totals[c] > 0.0 => v.map(|v| v / totals[c]),
                _ => v,
            };
            match (stacking, v) {
                (Stacking::Apart, _) | (_, None) => {
                    // A gap stands on what is under it, so the next run joins there.
                    base[c] = if stacking == Stacking::Apart {
                        0.0
                    } else {
                        ups[c]
                    };
                    top.push(v);
                }
                (_, Some(v)) if v >= 0.0 => {
                    base[c] = ups[c];
                    ups[c] += v;
                    top.push(Some(ups[c]));
                }
                (_, Some(v)) => {
                    base[c] = downs[c];
                    downs[c] += v;
                    top.push(Some(downs[c]));
                }
            }
        }
        layers.push((top, base));
    }
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (top, base) in &layers {
        for (t, b) in top.iter().zip(base) {
            if let Some(t) = t {
                lo = lo.min(*t);
                hi = hi.max(*t);
                if area {
                    lo = lo.min(*b);
                    hi = hi.max(*b);
                }
            }
        }
    }
    if !lo.is_finite() {
        (lo, hi) = (0.0, 1.0);
    }
    let spec = AxisSpec {
        horizontal: false,
        lo,
        hi,
        // An area fills down to its baseline, so its axis holds zero.
        zero: area,
        percent: stacking == Stacking::Percent,
        x: None,
    };
    let ax = axes(chart, data, body, body_at, out, spec);
    let plot = ax.plot;
    let at =
        |c: usize, v: f64| Point::new((c as f64 + 0.5) * ax.band, (1.0 - ax.t.t(v)) * plot.height);

    for (si, s) in data.series.iter().enumerate() {
        let (top, base) = &layers[si];
        let style = chart.series.iter().find(|st| st.column == s.column);
        let series_key = key(PartRole::Series, s.column.0, 0);
        out.frame_pinned(
            series_key,
            Some(ax.plot_key),
            &format!("Series {}", s.name),
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        // The runs a line draws: broken at an empty cell unless gaps connect.
        let mut runs: Vec<Vec<(usize, f64)>> = vec![Vec::new()];
        for (c, v) in top.iter().enumerate() {
            match v {
                Some(v) => runs.last_mut().expect("one run").push((c, *v)),
                None if !l.connect_gaps && !runs.last().expect("one run").is_empty() => {
                    runs.push(Vec::new())
                }
                None => {}
            }
        }
        runs.retain(|r| !r.is_empty());

        if area {
            let mut path = kurbo::BezPath::new();
            for run in &runs {
                let pts: Vec<Point> = run.iter().map(|(c, v)| at(*c, *v)).collect();
                curve(&mut path, &pts, l.curve);
                // Back along what it stands on, right to left.
                let floor: Vec<Point> = run.iter().rev().map(|(c, _)| at(*c, base[*c])).collect();
                if let Some(first) = floor.first() {
                    path.line_to(*first);
                }
                let mut back = kurbo::BezPath::new();
                curve(
                    &mut back,
                    &floor,
                    if stacking == Stacking::Apart {
                        super::Curve::Straight
                    } else {
                        l.curve
                    },
                );
                for el in back.elements().iter().skip(1) {
                    path.push(*el);
                }
                path.close_path();
            }
            out.path(
                key(PartRole::Area, s.column.0, 0),
                series_key,
                format!("Area · {}", s.name),
                path,
                plot,
                fill(s.fill.clone()),
                Vec::new(),
                l.area_opacity as f32,
            );
        }
        let mut path = kurbo::BezPath::new();
        for run in &runs {
            let pts: Vec<Point> = run.iter().map(|(c, v)| at(*c, *v)).collect();
            curve(&mut path, &pts, l.curve);
        }
        let dash = style.map_or(super::Dash::Solid, |st| st.dash);
        out.path(
            key(PartRole::Line, s.column.0, 0),
            series_key,
            format!("Line · {}", s.name),
            path,
            plot,
            Vec::new(),
            vec![line_stroke(s.fill.clone(), l.width, dash)],
            1.0,
        );
        if points_on {
            let size = l.point_size;
            let hollow = l.points == super::PointStyle::Hollow;
            let shape = style.and_then(|st| st.point).unwrap_or_default();
            for run in &runs {
                for (c, v) in run {
                    let p = at(*c, *v);
                    let bx = Rect::from_center_size(p, (size, size));
                    let (fills, strokes) = if hollow {
                        (
                            fill(BrushSource::Literal(crate::Brush::Solid(
                                peniko::Color::WHITE,
                            ))),
                            vec![line_stroke(
                                s.fill.clone(),
                                (l.width).min(size / 3.0),
                                super::Dash::Solid,
                            )],
                        )
                    } else {
                        mark_paint(o, &s.fill)
                    };
                    out.parts.push(Part {
                        key: key(PartRole::Point, s.column.0, *c as u32),
                        parent: Some(series_key),
                        name: format!("{} · {}", s.name, data.categories[*c]),
                        kind: point_kind(shape, bx.size()),
                        transform: Affine::translate(bx.origin().to_vec2()),
                        fills,
                        strokes,
                        opacity: 1.0,
                        insets: centred(
                            p.x / plot.width.max(1e-9) * 100.0,
                            p.y / plot.height.max(1e-9) * 100.0,
                        ),
                        display: None,
                        item: LayoutItem::default(),
                    });
                }
            }
        }
    }

    // Value labels sit above their point.
    if o.values.position != ValuePosition::Off {
        let values_key = key(PartRole::Values, 0, 0);
        out.frame_pinned(
            values_key,
            Some(ax.plot_key),
            "Values",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        let size = o.text.value_size;
        for (si, s) in data.series.iter().enumerate() {
            for (c, v) in layers[si].0.iter().enumerate() {
                let (Some(v), Some(raw)) = (v, s.values[c]) else {
                    continue;
                };
                let mut f = o.values.format.clone();
                if stacking == Stacking::Percent {
                    f.percent = true;
                }
                let decimals = if f.percent {
                    0
                } else {
                    scale::decimals_of(raw).min(3)
                };
                let shown_v = if stacking == Stacking::Percent {
                    raw / totals[c].max(1e-12)
                } else {
                    raw
                };
                let text = scale::format(shown_v, &f, decimals);
                let b = out.text.measure(&text, size);
                let p = at(c, *v);
                let lift = VALUE_GAP + l.point_size.max(l.width) / 2.0;
                let top_y = p.y - lift - b.height();
                let mut insets = centred_x(p.x / plot.width.max(1e-9) * 100.0);
                insets.top = pct(top_y / plot.height.max(1e-9) * 100.0);
                out.label(
                    key(PartRole::ValueLabel, s.column.0, c as u32),
                    values_key,
                    format!("Value {text}"),
                    &text,
                    size,
                    Point::new(p.x - b.width() / 2.0 - b.x0, top_y - b.y0),
                    insets,
                    0.0,
                );
            }
        }
    }
    ax.category_labels(data, out);
    Ok(())
}

/// `pts` appended to `path` as `curve` draws them, starting with a move.
///
/// *Smooth* is a monotone cubic (Fritsch–Carlson): each point's tangent is
/// limited so the curve never overshoots a value the data does not have.
/// *Step* turns halfway between points.
fn curve(path: &mut kurbo::BezPath, pts: &[Point], curve: super::Curve) {
    let Some(first) = pts.first() else { return };
    path.move_to(*first);
    match curve {
        super::Curve::Straight => {
            for p in &pts[1..] {
                path.line_to(*p);
            }
        }
        super::Curve::Step => {
            for w in pts.windows(2) {
                let mid = (w[0].x + w[1].x) / 2.0;
                path.line_to((mid, w[0].y));
                path.line_to((mid, w[1].y));
                path.line_to(w[1]);
            }
        }
        super::Curve::Smooth => {
            let n = pts.len();
            if n < 3 {
                for p in &pts[1..] {
                    path.line_to(*p);
                }
                return;
            }
            let slope = |i: usize| (pts[i + 1].y - pts[i].y) / (pts[i + 1].x - pts[i].x);
            let d: Vec<f64> = (0..n - 1).map(slope).collect();
            let mut m = vec![0.0; n];
            m[0] = d[0];
            m[n - 1] = d[n - 2];
            for i in 1..n - 1 {
                m[i] = if d[i - 1] * d[i] <= 0.0 {
                    0.0
                } else {
                    (d[i - 1] + d[i]) / 2.0
                };
            }
            for i in 0..n - 1 {
                if d[i] == 0.0 {
                    m[i] = 0.0;
                    m[i + 1] = 0.0;
                    continue;
                }
                let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
                let s = a * a + b * b;
                if s > 9.0 {
                    let t = 3.0 / s.sqrt();
                    m[i] = t * a * d[i];
                    m[i + 1] = t * b * d[i];
                }
            }
            for i in 0..n - 1 {
                let (p0, p1) = (pts[i], pts[i + 1]);
                let h = (p1.x - p0.x) / 3.0;
                path.curve_to(
                    (p0.x + h, p0.y + m[i] * h),
                    (p1.x - h, p1.y - m[i + 1] * h),
                    (p1.x, p1.y),
                );
            }
        }
    }
}

/// A series line's stroke: round joins and caps, dashed as the series says.
fn line_stroke(brush: BrushSource, width: f64, dash: super::Dash) -> Stroke {
    let (dashes, cap) = match dash {
        super::Dash::Solid => (Vec::new(), kurbo::Cap::Round),
        super::Dash::Dashed => (vec![width * 3.0, width * 2.0], kurbo::Cap::Butt),
        super::Dash::Dotted => (vec![0.0, width * 2.0], kurbo::Cap::Round),
    };
    Stroke {
        brush,
        width,
        width_var: None,
        join: kurbo::Join::Round,
        cap,
        dashes,
        dash_offset: 0.0,
        sides: crate::node::StrokeSides::All,
        miter_limit: 4.0,
        dash_fit: false,
        align: crate::node::StrokeAlign::Center,
        visible: true,
    }
}

/// Insets centring a box on (`x`, `y`) percent of its parent.
fn centred(x: f64, y: f64) -> Insets {
    let mut i = centred_x(x);
    let v = centred_y(y);
    i.top = v.top;
    i.bottom = v.bottom;
    i.margin_auto.top = true;
    i.margin_auto.bottom = true;
    i
}

/// Insets of 0 on every side: a container covering its parent.
fn fill_insets() -> Insets {
    Insets {
        left: px(0.0),
        right: px(0.0),
        top: px(0.0),
        bottom: px(0.0),
        margin_auto: AutoMargins::default(),
    }
}

/// Insets centring a box horizontally on `x` percent of its parent.
fn centred_x(x: f64) -> Insets {
    Insets {
        left: pct(x),
        right: pct(100.0 - x),
        margin_auto: AutoMargins {
            left: true,
            right: true,
            ..AutoMargins::default()
        },
        ..Insets::default()
    }
}

/// Insets centring a box vertically on `y` percent of its parent.
fn centred_y(y: f64) -> Insets {
    Insets {
        top: pct(y),
        bottom: pct(100.0 - y),
        margin_auto: AutoMargins {
            top: true,
            bottom: true,
            ..AutoMargins::default()
        },
        ..Insets::default()
    }
}

/// A 1 px rule across the plot at `share` along the value axis, and its insets.
fn rule_at(horizontal: bool, share: f64, plot: Size) -> (Rect, Insets) {
    if horizontal {
        let x = share * plot.width;
        let mut insets = centred_x(share * 100.0);
        insets.top = px(0.0);
        insets.bottom = px(0.0);
        (Rect::new(x - 0.5, 0.0, x + 0.5, plot.height), insets)
    } else {
        let y = (1.0 - share) * plot.height;
        let mut insets = centred_y((1.0 - share) * 100.0);
        insets.left = px(0.0);
        insets.right = px(0.0);
        (Rect::new(0.0, y - 0.5, plot.width, y + 0.5), insets)
    }
}

/// A tick mark in an axis frame `frame` big: a 1 px stub [`TICK_LEN`] long at
/// `along` (a share of the frame along its axis, from its left or top), across
/// the axis from the frame's start edge (left or top) or its end. Centred like a
/// tick label, so it keeps its size while its place follows the plot.
fn tick_mark(stub_horizontal: bool, along: f64, from_start: bool, frame: Size) -> (Rect, Insets) {
    if stub_horizontal {
        let y = along * frame.height;
        let x = if from_start {
            0.0
        } else {
            frame.width - TICK_LEN
        };
        let mut insets = centred_y(along * 100.0);
        if from_start {
            insets.left = px(0.0);
        } else {
            insets.right = px(0.0);
        }
        (Rect::new(x, y - 0.5, x + TICK_LEN, y + 0.5), insets)
    } else {
        let x = along * frame.width;
        let y = if from_start {
            0.0
        } else {
            frame.height - TICK_LEN
        };
        let mut insets = centred_x(along * 100.0);
        if from_start {
            insets.top = px(0.0);
        } else {
            insets.bottom = px(0.0);
        }
        (Rect::new(x - 0.5, y, x + 0.5, y + TICK_LEN), insets)
    }
}

/// An axis title's corner, insets and turn in its axis frame `frame` big, at the
/// frame's start edge (left or top) or, `at_end`, its end, centred along it.
/// A side axis's title reads upwards, turned a quarter; its insets place the
/// unturned box, which layout turns about its centre.
fn title_place(b: Rect, frame: Size, turned: bool, at_end: bool) -> (Point, Insets, f64) {
    if turned {
        let cx = if at_end {
            frame.width - b.height() / 2.0
        } else {
            b.height() / 2.0
        };
        let cy = frame.height / 2.0;
        let mut insets = centred_y(50.0);
        let reach = b.height() / 2.0 - b.width() / 2.0;
        if at_end {
            insets.right = px(reach);
        } else {
            insets.left = px(reach);
        }
        let at = Point::new(cx - b.width() / 2.0 - b.x0, cy - b.height() / 2.0 - b.y0);
        (at, insets, -std::f64::consts::FRAC_PI_2)
    } else {
        let top = if at_end {
            frame.height - b.height()
        } else {
            0.0
        };
        let mut insets = centred_x(50.0);
        if at_end {
            insets.bottom = px(0.0);
        } else {
            insets.top = px(0.0);
        }
        let at = Point::new(frame.width / 2.0 - b.width() / 2.0 - b.x0, top - b.y0);
        (at, insets, 0.0)
    }
}

/// Every `n`th label of a band `band` long that keeps labels `need` long apart.
fn thin_every(need: f64, band: f64) -> usize {
    if band <= 0.0 {
        return 1;
    }
    ((need / band).ceil() as usize).max(1)
}

/// How far category labels turn and how many are kept, for columns: flat where
/// they fit; *Auto* turns them 45° and then thins them where they would touch.
fn category_turn(angle: LabelAngle, boxes: &[Rect], band: f64, label_h: f64) -> (f64, usize) {
    let widest = boxes.iter().map(Rect::width).fold(0.0, f64::max);
    let room = band - 4.0;
    let slant = -std::f64::consts::FRAC_PI_4;
    let upright = -std::f64::consts::FRAC_PI_2;
    match angle {
        LabelAngle::Flat => (0.0, thin_every(widest + 4.0, band)),
        LabelAngle::Slanted => (slant, thin_every(label_h * 1.5, band)),
        LabelAngle::Upright => (upright, thin_every(label_h * 1.2, band)),
        LabelAngle::Auto if widest <= room => (0.0, 1),
        LabelAngle::Auto => (slant, thin_every(label_h * 1.5, band)),
    }
}

/// Where a value label sits on its bar `bar` (in the plot `plot`), its box `b`:
/// its corner and its insets, or `None` when it does not fit inside.
fn value_place(
    at: ValuePosition,
    horizontal: bool,
    up: bool,
    bar: Rect,
    b: Rect,
    plot: Size,
) -> Option<(Point, Insets)> {
    let fits = b.width() + 2.0 <= bar.width() && b.height() + 2.0 <= bar.height();
    let share_x = |x: f64| x / plot.width.max(1e-9) * 100.0;
    let share_y = |y: f64| y / plot.height.max(1e-9) * 100.0;
    let c = bar.center();
    if horizontal {
        let y = c.y - b.height() / 2.0 - b.y0;
        let mut insets = centred_y(share_y(c.y));
        let x = match at {
            ValuePosition::Off => return None,
            ValuePosition::Inside if fits => c.x - b.width() / 2.0,
            ValuePosition::End if fits && up => bar.x1 - VALUE_GAP - b.width(),
            ValuePosition::End if fits => bar.x0 + VALUE_GAP,
            ValuePosition::Outside if up => bar.x1 + VALUE_GAP,
            ValuePosition::Outside => bar.x0 - VALUE_GAP - b.width(),
            _ => return None,
        };
        insets.left = pct(share_x(x));
        Some((Point::new(x - b.x0, y), insets))
    } else {
        let x = c.x - b.width() / 2.0 - b.x0;
        let mut insets = centred_x(share_x(c.x));
        let y = match at {
            ValuePosition::Off => return None,
            ValuePosition::Inside if fits => c.y - b.height() / 2.0,
            ValuePosition::End if fits && up => bar.y0 + VALUE_GAP,
            ValuePosition::End if fits => bar.y1 - VALUE_GAP - b.height(),
            ValuePosition::Outside if up => bar.y0 - VALUE_GAP - b.height(),
            ValuePosition::Outside => bar.y1 + VALUE_GAP,
            _ => return None,
        };
        insets.top = pct(share_y(y));
        Some((Point::new(x, y - b.y0), insets))
    }
}
