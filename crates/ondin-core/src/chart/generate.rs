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
use kurbo::{Affine, Point, Rect, RoundedRectRadii, Size};

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
    let data = read(chart);
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
        p if data.series.is_empty() || matches!(chart.kind, ChartKind::Sankey) => {
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
    let legend = legend_at.map(|at| legend_items(&data, o.text.legend_size, &text, at, inner));
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
    match chart.kind {
        ChartKind::Column | ChartKind::Bar => {
            bars(chart, &data, body, body_at, &mut out)?;
        }
        // The other kinds land with the generator's second half; until then a
        // body with nothing in it.
        _ => out.frame(
            key(PartRole::Body, 0, 0),
            None,
            "Body",
            Rect::from_origin_size(body_at, body),
            None,
            body_item(row),
        ),
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
    Ok(Generated {
        display,
        parts,
        left_out: data.left_out,
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
    data: &Data,
    size: f64,
    text: &Text,
    at: LegendPosition,
    inner: Size,
) -> LegendLayout {
    let column = matches!(at, LegendPosition::Left | LegendPosition::Right);
    let mut items = Vec::new();
    let (mut x, mut y, mut line_h, mut widest) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for s in &data.series {
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
                series: s.column.0,
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

    // The value axis's labels decide its thickness, and the category labels
    // the other axis's.
    let va = &o.value_axis;
    let ca = &o.category_axis;
    let tick_size = o.text.tick_size;
    let label_h = out.text.measure("0", tick_size).height();
    let mut value_fmt = va.format.clone();
    if stacking == Stacking::Percent && !value_fmt.percent {
        value_fmt.percent = true;
    }
    // A first guess of the plot's length along the value axis, for the count.
    let along = if horizontal { body.width } else { body.height } * 0.8;
    let mut axis_va = va.clone();
    axis_va.start_at_zero = true;
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
    let va_thick = if va_show {
        if horizontal {
            label_h + AXIS_GAP
        } else {
            tick_boxes.iter().map(Rect::width).fold(0.0, f64::max) + AXIS_GAP
        }
    } else {
        0.0
    };

    // Category labels: flat, or turned and thinned where they would touch.
    let cat_boxes: Vec<Rect> = data
        .categories
        .iter()
        .map(|c| out.text.measure(c, tick_size))
        .collect();
    let band_len_guess = if horizontal {
        body.height
    } else {
        body.width - va_thick
    } / n_cat.max(1) as f64;
    let (turn, every) = if horizontal {
        (0.0, thin_every(label_h * 1.2, band_len_guess))
    } else {
        category_turn(ca.angle, &cat_boxes, band_len_guess, label_h)
    };
    let ca_thick = if !ca_show {
        0.0
    } else if horizontal {
        cat_boxes.iter().map(Rect::width).fold(0.0, f64::max) + AXIS_GAP
    } else {
        let (s, c) = turn.sin_cos();
        cat_boxes
            .iter()
            .map(|b| b.width() * s.abs() + b.height() * c.abs())
            .fold(0.0, f64::max)
            + AXIS_GAP
    };

    // The body grid: the value axis beside the plot, the category axis under it
    // (a bar chart has them the other way round).
    let (left_w, bottom_h) = if horizontal {
        (ca_thick, va_thick)
    } else {
        (va_thick, ca_thick)
    };
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
    let columns = if left_at_end {
        vec![fr.clone(), px_track(left_w)]
    } else {
        vec![px_track(left_w), fr.clone()]
    };
    let rows = if bottom_at_top {
        vec![px_track(bottom_h), fr.clone()]
    } else {
        vec![fr, px_track(bottom_h)]
    };
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
    let (plot_col, side_col) = if left_at_end { (1, 2) } else { (2, 1) };
    let (plot_row, under_row) = if bottom_at_top { (2, 1) } else { (1, 2) };
    let cell = |col: i16, row: i16| LayoutItem {
        width: Dimension::Percent(100.0),
        height: Dimension::Percent(100.0),
        grid_column: GridLines {
            start: GridPlacement::Line(col),
            end: GridPlacement::Auto,
        },
        grid_row: GridLines {
            start: GridPlacement::Line(row),
            end: GridPlacement::Auto,
        },
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
        cell(side_col, plot_row),
    );
    out.frame(
        under_key,
        Some(body_key),
        axis_name(under_role),
        under_box,
        None,
        cell(plot_col, under_row),
    );
    out.frame(
        plot_key,
        Some(body_key),
        "Plot",
        plot_box,
        None,
        cell(plot_col, plot_row),
    );

    // Along the value axis, as a share of the plot from its start: up from the
    // bottom for columns, right from the left for bars.
    let along_len = if horizontal { plot.width } else { plot.height };
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
                let at = Point::new(cx - b.width() / 2.0 - b.x0, AXIS_GAP - b.y0);
                let at = if bottom_at_top {
                    Point::new(at.x, frame_h - AXIS_GAP - b.height() - b.y0)
                } else {
                    at
                };
                let mut insets = centred_x(share * 100.0);
                if bottom_at_top {
                    insets.bottom = px(AXIS_GAP);
                } else {
                    insets.top = px(AXIS_GAP);
                }
                out.label(k, value_axis_key, name, s, tick_size, at, insets, 0.0);
            } else {
                let cy = (1.0 - share) * frame_h;
                let x = if left_at_end {
                    AXIS_GAP - b.x0
                } else {
                    frame_w - AXIS_GAP - b.width() - b.x0
                };
                let at = Point::new(x, cy - b.height() / 2.0 - b.y0);
                let mut insets = centred_y((1.0 - share) * 100.0);
                if left_at_end {
                    insets.left = px(AXIS_GAP);
                } else {
                    insets.right = px(AXIS_GAP);
                }
                out.label(k, value_axis_key, name, s, tick_size, at, insets, 0.0);
            }
        }
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
            let (bx, insets) = rule_at(horizontal, share, plot);
            out.rect(
                k,
                grid_key,
                name,
                bx,
                Spec {
                    fill: va.grid.colour.clone(),
                    radii: RoundedRectRadii::default(),
                    insets,
                },
            );
        }
    }
    if ca.show && ca.line {
        // The baseline, at zero.
        let (bx, insets) = rule_at(horizontal, value_share(0.0), plot);
        out.rect(
            key(PartRole::AxisLine, 1, 0),
            grid_key,
            "Baseline".into(),
            bx,
            Spec {
                fill: ca.colour.clone(),
                radii: RoundedRectRadii::default(),
                insets,
            },
        );
    }

    // The bars.
    let band = across_len / n_cat.max(1) as f64;
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

    // Category labels, every `every`th, centred on their band.
    if ca_show {
        let cat_axis_key = key(PartRole::CategoryAxis, 0, 0);
        let frame = if horizontal { side_box } else { under_box };
        for (c, name) in data.categories.iter().enumerate() {
            if c % every != 0 {
                continue;
            }
            let b = cat_boxes[c];
            let centre = (c as f64 + 0.5) * band;
            let k = key(PartRole::CategoryLabel, 0, c as u32);
            let label_name = format!("Category {name}");
            if horizontal {
                let x = if left_at_end {
                    AXIS_GAP - b.x0
                } else {
                    frame.width() - AXIS_GAP - b.width() - b.x0
                };
                let at = Point::new(x, centre - b.height() / 2.0 - b.y0);
                let mut insets = centred_y(centre / across_len * 100.0);
                if left_at_end {
                    insets.left = px(AXIS_GAP);
                } else {
                    insets.right = px(AXIS_GAP);
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
                    frame.height() - AXIS_GAP - reach / 2.0
                } else {
                    AXIS_GAP + reach / 2.0
                };
                let at = Point::new(
                    centre - b.width() / 2.0 - b.x0,
                    cy - b.height() / 2.0 - b.y0,
                );
                let mut insets = centred_x(centre / across_len * 100.0);
                let edge = AXIS_GAP + reach / 2.0 - b.height() / 2.0;
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
    let _ = along_len;
    Ok(())
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
